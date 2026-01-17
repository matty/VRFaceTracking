use netclr::{init_dotnet_host, DotNetModuleWrapper};
use api::{UnifiedTrackingData, TrackingModule};
use std::path::PathBuf;
use std::fs;

#[test]
fn test_load_and_run_module() {
    let _ = env_logger::builder().is_test(true).try_init();

    // 1. Initialize Host
    init_dotnet_host().expect("Failed to init dotnet host");

    // 2. Locate Artifacts
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    // Root is vrft_d/netclr
    let bridge_dll = root.join("VrcftBridge/bin/Release/net10.0/VrcftBridge.dll");
    let module_dll = root.join("tests/TestModule/bin/Release/net10.0/TestModule.dll");
    let module_config = root.join("tests/TestModule/bin/Release/net10.0/TestModule.runtimeconfig.json");
    let bridge_config = root.join("VrcftBridge/bin/Release/net10.0/VrcftBridge.runtimeconfig.json");

    assert!(bridge_dll.exists(), "Bridge DLL not found at {:?}", bridge_dll);
    assert!(module_dll.exists(), "Module DLL not found at {:?}", module_dll);

    // 3. Setup Test Dir
    let test_dir = root.join("../../target/test_run_netclr");
    if test_dir.exists() {
        fs::remove_dir_all(&test_dir).unwrap();
    }
    fs::create_dir_all(&test_dir).unwrap();

    let dest_module = test_dir.join("TestModule.dll");
    let dest_bridge = test_dir.join("VrcftBridge.dll");
    let dest_config = test_dir.join("TestModule.runtimeconfig.json");
    let dest_bridge_config = test_dir.join("VrcftBridge.runtimeconfig.json");

    fs::copy(&module_dll, &dest_module).unwrap();
    fs::copy(&bridge_dll, &dest_bridge).unwrap();
    fs::copy(&module_config, &dest_config).unwrap();
    fs::copy(&bridge_config, &dest_bridge_config).unwrap();
    
    // Copy Core DLL too if it's not self-contained
    let core_dll = root.join("VRCFaceTracking.Core/bin/Release/net10.0/VRCFaceTracking.Core.dll");
    assert!(core_dll.exists(), "Core DLL not found at {:?}", core_dll);
    fs::copy(&core_dll, test_dir.join("VRCFaceTracking.Core.dll")).unwrap();

    // 4. Load Module
    let mut wrapper = DotNetModuleWrapper::load(&dest_module).expect("Failed to load module");

    // 5. Test Update
    let mut data = UnifiedTrackingData::default();
    wrapper.update(&mut data).expect("Update failed");

    // Check values set by TestModule
    assert_eq!(data.eye.left.openness, 0.42);
    // Index 0 shape
    assert_eq!(data.shapes[0].weight, 0.99);
    
    wrapper.unload();
}
