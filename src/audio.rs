//! Audio capture through WASAPI: system sound (loopback of all active output devices, or only
//! the default one) and the default microphone. The mixed samples are streamed to FFmpeg over
//! a local TCP connection (FFmpeg connects to this process), which encodes them into the same
//! segments as the video.
//!
//! The device list is checked every second, so new, removed or switched devices are picked up
//! without restarting FFmpeg. Windows converts every device to one fixed format.
//!
//! Every packet is placed on a common timeline by its capture time and the devices are added
//! up. WASAPI loopback delivers no data while nothing is playing, so gaps stay silent and the
//! stream runs at real-time speed to keep audio and video in sync.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::core::HSTRING;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, DEVICE_STATE_ACTIVE, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

/// Sample format every device is converted to: 32 bit float, stereo, 48 kHz.
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BLOCK_ALIGN: usize = 8;
/// The mix is sent this many frames behind real time, so that all packets have arrived.
const LATENCY: u64 = RATE as u64 / 10;
/// Packets that deviate less than this from the end of the previous one are joined seamlessly.
const SNAP: i64 = RATE as i64 / 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Everything that plays on the output devices.
    System,
    /// The default microphone.
    Microphone,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::System => "Systemton",
            Source::Microphone => "Mikrofon",
        }
    }
}

/// Sample format of a source, as FFmpeg needs it for a raw input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioFormat {
    pub ffmpeg: &'static str,
    pub rate: u32,
    pub channels: u16,
}

/// A running audio source. Dropping the handle without `stop` leaves the thread to end by itself
/// once FFmpeg closes the connection.
pub struct AudioHandle {
    pub format: AudioFormat,
    pub port: u16,
    stop: Arc<AtomicBool>,
    go: Sender<PathBuf>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for AudioHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl AudioHandle {
    /// Opens the device(s) and a local listener. With `all_devices` every active output device
    /// is recorded (system sound only). Returns an error text when no device can be opened,
    /// e.g. when the microphone is blocked in the Windows privacy settings.
    pub fn start(source: Source, all_devices: bool) -> Result<AudioHandle, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (go_tx, go_rx) = channel::<PathBuf>();
        let (ready_tx, ready_rx) = channel::<Result<(AudioFormat, u16), String>>();
        let thread_stop = Arc::clone(&stop);
        let all = all_devices && source == Source::System;
        let thread = thread::Builder::new()
            .name(format!("deuclip-audio-{}", source.name()))
            .spawn(move || run(source, all, thread_stop, go_rx, ready_tx))
            .map_err(|e| format!("Audio-Thread nicht startbar: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(6)) {
            Ok(Ok((format, port))) => Ok(AudioHandle {
                format,
                port,
                stop,
                go: go_tx,
                thread: Some(thread),
            }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("Das Audiogerät antwortet nicht.".to_string())
            }
        }
    }

    /// FFmpeg is running: it may now connect and receive the audio.
    pub fn go(&self, first_segment: PathBuf) {
        let _ = self.go.send(first_segment);
    }

    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// One recorded device.
struct Client {
    audio: IAudioClient,
    capture: IAudioCaptureClient,
    /// Windows id of the device.
    id: String,
    /// Timeline position right after the previous packet.
    next: Option<i64>,
}

/// Adds the packets of all devices up on one timeline.
struct Mixer {
    /// Interleaved stereo samples, starting at frame `sent`.
    pending: VecDeque<f32>,
    sent: u64,
}

impl Mixer {
    fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            sent: 0,
        }
    }

    /// Adds interleaved stereo samples that start at frame `position`.
    fn add(&mut self, position: i64, samples: &[f32]) {
        let offset = position - self.sent as i64;
        // Parts that are too old are dropped, implausible far future ones as well.
        let skip = (-offset).max(0) as usize * CHANNELS as usize;
        let start = offset.max(0) as usize;
        if skip >= samples.len() || start > RATE as usize * 5 {
            return;
        }
        let samples = &samples[skip..];
        let first = start * CHANNELS as usize;
        if self.pending.len() < first + samples.len() {
            self.pending.resize(first + samples.len(), 0.0);
        }
        for (i, s) in samples.iter().enumerate() {
            self.pending[first + i] += *s;
        }
    }

    /// Returns the bytes of all frames before `until`. Frames without data are silent.
    fn take(&mut self, until: u64, out: &mut Vec<u8>) {
        let frames = until.saturating_sub(self.sent);
        for _ in 0..frames * CHANNELS as u64 {
            let sample = self.pending.pop_front().unwrap_or(0.0).clamp(-1.0, 1.0);
            out.extend_from_slice(&sample.to_le_bytes());
        }
        self.sent += frames;
    }
}

/// Current time of the performance counter in units of 100 ns, as WASAPI reports it.
fn now_100ns() -> u64 {
    let mut counter = 0i64;
    let mut frequency = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    (counter as u128 * 10_000_000 / frequency.max(1) as u128) as u64
}

