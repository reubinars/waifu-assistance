//! Event-driven SMTC adapter. WinRT work and commands stay on one MTA thread;
//! callbacks only enqueue a wake, and every registration is revoked on teardown.

use super::{ArtImage, MediaCommand, MediaSource, SourceEvent, UiSender};
use crate::log;
use island_core::color::dominant_color;
use island_core::media::{MediaSnapshot, Playback};
use island_core::smtc::{ArtFingerprint, ArtRetry, SessionSelection, Timeline, TrackTimeline, is_null_result};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionMediaProperties as Properties,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::core::{Error, Result};

enum Wake {
    Refresh,
    Session { id: u64, timeline: bool },
    Command(MediaCommand),
    Stop,
}

pub struct SmtcSource {
    tx: Option<Sender<Wake>>,
}

impl SmtcSource {
    pub fn new() -> Self {
        Self { tx: None }
    }
}

impl MediaSource for SmtcSource {
    fn start(&mut self, ui: UiSender) {
        if self.tx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.tx = Some(tx.clone());
        thread::spawn(move || worker(ui, tx, rx));
    }

    fn command(&self, command: MediaCommand) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Wake::Command(command));
        }
    }
}

impl Drop for SmtcSource {
    fn drop(&mut self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Wake::Stop);
        }
    }
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

fn worker(ui: UiSender, tx: Sender<Wake>, rx: Receiver<Wake>) {
    if let Err(e) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
        log::error!("SMTC MTA initialization failed: {e}");
        return;
    }
    let _apartment = Apartment;
    // IDs are never reused, including after manager recovery: late callbacks
    // from revoked sessions cannot publish into a newly selected session.
    let mut next_id = 0;
    loop {
        match run_manager(&ui, &tx, &rx, &mut next_id) {
            Ok(()) => return,
            Err(e) => {
                log::warning!("SMTC unavailable: {e}; retrying manager in 5 s");
                ui.send(SourceEvent::Media(None));
                ui.send(SourceEvent::Art(None));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(Wake::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                _ => {} // stale wakes / commands while the manager is unavailable
            }
        }
    }
}

struct ManagerEvents {
    manager: Manager,
    current: Option<i64>,
    sessions: Option<i64>,
}

impl ManagerEvents {
    fn new(manager: Manager, tx: &Sender<Wake>) -> Result<Self> {
        let mut events = Self { manager, current: None, sessions: None };
        let wake = tx.clone();
        events.current = Some(events.manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
            let _ = wake.send(Wake::Refresh);
            Ok(())
        }))?);
        let wake = tx.clone();
        events.sessions = Some(events.manager.SessionsChanged(&TypedEventHandler::new(move |_, _| {
            let _ = wake.send(Wake::Refresh);
            Ok(())
        }))?);
        Ok(events)
    }
}

impl Drop for ManagerEvents {
    fn drop(&mut self) {
        if let Some(token) = self.current {
            let _ = self.manager.RemoveCurrentSessionChanged(token);
        }
        if let Some(token) = self.sessions {
            let _ = self.manager.RemoveSessionsChanged(token);
        }
    }
}

struct SessionEvents {
    session: Session,
    id: u64,
    media: Option<i64>,
    playback: Option<i64>,
    timeline: Option<i64>,
    last_playback: Playback,
}

impl SessionEvents {
    fn new(session: Session, id: u64, tx: &Sender<Wake>) -> Result<Self> {
        let mut events =
            Self { session, id, media: None, playback: None, timeline: None, last_playback: Playback::Stopped };
        let wake = tx.clone();
        events.media = Some(events.session.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = wake.send(Wake::Session { id, timeline: false });
            Ok(())
        }))?);
        // Observe playback for every session so a newly playing background
        // player can take over even if Windows leaves CurrentSession unchanged.
        let wake = tx.clone();
        events.playback = Some(events.session.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            let _ = wake.send(Wake::Refresh);
            Ok(())
        }))?);
        let wake = tx.clone();
        events.timeline = Some(events.session.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = wake.send(Wake::Session { id, timeline: true });
            Ok(())
        }))?);
        Ok(events)
    }
}

impl Drop for SessionEvents {
    fn drop(&mut self) {
        if let Some(token) = self.media {
            let _ = self.session.RemoveMediaPropertiesChanged(token);
        }
        if let Some(token) = self.playback {
            let _ = self.session.RemovePlaybackInfoChanged(token);
        }
        if let Some(token) = self.timeline {
            let _ = self.session.RemoveTimelinePropertiesChanged(token);
        }
    }
}

