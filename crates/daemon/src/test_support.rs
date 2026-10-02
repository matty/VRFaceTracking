//! Helpers for the daemon's tests. Both the library and `vrft_d` include
//! this file, as neither sees the other's test-only code, and each uses only
//! part of it.
#![allow(dead_code)]
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A new empty folder under the system's temp folder, its own even among
/// tests running at once.
pub fn temp_dir(tag: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let next = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("vrft_{tag}_{}_{next}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A minimal PE image whose COM-descriptor data directory (index 14) has
/// the given `com_size`: managed when it isn't 0. `magic` selects PE32
/// (0x10b) or PE32+ (0x20b), which changes where the data directory array
/// begins.
pub fn make_pe(magic: u16, com_size: u32) -> Vec<u8> {
    let pe_off: usize = 0x80; // PE header offset (e_lfanew)
    let dd_off: usize = if magic == 0x20b { 112 } else { 96 };
    let opt_off = pe_off + 24; // 4 (sig) + 20 (COFF header)
    let com_size_off = opt_off + dd_off + 14 * 8 + 4; // +4 = skip VirtualAddress
    let mut buf = vec![0u8; com_size_off + 4];

    buf[..2].copy_from_slice(b"MZ");
    // e_lfanew at 0x3C
    buf[0x3C..0x40].copy_from_slice(&(pe_off as u32).to_le_bytes());
    // "PE\0\0" signature
    buf[pe_off..pe_off + 4].copy_from_slice(b"PE\0\0");
    // Optional header magic
    buf[opt_off..opt_off + 2].copy_from_slice(&magic.to_le_bytes());
    // COM descriptor Size
    buf[com_size_off..com_size_off + 4].copy_from_slice(&com_size.to_le_bytes());
    buf
}

/// A managed (.NET) module library, as far as the plugin loader can tell.
pub fn managed_dll() -> Vec<u8> {
    make_pe(0x10b, 0x48)
}
