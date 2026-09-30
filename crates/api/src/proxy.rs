//! Proxy module that communicates with a .NET runtime process via shared memory.
//!
//! Uses raw Windows API for compatibility with .NET's MemoryMappedFile.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Child, Command};

use crate::{ModuleLogger, TrackingModule, UnifiedTrackingData};

/// Shared memory name (must match the .NET side exactly).
const SHMEM_NAME: &str = "Local\\VRCFT_TrackingData";

/// Size of the marshaled data structure (must match .NET MarshaledTrackingData).
const SHMEM_SIZE: usize = std::mem::size_of::<MarshaledTrackingData>();

pub struct ProxyModule {
    child: Option<Child>,
    shmem_handle: Option<windows::Win32::Foundation::HANDLE>,
    shmem_ptr: Option<*mut std::ffi::c_void>,
    proxy_exe: Option<std::path::PathBuf>,
    module_dll: Option<std::path::PathBuf>,
    last_runtime_heartbeat: u64,
    last_runtime_update: std::time::Instant,
    /// The smallest and largest pupil diameters seen, as VRCFaceTracking
    /// learns them: its modules never set a dilation range themselves.
    dilation_range: Option<(f32, f32)>,
}

// SAFETY: The shared memory pointer is only accessed from a single thread.
unsafe impl Send for ProxyModule {}

#[repr(C, packed)]
struct MarshaledTrackingData {
    left_eye_gaze_x: f32,
    left_eye_gaze_y: f32,
    left_eye_pupil_diameter_mm: f32,
    left_eye_openness: f32,

    right_eye_gaze_x: f32,
    right_eye_gaze_y: f32,
    right_eye_pupil_diameter_mm: f32,
    right_eye_openness: f32,

    eye_max_dilation: f32,
    eye_min_dilation: f32,
    eye_left_diameter: f32,
    eye_right_diameter: f32,

    head_yaw: f32,
    head_pitch: f32,
    head_roll: f32,
    head_pos_x: f32,
    head_pos_y: f32,
    head_pos_z: f32,

    shapes: [f32; 200],
    main_app_heartbeat: u64,
    runtime_heartbeat: u64,
}

impl ProxyModule {
    pub fn new() -> Self {
        Self {
            child: None,
            shmem_handle: None,
            shmem_ptr: None,
            proxy_exe: None,
            module_dll: None,
            last_runtime_heartbeat: 0,
            last_runtime_update: std::time::Instant::now(),
            dilation_range: None,
        }
    }

    pub fn start(&mut self, proxy_exe: &Path, module_dll: &Path) -> Result<()> {
        self.proxy_exe = Some(proxy_exe.to_path_buf());
        self.module_dll = Some(module_dll.to_path_buf());

        self.spawn_child()?;
        self.connect_shmem()?;

        log::info!("Successfully connected to shared memory: {}", SHMEM_NAME);
        Ok(())
    }

    fn spawn_child(&mut self) -> Result<()> {
        let proxy_exe = self.proxy_exe.as_ref().context("proxy_exe not set")?;
        let module_dll = self.module_dll.as_ref().context("module_dll not set")?;

        // Get current Rust log level and pass it to the .NET host
        let log_level = match log::max_level() {
            log::LevelFilter::Trace => "trace",
            log::LevelFilter::Debug => "debug",
            log::LevelFilter::Info => "info",
            log::LevelFilter::Warn => "warn",
            log::LevelFilter::Error => "error",
            log::LevelFilter::Off => "error",
        };

        let child = Command::new(proxy_exe)
            .arg(module_dll)
            .arg(log_level)
            .spawn()
            .context("Failed to spawn VrcftRuntime")?;

        self.child = Some(child);
        Ok(())
    }

