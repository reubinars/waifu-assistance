//! The island's state and its frame: what to show, the animator, and drawing it.

use crate::animator::{Animator, Face};
use crate::draw::{Color, Painter};
use crate::fullscreen;
use crate::gfx::Gfx;
use crate::log;
use crate::sources::{AgentSource, ArtImage, DemoAction, MediaCommand, MediaSource, SourceEvent, UiSender};
use crate::startup;
use crate::tray::{self, Tray};
use crate::ui_collapsed::{self, SPINNER_TICK_MS, Scene};
use crate::ui_expanded::{self, Hot, Hover};
use crate::window::{Event, VK_ESCAPE, Windows};
use island_core::config::Config;
use island_core::media::MediaClock;
use island_core::session::HerdrSession;
use island_core::view::layout::{MAX_ROWS, SEEK};
use island_core::view::{
    Collapsed, DisplayMode, EXPANDED, ExpandedHit, Shown, Tab, WINDOW, choose_collapsed, clock_label, collapsed_size,
    corner_radii, default_tab, hit_expanded, hit_rounded, island_rect, jump_target, shown, tabs_visible, time_in_state,
};
use island_core::{Point, Rect, Size};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{D2DERR_RECREATE_TARGET, HANDLE};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, PostQuitMessage, SetTimer};
use windows::core::{Error, Result};

const COLLAPSED_RADIUS: f32 = 20.0;
const EXPANDED_RADIUS: f32 = 32.0;

pub const TIMER_LINGER: usize = 1;
pub const TIMER_SPINNER: usize = 2;
pub const TIMER_RETRY: usize = 3;
/// Collapse after the mouse has been outside the expanded island for 2 s.
pub const TIMER_LEAVE: usize = 4;
/// Redraws the agents tab's time-in-state labels.
pub const TIMER_TICK: usize = 5;

const LEAVE_COLLAPSE: Duration = Duration::from_secs(2);
/// Deactivation right after expanding (the click that expanded) doesn't collapse.
const DEACTIVATE_ARM: Duration = Duration::from_millis(150);

struct Device {
    gfx: Gfx,
    painter: Painter,
    /// Glow, shadow and island fill: blurred, so redrawn only when they change.
    backdrop: Option<(Backdrop, ID2D1Bitmap1)>,
}

/// Everything the backdrop depends on.
#[derive(Clone, Copy, PartialEq)]
struct Backdrop {
    rect: Rect,
    radii: [f32; 4],
    glow: f32,
    glow_color: Color,
    mode: DisplayMode,
}

impl Device {
    fn new(win: &Windows) -> Result<Device> {
        let gfx = Gfx::new(win.visual, win.size_px(), win.scale)?;
        let painter = Painter::new(gfx.dc.clone(), gfx.factory.clone())?;
        Ok(Device { gfx, painter, backdrop: None })
    }
}

pub struct App {
    pub config: Config,
    win: Windows,
    session: HerdrSession,
    clock: MediaClock,
    art: Option<ArtImage>,
    art_bmp: Option<ID2D1Bitmap1>,
    collapsed: Collapsed,
    expanded: bool,
    expanded_at: Option<Instant>,
    tab: Tab,
    hover: Hover,
    /// Seekbar position (0..=1) while it is dragged.
    drag: Option<f32>,
    anim: Animator,
    device: Option<Device>,
    media: Box<dyn MediaSource>,
    agent_src: Box<dyn AgentSource>,
    t0: Instant,
    dirty: bool,
    animating: bool,
    spinner_on: bool,
    tick_on: bool,
    tray: Option<Tray>,
    /// Hidden from the tray menu.
    user_hidden: bool,
    /// The foreground window covers the main monitor.
    fullscreen: bool,
    /// Where tray changes are saved; `None` (demo) keeps them in memory and leaves the registry alone.
    config_path: Option<std::path::PathBuf>,
}