fn playback(session: &Session) -> Result<Playback> {
    Ok(match session.GetPlaybackInfo()?.PlaybackStatus()? {
        Status::Playing => Playback::Playing,
        Status::Paused => Playback::Paused,
        _ => Playback::Stopped,
    })
}

fn run_manager(ui: &UiSender, tx: &Sender<Wake>, rx: &Receiver<Wake>, next_id: &mut u64) -> Result<()> {
    let manager = ManagerEvents::new(Manager::RequestAsync()?.join()?, tx)?;
    log::info!("SMTC manager connected");
    let mut sessions: Vec<SessionEvents> = Vec::new();
    let mut selection = SessionSelection::default();
    let mut selected = None;
    let mut track = None;
    let mut art_retry = ArtRetry::default();
    let mut timeline = TrackTimeline::default();
    let mut raw_timeline = None;
    let mut wake = Wake::Refresh;
    loop {
        let mut publish = false;
        let mut timeline_event = false;
        let mut metadata_event = false;
        let mut session_error = None;
        match wake {
            Wake::Stop => return Ok(()),
            Wake::Refresh => {
                let live = manager.manager.GetSessions()?;
                let live: Vec<_> = (0..live.Size()?).map(|i| live.GetAt(i)).collect::<Result<_>>()?;
                sessions.retain(|s| live.contains(&s.session));
                for session in live {
                    if !sessions.iter().any(|s| s.session == session) {
                        *next_id += 1;
                        sessions.push(SessionEvents::new(session, *next_id, tx)?);
                    }
                }
                // A null current session is normal when there is no player.
                let current = match manager.manager.GetCurrentSession() {
                    Ok(session) => Some(session),
                    Err(e) if is_null_result(e.code().0) => None,
                    Err(e) => return Err(e),
                };
                let current_id = sessions.iter().find(|s| Some(&s.session) == current.as_ref()).map(|s| s.id);
                let candidates: Vec<_> = sessions
                    .iter_mut()
                    .map(|s| {
                        match playback(&s.session) {
                            Ok(state) => s.last_playback = state,
                            Err(e) => log::warning!("SMTC session {} playback unavailable: {e}", s.id),
                        }
                        (s.id, s.last_playback == Playback::Playing)
                    })
                    .collect();
                log::debug!("SMTC sessions (id, playing): {candidates:?}, current: {current_id:?}");
                let chosen = selection.choose(&candidates, current_id);
                if chosen != selected {
                    selected = chosen;
                    track = None;
                    art_retry = ArtRetry::default();
                    timeline = TrackTimeline::default();
                    raw_timeline = None;
                    ui.send(SourceEvent::Media(None));
                    ui.send(SourceEvent::Art(None));
                    log::info!("SMTC selected session {selected:?}");
                }
                if sessions.is_empty() {
                    log::debug!("SMTC: no media sessions");
                }
                publish = selected.is_some();
            }
            Wake::Session { id, timeline: is_timeline } => {
                if selected == Some(id) {
                    publish = true;
                    timeline_event = is_timeline;
                    metadata_event = !is_timeline;
                }
            }
            Wake::Command(command) => {
                if let Some(s) = sessions.iter().find(|s| Some(s.id) == selected) {
                    let result = (|| -> Result<bool> {
                        Ok(match command {
                            MediaCommand::PlayPause => s.session.TryTogglePlayPauseAsync()?.join()?,
                            MediaCommand::Next => s.session.TrySkipNextAsync()?.join()?,
                            MediaCommand::Previous => s.session.TrySkipPreviousAsync()?.join()?,
                            MediaCommand::Seek(seconds) => {
                                if let Some(ticks) = raw_timeline.and_then(|t: Timeline| t.seek_ticks(seconds)) {
                                    s.session.TryChangePlaybackPositionAsync(ticks)?.join()?
                                } else {
                                    false
                                }
                            }
                        })
                    })();
                    match result {
                        Ok(accepted) => log::info!("SMTC command {command:?}: accepted={accepted}"),
                        Err(e) => session_error = Some(e),
                    }
                }
            }
        }
        if publish {
            let result = (|| -> Result<()> {
                let s = &sessions.iter().find(|s| Some(s.id) == selected).unwrap().session;
                let properties = s.TryGetMediaPropertiesAsync()?.join()?;
                let title = properties.Title()?.to_string();
                let artist = properties.Artist()?.to_string();
                let album = properties.AlbumTitle()?.to_string();
                let key = format!("{:?}", (&title, &artist, &album));
                let times = s.GetTimelineProperties()?;
                let raw =
                    Timeline::new(times.StartTime()?.Duration, times.EndTime()?.Duration, times.Position()?.Duration);
                let (position_s, duration_s) = timeline.update(key.clone(), raw, timeline_event);
                raw_timeline = (duration_s > 0.0).then_some(raw);
                let snap = MediaSnapshot {
                    title,
                    artist,
                    album,
                    playback: playback(s)?,
                    position_s,
                    duration_s,
                    source_app: s.SourceAppUserModelId()?.to_string(),
                };
                log::debug!("SMTC snapshot: {snap:?}");
                ui.send(SourceEvent::Media(Some(snap)));
                if track.as_ref() != Some(&key) {
                    track = Some(key.clone());
                    ui.send(SourceEvent::Art(None));
                }
                if art_retry.should_read(&key, metadata_event) {
                    let mut art = match read_art(&properties) {
                        Ok(art) => art,
                        Err(e) => {
                            log::warning!("SMTC album art unavailable: {e}");
                            None
                        }
                    };
                    let hash = art.as_ref().and_then(|image| {
                        let fingerprint = ArtFingerprint::new(&image.bgra);
                        log::debug!(
                            "SMTC album art decoded: hash={:016x}, opaque-pixel fraction={:.4}, visible={}",
                            fingerprint.hash,
                            fingerprint.opaque_fraction,
                            fingerprint.visible
                        );
                        fingerprint.visible.then_some(fingerprint.hash)
                    });
                    if hash.is_none() {
                        art = None;
                    }
                    if art_retry.should_publish(hash) {
                        log::debug!("SMTC album art published: {hash:?}");
                        ui.send(SourceEvent::Art(art));
                    }
                }
                Ok(())
            })();
            if let Err(e) = result {
                session_error = Some(e);
            }
        }
        if let Some(e) = session_error {
            log::warning!("SMTC session {selected:?} operation failed: {e}");
            // A broken player must not tear down other sessions or impose the
            // manager's 5 s backoff. Only membership loss clears existing UI.
            let live = manager.manager.GetSessions()?;
            let mut present = false;
            for i in 0..live.Size()? {
                let candidate = live.GetAt(i)?;
                present |= sessions.iter().any(|s| Some(s.id) == selected && s.session == candidate);
            }
            if selected.is_some() && !present {
                ui.send(SourceEvent::Media(None));
                ui.send(SourceEvent::Art(None));
                // Reconcile registrations and the selection through the usual
                // manager event path; do not loop on a still-present bad player.
                let _ = tx.send(Wake::Refresh);
            }
        }
        wake = match rx.recv() {
            Ok(wake) => wake,
            Err(_) => return Ok(()),
        };
    }
}

