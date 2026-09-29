//! Stamps `VRFT_VERSION` into the build; see `build-support/version.rs`.
#[path = "../../build-support/version.rs"]
mod version;

fn main() {
    version::stamp();
}