impl App {
    pub fn new(
        config: Config,
        config_path: Option<std::path::PathBuf>,
        win: Windows,
        clock: MediaClock,
        media: Box<dyn MediaSource>,
        agent_src: Box<dyn AgentSource>,
    ) -> App {
        let collapsed = Collapsed::Idle;
        let anim = Animator::new(collapsed_size(&collapsed), COLLAPSED_RADIUS, Face::Idle);
        let tray = Tray::add(win.tray).inspect_err(|e| log::error!("tray icon: {e}")).ok();
        App {
            tray,
            user_hidden: false,
            fullscreen: false,
            config_path,
            config,
            win,
            session: HerdrSession::default(),
            clock,
            art: None,
            art_bmp: None,
            collapsed,
            expanded: false,
            expanded_at: None,
            tab: Tab::Media,
            hover: Hover::default(),
            drag: None,
            anim,
            device: None,
            media,
            agent_src,
            t0: Instant::now(),
            dirty: true,
            animating: false,
            spinner_on: false,
            tick_on: false,
        }
    }

    pub fn start_sources(&mut self, tx: UiSender) {
        self.media.start(tx.clone());
        self.agent_src.start(tx);
    }

    fn secs(&self, now: Instant) -> f64 {
        now.saturating_duration_since(self.t0).as_secs_f64()
    }

    fn mode(&self) -> DisplayMode {
        self.config.display_mode
    }

    pub fn on_source_event(&mut self, e: SourceEvent) {
        let now = Instant::now();
        match e {
            SourceEvent::Media(snap) => self.clock.update(snap, now),
            SourceEvent::Art(art) => {
                self.art = art;
                self.art_bmp = None;
            }
            SourceEvent::Herdr(msg) => {
                for line in self.session.handle(msg, now) {
                    self.agent_src.send(&line);
                }
            }
            SourceEvent::BridgeDown => self.session.bridge_down(),
            SourceEvent::Demo(a) => match a {
                DemoAction::Expand(tab) => self.expand(tab, false, now),
                DemoAction::Collapse => self.collapse(),
                DemoAction::Mode(m) => self.set_mode(m, now),
            },
        }
        self.retarget(now);
        self.apply_visibility();
    }

    pub fn on_window_event(&mut self, e: Event) {
        let now = Instant::now();
        match e {
            Event::Timer(TIMER_LINGER) => {
                self.kill_timer(TIMER_LINGER);
                self.retarget(now);
            }
            Event::Timer(TIMER_SPINNER) => self.dirty = true,
            Event::Timer(TIMER_RETRY) => {
                self.kill_timer(TIMER_RETRY);
                self.dirty = true;
            }
            Event::Timer(TIMER_LEAVE) => {
                self.kill_timer(TIMER_LEAVE);
                self.collapse();
            }
            Event::Timer(TIMER_TICK) => self.dirty = true,
            Event::Timer(_) => {}
            Event::Reposition => {
                self.reposition();
                fullscreen::check(self.win.visual);
            }
            Event::MouseMove(x, y) => self.on_mouse_move(self.win.to_dip(x, y)),
            Event::MouseLeave => self.on_mouse_leave(),
            Event::MouseDown(x, y) => self.on_mouse_down(self.win.to_dip(x, y)),
            Event::MouseUp(x, y) => self.on_click(self.win.to_dip(x, y)),
            Event::Key(vk) => self.on_key(vk),
            Event::Activate(false) => self.on_deactivate(now),
            Event::Activate(true) => {}
            Event::CaptureLost => {
                if self.drag.take().is_some() {
                    self.hover.set_pressed(None, self.secs(now));
                    self.dirty = true;
                }
            }
            Event::TrayMenu(x, y) => self.on_tray_menu(x, y),
            Event::TaskbarCreated => {
                log::info!("taskbar created: re-adding the tray icon");
                if let Some(t) = &self.tray {
                    t.readd();
                }
            }
            Event::Fullscreen(on) => {
                log::info!("fullscreen {on}");
                self.fullscreen = on;
            }
        }
        self.apply_visibility();
    }

