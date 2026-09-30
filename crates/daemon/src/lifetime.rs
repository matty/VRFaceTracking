//! How long the daemon runs, and what goes with it: one daemon at a time,
//! the programs it starts ending when it ends, stopping with the desktop app
//! that started it, and a deadline on stopping once asked.
use anyhow::Result;
use log::{info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use vrft_protocol::{DAEMON_INSTANCE, DAEMON_PID, OWNER_PID_ARG};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
#[cfg(windows)]
use windows::core::PCWSTR;
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

/// How long stopping may take once asked before the daemon ends itself. A
/// module stuck in its update would otherwise keep it running. Shorter than
/// the desktop app waits as it closes, so the daemon ends itself first.
const STOP_DEADLINE: Duration = Duration::from_secs(4);

/// How long a starting daemon waits for the instance. The app's check takes
/// it for a moment to see whether a daemon holds it.
#[cfg(windows)]
const CLAIM_WAIT: Duration = Duration::from_millis(500);

#[cfg(windows)]
const ALREADY_RUNNING: &str =
    "Another vrft_d is already running, so this one isn't starting: both would send everything to VRChat";

/// Held while this is the running daemon. A mutex belongs to the thread that
/// takes it, so this is dropped on the thread that claimed it: `main`'s.
pub struct Instance {
    #[cfg(windows)]
    mutex: OwnedHandle,
    /// This process's id, where the app finds it while it doesn't answer.
    #[cfg(windows)]
    _pid: Option<OwnedHandle>,
}

#[cfg(windows)]
impl Drop for Instance {
    fn drop(&mut self) {
        use windows::Win32::System::Threading::ReleaseMutex;
        // SAFETY: the handle is open. On another thread this fails, and the
        // mutex is let go as the claiming thread ends.
        let _ = unsafe { ReleaseMutex(HANDLE(self.mutex.as_raw_handle())) };
    }
}

/// Marks this process as the running daemon, or fails if another daemon is
/// running: both would send every expression to VRChat.
pub fn claim_instance() -> Result<Instance> {
    claim(DAEMON_INSTANCE, DAEMON_PID)
}

/// Takes the mutex `name` and publishes this process's id as `pid_name`.
/// Owning the mutex, not its being there, is what makes the daemon: the
/// app's check, or a daemon on its way out, can have it open without owning
/// it.
#[cfg(windows)]
fn claim(name: &str, pid_name: &str) -> Result<Instance> {
    use windows::Win32::Foundation::{
        ERROR_ACCESS_DENIED, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
    let wide = wide(name);
    // SAFETY: the name is NUL-terminated and outlives the call, and the
    // handle is owned, and closed, from here on.
    let mutex = match unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) } {
        Ok(handle) => unsafe { OwnedHandle::from_raw_handle(handle.0) },
        // Made by a daemon running as administrator.
        Err(error) if error.code() == ERROR_ACCESS_DENIED.to_hresult() => {
            anyhow::bail!(ALREADY_RUNNING)
        }
        Err(error) => return Err(error.into()),
    };
    let wait = CLAIM_WAIT.as_millis() as u32;
    // SAFETY: the handle is open for the wait.
    let event = unsafe { WaitForSingleObject(HANDLE(mutex.as_raw_handle()), wait) };
    if event == WAIT_TIMEOUT {
        anyhow::bail!(ALREADY_RUNNING);
    }
    // Abandoned, it's taken all the same: the daemon that held it ended
    // without letting go.
    if event != WAIT_OBJECT_0 && event != WAIT_ABANDONED {
        return Err(windows::core::Error::from_thread().into());
    }
    let pid = publish_pid(pid_name)
        .inspect_err(|error| warn!("The app can't end vrft_d while it doesn't answer: {error}"))
        .ok();
    Ok(Instance { mutex, _pid: pid })
}

#[cfg(not(windows))]
fn claim(_name: &str, _pid_name: &str) -> Result<Instance> {
    Ok(Instance {})
}

/// Writes this process's id to the shared memory `name`, which lasts while
/// the returned handle is open.
#[cfg(windows)]
fn publish_pid(name: &str) -> windows::core::Result<OwnedHandle> {
    use windows::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows::Win32::System::Memory::{
        CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_WRITE, PAGE_READWRITE,
    };
    let wide = wide(name);
    let size = std::mem::size_of::<u32>();
    // SAFETY: the name is NUL-terminated and outlives the call, the mapping
    // is owned from creation, and the view is `size` bytes, written once and
    // unmapped.
    unsafe {
        let mapping = OwnedHandle::from_raw_handle(
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                0,
                size as u32,
                PCWSTR(wide.as_ptr()),
            )?
            .0,
        );
        let view = MapViewOfFile(HANDLE(mapping.as_raw_handle()), FILE_MAP_WRITE, 0, 0, size);
        if view.Value.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        view.Value.cast::<u32>().write_unaligned(std::process::id());
        UnmapViewOfFile(view)?;
        Ok(mapping)
    }
}

/// `name` as a NUL-terminated wide string.
#[cfg(windows)]
fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}

