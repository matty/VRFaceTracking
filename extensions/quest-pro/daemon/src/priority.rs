//! Tongue training gives way to VRChat: while VRChat runs, the trainer runs
//! below normal priority, so it only takes CPU time VRChat doesn't need.
use std::process::Child;

const VRCHAT: &str = "VRChat.exe";

/// Whether VRChat is running on this PC.
pub fn vrchat_running() -> bool {
    running(VRCHAT)
}

/// Whether a program whose executable is named `exe` is running.
#[cfg(windows)]
fn running(exe: &str) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    // SAFETY: the snapshot handle is checked, only read through the ToolHelp
    // API with a correctly sized entry, and closed before returning.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return false;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut found = false;
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more && !found {
            let length = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            found = String::from_utf16_lossy(&entry.szExeFile[..length]).eq_ignore_ascii_case(exe);
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
fn running(_exe: &str) -> bool {
    false
}

/// Runs `child` below normal priority, or at normal priority again.
#[cfg(windows)]
pub fn set_below_normal(child: &Child, below: bool) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Threading::{
        SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    };
    let class = if below {
        BELOW_NORMAL_PRIORITY_CLASS
    } else {
        NORMAL_PRIORITY_CLASS
    };
    // SAFETY: the handle is the child's own, open while `child` is.
    unsafe { SetPriorityClass(HANDLE(child.as_raw_handle()), class) }
        .map_err(|error| error.message())
}

#[cfg(not(windows))]
pub fn set_below_normal(_child: &Child, _below: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn finds_running_programs_by_name() {
        let me = std::env::current_exe().unwrap();
        assert!(running(me.file_name().unwrap().to_str().unwrap()));
        assert!(!running("not-a-running-program-vrft.exe"));
    }

    #[test]
    fn lowers_and_restores_priority() {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{
            GetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        };
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let class = |child: &Child| unsafe { GetPriorityClass(HANDLE(child.as_raw_handle())) };
        set_below_normal(&child, true).unwrap();
        assert_eq!(class(&child), BELOW_NORMAL_PRIORITY_CLASS.0);
        set_below_normal(&child, false).unwrap();
        assert_eq!(class(&child), NORMAL_PRIORITY_CLASS.0);
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