    fn connect_shmem(&mut self) -> Result<()> {
        // Wait for the host to create shared memory, then open it
        let mut retry = 0;
        let max_retries = 100; // 10 seconds total

        let (handle, ptr) = loop {
            match Self::open_shared_memory() {
                Ok(result) => break result,
                Err(e) => {
                    // A host that couldn't load the module exits without
                    // creating the shared memory; don't wait out the retries.
                    if let Some(status) = self
                        .child
                        .as_mut()
                        .and_then(|child| child.try_wait().ok().flatten())
                    {
                        anyhow::bail!(
                            "VrcftRuntime exited ({status}) before it was ready. Its log says why."
                        );
                    }
                    if retry >= max_retries {
                        return Err(e).context(format!(
                            "Failed to open shared memory '{}' after {} retries",
                            SHMEM_NAME, max_retries
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    retry += 1;
                }
            }
        };

        self.shmem_handle = Some(handle);
        self.shmem_ptr = Some(ptr);
        self.last_runtime_update = std::time::Instant::now();
        Ok(())
    }

    /// Opens the shared memory created by the .NET proxy host using Windows API.
    fn open_shared_memory() -> Result<(windows::Win32::Foundation::HANDLE, *mut std::ffi::c_void)> {
        use windows::core::PCSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{
            MapViewOfFile, OpenFileMappingA, FILE_MAP_READ, FILE_MAP_WRITE,
        };

        // Convert the name to a null-terminated C string
        let name_cstr = std::ffi::CString::new(SHMEM_NAME).context("Invalid shared memory name")?;

        unsafe {
            // Open existing file mapping
            let handle = OpenFileMappingA(
                (FILE_MAP_READ | FILE_MAP_WRITE).0,
                false,
                PCSTR::from_raw(name_cstr.as_ptr() as *const u8),
            )
            .context("OpenFileMappingA failed")?;

            // Map the view
            let ptr = MapViewOfFile(handle, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, SHMEM_SIZE);

            if ptr.Value.is_null() {
                let _ = CloseHandle(handle);
                anyhow::bail!("MapViewOfFile returned null");
            }

            Ok((handle, ptr.Value))
        }
    }
}

impl ProxyModule {
    /// Widens the learned dilation range to take in this frame's pupils,
    /// and gives the frame that range.
    fn learn_dilation(&mut self, data: &mut UnifiedTrackingData) {
        for diameter in [
            data.eye.left.pupil_diameter_mm,
            data.eye.right.pupil_diameter_mm,
        ] {
            if diameter.is_finite() && diameter > 0.0 {
                let (min, max) = self.dilation_range.get_or_insert((diameter, diameter));
                *min = min.min(diameter);
                *max = max.max(diameter);
            }
        }
        if let Some((min, max)) = self.dilation_range {
            data.eye.min_dilation = min;
            data.eye.max_dilation = max;
        }
    }
}

impl Default for ProxyModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ProxyModule {
    /// Beats the daemon's heartbeat and, when the runtime has written a
    /// frame since the last read, copies it into `data`. The runtime writes
    /// one each time its heartbeat beats. A frame read again isn't new:
    /// counting it as one let the daemon's loop spin a whole core when
    /// tracking has no frame limit.
    fn read_frame(&mut self, data: &mut UnifiedTrackingData) -> bool {
        let Some(ptr) = self.shmem_ptr else {
            return false;
        };
        unsafe {
            let m_data_mut = &mut *(ptr as *mut MarshaledTrackingData);

            // Increment main app heartbeat
            m_data_mut.main_app_heartbeat = m_data_mut.main_app_heartbeat.wrapping_add(1);

            let m_data = &*m_data_mut;

            if m_data.runtime_heartbeat == self.last_runtime_heartbeat {
                return false;
            }
            self.last_runtime_heartbeat = m_data.runtime_heartbeat;
            self.last_runtime_update = std::time::Instant::now();

            // Copied across unchanged: the .NET side already stores gaze in
            // the convention documented on UnifiedSingleEyeData::gaze, so
            // this path must not reorder or rescale the components.
            data.eye.left.gaze.x = m_data.left_eye_gaze_x;
            data.eye.left.gaze.y = m_data.left_eye_gaze_y;
            data.eye.left.pupil_diameter_mm = m_data.left_eye_pupil_diameter_mm;
            data.eye.left.openness = m_data.left_eye_openness;

            data.eye.right.gaze.x = m_data.right_eye_gaze_x;
            data.eye.right.gaze.y = m_data.right_eye_gaze_y;
            data.eye.right.pupil_diameter_mm = m_data.right_eye_pupil_diameter_mm;
            data.eye.right.openness = m_data.right_eye_openness;

            data.eye.max_dilation = m_data.eye_max_dilation;
            data.eye.min_dilation = m_data.eye_min_dilation;
            if data.eye.max_dilation <= data.eye.min_dilation {
                self.learn_dilation(data);
            }
            data.eye.left_diameter = m_data.eye_left_diameter;
            data.eye.right_diameter = m_data.eye_right_diameter;

            data.head.head_yaw = m_data.head_yaw;
            data.head.head_pitch = m_data.head_pitch;
            data.head.head_roll = m_data.head_roll;
            data.head.head_pos_x = m_data.head_pos_x;
            data.head.head_pos_y = m_data.head_pos_y;
            data.head.head_pos_z = m_data.head_pos_z;

            for i in 0..data.shapes.len().min(200) {
                data.shapes[i].weight = m_data.shapes[i];
            }
        }
        true
    }
}

impl TrackingModule for ProxyModule {
    fn initialize(&mut self, _logger: ModuleLogger) -> Result<()> {
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        let fresh = self.read_frame(data);

        // Check for crash or timeout
        let should_restart = if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    log::warn!("VrcftRuntime exited with status: {}. Restarting...", status);
                    true
                }
                Ok(None) => {
                    // Still running, check heartbeat. The host beats from its
                    // own loop, apart from the module's Update(), so a module
                    // that blocks while it waits for its device doesn't stop
                    // it; a lost heartbeat means the host itself is stuck.
                    if self.last_runtime_update.elapsed() > std::time::Duration::from_secs(5) {
                        log::warn!("VrcftRuntime heartbeat lost. Restarting...");
                        let _ = self.child.as_mut().unwrap().kill();
                        true
                    } else {
                        false
                    }
                }
                Err(e) => {
                    log::error!("Error checking child process: {}. Restarting...", e);
                    true
                }
            }
        } else {
            true
        };

        if should_restart {
            self.unload();
            if let Err(e) = self.spawn_child() {
                log::error!("Failed to restart VrcftRuntime: {}", e);
            } else if let Err(e) = self.connect_shmem() {
                log::error!("Failed to reconnect to shared memory: {}", e);
            } else {
                log::info!("VrcftRuntime restarted successfully.");
            }
        }

        if fresh {
            Ok(())
        } else {
            anyhow::bail!("No new frame")
        }
    }

