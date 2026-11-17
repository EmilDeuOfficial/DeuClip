//! Embeds the icon and version information into the EXE.
//! The installer reads the version from this resource (ProductVersion).

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
            // A missing resource compiler must not block the build.
            println!("cargo:warning=Could not embed resources: {e}");
        }
    }
}
