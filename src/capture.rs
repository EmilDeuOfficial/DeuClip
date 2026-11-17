//! Capture engine: FFmpeg as a child process (ddagrab), segment buffer and clip creation.
//!
//! FFmpeg continuously writes segments of about 1 second (MPEG-TS) into a temporary
//! folder. On a hotkey the latest segments are joined into an MP4 with the concat
//! demuxer, without re-encoding.

use crate::config::Quality;
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command as Proc, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use windows::core::PCWSTR;
use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// Extra retention time of the segments in seconds.
pub const RESERVE_SECS: u64 = 10;
const STARTUP_CHECK: Duration = Duration::from_millis(2200);

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MonitorInfo {
    /// Output index (output_idx for ddagrab).
    pub index: u32,
    /// Windows name, e.g. \\.\DISPLAY1
    pub name: String,
    /// Monitor name from EDID, e.g. "LG ULTRAGEAR". May be empty.
    pub friendly: String,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
}

impl MonitorInfo {
    /// Display name: the monitor's own name, otherwise the Windows display name.
    pub fn short_name(&self) -> String {
        if self.friendly.trim().is_empty() {
            self.name.trim_start_matches(r"\\.\").to_string()
        } else {
            self.friendly.trim().to_string()
        }
    }

    pub fn label(&self) -> String {
        let mut s = format!("{}  |  {}x{}", self.short_name(), self.width, self.height);
        if self.primary {
            s.push_str("  |  Primär");
        }
        s
    }
}

#[derive(Clone, Debug)]
pub struct Settings {
    pub ffmpeg: PathBuf,
    pub monitor_index: u32,
    pub fps: u32,
    pub scale_height: u32,
    pub quality: Quality,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecState {
    Stopped,
    Starting,
    Running { encoder: String },
    Error(String),
}

pub enum Command {
    /// Start (or restart) the capture.
    Start(Settings),
    Stop,
    /// Update the output folder and the retention time.
    Configure {
        output_dir: PathBuf,
        retention_secs: u64,
    },
    Save {
        slot: usize,
        duration: u32,
    },
    Shutdown,
}

pub enum Event {
    State(RecState),
    Saved {
        slot: usize,
        path: PathBuf,
        got_secs: u32,
        wanted_secs: u32,
        partial: bool,
    },
    SaveFailed {
        slot: usize,
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn proc(path: &Path) -> Proc {
    let mut c = Proc::new(path);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Finds ffmpeg.exe: configured path, next to the EXE, then PATH.
pub fn find_ffmpeg(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        if p.is_file() {
            return Some(p);
        }
        let q = p.join("ffmpeg.exe");
        if q.is_file() {
            return Some(q);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let p = dir.join("ffmpeg.exe");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let p = dir.join("ffmpeg.exe");
            if p.is_file() {
                return Some(p);
            }
        }
    }
    // FFmpeg installed via winget, in case the PATH has not been refreshed yet.
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let p = PathBuf::from(local)
            .join("Microsoft")
            .join("WinGet")
            .join("Links")
            .join("ffmpeg.exe");
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Lists the monitors of the first graphics adapter (same order as ddagrab).
pub fn list_monitors() -> Vec<MonitorInfo> {
    let mut result = Vec::new();
    let names = friendly_names();
    unsafe {
        let factory: IDXGIFactory1 = match CreateDXGIFactory1() {
            Ok(f) => f,
            Err(e) => {
                log::error!("DXGI factory not available: {e}");
                return fallback_monitors();
            }
        };
        let adapter = match factory.EnumAdapters1(0) {
            Ok(a) => a,
            Err(e) => {
                log::error!("No graphics adapter found: {e}");
                return fallback_monitors();
            }
        };
        let mut i = 0u32;
        while let Ok(output) = adapter.EnumOutputs(i) {
            if let Ok(desc) = output.GetDesc() {
                if desc.AttachedToDesktop.as_bool() {
                    let r = desc.DesktopCoordinates;
                    let len = desc
                        .DeviceName
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(desc.DeviceName.len());
                    let name = String::from_utf16_lossy(&desc.DeviceName[..len]);
                    let mut info = MONITORINFO {
                        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                        ..Default::default()
                    };
                    let primary = GetMonitorInfoW(desc.Monitor, &mut info).as_bool()
                        && (info.dwFlags & 1) != 0;
                    let friendly = names
                        .iter()
                        .find(|(gdi, _)| gdi.eq_ignore_ascii_case(&name))
                        .map(|(_, f)| f.clone())
                        .unwrap_or_default();
                    result.push(MonitorInfo {
                        index: i,
                        friendly,
                        name,
                        width: r.right - r.left,
                        height: r.bottom - r.top,
                        primary,
                    });
                }
            }
            i += 1;
        }
    }
    if result.is_empty() {
        return fallback_monitors();
    }
    result
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Returns pairs of Windows display name (\\.\DISPLAY1) and monitor name.
fn friendly_names() -> Vec<(String, String)> {
    let mut out = Vec::new();
    unsafe {
        let mut paths = 0u32;
        let mut modes = 0u32;
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut paths, &mut modes).0 != 0 {
            return out;
        }
        let mut path_buf = vec![DISPLAYCONFIG_PATH_INFO::default(); paths as usize];
        let mut mode_buf = vec![DISPLAYCONFIG_MODE_INFO::default(); modes as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut paths,
            path_buf.as_mut_ptr(),
            &mut modes,
            mode_buf.as_mut_ptr(),
            None,
        )
        .0 != 0
        {
            return out;
        }
        for path in path_buf.iter().take(paths as usize) {
            let mut source = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            source.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            source.header.adapterId = path.sourceInfo.adapterId;
            source.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut source.header) != 0 {
                continue;
            }
            let mut target = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            target.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            target.header.adapterId = path.targetInfo.adapterId;
            target.header.id = path.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut target.header) != 0 {
                continue;
            }
            out.push((
                wide_to_string(&source.viewGdiDeviceName),
                wide_to_string(&target.monitorFriendlyDeviceName),
            ));
        }
    }
    out
}

fn fallback_monitors() -> Vec<MonitorInfo> {
    vec![MonitorInfo {
        index: 0,
        name: "DISPLAY1".to_string(),
        friendly: String::new(),
        width: 0,
        height: 0,
        primary: true,
    }]
}

struct Probe {
    nvenc: bool,
    qsv: bool,
    amf: bool,
    ddagrab: bool,
}

fn run_text(ffmpeg: &Path, args: &[&str]) -> String {
    proc(ffmpeg)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn probe(ffmpeg: &Path) -> Probe {
    let encoders = run_text(ffmpeg, &["-hide_banner", "-encoders"]);
    let filters = run_text(ffmpeg, &["-hide_banner", "-filters"]);
    Probe {
        nvenc: encoders.contains("h264_nvenc"),
        qsv: encoders.contains("h264_qsv"),
        amf: encoders.contains("h264_amf"),
        ddagrab: filters.contains("ddagrab"),
    }
}

// ---------------------------------------------------------------------------
// Job object: FFmpeg is terminated if DeuClip crashes
// ---------------------------------------------------------------------------

struct Job(HANDLE);
unsafe impl Send for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn create_job() -> Option<Job> {
    unsafe {
        let handle = CreateJobObjectW(None, PCWSTR::null()).ok()?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok.is_err() {
            let _ = CloseHandle(handle);
            return None;
        }
        Some(Job(handle))
    }
}

fn assign_to_job(job: &Job, child: &Child) {
    unsafe {
        let h = HANDLE(child.as_raw_handle());
        if let Err(e) = AssignProcessToJobObject(job.0, h) {
            log::warn!("Could not assign FFmpeg to the job object: {e}");
        }
    }
}

// ---------------------------------------------------------------------------
// FFmpeg invocation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Enc {
    Nvenc,
    Qsv,
    Amf,
    X264,
}

impl Enc {
    fn name(self) -> &'static str {
        match self {
            Enc::Nvenc => "h264_nvenc",
            Enc::Qsv => "h264_qsv",
            Enc::Amf => "h264_amf",
            Enc::X264 => "libx264",
        }
    }
}

#[derive(Clone, Copy)]
struct Candidate {
    enc: Enc,
    /// true: frames stay on the GPU (only without scaling).
    direct: bool,
}

fn candidates(p: &Probe, scaled: bool) -> Vec<Candidate> {
    let mut list = Vec::new();
    if p.nvenc {
        if !scaled {
            list.push(Candidate { enc: Enc::Nvenc, direct: true });
        }
        list.push(Candidate { enc: Enc::Nvenc, direct: false });
    }
    if p.qsv {
        list.push(Candidate { enc: Enc::Qsv, direct: false });
    }
    if p.amf {
        if !scaled {
            list.push(Candidate { enc: Enc::Amf, direct: true });
        }
        list.push(Candidate { enc: Enc::Amf, direct: false });
    }
    list.push(Candidate { enc: Enc::X264, direct: false });
    list
}

fn build_args(s: &Settings, cand: Candidate, dir: &Path) -> Vec<String> {
    let q = s.quality.value().to_string();
    let fps = s.fps.max(1);
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| {
        for i in items {
            a.push((*i).to_string());
        }
    };
    push(&["-hide_banner", "-loglevel", "warning", "-f", "lavfi", "-i"]);
    let source = format!(
        "ddagrab=output_idx={}:framerate={}:draw_mouse=1",
        s.monitor_index, fps
    );
    a.push(source);

    if !cand.direct {
        let scale = if s.scale_height > 0 {
            format!("scale=-2:{}", s.scale_height)
        } else {
            "scale=trunc(iw/2)*2:trunc(ih/2)*2".to_string()
        };
        let pix = if cand.enc == Enc::X264 { "yuv420p" } else { "nv12" };
        a.push("-vf".to_string());
        a.push(format!("hwdownload,format=bgra,{scale},format={pix}"));
    }

    let mut push = |items: &[&str]| {
        for i in items {
            a.push((*i).to_string());
        }
    };
    match cand.enc {
        Enc::Nvenc => push(&[
            "-c:v", "h264_nvenc", "-preset", "p4", "-rc", "vbr", "-cq", &q, "-b:v", "0",
            "-maxrate", "100M", "-bufsize", "200M",
        ]),
        Enc::Qsv => push(&[
            "-c:v", "h264_qsv", "-preset", "veryfast", "-global_quality", &q,
        ]),
        Enc::Amf => push(&[
            "-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", &q, "-qp_p", &q,
            "-qp_b", &q,
        ]),
        Enc::X264 => push(&[
            "-c:v", "libx264", "-preset", "ultrafast", "-crf", &q, "-sc_threshold", "0",
        ]),
    }
    let gop = fps.to_string();
    push(&["-g", &gop, "-bf", "0", "-an"]);
    push(&[
        "-f", "segment", "-segment_time", "1", "-segment_format", "mpegts",
        "-reset_timestamps", "1", "-segment_start_number", "0",
    ]);
    a.push(dir.join("seg_%08d.ts").to_string_lossy().into_owned());
    a
}

// ---------------------------------------------------------------------------
// Capture process
// ---------------------------------------------------------------------------

struct Recorder {
    child: Child,
    stdin: Option<ChildStdin>,
    dir: PathBuf,
    encoder: String,
    started: Instant,
    empty_warned: bool,
    tail: Arc<Mutex<VecDeque<String>>>,
}

impl Recorder {
    fn tail_text(&self) -> String {
        tail_text(&self.tail)
    }

