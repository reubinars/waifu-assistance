//! WSL bridge lifecycle and terminal activation. No pipe IO runs on the UI thread.

use super::{AgentSource, SourceEvent, UiSender};
use crate::log;
use island_core::config::Config;
use island_core::herdr::{Incoming, parse_line};
use island_core::platform::{BridgeBackoff, bridge_command, parse_wsl_list_verbose, should_run_bridge};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::windows::{io::AsRawHandle, process::CommandExt};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, Sender, SyncSender},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsIconic, IsWindowVisible, SW_RESTORE, SetForegroundWindow, ShowWindowAsync,
};
use windows::core::{BOOL, PCWSTR};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const WSL_CHECK: Duration = Duration::from_secs(15);
type Output = Arc<Mutex<Option<SyncSender<String>>>>;

enum Control {
    Stop,
    Exited(u64),
    PipeClosed(u64),
}

pub struct HerdrSource {
    config: Config,
    output: Output,
    control: Option<Sender<Control>>,
    focus: Sender<Option<isize>>,
}

impl HerdrSource {
    pub fn new(config: &Config) -> Self {
        let (focus, requests) = mpsc::channel::<Option<isize>>();
        let terminal_class = config.terminal_window_class.clone();
        thread::spawn(move || {
            for hwnd in requests {
                let Some(hwnd) = hwnd else {
                    log::info!("terminal window not found ({terminal_class})");
                    continue;
                };
                let hwnd = HWND(hwnd as _);
                unsafe {
                    if IsIconic(hwnd).as_bool() {
                        let _ = ShowWindowAsync(hwnd, SW_RESTORE);
                    }
                    let raised = SetForegroundWindow(hwnd).as_bool();
                    log::info!("terminal foreground: {raised}");
                }
            }
        });
        Self { config: config.clone(), output: Arc::new(Mutex::new(None)), control: None, focus }
    }
}

impl AgentSource for HerdrSource {
    fn start(&mut self, ui: UiSender) {
        if self.control.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.control = Some(tx.clone());
        let cfg = self.config.clone();
        let output = self.output.clone();
        thread::spawn(move || supervise(cfg, output, ui, tx, rx));
    }

    fn send(&self, line: &str) {
        // The lock protects only a sender swap. Neither the UI nor supervisor
        // holds it across pipe IO, waits, process launches or thread joins.
        let writer = self.output.lock().unwrap().clone();
        match writer.map(|tx| tx.try_send(line.to_owned())) {
            Some(Ok(())) => {}
            _ => log::debug!("bridge request dropped (offline or writer queue full)"),
        }
    }

    fn raise_terminal(&self) {
        struct Search {
            class: Vec<u16>,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, param: LPARAM) -> BOOL {
            let search = unsafe { &mut *(param.0 as *mut Search) };
            let mut class = [0u16; 256];
            let len = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
            if class[..len] == search.class && unsafe { IsWindowVisible(hwnd) }.as_bool() {
                search.found = Some(hwnd);
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut search = Search { class: self.config.terminal_window_class.encode_utf16().collect(), found: None };
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut _ as isize));
        }
        // Even restore/activation and diagnostic file IO cannot synchronously
        // wait on another thread while the island handles the user's click.
        let _ = self.focus.send(search.found.map(|hwnd| hwnd.0 as isize));
    }
}

impl Drop for HerdrSource {
    fn drop(&mut self) {
        self.output.lock().unwrap().take();
        if let Some(tx) = &self.control {
            let _ = tx.send(Control::Stop);
        }
    }
}

/// Closing the last job handle also kills our Windows children if island itself
/// is forcibly closed. It never terminates the WSL distro or the herdr server.
struct Job(HANDLE);
impl Job {
    fn new() -> io::Result<Self> {
        unsafe {
            let job = Self(CreateJobObjectW(None, PCWSTR::null()).map_err(io::Error::other)?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
            .map_err(io::Error::other)?;
            Ok(job)
        }
    }

    fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut child = command.creation_flags(CREATE_NO_WINDOW).spawn()?;
        if let Err(e) = unsafe { AssignProcessToJobObject(self.0, HANDLE(child.as_raw_handle())) } {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(e));
        }
        Ok(child)
    }

    fn terminate(&self) {
        unsafe {
            let _ = TerminateJobObject(self.0, 1);
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn wsl_running(cfg: &Config) -> io::Result<bool> {
    let job = Job::new()?;
    let output = job
        .spawn(
            Command::new("wsl.exe")
                .args(["-l", "-v"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped()),
        )?
        .wait_with_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("wsl -l -v exited {}", output.status)));
    }
    let running = should_run_bridge(&parse_wsl_list_verbose(&output.stdout), &cfg.wsl_distro);
    log::debug!("WSL configured distro running: {running}");
    Ok(running)
}

fn wait_delay(rx: &Receiver<Control>, delay: Duration) -> bool {
    let deadline = Instant::now() + delay;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Control::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return true,
            Err(mpsc::RecvTimeoutError::Timeout) => return false,
            _ => {} // notices from a previous child never shorten the timer
        }
    }
}