    fn on_tray_menu(&mut self, x: i32, y: i32) {
        let Some(tray) = &self.tray else { return };
        let picked = tray.show_menu(x, y, &self.config, !self.user_hidden);
        log::debug!("tray: {picked:?}");
        let now = Instant::now();
        match picked {
            Some(tray::Command::ToggleShown) => {
                self.user_hidden = !self.user_hidden;
                self.apply_visibility();
            }
            Some(tray::Command::Mode(m)) if m != self.mode() => {
                self.set_mode(m, now);
                self.retarget(now);
                self.save_config();
            }
            Some(tray::Command::ToggleStartup) => {
                let on = !self.config.start_with_windows;
                if self.config_path.is_some()
                    && let Err(e) = startup::set(on)
                {
                    log::warning!("start with Windows: {e}");
                    return;
                }
                self.config.start_with_windows = on;
                self.save_config();
            }
            Some(tray::Command::Quit) => unsafe { PostQuitMessage(0) },
            _ => {}
        }
    }

    fn set_mode(&mut self, m: DisplayMode, now: Instant) {
        self.config.display_mode = m;
        self.anim.set_hover(false, self.secs(now));
        self.reposition();
    }

    fn save_config(&self) {
        let Some(path) = &self.config_path else { return };
        if let Err(e) = std::fs::write(path, self.config.to_toml()) {
            log::warning!("could not write {}: {e}", path.display());
        }
    }

    /// Shows or hides the windows to match what should be visible.
    /// Over a fullscreen app only a blocked agent shows, and only collapsed.
    fn apply_visibility(&mut self) {
        let s = shown(self.user_hidden, self.fullscreen, &self.collapsed);
        if s != Shown::All {
            self.collapse();
        }
        let on = s != Shown::Hidden;
        if on != self.win.visible() {
            if !on {
                self.anim.pause();
            }
            self.win.set_visible(on);
            self.dirty = true;
        }
    }

    /// The expanded island's top-left once settled (DIPs); what clicks are tested against.
    fn expanded_origin(&self) -> Point {
        let r = island_rect(self.mode(), EXPANDED);
        Point { x: r.x, y: r.y }
    }

    fn expanded_hit(&mut self, p: Point) -> Option<ExpandedHit> {
        let o = self.expanded_origin();
        let present = self.clock.is_present(Instant::now());
        let tabs = tabs_visible(&self.session.agents, present);
        let rows = self.session.agents.rows(MAX_ROWS).len();
        let seekable = present && self.clock.seekable();
        hit_expanded(self.tab, tabs, rows, seekable, Point { x: p.x - o.x, y: p.y - o.y })
    }

    fn seek_fraction(&self, p: Point) -> f32 {
        ((p.x - self.expanded_origin().x - SEEK.x) / SEEK.w).clamp(0.0, 1.0)
    }

    pub fn on_mouse_move(&mut self, p: Point) {
        let t = self.secs(Instant::now());
        if self.drag.is_some() {
            self.on_drag_seek(self.seek_fraction(p), false);
            return;
        }
        self.kill_timer(TIMER_LEAVE);
        if self.expanded {
            let hot = self.expanded_hit(p).map(Hot::of);
            if hot != self.hover.hot {
                self.hover.set_hot(hot, t);
                self.dirty = true;
            }
        } else {
            // the input region is a rectangle-ish GDI region; only the drawn shape counts
            let mode = self.mode();
            let inside = hit_rounded(island_rect(mode, self.anim.size()), corner_radii(mode, self.anim.r.value), p);
            self.anim.set_hover(inside && self.collapsed == Collapsed::Media, t);
            self.dirty = true;
        }
    }

    pub fn on_mouse_leave(&mut self) {
        if self.drag.is_some() {
            return;
        }
        let t = self.secs(Instant::now());
        self.hover.set_hot(None, t);
        self.anim.set_hover(false, t);
        if self.expanded {
            self.set_timer(TIMER_LEAVE, LEAVE_COLLAPSE);
        }
        self.dirty = true;
    }

    pub fn on_mouse_down(&mut self, p: Point) {
        let now = Instant::now();
        if !self.expanded {
            let attention = matches!(self.collapsed, Collapsed::Blocked { .. } | Collapsed::Done { .. });
            match jump_target(&self.session.agents).filter(|_| attention) {
                Some(pane) => self.jump(&pane),
                None if self.fullscreen => {}
                None => self.expand(default_tab(&self.collapsed), true, now),
            }
            return;
        }
        let hit = self.expanded_hit(p);
        if let Some(ExpandedHit::Seek(f)) = hit {
            self.drag = Some(f);
            self.win.capture(true);
        }
        self.hover.set_pressed(hit.map(Hot::of), self.secs(now));
        self.dirty = true;
    }

