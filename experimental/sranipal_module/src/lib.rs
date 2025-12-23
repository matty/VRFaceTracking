pub mod ffi;
pub mod mapping;
pub mod sranipal;

use api::TrackingModule;
use sranipal::SRanipalModule;

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(SRanipalModule::new())
}
