# PowerShell script to build and run vrft_d with debug logging
#
# Stages build artifacts into a "run" directory with proper native plugin structure.

$ErrorActionPreference = "Stop"

$env:RUST_LOG = "info,vrft_d=debug,vd_module=debug"

$runDir = "run"
$pluginsDir = Join-Path $runDir "plugins/native"

Write-Host "Building workspace in debug mode..." -ForegroundColor Cyan
cargo build

if ($LASTEXITCODE -ne 0) {
    Write-Error "Build failed!"
    exit $LASTEXITCODE
}

# Prepare run directory structure
if (!(Test-Path $pluginsDir)) {
    Write-Host "Creating run directory structure..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Path $pluginsDir -Force | Out-Null
}

# Stage artifacts
Write-Host "Staging artifacts to $runDir..." -ForegroundColor Cyan
Copy-Item "target/debug/vrft_d.exe" -Destination $runDir -Force
Copy-Item "target/debug/vd_module.dll" -Destination $pluginsDir -Force
Copy-Item "config.json" -Destination $runDir -Force

Write-Host "Build successful. Launching vrft_d from $runDir..." -ForegroundColor Green
Push-Location $runDir
try {
    ./vrft_d.exe
}
finally {
    Pop-Location
}
