# PowerShell script to build and run vrft_d with debug logging

# Set environment variables for debug logging (only for our app to reduce noise)
$env:RUST_LOG = "info,vrft_d=debug,vd_module=debug"

Write-Host "Building workspace in debug mode..." -ForegroundColor Cyan
cargo build

if ($LASTEXITCODE -ne 0) {
    Write-Error "Build failed!"
    exit $LASTEXITCODE
}

# Ensure the plugins/native directory exists
$pluginsDir = "plugins/native"
if (!(Test-Path $pluginsDir)) {
    Write-Host "Creating $pluginsDir..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Path $pluginsDir | Out-Null
}

# Copy the built vd_module.dll to the plugins folder
Write-Host "Syncing vd_module.dll to $pluginsDir..." -ForegroundColor Cyan
Copy-Item "target/debug/vd_module.dll" -Destination $pluginsDir

Write-Host "Build successful. Launching vrft_d..." -ForegroundColor Green
./target/debug/vrft_d.exe
