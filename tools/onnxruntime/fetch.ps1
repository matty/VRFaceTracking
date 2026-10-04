# Fetches ONNX Runtime with DirectML for the tongue models' inference:
# onnxruntime.dll and DirectML.dll, plus their licenses, from NuGet.
#   ./tools/onnxruntime/fetch.ps1                      # into .local/onnxruntime/ for a development build
#   ./tools/onnxruntime/fetch.ps1 -Dest <package dir>  # beside vrft_d.exe in a release
# Without these files VRFT runs the models on Burn, as before.
param(
    [string]$Dest = "",
    [string]$Version = "1.24.4",
    [string]$DirectMLVersion = "1.15.4"
)
$ErrorActionPreference = "Stop"
if (-not $Dest) { $Dest = Join-Path $PSScriptRoot "../../.local/onnxruntime" }
New-Item -ItemType Directory -Force -Path $Dest | Out-Null
$staging = Join-Path ([IO.Path]::GetTempPath()) "vrft-onnxruntime-$PID"
New-Item -ItemType Directory -Force -Path $staging | Out-Null
try {
    foreach ($package in @(
            @{ Id = "Microsoft.ML.OnnxRuntime.DirectML"; Version = $Version; Files = @("runtimes/win-x64/native/onnxruntime.dll"); License = "LICENSE"; Name = "onnxruntime" },
            @{ Id = "Microsoft.AI.DirectML"; Version = $DirectMLVersion; Files = @("bin/x64-win/DirectML.dll"); License = "LICENSE.txt"; Name = "DirectML" })) {
        $archive = Join-Path $staging "$($package.Id).zip"
        Invoke-WebRequest "https://www.nuget.org/api/v2/package/$($package.Id)/$($package.Version)" -OutFile $archive
        $unpacked = Join-Path $staging $package.Id
        Expand-Archive $archive -DestinationPath $unpacked
        foreach ($file in $package.Files) {
            Copy-Item (Join-Path $unpacked $file) $Dest
        }
        $license = Get-ChildItem $unpacked -Filter $package.License -Recurse | Select-Object -First 1
        if ($license) { Copy-Item $license.FullName (Join-Path $Dest "$($package.Name)-LICENSE.txt") }
    }
} finally {
    Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Host "ONNX Runtime $Version with DirectML $DirectMLVersion in $Dest"