    /// Mouse up: acts on what was pressed, if the cursor is still on it.
    pub fn on_click(&mut self, p: Point) {
        if self.drag.is_some() {
            self.on_drag_seek(self.seek_fraction(p), true);
            return;
        }
        let pressed = self.hover.pressed.take();
        self.hover.set_pressed(None, self.secs(Instant::now()));
        self.dirty = true;
        if !self.expanded {
            return;
        }
        let Some(hit) = self.expanded_hit(p) else { return };
        if pressed != Some(Hot::of(hit)) {
            return;
        }
        match hit {
            ExpandedHit::Tab(tab) => {
                self.tab = tab;
                self.hover.reset();
                self.retarget(Instant::now());
            }
            ExpandedHit::Row(i) => {
                let pane = self.session.agents.rows(MAX_ROWS).get(i).map(|a| a.info.pane_id.clone());
                if let Some(pane) = pane {
                    self.jump(&pane);
                }
            }
            ExpandedHit::Prev => self.media.command(MediaCommand::Previous),
            ExpandedHit::PlayPause => self.media.command(MediaCommand::PlayPause),
            ExpandedHit::Next => self.media.command(MediaCommand::Next),
            ExpandedHit::Seek(_) => {}
        }
    }

    pub fn on_key(&mut self, vk: u32) {
        if vk == VK_ESCAPE {
            self.collapse();
        }
    }

    pub fn on_deactivate(&mut self, now: Instant) {
        if self.expanded && self.expanded_at.is_some_and(|at| now.saturating_duration_since(at) > DEACTIVATE_ARM) {
            self.collapse();
        }
    }

    /// Seekbar drag: shows `fraction` while held; on release seeks there.
    pub fn on_drag_seek(&mut self, fraction: f32, released: bool) {
        self.dirty = true;
        if !released {
            self.drag = Some(fraction);
            return;
        }
        self.drag = None;
        self.win.capture(false);
        self.hover.set_pressed(None, self.secs(Instant::now()));
        let now = Instant::now();
        if let Some(snap) = self.clock.snapshot().filter(|_| self.clock.seekable()).cloned() {
            let pos = fraction as f64 * snap.duration_s;
            self.media.command(MediaCommand::Seek(pos));
            // show the new position until the source reports it
            self.clock.update(Some(island_core::media::MediaSnapshot { position_s: pos, ..snap }), now);
        }
    }

    /// §4.5: focus the pane, raise the terminal, collapse.
    fn jump(&mut self, pane: &str) {
        log::info!("jump to {pane}");
        let line = self.session.focus(pane);
        self.agent_src.send(&line);
        self.agent_src.raise_terminal();
        self.collapse();
    }

    fn expand(&mut self, tab: Tab, activate: bool, now: Instant) {
        self.expanded = true;
        self.expanded_at = Some(now);
        self.tab = tab;
        self.hover.reset();
        self.anim.set_hover(false, self.secs(now));
        if activate {
            self.win.activate();
        }
        self.retarget(now);
    }

    fn collapse(&mut self) {
        if !self.expanded {
            return;
        }
        self.expanded = false;
        self.expanded_at = None;
        if self.drag.take().is_some() {
            self.win.capture(false);
        }
        self.hover.reset();
        self.anim.set_hover(false, self.secs(Instant::now()));
        self.kill_timer(TIMER_LEAVE);
        self.win.deactivate();
        self.retarget(Instant::now());
    }

    fn reposition(&mut self) {
        let old = (self.win.origin, self.win.scale);
        self.win.place(self.mode());
        if (self.win.origin, self.win.scale) != old {
            // the swap chain and every device resource are sized for the old scale
            self.device = None;
            self.art_bmp = None;
        }
        self.dirty = true;
    }

    fn accent(&self) -> Option<Color> {
        let [r, g, b] = self.art.as_ref()?.accent?;
        Some(Color::rgba(r, g, b, 1.0))
    }

