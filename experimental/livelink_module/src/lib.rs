pub mod livelink;

use api::TrackingModule;
use livelink::LiveLinkModule;

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(LiveLinkModule::new())
}
