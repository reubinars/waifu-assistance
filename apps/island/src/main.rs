#![windows_subsystem = "windows"]

#[cfg(windows)]
mod animator;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod draw;
#[cfg(windows)]
mod fullscreen;
#[cfg(windows)]
mod gfx;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod pet;
#[cfg(windows)]
mod sources;
#[cfg(windows)]
mod startup;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod ui_collapsed;
#[cfg(windows)]
mod ui_expanded;
#[cfg(windows)]
mod window;

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    if let Err(e) = run() {
        log::error!("fatal: {e}");
    }
}

#[cfg(windows)]
fn load_config(dir: &std::path::Path) -> island_core::config::Config {
    use island_core::config::Config;
    let path = dir.join("config.toml");
    match std::fs::read_to_string(&path) {
        Ok(s) => Config::parse(&s).unwrap_or_else(|e| {
            log::warning!("config.toml is invalid, using defaults: {e}");
            Config::default()
        }),
        Err(_) => {
            let c = Config::default();
            if let Err(e) = std::fs::write(&path, c.to_toml()) {
                log::warning!("could not write {}: {e}", path.display());
            }
            c
        }
    }
}

#[cfg(windows)]
fn run() -> windows::core::Result<()> {
    use island_core::media::MediaClock;
    use sources::demo::{DEMO_LINGER, DemoAgents, DemoMedia};
    use sources::herdr::HerdrSource;
    use sources::smtc::SmtcSource;
    use sources::{AgentSource, MediaSource, UiSender};
    use std::sync::mpsc;
    use std::time::Instant;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{CreateMutexW, INFINITE};
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
        TranslateMessage, WM_QUIT, WaitMessage,
    };
    use windows::core::w;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = args.iter().any(|a| a == "--demo");
    let debug = args.iter().any(|a| a == "--debug");

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    // held for the life of the process
    let _instance = unsafe { CreateMutexW(None, true, w!("Local\\island-single-instance"))? };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        return Ok(());
    }

    let dir = log::data_dir();
    log::init(&dir, debug);
    log::info!("island {} starting (demo: {demo})", env!("CARGO_PKG_VERSION"));
    let config = load_config(&dir);
    if !demo {
        startup::sync(config.start_with_windows);
    }

    let win = window::create(config.display_mode, config.spot())?;
    let visual = win.visual;
    let (tx, rx) = mpsc::channel();
    let ui = UiSender::new(tx, win.visual);
    let (clock, media, agents): (MediaClock, Box<dyn MediaSource>, Box<dyn AgentSource>) = if demo {
        (MediaClock::with_linger(DEMO_LINGER), Box::new(DemoMedia::default()), Box::new(DemoAgents))
    } else {
        (MediaClock::default(), Box::new(SmtcSource::new()), Box::new(HerdrSource::new(&config)))
    };
    let config_path = (!demo).then(|| dir.join("config.toml"));
    let mut app = app::App::new(config, config_path, win, clock, media, agents);
    app.start_sources(ui);
    // keeps the hooks alive for the life of the loop
    let _fullscreen = fullscreen::install(visual);

    let mut msg = MSG::default();
    loop {
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    log::info!("quit");
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for e in window::take_events() {
            app.on_window_event(e);
        }
        while let Ok(e) = rx.try_recv() {
            app.on_source_event(e);
        }
        if !app.wants_frame() {
            unsafe { WaitMessage()? };
            continue;
        }
        match app.waitable() {
            // no device yet: building it is part of the frame
            None => {
                app.frame(Instant::now());
            }
            Some(h) => {
                let handles = [h];
                let woke = unsafe {
                    MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE)
                };
                // the waitable is a semaphore: once it fires, a frame must be presented
                if woke == WAIT_OBJECT_0 {
                    app.frame(Instant::now());
                }
            }
        }
    }
}