    /// Recomputes what the island should show and sets the animation targets.
    fn retarget(&mut self, now: Instant) {
        let t = self.secs(now);
        let present = self.clock.is_present(now);
        let collapsed = choose_collapsed(&self.session.agents, present);
        if collapsed != self.collapsed {
            log::debug!("collapsed {collapsed:?}");
            self.collapsed = collapsed;
        }
        if self.expanded && !tabs_visible(&self.session.agents, present) {
            if !self.session.agents.is_empty() {
                self.tab = Tab::Agents;
            } else if present {
                self.tab = Tab::Media;
            }
        }
        let (size, radius, face) = if self.expanded {
            (EXPANDED, EXPANDED_RADIUS, Face::Expanded(self.tab))
        } else {
            let face = match &self.collapsed {
                Collapsed::Idle => Face::Idle,
                Collapsed::Media => Face::Media,
                Collapsed::Working { count } => Face::Working { count: *count },
                Collapsed::Done { label, .. } => Face::Done { label: label.clone() },
                Collapsed::Blocked { label, .. } => Face::Blocked { label: label.clone() },
            };
            (collapsed_size(&self.collapsed), COLLAPSED_RADIUS, face)
        };
        self.anim.set_target(size, radius);
        self.anim.set_face(face, t);
        let accent = self.accent();
        let white = Color::white(1.0);
        let playing = present && self.clock.is_playing();
        let glow_on = playing && accent.is_some_and(|c| c != white);
        self.anim.set_glow(glow_on, accent.unwrap_or(white), t);
        self.anim.set_wave_color(accent.unwrap_or(white), t);
        match self.clock.linger_left(now) {
            Some(left) => self.set_timer(TIMER_LINGER, left + Duration::from_millis(1)),
            None => self.kill_timer(TIMER_LINGER),
        }
        self.dirty = true;
    }

    fn set_timer(&self, id: usize, after: Duration) {
        let ms = after.as_millis().clamp(1, u32::MAX as u128) as u32;
        unsafe { SetTimer(Some(self.win.visual), id, ms, None) };
    }

