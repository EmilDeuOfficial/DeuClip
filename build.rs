//! Bettet Icon und Versionsinformationen in die EXE ein.
//! Der Installer liest die Version aus dieser Ressource (ProductVersion).

fn main() {
    println!("cargo:rerun-if-changed=img/app.ico");
    println!("cargo:rerun-if-changed=Cargo.toml");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("img/app.ico");
        res.set("ProductName", "DeuClip");
        res.set("FileDescription", "DeuClip - Instant-Replay-Clip-Tool");
        res.set("CompanyName", "EmilDeuOfficial");
        res.set("LegalCopyright", "Copyright (c) 2026 EmilDeuOfficial");
        res.set("OriginalFilename", "DeuClip.exe");
        if let Err(e) = res.compile() {
            // Ein fehlender Resource-Compiler soll den Build nicht blockieren.
            println!("cargo:warning=Ressourcen konnten nicht eingebettet werden: {e}");
        }
    }
}
