// This is a test plugin demonstrating module logging capabilities.

use anyhow::Result;
use api::{ModuleLogger, TrackingModule, UnifiedTrackingData};

pub struct TestLogger {
    frame_count: u64,
    logger: Option<ModuleLogger>,
}

impl TestLogger {
    fn new() -> Self {
        Self {
            frame_count: 0,
            logger: None,
        }
    }
}

impl TrackingModule for TestLogger {
    fn initialize(&mut self, logger: ModuleLogger) -> Result<()> {
        logger.info("Initializing test logger plugin");
        logger.debug("Debug: Plugin initialization details");
        logger.trace("Trace: Very detailed initialization info");
        self.logger = Some(logger);
        Ok(())
    }

    fn update(&mut self, _data: &mut UnifiedTrackingData) -> Result<()> {
        self.frame_count += 1;

        if let Some(logger) = &self.logger {
            // Log at various levels during the first few frames
            match self.frame_count {
                1 => {
                    logger.info(&format!("Update called - frame {}", self.frame_count));
                    logger.info("This plugin demonstrates module logging capabilities");
                }
                2 => {
                    logger.warn(&format!(
                        "This is a warning from the plugin - frame {}",
                        self.frame_count
                    ));
                }
                3 => {
                    logger.debug(&format!(
                        "Debug message - frame {} (only visible with RUST_LOG=debug)",
                        self.frame_count
                    ));
                }
                5 => {
                    logger.trace(&format!(
                        "Trace message - frame {} (only visible with RUST_LOG=trace)",
                        self.frame_count
                    ));
                }
                10 => {
                    logger.info("Frame 10 reached. Plugin will now run silently.");
                }
                100 => {
                    logger.info("Frame 100 milestone");
                }
                1000 => {
                    logger.info("Frame 1000 milestone");
                }
                _ => {
                    // Silent after initial frames
                    logger.trace(&format!("Frame {}", self.frame_count));
                }
            }
        }

        Ok(())
    }

    fn unload(&mut self) {
        if let Some(logger) = &self.logger {
            logger.info(&format!(
                "Tearing down test logger plugin. Total frames processed: {}",
                self.frame_count
            ));
        }
    }
}

#[no_mangle]
#[allow(improper_ctypes_definitions)]
pub extern "C" fn create_module() -> *mut dyn TrackingModule {
    Box::into_raw(Box::new(TestLogger::new()))
}