    fn kill_timer(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.win.visual), id);
        }
    }

    /// Nothing is drawn while hidden; showing again marks the island dirty.
    pub fn wants_frame(&self) -> bool {
        self.win.visible() && (self.dirty || self.animating)
    }

    /// The swap chain's frame-latency handle, once the device exists.
    pub fn waitable(&self) -> Option<HANDLE> {
        self.device.as_ref().map(|d| d.gfx.waitable())
    }

    /// Whether the content itself moves every frame (sound wave, blocked pulse).
    fn continuous(&self) -> bool {
        match self.anim.face {
            Face::Media | Face::Expanded(Tab::Media) => self.clock.is_playing(),
            Face::Blocked { .. } => true,
            _ => false,
        }
    }

    /// Draws and presents one frame; returns whether to keep animating.
    pub fn frame(&mut self, now: Instant) -> bool {
        let t = self.secs(now);
        self.anim.step(t);
        let mode = self.mode();
        let rect = island_rect(mode, self.anim.size());
        self.win.shape_input(rect, self.anim.r.value, mode);
        self.dirty = false;
        if self.device.is_none() {
            match Device::new(&self.win) {
                Ok(d) => self.device = Some(d),
                Err(e) => {
                    log::error!("graphics device: {e}; retrying in 1 s");
                    self.set_timer(TIMER_RETRY, Duration::from_secs(1));
                    self.animating = false;
                    return false;
                }
            }
        }
        if let Err(e) = self.render(now, t) {
            self.device = None;
            self.art_bmp = None;
            if device_lost(&e) {
                log::warning!("graphics device lost, rebuilding: {e}");
                self.dirty = true;
                return true;
            }
            // anything else would fail the same way on a fresh device: don't spin on it
            log::error!("frame failed: {e}; rebuilding the device in 1 s");
            self.set_timer(TIMER_RETRY, Duration::from_secs(1));
            self.animating = false;
            return false;
        }
        self.animating = self.anim.busy(t) || self.hover.busy(t) || self.continuous();
        let tick = !self.animating && self.anim.face == Face::Expanded(Tab::Agents);
        if tick != self.tick_on {
            self.tick_on = tick;
            if tick {
                self.set_timer(TIMER_TICK, Duration::from_secs(1));
            } else {
                self.kill_timer(TIMER_TICK);
            }
        }
        let spin = !self.animating && matches!(self.anim.face, Face::Working { .. });
        if spin != self.spinner_on {
            self.spinner_on = spin;
            if spin {
                self.set_timer(TIMER_SPINNER, Duration::from_millis(SPINNER_TICK_MS as u64));
            } else {
                self.kill_timer(TIMER_SPINNER);
            }
        }
        if !self.animating {
            self.anim.pause();
        }
        self.animating
    }

    fn render(&mut self, now: Instant, t: f64) -> Result<()> {
        let Some(dev) = self.device.as_mut() else { return Ok(()) };
        if self.art_bmp.is_none()
            && let Some(a) = &self.art
        {
            match dev.painter.bitmap(&a.bgra, a.width, a.height) {
                Ok(b) => self.art_bmp = Some(b),
                Err(e) => {
                    // bad art must not fail every frame: drop it and show the placeholder
                    log::warning!("album art unusable: {e}");
                    self.art = None;
                }
            }
        }
        let present = self.clock.is_present(now);
        let media = match self.clock.snapshot().filter(|_| present).cloned() {
            None => None,
            Some(snap) => {
                let pos = match self.drag {
                    Some(f) => f as f64 * snap.duration_s,
                    None => self.clock.position(now),
                };
                Some(ui_expanded::Media {
                    playing: self.clock.is_playing(),
                    seekable: self.clock.seekable(),
                    progress: self.drag.unwrap_or_else(|| self.clock.progress(now)),
                    position: clock_label(pos),
                    duration: clock_label(snap.duration_s),
                    title: snap.title,
                    artist: snap.artist,
                })
            }
        };
        let rows = self
            .session
            .agents
            .rows(MAX_ROWS)
            .into_iter()
            .map(|a| ui_expanded::Row {
                status: a.info.status,
                name: a.display_name().to_string(),
                title: a.info.title.clone().unwrap_or_default(),
                folder: a.folder().to_string(),
                since: time_in_state(now.saturating_duration_since(a.since)),
            })
            .collect();
        let (title, artist) = media.as_ref().map_or(("", ""), |m| (m.title.as_str(), m.artist.as_str()));
        let (title, artist) = (title.to_string(), artist.to_string());
        let collapsed = Scene {
            t,
            art: self.art_bmp.as_ref(),
            playing: self.clock.is_playing(),
            wave: self.anim.wave_color(t),
            hover: self.anim.hover(t),
            title: &title,
            artist: &artist,
        };
        let expanded = ui_expanded::Scene {
            t,
            tabs: tabs_visible(&self.session.agents, present),
            rows,
            media,
            art: self.art_bmp.as_ref(),
            accent: self.art.as_ref().and_then(|a| a.accent).map(|[r, g, b]| Color::rgba(r, g, b, 1.0)),
            hover: &self.hover,
        };
        dev.gfx.begin();
        dev.painter.begin_frame();
        let drawn = draw_island(dev, &self.anim, self.config.display_mode, t, &collapsed, &expanded);
        let presented = dev.gfx.end();
        drawn.and(presented)
    }
}

fn outset(r: Rect, by: f32) -> Rect {
    Rect { x: r.x - by, y: r.y - by, w: r.w + 2.0 * by, h: r.h + 2.0 * by }
}

fn draw_island(
    dev: &mut Device,
    anim: &Animator,
    mode: DisplayMode,
    t: f64,
    scene: &Scene,
    ex: &ui_expanded::Scene,
) -> Result<()> {
    let size = anim.size();
    let rect = island_rect(mode, Size { w: size.w.max(1.0), h: size.h.max(1.0) });
    let radii = corner_radii(mode, anim.r.value);
    let (glow, glow_color) = anim.glow(t);
    let key = Backdrop { rect, radii, glow, glow_color, mode };
    let p = &mut dev.painter;
    match &dev.backdrop {
        Some((k, bmp)) if *k == key => p.blit(bmp),
        _ => {
            let bmp = match dev.backdrop.take() {
                Some((_, bmp)) => bmp,
                None => p.layer_bitmap()?,
            };
            p.draw_into(&bmp, |p| draw_backdrop(p, &key))?;
            p.blit(&bmp);
            dev.backdrop = Some((key, bmp));
        }
    }

    let shape = p.shape(rect, radii)?;
    p.push_layer(Some(&shape), 1.0);
    if mode == DisplayMode::Attached {
        p.line(rect.x, rect.y + 0.5, rect.x + rect.w, rect.y + 0.5, Color::white(0.04), 1.0);
    }
    let (k_new, k_old) = anim.face_opacity(t);
    let mut drawn = Ok(());
    if let Some(old) = &anim.old {
        drawn = draw_face(p, old, &anim.face, rect, k_old, scene, ex);
    }
    let other = anim.old.as_ref().unwrap_or(&anim.face);
    let drawn = drawn.and_then(|_| draw_face(p, &anim.face, other, rect, k_new, scene, ex));
    p.pop_layer();
    drawn
}

