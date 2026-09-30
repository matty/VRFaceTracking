//! The programs the daemon starts that end when it ends, however it ends.
//!
//! The daemon keeps the only handle to a job that ends every process in it
//! as that handle closes. Only what the daemon starts and must not outlive
//! it joins, through [`end_with_daemon`]: the .NET host for VRCFT modules,
//! which would keep the tracking device, and tongue training runs, which
//! would keep the CPU. Anything those, or a module, start for themselves,
//! such as a headset maker's runtime, stays out of the job and keeps running
//! after VRFT stops, as it would under VRCFaceTracking.
use std::process::Child;

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle as _, OwnedHandle};
#[cfg(windows)]
use windows::Win32::Foundation::HANDLE;

#[cfg(windows)]
static JOB: std::sync::OnceLock<OwnedHandle> = std::sync::OnceLock::new();

/// Makes `job` the one [`end_with_daemon`] adds programs to, holding it
/// open until the daemon exits. Only the daemon calls this, once.
#[cfg(windows)]
pub fn set_daemon_job(job: OwnedHandle) {
    if JOB.set(job).is_err() {
        log::warn!("The daemon's job was already set");
    }
}

/// Ends `child` when the daemon ends. Without the daemon's job, such as in a
/// test, it does nothing.
pub fn end_with_daemon(child: &Child) {
    #[cfg(windows)]
    {
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        let Some(job) = JOB.get() else {
            return;
        };
        // SAFETY: both handles are open for the call.
        let joined = unsafe {
            AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(child.as_raw_handle()))
        };
        if let Err(error) = joined {
            log::warn!(
                "Process {} may keep running after vrft_d stops: {error}",
                child.id()
            );
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use windows::Win32::System::JobObjects::{CreateJobObjectW, IsProcessInJob};

    #[test]
    fn a_child_joins_the_daemons_job() {
        use std::os::windows::io::FromRawHandle as _;
        // SAFETY: the job handle is owned from creation.
        let job = unsafe {
            OwnedHandle::from_raw_handle(
                CreateJobObjectW(None, windows::core::PCWSTR::null())
                    .unwrap()
                    .0,
            )
        };
        let raw = HANDLE(job.as_raw_handle());
        set_daemon_job(job);
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        end_with_daemon(&child);
        let mut joined = windows::core::BOOL(0);
        // SAFETY: both handles are open, and `joined` is written in place.
        unsafe { IsProcessInJob(HANDLE(child.as_raw_handle()), Some(raw), &mut joined) }.unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(joined.as_bool());
    }
}