unsafe fn enumerator() -> Result<IMMDeviceEnumerator, String> {
    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("Audio-Dienst nicht verfügbar: {e}"))
}

unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    let raw = device.GetId().ok()?;
    let id = raw.to_string().ok();
    CoTaskMemFree(Some(raw.0 as *const _));
    id
}

/// Name of a device as Windows shows it, for the log.
unsafe fn device_name(id: &str) -> String {
    let name = enumerator()
        .ok()
        .and_then(|e| e.GetDevice(&HSTRING::from(id)).ok())
        .and_then(|d| d.OpenPropertyStore(STGM_READ).ok())
        .and_then(|s| s.GetValue(&PKEY_Device_FriendlyName).ok())
        .map(|v| v.to_string())
        .unwrap_or_default();
    if name.is_empty() {
        id.to_string()
    } else {
        name
    }
}

/// Ids of the devices that should be recorded right now.
unsafe fn wanted_ids(source: Source, all: bool) -> Result<Vec<String>, String> {
    let enumerator = enumerator()?;
    if all {
        let devices = enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|e| format!("{}: Geräteliste nicht lesbar ({e})", source.name()))?;
        let count = devices
            .GetCount()
            .map_err(|e| format!("{}: Geräteliste nicht lesbar ({e})", source.name()))?;
        Ok((0..count)
            .filter_map(|i| devices.Item(i).ok())
            .filter_map(|d| device_id(&d))
            .collect())
    } else {
        let flow = if source == Source::System { eRender } else { eCapture };
        let device = enumerator
            .GetDefaultAudioEndpoint(flow, eConsole)
            .map_err(|e| format!("{}: kein Standardgerät gefunden ({e})", source.name()))?;
        Ok(device_id(&device).into_iter().collect())
    }
}

unsafe fn open_client(source: Source, id: &str) -> Result<Client, String> {
    let device = enumerator()?
        .GetDevice(&HSTRING::from(id))
        .map_err(|e| format!("{}: Gerät nicht gefunden ({e})", source.name()))?;
    let audio: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("{}: Gerät nicht nutzbar ({e})", source.name()))?;
    let format = WAVEFORMATEX {
        wFormatTag: 3, // IEEE float
        nChannels: CHANNELS,
        nSamplesPerSec: RATE,
        nAvgBytesPerSec: RATE * BLOCK_ALIGN as u32,
        nBlockAlign: BLOCK_ALIGN as u16,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if source == Source::System {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    audio
        .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, &format, None)
        .map_err(|e| format!("{}: Zugriff nicht möglich ({e})", source.name()))?;
    let capture: IAudioCaptureClient = audio
        .GetService()
        .map_err(|e| format!("{}: Aufnahme nicht startbar ({e})", source.name()))?;
    Ok(Client {
        audio,
        capture,
        id: id.to_string(),
        next: None,
    })
}

/// Opens new devices and closes removed ones. Returns the first error if nothing could be opened.
/// With `running` the new devices are started immediately.
unsafe fn refresh(
    source: Source,
    all: bool,
    running: bool,
    clients: &mut Vec<Client>,
    failed: &mut HashSet<String>,
) -> Result<(), String> {
    let wanted = wanted_ids(source, all)?;
    clients.retain(|c| {
        let keep = wanted.contains(&c.id);
        if !keep {
            let _ = c.audio.Stop();
            log::info!("{}: device removed from the recording", source.name());
        }
        keep
    });
    let mut first_error = None;
    for id in &wanted {
        if clients.iter().any(|c| &c.id == id) {
            continue;
        }
        let opened = open_client(source, id).and_then(|c| {
            if running {
                c.audio.Start().map_err(|e| format!("Start fehlgeschlagen: {e}"))?;
            }
            Ok(c)
        });
        match opened {
            Ok(c) => {
                failed.remove(id);
                clients.push(c);
                log::info!(
                    "{}: recording \"{}\" ({} device(s))",
                    source.name(),
                    device_name(id),
                    clients.len()
                );
            }
            Err(e) => {
                // Some virtual devices cannot be opened, report each only once.
                if failed.insert(id.clone()) {
                    log::warn!("{}: \"{}\" not usable: {e}", source.name(), device_name(id));
                }
                first_error.get_or_insert(e);
            }
        }
    }
    match first_error {
        Some(e) if clients.is_empty() => Err(e),
        _ => Ok(()),
    }
}

