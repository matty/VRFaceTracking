//! Proxy module that communicates with a .NET VrcftRuntime process via shared memory.
//!
//! Uses raw Windows API for compatibility with .NET's MemoryMappedFile.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Child, Command};

use crate::{ModuleLogger, TrackingModule, UnifiedTrackingData};

/// The shared memory name (must match the C# side exactly).
const SHMEM_NAME: &str = "Local\\VRCFT_TrackingData";

/// Size of the marshaled data structure (must match C# MarshaledTrackingData).
const SHMEM_SIZE: usize = std::mem::size_of::<MarshaledTrackingData>();

pub struct ProxyModule {
    child: Option<Child>,
    shmem_handle: Option<windows::Win32::Foundation::HANDLE>,
    shmem_ptr: Option<*mut std::ffi::c_void>,
}

// SAFETY: The shared memory pointer is only accessed from a single thread.
unsafe impl Send for ProxyModule {}

#[repr(C, packed)]
struct MarshaledTrackingData {
    left_eye_gaze_x: f32,
    left_eye_gaze_y: f32,
    left_eye_gaze_z: f32,
    left_eye_pupil_diameter_mm: f32,
    left_eye_openness: f32,

    right_eye_gaze_x: f32,
    right_eye_gaze_y: f32,
    right_eye_gaze_z: f32,
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
}

impl ProxyModule {
    pub fn new() -> Self {
        Self {
            child: None,
            shmem_handle: None,
            shmem_ptr: None,
        }
    }

    pub fn start(&mut self, proxy_exe: &Path, module_dll: &Path) -> Result<()> {
        let child = Command::new(proxy_exe)
            .arg(module_dll)
            .spawn()
            .context("Failed to spawn VrcftRuntime")?;

        self.child = Some(child);

        // Wait for the host to create shared memory, then open it
        let mut retry = 0;
        let max_retries = 100; // 10 seconds total

        let (handle, ptr) = loop {
            match Self::open_shared_memory() {
                Ok(result) => break result,
                Err(e) => {
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

        log::info!("Successfully connected to shared memory: {}", SHMEM_NAME);
        Ok(())
    }

    /// Opens the shared memory created by the .NET proxy host using Windows API.
    fn open_shared_memory(
    ) -> Result<(windows::Win32::Foundation::HANDLE, *mut std::ffi::c_void)> {
        use windows::core::PCSTR;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{
            MapViewOfFile, OpenFileMappingA, FILE_MAP_READ, FILE_MAP_WRITE,
        };

        // Convert the name to a null-terminated C string
        let name_cstr =
            std::ffi::CString::new(SHMEM_NAME).context("Invalid shared memory name")?;

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

impl Default for ProxyModule {
    fn default() -> Self {
        Self::new()
    }
}

impl TrackingModule for ProxyModule {
    fn initialize(&mut self, _logger: ModuleLogger) -> Result<()> {
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        if let Some(ptr) = self.shmem_ptr {
            unsafe {
                let m_data = &*(ptr as *const MarshaledTrackingData);

                data.eye.left.gaze.x = m_data.left_eye_gaze_x;
                data.eye.left.gaze.y = m_data.left_eye_gaze_y;
                data.eye.left.gaze.z = m_data.left_eye_gaze_z;
                data.eye.left.pupil_diameter_mm = m_data.left_eye_pupil_diameter_mm;
                data.eye.left.openness = m_data.left_eye_openness;

                data.eye.right.gaze.x = m_data.right_eye_gaze_x;
                data.eye.right.gaze.y = m_data.right_eye_gaze_y;
                data.eye.right.gaze.z = m_data.right_eye_gaze_z;
                data.eye.right.pupil_diameter_mm = m_data.right_eye_pupil_diameter_mm;
                data.eye.right.openness = m_data.right_eye_openness;

                data.eye.max_dilation = m_data.eye_max_dilation;
                data.eye.min_dilation = m_data.eye_min_dilation;
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
        }
        Ok(())
    }

    fn unload(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::UnmapViewOfFile;

        unsafe {
            if let Some(ptr) = self.shmem_ptr.take() {
                let _ = UnmapViewOfFile(windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: ptr,
                });
            }
            if let Some(handle) = self.shmem_handle.take() {
                let _ = CloseHandle(handle);
            }
        }

        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
        }
    }
}