    /// Stops FFmpeg cleanly ("q") and removes the buffer.
    fn stop(mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn tail_text(tail: &Arc<Mutex<VecDeque<String>>>) -> String {
    tail.lock()
        .map(|t| t.iter().cloned().collect::<Vec<_>>().join(" | "))
        .unwrap_or_default()
}

fn spawn_ffmpeg(ffmpeg: &Path, args: &[String], dir: &Path, encoder: &str) -> Result<Recorder, String> {
    let mut cmd = proc(ffmpeg);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("FFmpeg konnte nicht gestartet werden: {e}"))?;
    let stdin = child.stdin.take();
    let tail: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
    if let Some(stderr) = child.stderr.take() {
        let tail = Arc::clone(&tail);
        let label = encoder.to_string();
        let _ = thread::Builder::new()
            .name("deuclip-ffmpeg-log".into())
            .spawn(move || {
                for line in BufReader::new(stderr).split(b'\n').map_while(Result::ok) {
                    let text = String::from_utf8_lossy(&line).trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    log::warn!("ffmpeg[{label}]: {text}");
                    if let Ok(mut t) = tail.lock() {
                        if t.len() >= 6 {
                            t.pop_front();
                        }
                        t.push_back(text);
                    }
                }
            });
    }
    Ok(Recorder {
        child,
        stdin,
        dir: dir.to_path_buf(),
        encoder: encoder.to_string(),
        started: Instant::now(),
        empty_warned: false,
        tail,
    })
}

fn temp_root() -> PathBuf {
    std::env::temp_dir().join("DeuClip")
}

/// All finished segments in ascending order. The newest one (still being written) is excluded.
fn finished_segments(dir: &Path) -> Vec<PathBuf> {
    let mut files = all_segments(dir);
    files.pop();
    files.retain(|p| fs::metadata(p).map(|m| m.len() > 0).unwrap_or(false));
    files
}

fn all_segments(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with("seg_") && n.ends_with(".ts"))
                    .unwrap_or(false)
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Saving clips
// ---------------------------------------------------------------------------

struct SavedClip {
    path: PathBuf,
    got_secs: u32,
    partial: bool,
}

fn save_clip(
    ffmpeg: &Path,
    buffer_dir: &Path,
    output_dir: &Path,
    slot: usize,
    duration: u32,
) -> Result<SavedClip, String> {
    let segments = finished_segments(buffer_dir);
    if segments.is_empty() {
        return Err("Der Puffer ist noch leer. Bitte kurz warten und erneut versuchen.".to_string());
    }
    let wanted = duration as usize;
    let partial = segments.len() < wanted;
    let take = segments.len().min(wanted);
    let chosen = &segments[segments.len() - take..];

    fs::create_dir_all(output_dir)
        .map_err(|e| format!("Ausgabeordner konnte nicht angelegt werden: {e}"))?;

    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut out = output_dir.join(format!("clip_{}_{}.mp4", stamp, slot + 1));
    let mut n = 2;
    while out.exists() {
        out = output_dir.join(format!("clip_{}_{}_{}.mp4", stamp, slot + 1, n));
        n += 1;
    }

    let mut list = String::new();
    for seg in chosen {
        let p = seg.to_string_lossy().replace('\\', "/").replace('\'', "'\\''");
        list.push_str(&format!("file '{p}'\n"));
    }
    let list_path = buffer_dir.join(format!("concat_{}_{}.txt", slot + 1, stamp));
    fs::write(&list_path, list).map_err(|e| format!("Concat-Liste nicht schreibbar: {e}"))?;

    let output = proc(ffmpeg)
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"])
        .arg(&list_path)
        .args(["-c", "copy", "-movflags", "+faststart"])
        .arg(&out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let _ = fs::remove_file(&list_path);

    match output {
        Ok(o) if o.status.success() => Ok(SavedClip {
            path: out,
            got_secs: take as u32,
            partial,
        }),
        Ok(o) => {
            let _ = fs::remove_file(&out);
            let err = String::from_utf8_lossy(&o.stderr);
            let last = err.lines().last().unwrap_or("unbekannter Fehler").to_string();
            Err(format!("Zusammenfuegen fehlgeschlagen: {last}"))
        }
        Err(e) => {
            let _ = fs::remove_file(&out);
            Err(format!("FFmpeg konnte nicht gestartet werden: {e}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Engine thread
// ---------------------------------------------------------------------------

pub struct Engine {
    tx: Sender<Command>,
    handle: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn spawn(events: Sender<Event>) -> Engine {
        let (tx, rx) = channel::<Command>();
        let handle = thread::Builder::new()
            .name("deuclip-capture".into())
            .spawn(move || {
                let mut mgr = Manager {
                    events,
                    rx,
                    rec: None,
                    settings: None,
                    output_dir: crate::config::default_output_dir(),
                    retention_secs: 30 + RESERVE_SECS,
                    job: create_job(),
                };
                mgr.run();
            })
            .ok();
        Engine { tx, handle }
    }

    pub fn sender(&self) -> Sender<Command> {
        self.tx.clone()
    }

    pub fn send(&self, cmd: Command) {
        if self.tx.send(cmd).is_err() {
            log::error!("Capture thread is no longer reachable");
        }
    }

    /// Stops FFmpeg and waits for the thread.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Manager {
    events: Sender<Event>,
    rx: Receiver<Command>,
    rec: Option<Recorder>,
    settings: Option<Settings>,
    output_dir: PathBuf,
    retention_secs: u64,
    job: Option<Job>,
}

impl Manager {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn state(&self, s: RecState) {
        self.emit(Event::State(s));
    }

    fn run(&mut self) {
        // Remove leftovers of earlier runs.
        let _ = fs::remove_dir_all(temp_root());
        let mut ticks = 0u32;
        loop {
            match self.rx.recv_timeout(Duration::from_millis(500)) {
                Ok(Command::Start(s)) => self.start(s),
                Ok(Command::Stop) => {
                    self.stop_recording();
                    self.state(RecState::Stopped);
                }
                Ok(Command::Configure { output_dir, retention_secs }) => {
                    self.output_dir = output_dir;
                    self.retention_secs = retention_secs;
                }
                Ok(Command::Save { slot, duration }) => self.save(slot, duration),
                Ok(Command::Shutdown) => {
                    self.stop_recording();
                    let _ = fs::remove_dir_all(temp_root());
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    ticks = ticks.wrapping_add(1);
                    self.tick(ticks);
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.stop_recording();
                    let _ = fs::remove_dir_all(temp_root());
                    break;
                }
            }
        }
    }

    fn stop_recording(&mut self) {
        if let Some(rec) = self.rec.take() {
            rec.stop();
        }
    }

    fn start(&mut self, s: Settings) {
        self.stop_recording();
        self.settings = Some(s.clone());
        self.state(RecState::Starting);

        let probe = probe(&s.ffmpeg);
        if !probe.ddagrab {
            let msg = "Diese FFmpeg-Version enthaelt den Filter ddagrab nicht. Bitte ein aktuelles Windows-Build (ab Version 6.0) verwenden.".to_string();
            log::error!("{msg}");
            self.state(RecState::Error(msg));
            return;
        }

        let dir = temp_root().join(format!("buf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        if let Err(e) = fs::create_dir_all(&dir) {
            let msg = format!("Temporaerer Ordner nicht anlegbar: {e}");
            log::error!("{msg}");
            self.state(RecState::Error(msg));
            return;
        }

        let mut last_error = String::from("Kein Encoder konnte gestartet werden.");
        for cand in candidates(&probe, s.scale_height > 0) {
            let label = if cand.direct {
                format!("{} (GPU direct)", cand.enc.name())
            } else {
                cand.enc.name().to_string()
            };
            let args = build_args(&s, cand, &dir);
            log::info!("Starting FFmpeg with {label}");
            let mut rec = match spawn_ffmpeg(&s.ffmpeg, &args, &dir, &label) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("{e}");
                    last_error = e;
                    break;
                }
            };
            if let Some(job) = &self.job {
                assign_to_job(job, &rec.child);
            }
            let deadline = Instant::now() + STARTUP_CHECK;
            let mut exited = false;
            while Instant::now() < deadline {
                if let Ok(Some(status)) = rec.child.try_wait() {
                    log::warn!("FFmpeg with {label} exits immediately: {status}");
                    exited = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if exited {
                thread::sleep(Duration::from_millis(200));
                let tail = rec.tail_text();
                last_error = if tail.is_empty() {
                    format!("{label} lieferte keine Ausgabe")
                } else {
                    tail
                };
                for seg in all_segments(&dir) {
                    let _ = fs::remove_file(seg);
                }
                continue;
            }
            log::info!("Capture running with {label}");
            let enc = rec.encoder.clone();
            self.rec = Some(rec);
            self.state(RecState::Running { encoder: enc });
            return;
        }

        let _ = fs::remove_dir_all(&dir);
        let msg = format!("Aufnahme konnte nicht gestartet werden: {last_error}");
        log::error!("{msg}");
        self.state(RecState::Error(msg));
    }

    fn tick(&mut self, ticks: u32) {
        let mut failure: Option<String> = None;
        if let Some(rec) = self.rec.as_mut() {
            match rec.child.try_wait() {
                Ok(Some(status)) => {
                    let tail = rec.tail_text();
                    failure = Some(format!("FFmpeg wurde unerwartet beendet ({status}). {tail}"));
                }
                Ok(None) => {
                    if !rec.empty_warned
                        && rec.started.elapsed() > Duration::from_secs(10)
                        && all_segments(&rec.dir).is_empty()
                    {
                        rec.empty_warned = true;
                        let tail = rec.tail_text();
                        log::error!("FFmpeg is running but writes no segments. {tail}");
                        failure = Some(format!("FFmpeg liefert keine Daten. {tail}"));
                    }
                }
                Err(e) => failure = Some(format!("FFmpeg-Status nicht lesbar: {e}")),
            }
        }
        if let Some(msg) = failure {
            log::error!("{msg}");
            self.stop_recording();
            self.state(RecState::Error(msg));
            return;
        }

        // Clean up old segments every 2 seconds.
        if ticks % 4 == 0 {
            if let Some(rec) = &self.rec {
                let max_age = Duration::from_secs(self.retention_secs);
                let now = SystemTime::now();
                for seg in all_segments(&rec.dir) {
                    let old = fs::metadata(&seg)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| now.duration_since(t).ok())
                        .map(|age| age > max_age)
                        .unwrap_or(false);
                    if old {
                        let _ = fs::remove_file(&seg);
                    }
                }
            }
        }
    }

    fn save(&mut self, slot: usize, duration: u32) {
        let (Some(rec), Some(settings)) = (self.rec.as_ref(), self.settings.as_ref()) else {
            self.emit(Event::SaveFailed {
                slot,
                message: "Die Aufnahme laeuft nicht. Clip kann nicht gespeichert werden.".to_string(),
            });
            return;
        };
        let ffmpeg = settings.ffmpeg.clone();
        let buffer_dir = rec.dir.clone();
        let output_dir = self.output_dir.clone();
        let events = self.events.clone();
        let spawned = thread::Builder::new()
            .name("deuclip-save".into())
            .spawn(move || {
                match save_clip(&ffmpeg, &buffer_dir, &output_dir, slot, duration) {
                    Ok(clip) => {
                        log::info!(
                            "Clip gespeichert: {} ({} s, partial={})",
                            clip.path.display(),
                            clip.got_secs,
                            clip.partial
                        );
                        let _ = events.send(Event::Saved {
                            slot,
                            path: clip.path,
                            got_secs: clip.got_secs,
                            wanted_secs: duration,
                            partial: clip.partial,
                        });
                    }
                    Err(message) => {
                        log::error!("Saving the clip failed: {message}");
                        let _ = events.send(Event::SaveFailed { slot, message });
                    }
                }
            });
        if let Err(e) = spawned {
            self.emit(Event::SaveFailed {
                slot,
                message: format!("Speicher-Thread konnte nicht gestartet werden: {e}"),
            });
        }
    }
}