/// Reads all packets that are ready and puts them on the timeline that starts at `start`.
unsafe fn drain(client: &mut Client, start: u64, mixer: &mut Mixer, scratch: &mut Vec<f32>) -> windows::core::Result<()> {
    loop {
        if client.capture.GetNextPacketSize()? == 0 {
            return Ok(());
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        let mut captured_at = 0u64;
        client
            .capture
            .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut captured_at))?;
        let by_clock = (captured_at as i128 - start as i128) * RATE as i128 / 10_000_000;
        let by_clock = by_clock as i64;
        let timestamp_bad = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
        let position = match client.next {
            Some(next) if timestamp_bad || (next - by_clock).abs() < SNAP => next,
            _ => by_clock,
        };
        if !data.is_null() && flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 {
            let bytes = std::slice::from_raw_parts(data, frames as usize * BLOCK_ALIGN);
            scratch.clear();
            scratch.extend(
                bytes
                    .chunks_exact(4)
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            );
            mixer.add(position, scratch);
        }
        client.next = Some(position + frames as i64);
        client.capture.ReleaseBuffer(frames)?;
    }
}

fn run(
    source: Source,
    all: bool,
    stop: Arc<AtomicBool>,
    go_rx: std::sync::mpsc::Receiver<PathBuf>,
    ready_tx: Sender<Result<(AudioFormat, u16), String>>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut clients: Vec<Client> = Vec::new();
        let mut failed: HashSet<String> = HashSet::new();
        if let Err(e) = refresh(source, all, false, &mut clients, &mut failed) {
            let _ = ready_tx.send(Err(e));
            return;
        }
        if clients.is_empty() {
            let _ = ready_tx.send(Err(format!("{}: kein Gerät gefunden", source.name())));
            return;
        }
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(l) => l,
            Err(e) => {
                let _ = ready_tx.send(Err(format!("Lokaler Port nicht verfügbar: {e}")));
                return;
            }
        };
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
        let format = AudioFormat {
            ffmpeg: "f32le",
            rate: RATE,
            channels: CHANNELS,
        };
        let _ = ready_tx.send(Ok((format, port)));

        // Wait until FFmpeg is running.
        let _first_segment = loop {
            match go_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(p) => break p,
                Err(RecvTimeoutError::Timeout) => {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        };

        // FFmpeg connects to us.
        let _ = listener.set_nonblocking(true);
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut stream = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if stop.load(Ordering::SeqCst) || Instant::now() > deadline {
                        return;
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(_) => return,
            }
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

        clients.retain(|c| match c.audio.Start() {
            Ok(()) => true,
            Err(e) => {
                log::warn!("{}: start failed: {e}", source.name());
                false
            }
        });

        let start = now_100ns();
        let mut mixer = Mixer::new();
        let mut samples: Vec<f32> = Vec::new();
        let mut output: Vec<u8> = Vec::new();
        let mut check_at = Instant::now() + Duration::from_secs(1);
        // Only for testing: reopen all devices on every check.
        let reopen_always = std::env::var_os("DEUCLIP_TEST_AUDIO_REOPEN").is_some();
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));

            if Instant::now() >= check_at {
                check_at = Instant::now() + Duration::from_secs(1);
                if reopen_always {
                    for c in clients.drain(..) {
                        let _ = c.audio.Stop();
                    }
                }
                if let Err(e) = refresh(source, all, true, &mut clients, &mut failed) {
                    log::debug!("{e}");
                }
            }

            // A device that reports an error is closed and opened again by the next check.
            clients.retain_mut(|c| match drain(c, start, &mut mixer, &mut samples) {
                Ok(()) => true,
                Err(e) => {
                    log::warn!("{}: device lost: {e}", source.name());
                    let _ = c.audio.Stop();
                    false
                }
            });

            let elapsed = now_100ns().saturating_sub(start) as u128 * RATE as u128 / 10_000_000;
            output.clear();
            mixer.take((elapsed as u64).saturating_sub(LATENCY), &mut output);
            if !output.is_empty() && stream.write_all(&output).is_err() {
                break;
            }
        }
        for c in &clients {
            let _ = c.audio.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect()
    }

    #[test]
    fn mixes_devices_and_fills_gaps() {
        let mut mixer = Mixer::new();
        // Two devices overlap in frame 1, frame 2 has no data.
        mixer.add(0, &[0.25, 0.25, 0.5, 0.5]);
        mixer.add(1, &[0.25, -0.25]);
        let mut out = Vec::new();
        mixer.take(3, &mut out);
        assert_eq!(samples(&out), vec![0.25, 0.25, 0.75, 0.25, 0.0, 0.0]);
    }

    #[test]
    fn drops_old_samples_and_limits_the_level() {
        let mut mixer = Mixer::new();
        let mut out = Vec::new();
        mixer.take(1, &mut out);
        // Frame 0 is already sent, only frame 1 remains. Two loud sources are clipped.
        mixer.add(0, &[0.9, 0.9, 0.8, -0.8]);
        mixer.add(1, &[0.8, -0.8]);
        out.clear();
        mixer.take(2, &mut out);
        assert_eq!(samples(&out), vec![1.0, -1.0]);
    }
}
