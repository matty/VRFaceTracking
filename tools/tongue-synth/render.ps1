# Renders a synthetic tongue recording with Blender, headless.
#   ./tools/tongue-synth/render.ps1 -Count 500
# Blender is $env:VRFT_BLENDER, else the newest stable Blender Launcher build.
param(
    [int]$Count = 200,
    [int]$Seed = 0,
    [int]$Identities = 0,
    [string]$Out = ""
)
$ErrorActionPreference = "Stop"

$blender = $env:VRFT_BLENDER
if (-not $blender) {
    $build = Get-ChildItem "$HOME/.blender/stable" -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path (Join-Path $_.FullName "blender.exe") } |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $build) { throw "No Blender found. Set VRFT_BLENDER to blender.exe." }
    $blender = Join-Path $build.FullName "blender.exe"
}

$script = Join-Path $PSScriptRoot "render_tongue.py"
$arguments = @("-b", "--factory-startup", "-P", $script, "--", "--count", $Count, "--identities", $Identities)
if ($Seed) { $arguments += @("--seed", $Seed) }
if ($Out) { $arguments += @("--out", $Out) }
& $blender @arguments 2>&1 | Select-String -Pattern "^tongue-synth:|Error|Traceback"
