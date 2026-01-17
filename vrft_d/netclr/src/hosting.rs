use netcorehost::{nethost, pdcstring::PdCString, hostfxr};
use netcorehost::hostfxr::{HostfxrContext, InitializedForRuntimeConfig};
use std::path::Path;
use once_cell::sync::OnceCell;
use crate::NetClrError;

static HOSTFXR: OnceCell<hostfxr::Hostfxr> = OnceCell::new();

/// Initialize the .NET host (call once at startup)
pub fn init_dotnet_host() -> Result<(), NetClrError> {
    let hostfxr = nethost::load_hostfxr()
        .map_err(|e| NetClrError::RuntimeInit(e.to_string()))?;
    HOSTFXR.set(hostfxr).map_err(|_| 
        NetClrError::RuntimeInit("Already initialized".into()))?;
    Ok(())
}

/// Load a .NET assembly and get a function pointer to a managed method
pub fn load_assembly(dll_path: &Path) -> Result<HostfxrContext<InitializedForRuntimeConfig>, NetClrError> {
    let hostfxr = HOSTFXR.get()
        .ok_or(NetClrError::RuntimeInit("Not initialized".into()))?;
    
    // Find or create runtimeconfig.json alongside the DLL
    let config_path = dll_path.with_extension("runtimeconfig.json");
    
    let context = hostfxr.initialize_for_runtime_config(
        PdCString::from_os_str(config_path.as_os_str())
            .map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?
    ).map_err(|e| NetClrError::AssemblyLoad(e.to_string()))?;
    
    Ok(context)
}
