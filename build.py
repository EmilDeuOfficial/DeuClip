"""
Build script: Rust app -> EXE (cargo) -> installer (Inno Setup)
Usage: python build.py

Output in the dist/ folder:
  DeuClip.exe          single EXE without an installer
  DeuClip_Setup.exe    installer (downloads FFmpeg when needed)
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
        print("[ERROR] Version not found in Cargo.toml.")
        sys.exit(1)
    return match.group(1)


def short_version(version: str) -> str:
    """1.0.0 -> 1.0, 1.2.0 -> 1.2, 1.2.3 stays (the form used in the README and the release tag)."""
    parts = version.split(".")
    while len(parts) > 2 and parts[-1] == "0":
        parts.pop()
    return ".".join(parts)


def check_version(version: str) -> None:
    """The README badge must show the version that is being built."""
    version = short_version(version)
    badge = re.search(r"badge/version-v([\w.]+)-", (ROOT / "README.md").read_text(encoding="utf-8"))
    if not badge or badge.group(1) != version:
        found = badge.group(1) if badge else "none"
        print(f"[ERROR] README badge shows v{found}, Cargo.toml says v{version}.")
        sys.exit(1)


def run(cmd: list, **kwargs) -> None:
    print("\n>>> " + " ".join(str(c) for c in cmd) + "\n")
    result = subprocess.run(cmd, **kwargs)
    if result.returncode != 0:
        print(f"[ERROR] Command failed (exit code {result.returncode})")
        sys.exit(result.returncode)


def find_inno() -> str | None:
    for p in INNO_PATHS:
        if os.path.isfile(p):
            return p
    return shutil.which("ISCC")


def install_inno() -> str:
    print("[*] Inno Setup not found. Installing via winget ...")
    result = subprocess.run(
        ["winget", "install", "JRSoftware.InnoSetup",
         "--accept-source-agreements", "--accept-package-agreements"],
    )
    if result.returncode != 0:
        print("[ERROR] Could not install Inno Setup automatically.")
        print("        Please install it manually: https://jrsoftware.org/isdl.php")
        sys.exit(1)
    found = find_inno()
    if not found:
        print("[ERROR] Inno Setup installed, but ISCC.exe was not found.")
        print("        Run the script again in a new terminal.")
        sys.exit(1)
    return found


def build_exe() -> None:
    print("[*] Building the EXE with cargo ...")
    run(["cargo", "build", "--release"], cwd=ROOT)
    if not CARGO_EXE.exists():
        print("[ERROR] cargo output not found.")
        sys.exit(1)
    DIST.mkdir(exist_ok=True)
    target = DIST / "DeuClip.exe"
    shutil.copy2(CARGO_EXE, target)
    print(f"[OK] DeuClip.exe built ({target.stat().st_size / 1024 / 1024:.1f} MB)")


def build_installer(iscc: str) -> None:
    print("[*] Building the installer with Inno Setup ...")
    run([iscc, str(ROOT / "installer.iss")], cwd=ROOT)
    installer = DIST / "DeuClip_Setup.exe"
    if not installer.exists():
        print("[ERROR] Installer not found after Inno Setup.")
        sys.exit(1)
    print("\n" + "=" * 60)
    print(f"  Installer ready: {installer}")
    print(f"  Size: {installer.stat().st_size / 1024 / 1024:.1f} MB")
    print("=" * 60 + "\n")


def main() -> None:
    print("=" * 60)
    print("  DeuClip - Build Pipeline")
    print("=" * 60)

    version = app_version()
    check_version(version)
    print(f"[*] Version {short_version(version)} (release tag v{short_version(version)})")

    build_exe()
    iscc = find_inno() or install_inno()
    build_installer(iscc)
    print("[DONE] Build complete.")


if __name__ == "__main__":
    main()
