use api::TrackingModule;
use pico::PicoModule;

mod config_setup;
mod data;
mod mapping;
mod pico;

#[no_mangle]
pub extern "C" fn create_module() -> Box<dyn TrackingModule> {
    Box::new(PicoModule::new())
}
