//! Embeds the app icon (the white face mark on a blue tile, drawn in
//! `resources/app-icon*.svg`) into vrft_gui.exe, and rebuilds when a
//! translation in `locales/` changes, and stamps in `VRFT_VERSION` (see
//! `build-support/version.rs`).
#[path = "../../build-support/version.rs"]
mod version;

fn main() {
    version::stamp();
    println!("cargo:rerun-if-changed=resources/app.rc");
    println!("cargo:rerun-if-changed=resources/app.ico");
    // rust-i18n embeds these at compile time but doesn't track them.
    println!("cargo:rerun-if-changed=locales");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("resources/app.rc", embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
