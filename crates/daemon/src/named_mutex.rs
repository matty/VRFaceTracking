//! Windows named mutexes, which processes share by name.
use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

/// Held while it lives. A mutex belongs to the thread that takes it, so this
/// is dropped on that thread.
pub struct NamedMutex {
    handle: OwnedHandle,
}

impl NamedMutex {
    /// Takes the mutex `name`, made if it isn't there yet, waiting up to
    /// `wait` while another holds it. `None` when the wait runs out. One
    /// abandoned, by a holder that ended without letting go, is taken all
    /// the same.
    pub fn acquire(name: &str, wait: Duration) -> windows::core::Result<Option<Self>> {
        let wide = wide(name);
        // SAFETY: the name is NUL-terminated and outlives the call, and the
        // handle is owned, and closed, from here on.
        let handle = unsafe {
            OwnedHandle::from_raw_handle(CreateMutexW(None, false, PCWSTR(wide.as_ptr()))?.0)
        };
        let wait = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the handle is open for the wait.
        let event = unsafe { WaitForSingleObject(HANDLE(handle.as_raw_handle()), wait) };
        if event == WAIT_TIMEOUT {
            return Ok(None);
        }
        if event != WAIT_OBJECT_0 && event != WAIT_ABANDONED {
            return Err(windows::core::Error::from_thread());
        }
        Ok(Some(Self { handle }))
    }
}

impl Drop for NamedMutex {
    fn drop(&mut self) {
        // SAFETY: the handle is open. On another thread this fails, and the
        // mutex is let go as the taking thread ends.
        let _ = unsafe { ReleaseMutex(HANDLE(self.handle.as_raw_handle())) };
    }
}

/// `name` as a NUL-terminated wide string.
pub fn wide(name: &str) -> Vec<u16> {
    name.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(tag: &str) -> String {
        format!(
            "Local\\VRFaceTracking.test.mutex.{tag}.{}",
            std::process::id()
        )
    }

    /// Whether another thread can take `name` straight away.
    fn free_elsewhere(name: &str) -> bool {
        std::thread::scope(|scope| {
            scope
                .spawn(|| NamedMutex::acquire(name, Duration::ZERO).unwrap().is_some())
                .join()
                .unwrap()
        })
    }

    #[test]
    fn one_thread_holds_it_until_dropped() {
        let name = name("hold");
        let held = NamedMutex::acquire(&name, Duration::ZERO).unwrap().unwrap();
        assert!(!free_elsewhere(&name));
        drop(held);
        assert!(free_elsewhere(&name));
    }
}
