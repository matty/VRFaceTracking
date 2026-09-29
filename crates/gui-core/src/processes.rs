//! Other programs on this PC: starting console tools without a window, and
//! finding running programs by the name of their executable.
use std::path::PathBuf;
use std::process::Command;

/// Keeps a console tool from opening a console window. A release of this app
/// has no console for it to share.
pub fn hidden(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Process ids of every running program whose executable is named `exe`.
#[cfg(windows)]
pub fn find(exe: &str) -> Vec<u32> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    let mut found = Vec::new();
    // SAFETY: the snapshot handle is checked, only read through the ToolHelp
    // API with a correctly sized entry, and closed before returning.
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return found;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            let length = entry
                .szExeFile
                .iter()
                .position(|&unit| unit == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
            if name.eq_ignore_ascii_case(exe) {
                found.push(entry.th32ProcessID);
            }
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

#[cfg(not(windows))]
pub fn find(_exe: &str) -> Vec<u32> {
    Vec::new()
}

/// Ends every running program whose executable is named `exe`, without
/// asking it first. Only for one that has stopped answering.
pub fn end(exe: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("taskkill");
        command.args(["/IM", exe, "/F"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new("pkill");
        command.args(["-x", exe]);
        command
    };
    let status = hidden(&mut command).status()?;
    if status.success() || find(exe).is_empty() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("couldn't end {exe}")))
    }
}

/// Where a running program's executable is, when Windows lets this app ask.
#[cfg(windows)]
pub fn executable(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    // SAFETY: the process handle is checked and closed before returning, and
    // the name is only read up to the length Windows reports writing.
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let named = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        );
        let _ = CloseHandle(process);
        named.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buffer[..length as usize],
        )))
    }
}

#[cfg(not(windows))]
pub fn executable(_pid: u32) -> Option<PathBuf> {
    None
}
