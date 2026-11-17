# DeuClip

![Version](https://img.shields.io/badge/version-v1.1.0-22C55E)
![Plattform](https://img.shields.io/badge/plattform-Windows%2010%2F11-1C1C1C)
![Sprache](https://img.shields.io/badge/sprache-Rust-1C1C1C)
![Lizenz](https://img.shields.io/badge/lizenz-MIT-22C55E)

DeuClip ist ein leichtgewichtiges Clip-Tool für Windows. Es hält die letzten Sekunden deines Bildschirms im Hintergrund vor und speichert sie per Hotkey als MP4.

## Features

- Zwei unabhängige globale Hotkeys mit je eigener Clip-Dauer (5 bis 600 Sekunden, Validierung in der GUI)
- Hotkeys werden per Tastendruck aufgenommen (Taste plus optional Strg, Alt, Shift, Win), doppelte Kombinationen werden abgelehnt
- Verständliche Fehlermeldung, wenn eine Kombination von einer anderen Anwendung belegt ist
- Bildschirmauswahl per Dropdown mit Monitorname, Auflösung und Markierung des Primärmonitors, Neustart der Aufnahme beim Wechsel
- Ausgabeordner über nativen Ordnerdialog, Standard ist `Videos\Clips`, wird bei Bedarf angelegt
- Windows-Toast nach dem Speichern ("Clip gespeichert" mit Dateiname) und bei Fehlern
- Reicht der Puffer nicht aus, wird gespeichert was vorhanden ist und die Benachrichtigung weist darauf hin
- Dateiname: `clip_JJJJ-MM-TT_HH-MM-SS_<Hotkey-Nummer>.mp4`
- Aufnahme über FFmpeg mit `ddagrab` (Desktop Duplication), keine Screenshot-Schleifen in Rust
- Encoder-Auswahl: `h264_nvenc`, dann `h264_qsv`, dann `h264_amf`, Fallback `libx264` mit Preset `ultrafast`
- Rollender Puffer aus 1-Sekunden-Segmenten, Clips werden per Concat-Demuxer ohne Neukodierung (`-c copy`) zusammengefügt
- Konfigurierbar: Bildrate, Clip-Auflösung von 480p bis 2160p (4K), Qualität, FFmpeg-Pfad
- Es wird nie hochskaliert: 4K nimmt nur auf einem 4K-Bildschirm in voller Größe auf
- Installer (`DeuClip_Setup.exe`) mit deutscher und englischer Oberfläche, lädt FFmpeg bei Bedarf nach
- Konfiguration als JSON in `%APPDATA%\DeuClip\config.json`, Datei-Log in `%APPDATA%\DeuClip\deuclip.log`
- Optionaler Autostart mit Windows über den Registry-Run-Key
- Dunkles Design mit grünem Akzent, zentral in `theme.rs`
- FFmpeg läuft ohne Konsolenfenster und wird beendet, auch wenn DeuClip abstürzt (Job-Objekt)
- GUI blockiert nie: Aufnahme, Speichern, Dialoge und Benachrichtigungen laufen in eigenen Threads

## Benchmark

Gemessen mit dem Release-Build auf Windows 11, AMD Ryzen 7 5800X3D, 1920x1080 bei 30 FPS, Encoder `h264_nvenc` (GPU direkt), Fenster geöffnet. Die Werte schwanken je nach System, Messzeitpunkt und Grafiktreiber.

| Messung | Wert |
| --- | --- |
| DeuClip CPU im Leerlauf (40 s Mittel, Anteil an der Gesamt-CPU) | 0,08 bis 0,24 % |
| DeuClip RAM im Leerlauf (Working Set) | ca. 106 bis 170 MB |
| DeuClip RAM beim Speichern (Spitze, Working Set) | ca. 112 MB |
| DeuClip CPU-Zeit während eines Speichervorgangs | 0,06 s |
| FFmpeg CPU während der Aufnahme (Anteil an der Gesamt-CPU) | ca. 0,02 % |
| Speichern eines 30-Sekunden-Clips | ca. 1,0 s, Datei ca. 8,7 MB |

Hinweis: Der Arbeitsspeicher der GUI liegt über dem angestrebten Wert von 50 MB. Ein großer Teil davon entfällt auf den OpenGL-Treiber der Grafikkarte.

## Requirements

| Abhängigkeit | Zweck |
| --- | --- |
| Windows 10 oder 11 | Betriebssystem |
| `ffmpeg.exe` mit dem Filter `ddagrab` (ab FFmpeg 6.0) | Aufnahme und Clip-Erzeugung, Download: [ffmpeg.org](https://ffmpeg.org/download.html), der Installer lädt es bei Bedarf selbst |
| Rust-Toolchain | nur für den Build |
| Python 3 und [Inno Setup 6](https://jrsoftware.org/isdl.php) | nur für `build.py` (Installer) |

## Usage

### Installer

1. `DeuClip_Setup.exe` starten. Der Installer legt DeuClip pro Benutzer ab, richtet Startmenü und optional eine Desktop-Verknüpfung ein und lädt FFmpeg herunter, falls es weder im `PATH` noch neben der EXE liegt.
2. DeuClip starten. Der Status unten zeigt "Aufnahme läuft", sobald FFmpeg angelaufen ist.

### Selbst bauen

1. Nur die EXE:

   ```bash
   cargo build --release
   ```

   Die EXE liegt danach unter `target\release\deuclip.exe`.
2. EXE und Installer (Ergebnis in `dist\`):

   ```bash
   python build.py
   ```

3. `ffmpeg.exe` neben die EXE legen, in den `PATH` aufnehmen oder im Bereich "System" der GUI den Pfad wählen.

### Einrichten

1. Hotkeys einrichten: Auf das Feld der Tastenkombination klicken, die gewünschte Kombination drücken (Esc bricht ab) und die Clip-Dauer in Sekunden eintragen.
2. Hotkey drücken. Der Clip landet im Ausgabeordner, danach erscheint eine Benachrichtigung.

## Clip-Auflösung

| Auswahl | Verhalten |
| --- | --- |
| Wie Bildschirm | Keine Skalierung, Frames bleiben bei NVIDIA und AMD auf der GPU (geringste Last) |
| 2160p (4K), 1440p, 1080p, 720p, 480p | Skaliert auf diese Höhe, nie hochskaliert |

4K bedeutet große Dateien und deutlich mehr Last. Ohne Hardware-Encoder ist 4K mit `libx264` auf vielen PCs zu langsam.

## File Structure

```text
DeuClip/
├── .gitignore
├── Cargo.toml
├── Cargo.lock
├── LICENSE
├── README.md
├── build.rs                Bettet Icon und Version in die EXE ein
├── build.py                Cargo-Build und Inno-Setup-Build (Ergebnis in dist/)
├── installer.iss           Inno-Setup-Skript (Installer, laedt FFmpeg bei Bedarf)
├── img/
│   ├── app.ico             App-Icon
│   └── app-icon.svg
├── tools/
│   └── make_icon.py        Erzeugt das Icon
└── src/
    ├── main.rs             Einstieg, Fenster
    ├── app.rs              GUI und Verbindung zu Engine, Hotkeys, Konfiguration
    ├── theme.rs            Farben, Schrift und Widgets
    ├── capture.rs          FFmpeg-Aufnahme, Segmentpuffer, Clip speichern, Monitorliste
    ├── hotkeys.rs          Globale Hotkeys
    ├── config.rs           config.json
    ├── notify.rs           Windows-Toasts
    ├── autostart.rs        Autostart ueber die Registry
    └── logging.rs          Datei-Logger
```

Beim Bauen entsteht zusätzlich `dist/` (nicht im Repository) mit `DeuClip.exe` und `DeuClip_Setup.exe`.

Zur Laufzeit erzeugte Dateien:

- `%APPDATA%\DeuClip\config.json`: Hotkeys, Dauern, Monitor, Ausgabeordner, Qualität, Bildrate, Skalierung, FFmpeg-Pfad
- `%APPDATA%\DeuClip\deuclip.log`: Logdatei (wird ab 1 MB nach `deuclip.old.log` rotiert)
- `%TEMP%\DeuClip\`: temporärer Segmentpuffer, wird beim Beenden entfernt

## Changelog

### v1.1.0

- Neue Clip-Auflösung 2160p (4K), es wird nie hochskaliert
- Installer `DeuClip_Setup.exe` (Inno Setup, Deutsch und Englisch), lädt FFmpeg bei Bedarf nach
- `build.py` baut EXE und Installer, Icon und Versionsinformationen sind in die EXE eingebettet
- Anzeigename des Monitors, klarere Namen der Auflösungen, Segoe UI als Schrift, Beschriftungen nicht mehr markierbar

### v1.0.0

- Zwei globale Hotkeys mit eigener Clip-Dauer und Tastenaufnahme in der GUI
- Rollender Segmentpuffer über FFmpeg und `ddagrab`, Hardware-Encoder mit Fallback auf `libx264`
- Clip-Erzeugung ohne Neukodierung, Hinweis bei unvollständigem Puffer
- Monitorauswahl, Ausgabeordner, Qualität, Bildrate und Skalierung einstellbar
- Windows-Benachrichtigungen, Autostart, Konfiguration und Logdatei
- Dunkles Design mit grünem Akzent

## License

MIT, siehe Datei [LICENSE](LICENSE).

## About

DeuClip ist ein Instant-Replay-Clip-Tool für Windows, geschrieben in Rust.

Topics: `rust`, `windows`, `screen-recording`, `ffmpeg`, `egui`, `instant-replay`, `clip`
