use crate::hosting::load_assembly;
use crate::marshaling::MarshaledTrackingData;
use crate::NetClrError;
use api::{ModuleLogger, TrackingModule, UnifiedTrackingData};
use anyhow::{Context, Result};
use std::path::Path;
use netcorehost::pdcstring::PdCString;
use netcorehost::hostfxr::{HostfxrContext, InitializedForRuntimeConfig};
use std::ffi::{CStr, CString};
use log::{debug, error, info, trace, warn};

// Matches C# delegate: void LogCallback(int level, IntPtr target, IntPtr message)
type LogCallback = extern "C" fn(i32, *const i8, *const i8);

extern "C" fn bridge_logger(level: i32, target: *const i8, message: *const i8) {
    let target_str = unsafe { CStr::from_ptr(target).to_string_lossy() };
    let message_str = unsafe { CStr::from_ptr(message).to_string_lossy() };
    
    // api::LogLevel enum: Error=1, Warn=2, Info=3, Debug=4, Trace=5
    match level {
        1 => error!(target: &target_str, "{}", message_str),
        2 => warn!(target: &target_str, "{}", message_str),
        3 => info!(target: &target_str, "{}", message_str),
        4 => debug!(target: &target_str, "{}", message_str),
        5 => trace!(target: &target_str, "{}", message_str),
        _ => info!(target: &target_str, "[Unknown Level {}] {}", level, message_str),
    }
}

pub struct DotNetModuleWrapper {
    _name: String,
    _context: HostfxrContext<InitializedForRuntimeConfig>,
    update_fn: extern "system" fn(*mut MarshaledTrackingData),
    teardown_fn: extern "system" fn(),
}

impl DotNetModuleWrapper {
    pub fn load(module_path: &Path) -> Result<Self> {
        let name = module_path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        // Locate VrcftBridge.dll
        // We look in the same directory as the module if possible, or CWD
        let mut bridge_path = module_path.parent().unwrap_or(Path::new("")).join("VrcftBridge.dll");
        if !bridge_path.exists() {
             bridge_path = Path::new("VrcftBridge.dll").to_path_buf();
        }
        if !bridge_path.exists() {
             return Err(NetClrError::AssemblyLoad("VrcftBridge.dll not found in module dir or CWD".into()).into());
        }

        let context = load_assembly(&bridge_path)?;
        let bridge_path_pd = PdCString::from_os_str(bridge_path.as_os_str())
            .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
        let loader = context.get_delegate_loader_for_assembly(bridge_path_pd)?;

        let type_name_str = "VrcftBridge.ModuleHost, VrcftBridge";
        let type_name = PdCString::from_os_str(type_name_str)
            .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
        
        let load_method = PdCString::from_os_str("LoadModule")
             .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
        let update_method = PdCString::from_os_str("Update")
             .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
        let teardown_method = PdCString::from_os_str("Teardown")
             .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
        
        // Use get_function_with_unmanaged_callers_only for UnmanagedCallersOnly methods
        // Signature: int LoadModule(IntPtr assemblyPath, LogCallback logger)
        let load_fn = loader.get_function_with_unmanaged_callers_only::<extern "system" fn(*const i8, LogCallback) -> i32>(
            &type_name,
            &load_method,
        ).context("Failed to get LoadModule delegate")?;
        
        let update_fn = loader.get_function_with_unmanaged_callers_only::<extern "system" fn(*mut MarshaledTrackingData)>(
            &type_name,
            &update_method,
        ).context("Failed to get Update delegate")?;
        
        let teardown_fn = loader.get_function_with_unmanaged_callers_only::<extern "system" fn()>(
            &type_name,
            &teardown_method,
        ).context("Failed to get Teardown delegate")?;

        // Load the user module
        let abs_module_path = std::fs::canonicalize(module_path)
            .context(format!("Failed to canonicalize module path {:?}", module_path))?;
        
        let path_str = abs_module_path.to_string_lossy().to_string();
        let path_c = CString::new(path_str).context("Failed to create CString from path")?;
        
        // Pass the bridge_logger function pointer
        let res = load_fn(path_c.as_ptr(), bridge_logger);
        if res != 0 {
             return Err(NetClrError::AssemblyLoad(format!("Bridge LoadModule returned error code {}", res)).into());
        }

        Ok(Self {
            _name: name,
            _context: context,
            update_fn: *update_fn,
            teardown_fn: *teardown_fn,
        })
    }
}

impl TrackingModule for DotNetModuleWrapper {
    fn initialize(&mut self, _logger: ModuleLogger) -> Result<()> {
        // Logging is handled by the global bridge_logger passed during load()
        // We can ignore the per-module logger for now as VRCFT architecture 
        // implies a singleton/static context for many things.
        Ok(())
    }

    fn update(&mut self, data: &mut UnifiedTrackingData) -> Result<()> {
        let mut marshaled = MarshaledTrackingData::from(&*data);
        (self.update_fn)(&mut marshaled);
        *data = UnifiedTrackingData::from(&marshaled);
        Ok(())
    }

    fn unload(&mut self) {
        (self.teardown_fn)();
    }
}
