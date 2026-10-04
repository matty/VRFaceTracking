# Renders a synthetic five-camera recording of GNM heads with Blender, headless.
#   ./tools/face-synth/render.ps1 -Count 500
#   ./tools/face-synth/render.ps1 -Count 300 -Fast -Jobs 3   # a quick check
# Blender is $env:VRFT_BLENDER, else the newest stable Blender Launcher build.
# -Jobs runs that many Blender processes side by side, each writing its own
# recording; training takes them all. -CameraIds names the eye and brow
# cameras in the calibration file, -Expressions a precompute_expressions.py
# file, and -Engine cycles renders on the CPU.
param(
    [int]$Count = 200,
    [int]$Seed = 0,
    [int]$Identities = 0,
    [string]$Out = "",
    [string]$Calibration = "",
    [string]$CameraIds = "",
    [string]$Expressions = "",
    [ValidateSet("eevee", "cycles")][string]$Engine = "eevee",
    [int]$Jobs = 1,
    [switch]$Fast
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

$script = Join-Path $PSScriptRoot "render_face.py"
$common = @()
if ($Out) { $common += @("--out", $Out) }
if ($Calibration) { $common += @("--calibration", $Calibration) }
if ($Fast) { $common += "--fast" }
if ($CameraIds) { $common += @("--camera-ids", $CameraIds) }
if ($Expressions) { $common += @("--expressions", $Expressions) }
$common += @("--engine", $Engine)
$filter = "^face-synth:|Error|Traceback"

if ($Jobs -le 1) {
    $arguments = @("-b", "--factory-startup", "-P", $script, "--", "--count", $Count, "--identities", $Identities)
    if ($Seed) { $arguments += @("--seed", $Seed) }
    & $blender @arguments @common 2>&1 | Select-String -Pattern $filter
    return
}

$running = @()
for ($k = 0; $k -lt $Jobs; $k++) {
    $share = [math]::Floor($Count / $Jobs) + [int]($k -lt $Count % $Jobs)
    if ($share -le 0) { continue }
    $people = if ($Identities) { [math]::Max(1, [math]::Ceiling($Identities / $Jobs)) } else { 0 }
    $jobSeed = if ($Seed) { $Seed + $k } else { Get-Random -Maximum 2000000000 }
    $arguments = @("-b", "--factory-startup", "-P", $script, "--", "--count", $share,
        "--identities", $people, "--seed", $jobSeed) + $common
    $log = Join-Path ([IO.Path]::GetTempPath()) "face-synth-$PID-$k.log"
    # Start-Process joins arguments with spaces, so quote each one.
    $quoted = $arguments | ForEach-Object { '"' + $_ + '"' }
    $process = Start-Process $blender -ArgumentList $quoted -NoNewWindow -PassThru `
        -RedirectStandardOutput $log -RedirectStandardError "$log.err"
    $running += [pscustomobject]@{ Process = $process; Log = $log }
}
$running.Process | Wait-Process
foreach ($job in $running) {
    Get-Content $job.Log, "$($job.Log).err" -ErrorAction SilentlyContinue |
        Select-String -Pattern "^face-synth: wrote|Error|Traceback"
    Remove-Item $job.Log, "$($job.Log).err" -ErrorAction SilentlyContinue
}