/// One face at opacity `k`; `other` is the face it is fading against.
fn draw_face(
    p: &mut Painter,
    face: &Face,
    other: &Face,
    rect: Rect,
    k: f32,
    scene: &Scene,
    ex: &ui_expanded::Scene,
) -> Result<()> {
    let Face::Expanded(tab) = face else { return ui_collapsed::draw(p, face, rect, k, scene) };
    // centred on the island as it grows, top-aligned
    let (ox, oy) = (rect.x + rect.w / 2.0 - EXPANDED.w / 2.0, rect.y);
    // entering from (or leaving to) the collapsed island: scale 0.92→1; switching tabs: no scale
    let scale = if matches!(other, Face::Expanded(_)) { 1.0 } else { 0.92 + 0.08 * k };
    let restore = p.scale_about(scale, ox + EXPANDED.w / 2.0, oy + EXPANDED.h / 2.0);
    let drawn = ui_expanded::draw(p, *tab, ox, oy, k, ex);
    p.set_transform(restore);
    drawn
}

/// Glow (§6.5), shadow, and the black island.
fn draw_backdrop(p: &mut Painter, b: &Backdrop) -> Result<()> {
    let (rect, radii) = (b.rect, b.radii);
    let shape = p.shape(rect, radii)?;
    // the glow shows only outside the shape grown by 2 px
    if b.glow > 0.001 {
        let grown_radii = radii.map(|r| if r > 0.0 { r + 2.0 } else { 0.0 });
        let grown = p.shape(outset(rect, 2.0), grown_radii)?;
        let window = p.rect_geometry(Rect { x: -100.0, y: -100.0, w: WINDOW.w + 200.0, h: WINDOW.h + 200.0 })?;
        let mask = p.exclude(&window, &grown)?;
        match b.mode {
            DisplayMode::Pill => {
                p.push_layer(Some(&mask), 0.4 * b.glow);
                p.shadow(&grown, 0.0, 0.0, 15.0, b.glow_color)?;
                p.shadow(&grown, 0.0, 0.0, 40.0, b.glow_color)?;
            }
            DisplayMode::Attached => {
                let bottom = rect.y + rect.h + 2.0;
                let band = p.rect_geometry(Rect { x: rect.x - 2.0, y: bottom - 12.0, w: rect.w + 4.0, h: 12.0 })?;
                p.push_layer(Some(&mask), 0.5 * b.glow);
                p.shadow(&band, 0.0, 4.0, 15.0, b.glow_color)?;
                p.shadow(&band, 0.0, 8.0, 40.0, b.glow_color)?;
            }
        }
        p.pop_layer();
    }
    // CSS paints the first listed shadow on top
    match b.mode {
        DisplayMode::Pill => {
            p.shadow(&shape, 0.0, 0.0, 1.0, Color::white(0.05))?;
            p.shadow(&shape, 0.0, 2.0, 16.0, Color::BLACK.with_alpha(0.6))?;
        }
        DisplayMode::Attached => p.shadow(&shape, 0.0, 4.0, 20.0, Color::BLACK.with_alpha(0.5))?,
    }
    p.fill(&shape, Color::BLACK);
    Ok(())
}

/// The errors after which the device must be rebuilt (and rebuilding helps).
fn device_lost(e: &Error) -> bool {
    [D2DERR_RECREATE_TARGET, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET, DXGI_ERROR_DEVICE_HUNG].contains(&e.code())
}