fn read_art(properties: &Properties) -> Result<Option<ArtImage>> {
    let thumbnail = match properties.Thumbnail() {
        Ok(thumbnail) => thumbnail,
        // a null thumbnail is normal for live streams
        Err(e) if is_null_result(e.code().0) => return Ok(None),
        Err(e) => return Err(e),
    };
    let stream = thumbnail.OpenReadAsync()?.join()?;
    let result = (|| {
        let size = stream.Size()?;
        // Art is a tiny UI asset; avoid an unbounded allocation from a player.
        if size == 0 {
            return Ok(None);
        }
        if size > 16 * 1024 * 1024 {
            return Err(Error::from_hresult(E_INVALIDARG));
        }
        let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
        let result = (|| {
            let loaded = reader.LoadAsync(size as u32)?.join()?;
            let mut bytes = vec![0; loaded as usize];
            reader.ReadBytes(&mut bytes)?;
            decode_art(&bytes).map(Some)
        })();
        let _ = reader.Close();
        result
    })();
    let _ = stream.Close();
    result
}

fn decode_art(bytes: &[u8]) -> Result<ArtImage> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        // bytes outlives decoder/converter/scaler: WIC holds the memory by reference.
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
        let frame = decoder.GetFrame(0)?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &frame,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(&converter, 140, 140, WICBitmapInterpolationModeFant)?;
        let mut bgra = vec![0; 140 * 140 * 4];
        scaler.CopyPixels(std::ptr::null(), 140 * 4, &mut bgra)?;
        let accent = dominant_color(&bgra, 140, 140);
        Ok(ArtImage { bgra, width: 140, height: 140, accent })
    }
}
