//! Audio capture through WASAPI: system sound (loopback of the default output device) and
//! the default microphone. The raw samples are streamed to FFmpeg over a local TCP connection
//! (FFmpeg connects to this process), which encodes them into the same segments as the video.
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
    eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
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

struct Client {
    audio: IAudioClient,
    capture: IAudioCaptureClient,
    format: AudioFormat,
    block_align: usize,
}

unsafe fn open_client(source: Source) -> Result<Client, String> {
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        .map_err(|e| format!("Audio-Dienst nicht verfügbar: {e}"))?;
    let flow = if source == Source::System { eRender } else { eCapture };
    let device = enumerator
        .GetDefaultAudioEndpoint(flow, eConsole)
        .map_err(|e| format!("{}: kein Standardgerät gefunden ({e})", source.name()))?;
    let audio: IAudioClient = device
        .Activate(CLSCTX_ALL, None)
        .map_err(|e| format!("{}: Gerät nicht nutzbar ({e})", source.name()))?;
    let pfmt = audio
        .GetMixFormat()
        .map_err(|e| format!("{}: Format nicht lesbar ({e})", source.name()))?;
    let fmt: WAVEFORMATEX = std::ptr::read_unaligned(pfmt);
    let (tag, bits, channels, rate, align) = (
        fmt.wFormatTag,
        fmt.wBitsPerSample,
        fmt.nChannels,
        fmt.nSamplesPerSec,
        fmt.nBlockAlign as usize,
    );
    let is_float = tag == 3
        || (tag == 0xFFFE && {
            let ext: WAVEFORMATEXTENSIBLE = std::ptr::read_unaligned(pfmt as *const WAVEFORMATEXTENSIBLE);
            ext.SubFormat.data1 == 3
        });
    let ffmpeg = match (is_float, bits) {
        (true, 32) => "f32le",
        (false, 16) => "s16le",
        (false, 32) => "s32le",
        _ => {
            CoTaskMemFree(Some(pfmt as *const _));
            return Err(format!(
                "{}: Format wird nicht unterstützt ({bits} Bit)",
                source.name()
            ));
        }
    };
    let flags = if source == Source::System { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    let init = audio.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, pfmt, None);
    CoTaskMemFree(Some(pfmt as *const _));
    init.map_err(|e| format!("{}: Zugriff nicht möglich ({e})", source.name()))?;
    let capture: IAudioCaptureClient = audio
        .GetService()
        .map_err(|e| format!("{}: Aufnahme nicht startbar ({e})", source.name()))?;
    Ok(Client {
        audio,
        capture,
        format: AudioFormat { ffmpeg, rate, channels },
        block_align: align,
    })
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
        let bytes = frames as usize * client.block_align;
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
        let _ = ready_tx.send(Ok((client.format.clone(), port)));

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

        let rate = client.format.rate as f64;
        let mut last = Instant::now();
        let mut device_ok = true;
        let mut retry_at = Instant::now();
        while !stop.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(10));
            let now = Instant::now();
            let elapsed = now.duration_since(last);
            last = now;

            scratch.clear();
            let mut got = 0usize;
            if device_ok {
                match read_packets(&client, &mut scratch) {
                    Ok(n) => got = n,
                    Err(e) => {
                        log::warn!("{}: Gerät verloren: {e}", source.name());
                        device_ok = false;
                        retry_at = now + Duration::from_secs(2);
                        scratch.clear();
                    }
                }
            } else if now >= retry_at {
                // E.g. the default output device was switched: try the new one.
                match open_client(source) {
                    Ok(c) if c.format == client.format => {
                        if c.audio.Start().is_ok() {
                            let _ = client.audio.Stop();
                            client = c;
                            device_ok = true;
                            log::info!("{}: neues Gerät übernommen", source.name());
                        }
                    }
                    Ok(_) => {
                        log::warn!("{}: neues Gerät hat ein anderes Format, es bleibt still", source.name());
                        retry_at = now + Duration::from_secs(10);
                    }
                    Err(_) => retry_at = now + Duration::from_secs(5),
                }
            }
            if got == 0 {
                // Nothing playing (or no device): silence keeps the timeline running.
                let frames = (elapsed.as_secs_f64() * rate) as usize;
                scratch.resize(frames * client.block_align, 0);
            }
            if !scratch.is_empty() && stream.write_all(&scratch).is_err() {
                break;
            }
        }
        let _ = client.audio.Stop();
    }
}
