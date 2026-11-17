"""
Build-Skript: Rust-App -> EXE (cargo) -> Installer (Inno Setup)
Aufruf: python build.py

Ergebnis im Ordner dist/:
  DeuClip.exe          einzelne EXE ohne Installer
  DeuClip_Setup.exe    Installer (laedt bei Bedarf FFmpeg nach)
"""
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent
DIST = ROOT / "dist"
CARGO_EXE = ROOT / "target" / "release" / "deuclip.exe"

INNO_PATHS = [
    r"C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    r"C:\Program Files\Inno Setup 6\ISCC.exe",
    os.path.expandvars(r"%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"),
]


def app_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version = "([^"]+)"', text, re.M)
    if not match:
        print("[ERROR] Version in Cargo.toml nicht gefunden.")
        sys.exit(1)
    return match.group(1)


def check_version(version: str) -> None:
    """Das README-Badge muss die Version zeigen, die gebaut wird."""
    badge = re.search(r"badge/version-v([\w.]+)-", (ROOT / "README.md").read_text(encoding="utf-8"))
    if not badge or badge.group(1) != version:
        found = badge.group(1) if badge else "keine"
        print(f"[ERROR] README-Badge zeigt v{found}, Cargo.toml sagt {version}.")
        sys.exit(1)


def run(cmd: list, **kwargs) -> None:
    print("\n>>> " + " ".join(str(c) for c in cmd) + "\n")
    result = subprocess.run(cmd, **kwargs)
    if result.returncode != 0:
        print(f"[ERROR] Befehl fehlgeschlagen (Exit-Code {result.returncode})")
        sys.exit(result.returncode)


def find_inno() -> str | None:
    for p in INNO_PATHS:
        if os.path.isfile(p):
            return p
    return shutil.which("ISCC")


def install_inno() -> str:
    print("[*] Inno Setup nicht gefunden. Installation per winget ...")
    result = subprocess.run(
        ["winget", "install", "JRSoftware.InnoSetup",
         "--accept-source-agreements", "--accept-package-agreements"],
    )
    if result.returncode != 0:
        print("[ERROR] Inno Setup konnte nicht automatisch installiert werden.")
        print("        Bitte manuell installieren: https://jrsoftware.org/isdl.php")
        sys.exit(1)
    found = find_inno()
    if not found:
        print("[ERROR] Inno Setup installiert, ISCC.exe aber nicht gefunden.")
        print("        Das Skript in einem neuen Terminal erneut starten.")
        sys.exit(1)
    return found


def build_exe() -> None:
    print("[*] Baue EXE mit cargo ...")
    run(["cargo", "build", "--release"], cwd=ROOT)
    if not CARGO_EXE.exists():
        print("[ERROR] cargo-Ausgabe nicht gefunden.")
        sys.exit(1)
    DIST.mkdir(exist_ok=True)
    target = DIST / "DeuClip.exe"
    shutil.copy2(CARGO_EXE, target)
    print(f"[OK] DeuClip.exe gebaut ({target.stat().st_size / 1024 / 1024:.1f} MB)")


def build_installer(iscc: str) -> None:
    print("[*] Baue Installer mit Inno Setup ...")
    run([iscc, str(ROOT / "installer.iss")], cwd=ROOT)
    installer = DIST / "DeuClip_Setup.exe"
    if not installer.exists():
        print("[ERROR] Installer nach Inno Setup nicht gefunden.")
        sys.exit(1)
    print("\n" + "=" * 60)
    print(f"  Installer fertig: {installer}")
    print(f"  Groesse: {installer.stat().st_size / 1024 / 1024:.1f} MB")
    print("=" * 60 + "\n")


def main() -> None:
    print("=" * 60)
    print("  DeuClip - Build Pipeline")
    print("=" * 60)

    version = app_version()
    check_version(version)
    print(f"[*] Version {version} (Release-Tag v{version})")

    build_exe()
    iscc = find_inno() or install_inno()
    build_installer(iscc)
    print("[DONE] Build abgeschlossen.")


if __name__ == "__main__":
    main()