/// Starts the job that the programs the daemon starts join, so they end when
/// it ends, however it ends. Which programs join, and why, is in
/// [`vrft_api::job`]. The daemon keeps the job's only handle, which Windows
/// closes as it exits, and isn't in the job itself.
pub fn end_children_with_daemon() {
    #[cfg(windows)]
    match kill_on_close_job() {
        Ok(job) => vrft_api::job::set_daemon_job(job),
        Err(error) => warn!("Programs vrft_d starts may keep running after it stops: {error}"),
    }
}

/// A job that ends every process in it when its last handle closes. What
/// they start stays out of it (silent breakaway), so only the programs put
/// in it end with it.
#[cfg(windows)]
fn kill_on_close_job() -> windows::core::Result<OwnedHandle> {
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
    };
    // SAFETY: the job handle is owned from creation, and the limits are
    // passed with their own size.
    unsafe {
        let job = OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null())?.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK;
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

    /// A test's own instance and process id names.
    #[cfg(windows)]
    fn names(tag: &str) -> (String, String) {
        let name = format!("Local\\VRFaceTracking.test.{tag}.{}", std::process::id());
        let pid = format!("{name}.pid");
        (name, pid)
    }

    /// Whether another thread can claim the instance, as another daemon
    /// would. The thread lets it go again as it ends.
    #[cfg(windows)]
    fn claimed_elsewhere(name: &str, pid: &str) -> bool {
        thread::scope(|scope| scope.spawn(|| claim(name, pid).is_ok()).join().unwrap())
    }

    /// The process id published as `name`, while it is.
    #[cfg(windows)]
    fn published_pid(name: &str) -> Option<u32> {
        use windows::Win32::System::Memory::{
            MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
        };
        let wide = wide(name);
        unsafe {
            let mapping = OwnedHandle::from_raw_handle(
                OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(wide.as_ptr()))
                    .ok()?
                    .0,
            );
            let view = MapViewOfFile(HANDLE(mapping.as_raw_handle()), FILE_MAP_READ, 0, 0, 4);
            let pid = view.Value.cast::<u32>().read_unaligned();
            UnmapViewOfFile(view).unwrap();
            Some(pid)
        }
    }

    #[cfg(windows)]
    #[test]
    fn only_one_daemon_holds_the_instance() {
        let (name, pid) = names("one");
        let first = claim(&name, &pid).unwrap();
        assert!(!claimed_elsewhere(&name, &pid));
        drop(first);
        assert!(claimed_elsewhere(&name, &pid));
    }

    #[cfg(windows)]
    #[test]
    fn having_the_instance_open_isnt_holding_it() {
        use windows::Win32::System::Threading::CreateMutexW;
        let (name, pid) = names("open");
        let wide = wide(&name);
        // Open without owning it, as the app's check or an exiting daemon.
        let open = unsafe {
            OwnedHandle::from_raw_handle(
                CreateMutexW(None, false, PCWSTR(wide.as_ptr())).unwrap().0,
            )
        };
        assert!(claimed_elsewhere(&name, &pid));
        drop(open);
    }

    #[cfg(windows)]
    #[test]
    fn the_daemon_publishes_its_process_id_while_it_runs() {
        let (name, pid) = names("pid");
        let instance = claim(&name, &pid).unwrap();
        assert_eq!(published_pid(&pid), Some(std::process::id()));
        drop(instance);
        assert_eq!(published_pid(&pid), None);
    }

    #[cfg(windows)]
    #[test]
    fn what_a_program_in_the_job_starts_stays_out_of_it() {
        use std::io::BufRead as _;
        use std::os::windows::io::AsRawHandle as _;
        use windows::Win32::Foundation::WAIT_TIMEOUT;
        use windows::Win32::System::JobObjects::{AssignProcessToJobObject, IsProcessInJob};
        use windows::Win32::System::Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
        };
        let job = kill_on_close_job().unwrap();
        // Starts ping, as a module might start its maker's runtime, says
        // its process id, and waits.
        let script = "$p = Start-Process ping -ArgumentList '-n','30','127.0.0.1' \
                      -WindowStyle Hidden -PassThru; $p.Id; Start-Sleep 30";
        let mut child = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", script])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // PowerShell takes far longer to start than this takes.
        unsafe {
            AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle()))
                .unwrap()
        };
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid: u32 = line.trim().parse().unwrap();
        let access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
        let handle =
            unsafe { OwnedHandle::from_raw_handle(OpenProcess(access, false, pid).unwrap().0) };
        let started = HANDLE(handle.as_raw_handle());
        let mut inside = windows::core::BOOL(0);
        unsafe { IsProcessInJob(started, Some(HANDLE(job.as_raw_handle())), &mut inside) }.unwrap();

        drop(job);
        let ended = exits_within(&mut child, Duration::from_secs(5));
        let running_on = unsafe { WaitForSingleObject(started, 0) } == WAIT_TIMEOUT;
        let _ = unsafe { TerminateProcess(started, 1) };
        assert!(!inside.as_bool(), "what it started isn't in the job");
        assert!(ended, "the program in the job ends with it");
        assert!(running_on, "what it started keeps running");
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
