//! How long the daemon runs, and what goes with it: one daemon at a time,
//! the programs it starts ending when it ends, stopping with the desktop app
//! that started it, and a deadline on stopping once asked.
use anyhow::Result;
use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use vrft_protocol::{DAEMON_INSTANCE, OWNER_PID_ARG};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

/// How long stopping may take once asked before the daemon ends itself. A
/// module stuck in its update would otherwise keep it running. Shorter than
/// the desktop app waits as it closes, so the daemon ends itself first.
const STOP_DEADLINE: Duration = Duration::from_secs(4);

/// Held while this is the running daemon.
pub struct Instance {
    #[cfg(windows)]
    _mutex: OwnedHandle,
}

/// Marks this process as the running daemon, or fails if another daemon is
/// running: both would send every expression to VRChat.
pub fn claim_instance() -> Result<Instance> {
    claim(DAEMON_INSTANCE)
}

#[cfg(windows)]
fn claim(name: &str) -> Result<Instance> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows::Win32::System::Threading::CreateMutexW;
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: the name is NUL-terminated and outlives the call, and the
    // handle is owned, and closed, from here on.
    let (mutex, existed) = unsafe {
        let handle = CreateMutexW(None, false, PCWSTR(wide.as_ptr()))?;
        let existed = GetLastError() == ERROR_ALREADY_EXISTS;
        (OwnedHandle::from_raw_handle(handle.0), existed)
    };
    if existed {
        anyhow::bail!(
            "Another vrft_d is already running, so this one isn't starting: both would send everything to VRChat"
        );
    }
    Ok(Instance { _mutex: mutex })
}

#[cfg(not(windows))]
fn claim(_name: &str) -> Result<Instance> {
    Ok(Instance {})
}

/// Makes every program the daemon starts end when the daemon ends, however
/// it ends: the .NET host for VRCFT modules would otherwise keep the
/// tracking device, and a tongue training run the CPU, after the daemon was
/// forced to stop. The daemon joins a job that ends everything in it once
/// its last handle closes, and the programs it starts join it too.
pub fn end_children_with_daemon() {
    #[cfg(windows)]
    {
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        use windows::Win32::System::Threading::GetCurrentProcess;
        let joined = kill_on_close_job().and_then(|job| {
            // SAFETY: both handles are valid for the call.
            unsafe { AssignProcessToJobObject(HANDLE(job.as_raw_handle()), GetCurrentProcess())? };
            Ok(job)
        });
        match joined {
            // Held open for the daemon's life. Windows closes it as the
            // daemon exits, which ends the rest of the job.
            Ok(job) => std::mem::forget(job),
            Err(error) => warn!("Programs vrft_d starts may keep running after it stops: {error}"),
        }
    }
}

/// A job that ends every process in it when its last handle closes.
#[cfg(windows)]
fn kill_on_close_job() -> windows::core::Result<OwnedHandle> {
    use windows::core::PCWSTR;
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    // SAFETY: the job handle is owned from creation, and the limits are
    // passed with their own size.
    unsafe {
        let job = OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null())?.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        Ok(job)
    }
}

/// The process `--owner-pid <pid>` names, if the daemon was given one.
pub fn owner_pid(arguments: &[String]) -> Option<u32> {
    let at = arguments
        .iter()
        .position(|argument| argument == OWNER_PID_ARG)?;
    arguments.get(at + 1)?.parse().ok()
}

/// Stops the daemon when process `owner`, the desktop app that started it,
/// exits. Even when the app crashes or is ended, its daemon doesn't run on
/// unseen.
pub fn stop_with_owner(owner: u32, running: Arc<AtomicBool>) {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::ERROR_INVALID_PARAMETER;
        use windows::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, INFINITE, PROCESS_SYNCHRONIZE,
        };
        // SAFETY: the handle is owned, and closed, from here on.
        let process = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, owner) } {
            Ok(handle) => unsafe { OwnedHandle::from_raw_handle(handle.0) },
            // There's no such process: the app has already closed.
            Err(error) if error.code() == ERROR_INVALID_PARAMETER.to_hresult() => {
                info!("The app that started vrft_d has already closed; stopping");
                running.store(false, Ordering::SeqCst);
                return;
            }
            Err(error) => {
                warn!("Can't follow the app that started vrft_d (process {owner}), so it keeps running after the app closes: {error}");
                return;
            }
        };
        thread::Builder::new()
            .name("owner-watch".into())
            .spawn(move || {
                // SAFETY: the handle stays open for the wait.
                unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), INFINITE) };
                if running.swap(false, Ordering::SeqCst) {
                    info!("The app that started vrft_d has closed; stopping");
                }
            })
            .expect("couldn't start the owner-watch thread");
    }
    #[cfg(not(windows))]
    let _ = (owner, running);
}

/// Once the daemon is asked to stop, gives it `STOP_DEADLINE` to finish,
/// then ends it.
pub fn end_if_stopping_stalls(running: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("stop-deadline".into())
        .spawn(move || {
            while running.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(100));
            }
            thread::sleep(STOP_DEADLINE);
            warn!(
                "vrft_d didn't finish stopping within {} s, so it's ending now",
                STOP_DEADLINE.as_secs()
            );
            std::process::exit(1);
        })
        .expect("couldn't start the stop-deadline thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(list: &[&str]) -> Vec<String> {
        list.iter().map(|argument| argument.to_string()).collect()
    }

    #[test]
    fn reads_the_owner_pid() {
        assert_eq!(owner_pid(&arguments(&["--owner-pid", "4242"])), Some(4242));
        assert_eq!(
            owner_pid(&arguments(&["--extensions-only", "--owner-pid", "7"])),
            Some(7)
        );
        assert_eq!(owner_pid(&arguments(&["--extensions-only"])), None);
        assert_eq!(owner_pid(&arguments(&["--owner-pid"])), None);
        assert_eq!(owner_pid(&arguments(&["--owner-pid", "app"])), None);
    }

    /// A program that runs for a while and then exits by itself.
    #[cfg(windows)]
    fn long_running() -> std::process::Child {
        std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[cfg(windows)]
    fn exits_within(child: &mut std::process::Child, limit: Duration) -> bool {
        let began = std::time::Instant::now();
        while began.elapsed() < limit {
            if child.try_wait().unwrap().is_some() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[cfg(windows)]
    #[test]
    fn only_one_process_holds_the_instance() {
        let name = format!("Local\\VRFaceTracking.test.{}", std::process::id());
        let first = claim(&name).unwrap();
        assert!(claim(&name).is_err());
        drop(first);
        assert!(claim(&name).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn closing_the_job_ends_what_is_in_it() {
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        let job = kill_on_close_job().unwrap();
        let mut child = long_running();
        unsafe {
            AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle()))
                .unwrap()
        };
        assert!(!exits_within(&mut child, Duration::from_millis(200)));
        drop(job);
        assert!(exits_within(&mut child, Duration::from_secs(5)));
    }

    #[cfg(windows)]
    #[test]
    fn stops_when_the_owner_exits() {
        let mut owner = long_running();
        let running = Arc::new(AtomicBool::new(true));
        stop_with_owner(owner.id(), running.clone());
        thread::sleep(Duration::from_millis(200));
        assert!(running.load(Ordering::SeqCst));
        owner.kill().unwrap();
        owner.wait().unwrap();
        let began = std::time::Instant::now();
        while running.load(Ordering::SeqCst) && began.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(!running.load(Ordering::SeqCst));
    }
}
