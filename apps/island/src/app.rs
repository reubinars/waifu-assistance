//! The island's state and its frame: what to show, the animator, and drawing it.

use crate::animator::{Animator, Face};
use crate::draw::{Color, Painter};
use crate::gfx::Gfx;
use crate::log;
use crate::sources::{AgentSource, ArtImage, MediaSource, SourceEvent, UiSender};
use crate::ui_collapsed::{self, SPINNER_TICK_MS, Scene};
use crate::window::{Event, Windows};
use island_core::config::Config;
use island_core::media::MediaClock;
use island_core::session::HerdrSession;
use island_core::view::{
    Collapsed, DisplayMode, EXPANDED, Tab, WINDOW, choose_collapsed, collapsed_size, corner_radii, island_rect,
};
use island_core::{Rect, Size};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap1;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::Result;

const COLLAPSED_RADIUS: f32 = 20.0;
const EXPANDED_RADIUS: f32 = 32.0;

pub const TIMER_LINGER: usize = 1;
pub const TIMER_SPINNER: usize = 2;
pub const TIMER_RETRY: usize = 3;

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
    tab: Tab,
    #[allow(dead_code)]
    hovered: bool,
    anim: Animator,
    device: Option<Device>,
    media: Box<dyn MediaSource>,
    agent_src: Box<dyn AgentSource>,
    t0: Instant,
    dirty: bool,
    animating: bool,
    spinner_on: bool,
}

impl App {
    pub fn new(
        config: Config,
        win: Windows,
        clock: MediaClock,
        media: Box<dyn MediaSource>,
        agent_src: Box<dyn AgentSource>,
    ) -> App {
        let collapsed = Collapsed::Idle;
        let anim = Animator::new(collapsed_size(&collapsed), COLLAPSED_RADIUS, Face::Idle);
        App {
            config,
            win,
            session: HerdrSession::default(),
            clock,
            art: None,
            art_bmp: None,
            collapsed,
            expanded: false,
            tab: Tab::Media,
            hovered: false,
            anim,
            device: None,
            media,
            agent_src,
            t0: Instant::now(),
            dirty: true,
            animating: false,
            spinner_on: false,
        }
    }

    pub fn start_sources(&mut self, tx: UiSender) {
        self.media.start(tx.clone());
        self.agent_src.start(tx);
    }

    fn secs(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.t0).as_secs_f32()
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
        }
        self.retarget(now);
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
            Event::Timer(_) => {}
            Event::Reposition => self.reposition(),
            Event::MouseMove(..) => self.hovered = true,
            Event::MouseLeave => self.hovered = false,
            Event::MouseDown(..) | Event::MouseUp(..) | Event::Key(_) | Event::Activate(_) => {}
        }
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

    pub fn wants_frame(&self) -> bool {
        self.dirty || self.animating
    }

    /// The swap chain's frame-latency handle, once the device exists.
    pub fn waitable(&self) -> Option<HANDLE> {
        self.device.as_ref().map(|d| d.gfx.waitable())
    }

    /// Whether the content itself moves every frame (sound wave, blocked pulse).
    fn continuous(&self) -> bool {
        match self.anim.face {
            Face::Media => self.clock.is_playing(),
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
        if let Err(e) = self.render(t) {
            log::warning!("frame failed, rebuilding the device: {e}");
            self.device = None;
            self.art_bmp = None;
            self.dirty = true;
            return true;
        }
        self.animating = self.anim.busy(t) || self.continuous();
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

    fn render(&mut self, t: f32) -> Result<()> {
        let Some(dev) = self.device.as_mut() else { return Ok(()) };
        if self.art_bmp.is_none()
            && let Some(a) = &self.art
        {
            self.art_bmp = Some(dev.painter.bitmap(&a.bgra, a.width, a.height)?);
        }
        dev.gfx.begin();
        dev.painter.begin_frame();
        let scene = Scene {
            t,
            art: self.art_bmp.as_ref(),
            playing: self.clock.is_playing(),
            wave: self.anim.wave_color(t),
        };
        let drawn = draw_island(dev, &self.anim, self.config.display_mode, t, &scene);
        let presented = dev.gfx.end();
        drawn.and(presented)
    }
}

fn outset(r: Rect, by: f32) -> Rect {
    Rect { x: r.x - by, y: r.y - by, w: r.w + 2.0 * by, h: r.h + 2.0 * by }
}

fn draw_island(dev: &mut Device, anim: &Animator, mode: DisplayMode, t: f32, scene: &Scene) -> Result<()> {
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
        drawn = ui_collapsed::draw(p, old, rect, k_old, scene);
    }
    let drawn = drawn.and_then(|_| ui_collapsed::draw(p, &anim.face, rect, k_new, scene));
    p.pop_layer();
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
