//! Other programs on this PC: starting console tools without a window,
//! finding running programs by the name of their executable, and holding on
//! to one program so it can be waited for or ended.
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::time::Duration;

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

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

/// Whether a program holds the named mutex `name`, such as the one each
/// running daemon holds.
#[cfg(windows)]
pub fn named_mutex_exists(name: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the name is NUL-terminated and outlives the call, and the
    // handle is closed straight away.
    unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, PCWSTR(wide.as_ptr())) }
        .map(|handle| drop(unsafe { OwnedHandle::from_raw_handle(handle.0) }))
        .is_ok()
}

#[cfg(not(windows))]
pub fn named_mutex_exists(_name: &str) -> bool {
    false
}

/// Where a running program's executable is, when Windows lets this app ask.
#[cfg(windows)]
pub fn executable(pid: u32) -> Option<PathBuf> {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is owned, and closed, from here on.
    let process = unsafe {
        OwnedHandle::from_raw_handle(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
                .ok()?
                .0,
        )
    };
    image_path(&process)
}

#[cfg(not(windows))]
pub fn executable(_pid: u32) -> Option<PathBuf> {
    None
}

/// Where the executable of the process `process` is.
#[cfg(windows)]
fn image_path(process: &OwnedHandle) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as u32;
    // SAFETY: the name is only read up to the length Windows reports writing.
    unsafe {
        QueryFullProcessImageNameW(
            HANDLE(process.as_raw_handle()),
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
        .ok()?;
    }
    Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..length as usize],
    )))
}

/// One running program, held by its handle: waiting for it or ending it
/// reaches that program and no other, even once its process id is reused.
pub struct Process {
    pid: u32,
    #[cfg(windows)]
    handle: OwnedHandle,
    #[cfg(not(windows))]
    child: std::sync::Mutex<Child>,
}

impl Process {
    /// A program this app started.
    pub fn from_child(child: Child) -> Self {
        let pid = child.id();
        #[cfg(windows)]
        {
            use std::os::windows::io::IntoRawHandle as _;
            // SAFETY: the child's own process handle, owned from here on.
            let handle = unsafe { OwnedHandle::from_raw_handle(child.into_raw_handle()) };
            Self { pid, handle }
        }
        #[cfg(not(windows))]
        Self {
            pid,
            child: std::sync::Mutex::new(child),
        }
    }

    /// The running program with process id `pid`, when Windows lets this
    /// app wait for it and end it.
    #[cfg(windows)]
    pub fn open(pid: u32) -> Option<Self> {
        use windows::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        let access = PROCESS_SYNCHRONIZE | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION;
        // SAFETY: the handle is owned, and closed, from here on.
        let handle =
            unsafe { OwnedHandle::from_raw_handle(OpenProcess(access, false, pid).ok()?.0) };
        Some(Self { pid, handle })
    }

    #[cfg(not(windows))]
    pub fn open(_pid: u32) -> Option<Self> {
        None
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Where its executable is.
    pub fn executable(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        return image_path(&self.handle);
        #[cfg(not(windows))]
        None
    }

    /// How it exited, once it has.
    pub fn exit_status(&self) -> Option<ExitStatus> {
        #[cfg(windows)]
        {
            use std::os::windows::process::ExitStatusExt as _;
            use windows::Win32::System::Threading::GetExitCodeProcess;
            if !self.wait(Duration::ZERO) {
                return None;
            }
            let mut code = 0u32;
            // SAFETY: the handle is open, and `code` is written in place.
            unsafe { GetExitCodeProcess(HANDLE(self.handle.as_raw_handle()), &mut code) }.ok()?;
            Some(ExitStatus::from_raw(code))
        }
        #[cfg(not(windows))]
        self.child.lock().unwrap().try_wait().ok().flatten()
    }

    pub fn has_exited(&self) -> bool {
        self.exit_status().is_some()
    }

    /// Waits up to `timeout` for it to exit, returning whether it has.
    pub fn wait(&self, timeout: Duration) -> bool {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::WAIT_OBJECT_0;
            use windows::Win32::System::Threading::WaitForSingleObject;
            // Short of INFINITE, which is u32::MAX.
            let millis = timeout.as_millis().min(u128::from(u32::MAX - 1)) as u32;
            // SAFETY: the handle is open for the wait.
            let event = unsafe { WaitForSingleObject(HANDLE(self.handle.as_raw_handle()), millis) };
            event == WAIT_OBJECT_0
        }
        #[cfg(not(windows))]
        {
            let began = std::time::Instant::now();
            loop {
                if self.has_exited() {
                    return true;
                }
                if began.elapsed() >= timeout {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    /// Ends it without asking it first. Only for one that has stopped
    /// answering.
    pub fn end(&self) -> std::io::Result<()> {
        #[cfg(windows)]
        {
            use windows::Win32::System::Threading::TerminateProcess;
            // SAFETY: the handle is open, with the right to terminate.
            let ended = unsafe { TerminateProcess(HANDLE(self.handle.as_raw_handle()), 1) };
            // Ending one that has just exited by itself fails, but it's ended.
            match ended {
                Err(_) if self.has_exited() => Ok(()),
                result => result.map_err(|error| std::io::Error::other(error.message())),
            }
        }
        #[cfg(not(windows))]
        self.child.lock().unwrap().kill()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn long_running() -> Child {
        let mut command = Command::new("ping");
        command
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null());
        hidden(&mut command).spawn().unwrap()
    }

    #[test]
    fn follows_and_ends_one_program() {
        let process = Process::from_child(long_running());
        assert!(!process.has_exited());
        assert!(!process.wait(Duration::from_millis(100)));
        let path = process.executable().unwrap();
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .eq_ignore_ascii_case("PING.EXE"));

        process.end().unwrap();
        assert!(process.wait(Duration::from_secs(5)));
        assert!(!process.exit_status().unwrap().success());
        // Ending it again, once it's gone, is no error.
        process.end().unwrap();
    }

    #[test]
    fn opens_a_running_program_by_its_id() {
        let child = long_running();
        let pid = child.id();
        let spawned = Process::from_child(child);
        let opened = Process::open(pid).unwrap();
        assert_eq!(opened.pid(), pid);
        opened.end().unwrap();
        assert!(spawned.wait(Duration::from_secs(5)));
    }

    #[test]
    fn reads_the_exit_code() {
        let mut command = Command::new("cmd");
        command.args(["/C", "exit 3"]);
        let process = Process::from_child(hidden(&mut command).spawn().unwrap());
        assert!(process.wait(Duration::from_secs(5)));
        assert_eq!(process.exit_status().unwrap().code(), Some(3));
    }

    #[test]
    fn sees_a_named_mutex_only_while_it_is_held() {
        use windows::core::PCWSTR;
        use windows::Win32::System::Threading::CreateMutexW;
        let name = format!("Local\\VRFaceTracking.test.gui.{}", std::process::id());
        assert!(!named_mutex_exists(&name));
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let held = unsafe {
            OwnedHandle::from_raw_handle(
                CreateMutexW(None, false, PCWSTR(wide.as_ptr())).unwrap().0,
            )
        };
        assert!(named_mutex_exists(&name));
        drop(held);
        assert!(!named_mutex_exists(&name));
    }
}
