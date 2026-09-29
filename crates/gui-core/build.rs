//! rust-i18n embeds `locales/` at compile time but doesn't track it, so
//! rebuild when a translation changes.
fn main() {
    println!("cargo:rerun-if-changed=locales");
}