    fn unload(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::UnmapViewOfFile;

        unsafe {
            if let Some(ptr) = self.shmem_ptr.take() {
                let _ =
                    UnmapViewOfFile(windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                        Value: ptr,
                    });
            }
            if let Some(handle) = self.shmem_handle.take() {
                let _ = CloseHandle(handle);
            }
        }

        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            // Wait for it to go, so the shared memory it holds goes too and
            // the next host (after a restart or a module switch) can't find
            // the old one.
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_frame_written_since_the_last_read_is_new() {
        // SAFETY: all zeroes is a valid MarshaledTrackingData, and the
        // allocation is only reached through this pointer until it's freed.
        let shared: *mut MarshaledTrackingData =
            Box::into_raw(Box::new(unsafe { std::mem::zeroed() }));
        let mut proxy = ProxyModule::new();
        proxy.shmem_ptr = Some(shared.cast());
        let mut data = UnifiedTrackingData::default();

        assert!(
            !proxy.read_frame(&mut data),
            "the runtime hasn't written yet"
        );
        unsafe {
            (*shared).runtime_heartbeat = 1;
            (*shared).left_eye_openness = 0.25;
        }
        assert!(proxy.read_frame(&mut data));
        assert_eq!(data.eye.left.openness, 0.25);
        assert!(!proxy.read_frame(&mut data), "the same frame again");
        unsafe { (*shared).runtime_heartbeat = 2 };
        assert!(proxy.read_frame(&mut data));
        // The daemon's side beats on every read, new frame or not.
        assert_eq!(unsafe { (*shared).main_app_heartbeat }, 4);

        proxy.shmem_ptr = None;
        drop(unsafe { Box::from_raw(shared) });
    }
}
