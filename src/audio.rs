//! Audio capture through WASAPI: system sound (loopback of the default output device) and
//! the default microphone. The raw samples are streamed to FFmpeg over a local TCP connection
//! (FFmpeg connects to this process), which encodes them into the same segments as the video.
//!
//! The capture follows the Windows default device: switching between speakers and a headset
//! moves the recording to the new device. Windows converts every device to one fixed format,
//! so FFmpeg does not have to be restarted.
//!
//! WASAPI loopback delivers no data while nothing is playing, so the stream is filled with
//! silence at real-time speed to keep audio and video in sync.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::{
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice,
    IMMDeviceEnumerator, MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Everything that plays on the default output device.
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
    /// Opens the device and a local listener. Returns an error text when the device is missing
    /// or access is denied (e.g. microphone blocked in the Windows privacy settings).
    pub fn start(source: Source) -> Result<AudioHandle, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (go_tx, go_rx) = channel::<PathBuf>();
        let (ready_tx, ready_rx) = channel::<Result<(AudioFormat, u16), String>>();
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name(format!("deuclip-audio-{}", source.name()))
            .spawn(move || run(source, thread_stop, go_rx, ready_tx))
            .map_err(|e| format!("Audio-Thread nicht startbar: {e}"))?;
        match ready_rx.recv_timeout(Duration::from_secs(4)) {
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

    /// FFmpeg is running: audio starts flowing as soon as this first segment file exists,
    /// which is when the video starts, so both streams begin together.
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

/// Sample format every device is converted to: 32 bit float, stereo, 48 kHz.
const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BLOCK_ALIGN: usize = 8;

struct Client {
    audio: IAudioClient,
    capture: IAudioCaptureClient,
    /// Windows id of the device that is being recorded.
    id: String,
}

unsafe fn default_device(source: Source) -> Result<IMMDevice, String> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("Audio-Dienst nicht verfügbar: {e}"))?;
    let flow = if source == Source::System { eRender } else { eCapture };
    enumerator
        .GetDefaultAudioEndpoint(flow, eConsole)
        .map_err(|e| format!("{}: kein Standardgerät gefunden ({e})", source.name()))
}

unsafe fn device_id(device: &IMMDevice) -> Option<String> {
    let raw = device.GetId().ok()?;
    let id = raw.to_string().ok();
    CoTaskMemFree(Some(raw.0 as *const _));
    id
}

unsafe fn open_client(source: Source) -> Result<Client, String> {
    let device = default_device(source)?;
    let id = device_id(&device).unwrap_or_default();
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
    Ok(Client { audio, capture, id })
}

/// Appends all packets that are ready. Returns the number of frames.
unsafe fn read_packets(client: &Client, out: &mut Vec<u8>) -> windows::core::Result<usize> {
    let mut total = 0usize;
    loop {
        let next = client.capture.GetNextPacketSize()?;
        if next == 0 {
            break;
        }
        let mut data: *mut u8 = std::ptr::null_mut();
        let mut frames = 0u32;
        let mut flags = 0u32;
        client.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
        let bytes = frames as usize * BLOCK_ALIGN;
        if data.is_null() || flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
            out.resize(out.len() + bytes, 0);
        } else {
            out.extend_from_slice(std::slice::from_raw_parts(data, bytes));
        }
        client.capture.ReleaseBuffer(frames)?;
        total += frames as usize;
    }
    Ok(total)
}

fn run(
    source: Source,
    stop: Arc<AtomicBool>,
    go_rx: std::sync::mpsc::Receiver<PathBuf>,
    ready_tx: Sender<Result<(AudioFormat, u16), String>>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let mut client = match open_client(source) {
            Ok(c) => c,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
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

        if let Err(e) = client.audio.Start() {
            log::warn!("{}: Start fehlgeschlagen: {e}", source.name());
            return;
        }

        let mut scratch: Vec<u8> = Vec::new();
        let rate = RATE as f64;
        let started = Instant::now();
        // Frames sent so far: silence is only added when the stream falls behind real time.
        let mut sent: u64 = 0;
        let mut device_ok = true;
        let mut check_at = Instant::now() + Duration::from_secs(1);
        let mut failure_logged = false;
        // Only for testing: reopen the device on every check.
        let reopen_always = std::env::var_os("DEUCLIP_TEST_AUDIO_REOPEN").is_some();
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
            let now = Instant::now();

            // Follow the Windows default device and recover from a lost one.
            if now >= check_at {
                check_at = now + Duration::from_secs(1);
                let current = default_device(source).ok().and_then(|d| device_id(&d));
                let changed = current.map(|id| id != client.id).unwrap_or(false);
                if changed || !device_ok || reopen_always {
                    let opened = open_client(source).and_then(|c| {
                        c.audio.Start().map_err(|e| format!("Start fehlgeschlagen: {e}"))?;
                        Ok(c)
                    });
                    match opened {
                        Ok(c) => {
                            let _ = client.audio.Stop();
                            client = c;
                            device_ok = true;
                            failure_logged = false;
                            log::info!("{}: recording the current default device", source.name());
                        }
                        Err(e) => {
                            if !failure_logged {
                                failure_logged = true;
                                log::warn!("{}: new device not usable yet: {e}", source.name());
                            }
                        }
                    }
                }
            }

            scratch.clear();
            let mut got = 0usize;
            if device_ok {
                match read_packets(&client, &mut scratch) {
                    Ok(n) => got = n,
                    Err(e) => {
                        log::warn!("{}: device lost: {e}", source.name());
                        device_ok = false;
                        scratch.clear();
                    }
                }
            }
            if got == 0 {
                // Nothing playing (or no device): silence keeps the timeline running.
                let expected = (started.elapsed().as_secs_f64() * rate) as u64;
                let behind = expected.saturating_sub(sent);
                if behind > (RATE / 10) as u64 {
                    let frames = behind - (RATE / 50) as u64;
                    scratch.resize(frames as usize * BLOCK_ALIGN, 0);
                }
            }
            if !scratch.is_empty() {
                if stream.write_all(&scratch).is_err() {
                    break;
                }
                sent += (scratch.len() / BLOCK_ALIGN) as u64;
            }
        }
        let _ = client.audio.Stop();
    }
}
