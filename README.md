# DeuClip

![Version](https://img.shields.io/badge/version-v1.0-22C55E)
![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-1C1C1C)
![Language](https://img.shields.io/badge/language-Rust-1C1C1C)
![License](https://img.shields.io/badge/license-MIT-22C55E)

DeuClip is a lightweight clip tool for Windows. It keeps the last seconds of your screen in a rolling buffer and saves them as an MP4 when you press a hotkey.

## Features

- Two independent global hotkeys, each with its own clip length (5 to 600 seconds, validated in the GUI)
- Hotkeys are recorded by pressing them (key plus optional Ctrl, Alt, Shift, Win), duplicate combinations are rejected
- Clear error message when a combination is already taken by another application
- Screen selection in a dropdown with monitor name, resolution and a primary monitor marker, the capture restarts on change
- Output folder via the native folder dialog, default is `Videos\Clips`, created when needed
- Windows toast after saving ("Clip gespeichert" with the file name) and on errors
- Small on-screen notification at the bottom left of the recorded monitor (green for saved, red for failed), it never takes focus and is excluded from screen capture, so it does not appear in clips
- If the buffer is too short, everything available is saved and the notification says so
- File name: `clip_YYYY-MM-DD_HH-MM-SS_<hotkey number>.mp4`
- Capture via FFmpeg with `ddagrab` (Desktop Duplication), no screenshot loops in Rust
- Encoder order: `h264_nvenc`, then `h264_qsv`, then `h264_amf`, fallback `libx264` with preset `ultrafast`
- Rolling buffer of 1-second segments, clips are joined with the concat demuxer without re-encoding (`-c copy`)
- Configurable: frame rate, clip resolution from 480p up to 2160p (4K), quality, FFmpeg path
- Never upscales: 4K records at full size only on a 4K screen
- Installer (`DeuClip_Setup.exe`) in German and English that downloads FFmpeg when it is missing
- Configuration as JSON in `%APPDATA%\DeuClip\config.json`, file log in `%APPDATA%\DeuClip\deuclip.log`
- Runs in the background: closing the window hides DeuClip in the tray, recording and hotkeys keep working, "Beenden" in the tray menu (or in the window) exits
- Optional autostart with Windows via the registry Run key, DeuClip then starts hidden in the tray (`--background`)
- Single instance: starting DeuClip again only brings the running window to the front
- Dark design with a green accent, defined in one place (`theme.rs`)
- FFmpeg runs without a console window and is terminated even if DeuClip crashes (job object)
- The GUI never blocks: capture, saving, dialogs and notifications run in their own threads

The user interface of the app is in German.

## Benchmark

Measured with the release build on Windows 11, AMD Ryzen 7 5800X3D, 1920x1080 at 30 FPS, encoder `h264_nvenc` (GPU direct), window open. Values vary with system, measuring time and graphics driver.

| Measurement | Value |
| --- | --- |
| DeuClip CPU at idle (40 s average, share of total CPU) | 0.08 to 0.24 % |
| DeuClip RAM at idle (working set) | about 106 to 170 MB |
| DeuClip RAM while saving (peak, working set) | about 112 MB |
| DeuClip CPU time during a save | 0.06 s |
| FFmpeg CPU while recording (share of total CPU) | about 0.02 % |
| Saving a 30 second clip | about 1.0 s, file about 8.7 MB |

Note: the RAM usage of the GUI is above the target of 50 MB. A large part of it comes from the OpenGL driver of the graphics card.

## Requirements

| Dependency | Purpose |
| --- | --- |
| Windows 10 or 11 | Operating system |
| `ffmpeg.exe` with the `ddagrab` filter (FFmpeg 6.0 or newer) | Capture and clip creation, download: [ffmpeg.org](https://ffmpeg.org/download.html), the installer downloads it when needed |
| Rust toolchain | Build only |
| Python 3 and [Inno Setup 6](https://jrsoftware.org/isdl.php) | `build.py` (installer) only |

## Usage

### Installer

1. Run `DeuClip_Setup.exe`. It installs DeuClip per user, creates a Start menu entry and an optional desktop shortcut, and downloads FFmpeg if it is neither in the `PATH` nor next to the EXE.
2. Start DeuClip. The status bar shows "Aufnahme läuft" (recording) as soon as FFmpeg is up.

### Build it yourself

1. EXE only:

   ```bash
   cargo build --release
   ```

   The EXE is then at `target\release\deuclip.exe`.
2. EXE and installer (output in `dist\`):

   ```bash
   python build.py
   ```

3. Put `ffmpeg.exe` next to the EXE, add it to the `PATH`, or choose its path in the "System" section of the GUI.

### Set up

1. Set up the hotkeys: click the key combination field, press the combination you want (Esc cancels) and enter the clip length in seconds.
2. Press a hotkey. The clip is written to the output folder and a notification appears.
3. Close the window to keep DeuClip running in the background. Open it again with the tray icon, exit with right click, "Beenden".

## Clip resolution

| Choice | Behavior |
| --- | --- |
| Like screen | No scaling, frames stay on the GPU with NVIDIA and AMD (lowest load) |
| 2160p (4K), 1440p, 1080p, 720p, 480p | Scales to this height, never upscales |

4K means large files and a lot more load. Without a hardware encoder, 4K with `libx264` is too slow on many PCs.

## File Structure

```text
DeuClip/
├── .gitignore
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── build.rs                Embeds icon and version into the EXE
├── build.py                Cargo build and Inno Setup build (output in dist/)
├── installer.iss           Inno Setup script (installer, downloads FFmpeg when needed)
├── img/
│   ├── app.ico             App icon
│   └── app-icon.svg
├── tools/
│   └── make_icon.py        Generates the icon
└── src/
    ├── main.rs             Entry point, window
    ├── app.rs              GUI and wiring of engine, hotkeys and configuration
    ├── theme.rs            Colors, font and widgets
    ├── capture.rs          FFmpeg capture, segment buffer, saving clips, monitor list
    ├── hotkeys.rs          Global hotkeys
    ├── config.rs           config.json
    ├── notify.rs           Windows toasts
    ├── overlay.rs          On-screen notification at the bottom left
    ├── autostart.rs        Autostart via the registry
    ├── tray.rs             Tray icon and menu
    ├── instance.rs         Single instance, shows the window on a second start
    └── logging.rs          File logger
```

Building also creates `dist/` (not in the repository) with `DeuClip.exe` and `DeuClip_Setup.exe`.

Files created at runtime:

- `%APPDATA%\DeuClip\config.json`: hotkeys, durations, monitor, output folder, quality, frame rate, scaling, FFmpeg path
- `%APPDATA%\DeuClip\deuclip.log`: log file (rotated to `deuclip.old.log` above 1 MB)
- `%TEMP%\DeuClip\`: temporary segment buffer, removed on exit

## Changelog

### v1.0

- Two global hotkeys with their own clip length and key recording in the GUI
- Rolling segment buffer via FFmpeg and `ddagrab`, hardware encoders with fallback to `libx264`
- Clip creation without re-encoding, notice when the buffer is incomplete
- Monitor selection with monitor names, output folder, quality, frame rate and clip resolution up to 2160p (4K), never upscaled
- Windows notifications, autostart, configuration and log file
- Installer `DeuClip_Setup.exe` (Inno Setup, German and English) that downloads FFmpeg when needed
- `build.py` builds the EXE and the installer, icon and version information are embedded in the EXE
- Dark design with a green accent

## License

MIT, see the [LICENSE](LICENSE) file.

## About

DeuClip is an instant replay clip tool for Windows, written in Rust.

Topics: `rust`, `windows`, `screen-recording`, `ffmpeg`, `egui`, `instant-replay`, `clip`
