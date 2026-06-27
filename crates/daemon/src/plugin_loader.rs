//! Detects whether a plugin `.dll` is a native Rust module or a managed
//! (.NET / VRCFT) module by inspecting its PE header.

use anyhow::{bail, Context, Result};
use log::warn;
use std::path::{Path, PathBuf};

/// What kind of runtime a discovered plugin requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    /// Native Rust cdylib loaded via `libloading`.
    Native,
    /// Managed .NET / VRCFT module hosted by `VrcftRuntime.exe`.
    Managed,
}

/// A plugin found on disk together with its detected runtime kind.
#[derive(Debug, Clone)]
pub struct DiscoveredPlugin {
    pub name: String,
    pub path: PathBuf,
    pub kind: PluginKind,
}

/// Read four little-endian bytes at `off`, or error if out of bounds.
fn read_u32(bytes: &[u8], off: usize) -> Result<u32> {
    let end = off.checked_add(4).context("offset overflow")?;
    let slice = bytes.get(off..end).context("read past end of file")?;
    Ok(u32::from_le_bytes(slice.try_into().unwrap()))
}

/// Read two little-endian bytes at `off`, or error if out of bounds.
fn read_u16(bytes: &[u8], off: usize) -> Result<u16> {
    let end = off.checked_add(2).context("offset overflow")?;
    let slice = bytes.get(off..end).context("read past end of file")?;
    Ok(u16::from_le_bytes(slice.try_into().unwrap()))
}

/// Classify a PE image already loaded into memory.
pub fn detect_plugin_kind_from_bytes(bytes: &[u8]) -> Result<PluginKind> {
    // DOS header: e_lfanew (offset to PE header) lives at 0x3C.
    let pe_off = read_u32(bytes, 0x3C)? as usize;

    // PE signature "PE\0\0".
    let sig = bytes
        .get(pe_off..pe_off + 4)
        .context("PE header offset past end of file")?;
    if sig != b"PE\0\0" {
        bail!("missing PE signature");
    }

    // Optional header begins after the 4-byte signature + 20-byte COFF header.
    let opt_off = pe_off + 24;
    let magic = read_u16(bytes, opt_off)?;
    // Data directory array offset within the optional header:
    //   PE32  (0x10b) -> 96, PE32+ (0x20b) -> 112.
    let dd_off = match magic {
        0x10b => 96usize,
        0x20b => 112usize,
        other => bail!("unrecognized optional header magic: {other:#x}"),
    };

    // COM descriptor is data directory index 14; each entry is 8 bytes
    // (VirtualAddress u32, Size u32). A non-zero Size means a CLR header
    // is present -> managed assembly.
    let com_size_off = opt_off + dd_off + 14 * 8 + 4;
    let com_size = read_u32(bytes, com_size_off)?;

    if com_size != 0 {
        Ok(PluginKind::Managed)
    } else {
        Ok(PluginKind::Native)
    }
}

/// Classify the plugin `.dll` at `path` by reading its PE header.
pub fn detect_plugin_kind(path: &Path) -> Result<PluginKind> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read plugin file {path:?}"))?;
    detect_plugin_kind_from_bytes(&bytes)
        .with_context(|| format!("failed to parse PE header of {path:?}"))
}

fn is_dynamic_lib(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext == "dll" || ext == "so" || ext == "dylib")
}

/// Recursively scan `dir` for plugin libraries, classifying each by PE header.
/// Files that cannot be read or parsed are skipped with a warning.
pub fn discover_plugins(dir: &Path) -> Vec<DiscoveredPlugin> {
    let mut out = Vec::new();
    discover_into(dir, &mut out);
    out
}

fn discover_into(dir: &Path, out: &mut Vec<DiscoveredPlugin>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!("Failed to read plugin directory {dir:?}: {e}");
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            discover_into(&path, out);
        } else if is_dynamic_lib(&path) {
            match detect_plugin_kind(&path) {
                Ok(kind) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    out.push(DiscoveredPlugin { name, path, kind });
                }
                Err(e) => warn!("Skipping {path:?}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a minimal in-memory PE image whose COM-descriptor data directory
    /// (index 14) has the given `com_size`. `magic` selects PE32 (0x10b) or
    /// PE32+ (0x20b), which changes where the data directory array begins.
    fn make_pe(magic: u16, com_size: u32) -> Vec<u8> {
        let pe_off: usize = 0x80; // PE header offset (e_lfanew)
        let dd_off: usize = if magic == 0x20b { 112 } else { 96 };
        let opt_off = pe_off + 24; // 4 (sig) + 20 (COFF header)
        let com_size_off = opt_off + dd_off + 14 * 8 + 4; // +4 = skip VirtualAddress
        let mut buf = vec![0u8; com_size_off + 4];

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

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vrft_plugin_test_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn pe32_with_com_descriptor_is_managed() {
        let bytes = make_pe(0x10b, 0x48);
        assert_eq!(
            detect_plugin_kind_from_bytes(&bytes).unwrap(),
            PluginKind::Managed
        );
    }

    #[test]
    fn pe32_without_com_descriptor_is_native() {
        let bytes = make_pe(0x10b, 0);
        assert_eq!(
            detect_plugin_kind_from_bytes(&bytes).unwrap(),
            PluginKind::Native
        );
    }

    #[test]
    fn pe32plus_with_com_descriptor_is_managed() {
        let bytes = make_pe(0x20b, 0x48);
        assert_eq!(
            detect_plugin_kind_from_bytes(&bytes).unwrap(),
            PluginKind::Managed
        );
    }

    #[test]
    fn truncated_file_errors() {
        let bytes = vec![0u8; 16];
        assert!(detect_plugin_kind_from_bytes(&bytes).is_err());
    }

    #[test]
    fn non_pe_signature_errors() {
        let mut bytes = make_pe(0x10b, 0);
        let pe_off = 0x80;
        bytes[pe_off] = b'X'; // corrupt the "PE\0\0" signature
        assert!(detect_plugin_kind_from_bytes(&bytes).is_err());
    }

    #[test]
    fn discovers_plugins_recursively_with_kinds() {
        let root = unique_tmp_dir("discover");
        // Native dll at top level.
        fs::write(root.join("native_mod.dll"), make_pe(0x10b, 0)).unwrap();
        // Managed dll in a subfolder (folder-style VRCFT module).
        let sub = root.join("vrcft_mod");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("managed_mod.dll"), make_pe(0x10b, 0x48)).unwrap();
        // A non-dll file that must be ignored.
        fs::write(root.join("readme.txt"), b"hello").unwrap();

        let mut found = discover_plugins(&root);
        found.sort_by(|a, b| a.name.cmp(&b.name));

        assert_eq!(found.len(), 2);
        assert_eq!(found[0].name, "managed_mod.dll");
        assert_eq!(found[0].kind, PluginKind::Managed);
        assert_eq!(found[1].name, "native_mod.dll");
        assert_eq!(found[1].kind, PluginKind::Native);

        fs::remove_dir_all(&root).unwrap();
    }
}
