//! .NET CoreCLR hosting for VRCFaceTracking modules

mod hosting;
mod marshaling;
mod module_wrapper;

pub use module_wrapper::DotNetModuleWrapper;
pub use hosting::init_dotnet_host;

/// Errors specific to .NET module loading
#[derive(Debug, thiserror::Error)]
pub enum NetClrError {
    #[error("Failed to initialize .NET runtime: {0}")]
    RuntimeInit(String),
    #[error("Failed to load assembly: {0}")]
    AssemblyLoad(String),
    #[error("Module does not inherit from ExtTrackingModule")]
    InvalidModuleType,
    #[error("Method not found: {0}")]
    MethodNotFound(String),
}