fn supervise(cfg: Config, output: Output, ui: UiSender, tx: Sender<Control>, rx: Receiver<Control>) {
    let mut retry = BridgeBackoff::default();
    let mut generation = 0;
    loop {
        if matches!(rx.try_recv(), Ok(Control::Stop)) {
            return;
        }
        match wsl_running(&cfg) {
            Ok(true) => {}
            result => {
                match result {
                    Ok(false) => log::info!("WSL unavailable; next check in 15 s"),
                    Err(e) => log::warning!("WSL check failed: {e}; next check in 15 s"),
                    _ => unreachable!(),
                }
                ui.send(SourceEvent::BridgeDown);
                if wait_delay(&rx, WSL_CHECK) {
                    return;
                }
                continue;
            }
        }
        generation += 1;
        let started = Instant::now();
        let stopped = match serve_bridge(&cfg, &output, &ui, &tx, &rx, generation) {
            Ok(stopped) => stopped,
            Err(e) => {
                log::warning!("bridge launch failed: {e}");
                false
            }
        };
        ui.send(SourceEvent::BridgeDown);
        log::info!("BridgeDown");
        if stopped {
            return;
        }
        retry.ran_for(started.elapsed());
        let delay = retry.failed();
        log::info!("bridge restart in {} s", delay.as_secs());
        if wait_delay(&rx, delay) {
            return;
        }
        // Always re-check the distro before launching, including on crash retries.
    }
}

fn drain(
    reader: impl Read + Send + 'static,
    ui: Option<UiSender>,
    tx: Sender<Control>,
    generation: u64,
) -> JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            let line = match line {
                Ok(line) => line,
                Err(e) => {
                    log::warning!("bridge pipe read: {e}");
                    break;
                }
            };
            if let Some(ui) = &ui {
                log::debug!("bridge -> {line}");
                match parse_line(&line) {
                    Ok(message) => {
                        if let Incoming::Bridge { connected } = &message {
                            log::info!("bridge connected: {connected}");
                        }
                        if let Incoming::AgentList { agents, .. } = &message {
                            log::info!("bridge agent list: {} agents", agents.len());
                        }
                        ui.send(SourceEvent::Herdr(message));
                    }
                    Err(e) => log::warning!("bad bridge line: {e}"),
                }
            } else {
                log::warning!("bridge stderr: {line}");
            }
        }
        log::debug!("bridge {} EOF", if ui.is_some() { "stdout" } else { "stderr" });
        if ui.is_some() {
            let _ = tx.send(Control::PipeClosed(generation));
        }
    })
}

fn serve_bridge(
    cfg: &Config,
    output: &Output,
    ui: &UiSender,
    tx: &Sender<Control>,
    rx: &Receiver<Control>,
    generation: u64,
) -> io::Result<bool> {
    let job = Job::new()?;
    let mut child = job.spawn(
        Command::new("wsl.exe")
            .args(bridge_command(cfg))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
    )?;
    log::info!("bridge spawned: wsl.exe pid {}", child.id());
    let mut stdin = child.stdin.take().unwrap();
    let (writer, requests) = mpsc::sync_channel::<String>(128);
    *output.lock().unwrap() = Some(writer);
    let wake = tx.clone();
    let write_thread = thread::spawn(move || {
        for line in requests {
            log::debug!("bridge <- {}", line.trim_end());
            let line = line.trim_end_matches(['\r', '\n']);
            if stdin
                .write_all(line.as_bytes())
                .and_then(|_| stdin.write_all(b"\n"))
                .and_then(|_| stdin.flush())
                .is_err()
            {
                let _ = wake.send(Control::PipeClosed(generation));
                break;
            }
        }
        // Dropping ChildStdin is essential: the Linux bridge exits on stdin EOF.
    });
    let read_thread = drain(child.stdout.take().unwrap(), Some(ui.clone()), tx.clone(), generation);
    let err_thread = drain(child.stderr.take().unwrap(), None, tx.clone(), generation);
    let handle = child.as_raw_handle() as isize;
    let wake = tx.clone();
    let monitor = thread::spawn(move || {
        let result = unsafe { WaitForSingleObject(HANDLE(handle as _), INFINITE) };
        if result != WAIT_OBJECT_0 {
            log::warning!("bridge process wait failed: {result:?}");
        }
        let _ = wake.send(Control::Exited(generation));
    });
    let stopped = loop {
        match rx.recv() {
            Ok(Control::Stop) | Err(_) => break true,
            Ok(Control::Exited(g) | Control::PipeClosed(g)) if g == generation => break false,
            _ => {}
        }
    };
    output.lock().unwrap().take();
    // Kill only this bridge's Windows job. Close both ends, reap wsl.exe, and
    // drain/join readers before BridgeDown so old lines cannot resurrect rows.
    job.terminate();
    let status = child.wait();
    log::info!("bridge reaped: {status:?}");
    for thread in [monitor, write_thread, read_thread, err_thread] {
        let _ = thread.join();
    }
    Ok(stopped)
}
