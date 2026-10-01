param([Parameter(Mandatory = $true)][string]$NdkRoot)

$ErrorActionPreference = 'Stop'
$clang = Join-Path $NdkRoot 'toolchains\llvm\prebuilt\windows-x86_64\bin\clang.exe'
if (-not (Test-Path -LiteralPath $clang)) { throw "Android NDK clang not found: $clang" }
$native = Join-Path $PSScriptRoot 'native'
$assets = Join-Path $PSScriptRoot 'app\src\main\assets\native'
New-Item -ItemType Directory -Force -Path $assets | Out-Null
& $clang --target=aarch64-linux-android28 -std=c11 -O3 -Wall -Wextra -fPIC -shared '-Wl,-z,max-page-size=16384' (Join-Path $native 'streamer.c') -o (Join-Path $assets 'libquestpro-camera-streamer-v9.so')
if ($LASTEXITCODE -ne 0) { throw 'Streamer build failed' }
& $clang --target=aarch64-linux-android28 -std=c11 -O3 -Wall -Wextra -fPIE -pie '-DSTREAM_PORT=27273' '-Wl,-z,max-page-size=16384' (Join-Path $native 'relay.c') -o (Join-Path $assets 'questpro-camera-relay-v9')
if ($LASTEXITCODE -ne 0) { throw 'Relay build failed' }
& $clang --target=aarch64-linux-android28 -std=c11 -O2 -Wall -Wextra -fPIE -pie '-Wl,-z,max-page-size=16384' (Join-Path $native 'injector.c') -o (Join-Path $assets 'questpro-camera-injector') -ldl
if ($LASTEXITCODE -ne 0) { throw 'Injector build failed' }
& $clang --target=aarch64-linux-android28 -std=c11 -O2 -Wall -Wextra -fPIC -shared '-Wl,-z,max-page-size=16384' (Join-Path $PSScriptRoot 'diagnostics\provider-map-snapshot.c') -o (Join-Path $PSScriptRoot 'diagnostics\libvrft-camera-diagnostic.so')
if ($LASTEXITCODE -ne 0) { throw 'Diagnostic build failed' }
# The v9 helpers replace the v8 ones; the app no longer ships or runs those.
foreach ($old in 'libquestpro-camera-streamer-v8.so', 'questpro-camera-relay-v8') {
    Remove-Item -LiteralPath (Join-Path $assets $old) -ErrorAction SilentlyContinue
}
