param(
    [string]$ReleaseZip = '',
    [string]$Python = '',
    # cpu: CPU PyTorch (default). cuda: NVIDIA GPU, PyTorch CUDA 12.8 wheels.
    # rocm: AMD Radeon GPU on Windows, AMD ROCm 7.2.1 PyTorch 2.9.1 wheels (Python 3.12).
    [ValidateSet('cpu', 'cuda', 'rocm')]
    [string]$Accelerator = 'cpu'
)

$ErrorActionPreference = 'Stop'
$Accelerator = $Accelerator.ToLowerInvariant()
$repo = $PSScriptRoot
$modelDir = Join-Path $repo 'models/quest-pro'
$runtimeDir = Join-Path $repo '.local/tongue-python'
# PowerShell names are case-insensitive: $python below is the same variable as
# the -Python parameter, so remember whether one was passed before overwriting it.
$pythonGiven = [bool]$Python
$python = if ($Python) { $Python } else { Join-Path $runtimeDir 'Scripts/python.exe' }
$releaseName = 'QproFaceTracking-0.1.10-poc.zip'
$releaseHash = 'DB40F4B8331A50CA6C2EC37372F1AB4B44CFBE6D21E09CA04244AAAA339CB18F'
$models = @{
    'qpro-stereo-tongue-v8-gate.pt' = '57D9D04F1A569E40836CBB7A7AF217986F8B65FFAD103FF86BC2A2D07AFC35FF'
    'qpro-stereo-tongue-v8-direction.pt' = '1900E8761C9CEAF89069121AF1016BA24C33849836A5B7EE94B4DFD9FC7DB396'
}
$cpuIndex = 'https://download.pytorch.org/whl/cpu'
$cudaIndex = 'https://download.pytorch.org/whl/cu128'
# Same AMD packages as Qpro-Enhanced-FT's Install-QproRocm.ps1.
$rocmSdkPackages = @(
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/rocm_sdk_core-7.2.1-py3-none-win_amd64.whl',
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/rocm_sdk_devel-7.2.1-py3-none-win_amd64.whl',
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/rocm_sdk_libraries_custom-7.2.1-py3-none-win_amd64.whl',
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/rocm-7.2.1.tar.gz'
)
$rocmTorchPackages = @(
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/torch-2.9.1%2Brocm7.2.1-cp312-cp312-win_amd64.whl',
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/torchaudio-2.9.1%2Brocm7.2.1-cp312-cp312-win_amd64.whl',
    'https://repo.radeon.com/rocm/windows/rocm-rel-7.2.1/torchvision-0.24.1%2Brocm7.2.1-cp312-cp312-win_amd64.whl'
)
$runtimePackages = @('numpy', 'opencv-python-headless')

function Invoke-Native {
    # Windows PowerShell 5.1 can promote a native command's stderr (an expected
    # import traceback, pip or uv progress) into a terminating NativeCommandError
    # while $ErrorActionPreference is Stop. Run with Continue; check the exit code.
    param([string]$FilePath, [string[]]$Arguments, [switch]$Quiet)
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        if ($Quiet) { & $FilePath @Arguments > $null 2>&1 }
        else { & $FilePath @Arguments | Out-Host }
        $LASTEXITCODE
    }
    finally { $ErrorActionPreference = $saved }
}

function Invoke-Checked {
    param([string]$Failure, [string]$FilePath, [string[]]$Arguments)
    if ((Invoke-Native -FilePath $FilePath -Arguments $Arguments) -ne 0) { throw $Failure }
}

# Python snippets must not contain double quotes: Windows PowerShell 5.1 drops
# them from native command arguments.
function Test-Python {
    param([string]$Code)
    (Invoke-Native -FilePath $python -Arguments @('-c', $Code) -Quiet) -eq 0
}

function Install-Packages {
    # uv-created runtimes have no pip, so prefer uv when it is available.
    param([string]$Failure, [string[]]$Packages)
    if ($uv) { Invoke-Checked -Failure $Failure -FilePath $uv.Source -Arguments (@('pip', 'install', '--python', $python) + $Packages) }
    else { Invoke-Checked -Failure $Failure -FilePath $python -Arguments (@('-m', 'pip', 'install') + $Packages) }
}

function Remove-Packages {
    param([string[]]$Packages)
    if ($uv) { $null = Invoke-Native -FilePath $uv.Source -Arguments (@('pip', 'uninstall', '--python', $python) + $Packages) }
    else { $null = Invoke-Native -FilePath $python -Arguments (@('-m', 'pip', 'uninstall', '-y') + $Packages) }
}

New-Item -ItemType Directory -Force -Path $modelDir | Out-Null
# The release ZIP only supplies the demo pair; skip it when both are already verified.
$modelsReady = @($models.Keys | Where-Object {
    $destination = Join-Path $modelDir $_
    -not ((Test-Path -LiteralPath $destination) -and
        (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -eq $models[$_])
}).Count -eq 0
if ($modelsReady) {
    Write-Host 'Demo tongue model pair already installed and verified'
}
else {
    if (-not $ReleaseZip) {
        $ReleaseZip = Join-Path $repo ".local/$releaseName"
        if (-not (Test-Path -LiteralPath $ReleaseZip)) {
            New-Item -ItemType Directory -Force -Path (Split-Path $ReleaseZip) | Out-Null
            $release = Invoke-RestMethod 'https://api.github.com/repos/n0tmast3r/Qpro-Enhanced-FT/releases/tags/v0.1.10'
            $asset = $release.assets | Where-Object name -EQ $releaseName | Select-Object -First 1
            if (-not $asset) { throw "Release asset $releaseName was not found" }
            Write-Host "Downloading $releaseName from the Qpro-Enhanced-FT v0.1.10 release"
            Invoke-WebRequest $asset.browser_download_url -OutFile $ReleaseZip
        }
    }
    $ReleaseZip = (Resolve-Path -LiteralPath $ReleaseZip).Path
    if ((Get-FileHash -LiteralPath $ReleaseZip -Algorithm SHA256).Hash -ne $releaseHash) {
        throw 'Release ZIP SHA-256 does not match the verified v0.1.10 archive'
    }

    Add-Type -AssemblyName System.IO.Compression
    $archive = [IO.Compression.ZipFile]::OpenRead($ReleaseZip)
    try {
        foreach ($name in $models.Keys) {
            $destination = Join-Path $modelDir $name
            if ((Test-Path -LiteralPath $destination) -and
                (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash -eq $models[$name]) {
                continue
            }
            if (Test-Path -LiteralPath $destination) {
                throw "Preserving existing model $destination because its hash differs from the demo. Back up or move this pair before installing the demo; setup never overwrites personal weights."
            }
            $entry = $archive.Entries | Where-Object FullName -EQ "QproFaceTracking-0.1.10-poc/models/$name" | Select-Object -First 1
            if (-not $entry) { throw "Model $name is missing from the release ZIP" }
            $inputStream = $entry.Open()
            $pendingModel = "$destination.download"
            $outputStream = [IO.File]::Create($pendingModel)
            try { $inputStream.CopyTo($outputStream) }
            finally { $outputStream.Dispose(); $inputStream.Dispose() }
            if ((Get-FileHash -LiteralPath $pendingModel -Algorithm SHA256).Hash -ne $models[$name]) {
                throw "Model $name failed SHA-256 verification"
            }
            Move-Item -LiteralPath $pendingModel -Destination $destination
        }
    }
    finally { $archive.Dispose() }
}

$uv = Get-Command uv -ErrorAction SilentlyContinue
if (-not (Test-Path -LiteralPath $python)) {
    if ($pythonGiven) { throw "Python runtime not found: $python" }
    if ($uv) {
        Invoke-Checked -Failure 'Could not create Python 3.12 runtime' -FilePath $uv.Source -Arguments @('venv', '--python', '3.12', $runtimeDir)
    }
    else {
        $py = Get-Command py -ErrorAction SilentlyContinue
        if (-not $py) { throw 'Install uv or Python 3.12, then rerun setup-quest-pro-tongue.ps1' }
        Invoke-Checked -Failure 'Could not create Python 3.12 runtime' -FilePath $py.Source -Arguments @('-3.12', '-m', 'venv', $runtimeDir)
    }
}

switch ($Accelerator) {
    'cpu' {
        if (-not (Test-Python 'import torch, cv2, numpy')) {
            Install-Packages -Failure 'PyTorch installation failed' -Packages @('torch', '--index-url', $cpuIndex)
            Install-Packages -Failure 'Tongue inference dependency installation failed' -Packages $runtimePackages
        }
    }
    'cuda' {
        if (-not (Test-Python 'import torch; assert torch.version.cuda and not torch.version.hip')) {
            # Replace a CPU or ROCm build rather than keeping an installed torch.
            Remove-Packages @('torch', 'torchvision', 'torchaudio')
            Install-Packages -Failure 'CUDA PyTorch installation failed' -Packages @('torch', '--index-url', $cudaIndex)
        }
        if (-not (Test-Python 'import cv2, numpy')) {
            Install-Packages -Failure 'Tongue inference dependency installation failed' -Packages $runtimePackages
        }
    }
    'rocm' {
        # Port of Qpro-Enhanced-FT Install-QproRocm.ps1, installed into this runtime.
        if (-not (Test-Python 'import sys; assert sys.version_info[:2] == (3, 12)')) {
            throw "AMD ROCm PyTorch wheels need Python 3.12, but $python is another version. Remove $runtimeDir (or pass -Python with a Python 3.12 runtime) and rerun."
        }
        $sitePackages = (& $python -c 'import sysconfig; print(sysconfig.get_paths()[''purelib''])' | Select-Object -Last 1).Trim()
        $siteCustomize = Join-Path $sitePackages 'sitecustomize.py'
        if (Test-Path -LiteralPath $siteCustomize) {
            throw "Remove the existing Python startup customization before continuing: $siteCustomize"
        }
        if (-not (Test-Python 'import cv2, numpy, torch; assert torch.version.hip and torch.cuda.is_available()')) {
            if (-not (Test-Python 'import pip')) {
                if ($uv) { Invoke-Checked -Failure 'Could not add pip to the tongue runtime' -FilePath $uv.Source -Arguments @('pip', 'install', '--python', $python, 'pip') }
                else { Invoke-Checked -Failure 'Could not add pip to the tongue runtime' -FilePath $python -Arguments @('-m', 'ensurepip', '--upgrade') }
            }
            $pip = @('-m', 'pip', 'install', '--disable-pip-version-check')
            Invoke-Checked -Failure 'Updating pip in the tongue runtime failed' -FilePath $python -Arguments ($pip + @('--upgrade', 'pip'))
            Invoke-Checked -Failure 'Installing AMD ROCm 7.2.1 components failed' -FilePath $python -Arguments ($pip + @('--no-cache-dir') + $rocmSdkPackages)
            Invoke-Checked -Failure 'Installing AMD ROCm PyTorch 2.9.1 failed' -FilePath $python -Arguments ($pip + @('--no-cache-dir') + $rocmTorchPackages)
            Invoke-Checked -Failure 'Tongue inference dependency installation failed' -FilePath $python -Arguments ($pip + @('numpy>=2.0,<3', 'opencv-python-headless>=4.10,<6'))
        }
    }
}

if ($Accelerator -eq 'cpu') {
    Invoke-Checked -Failure 'Tongue runtime verification failed' -FilePath $python -Arguments @('-c', 'import torch, cv2, numpy; print(''Tongue runtime ready: PyTorch'', torch.__version__)')
}
else {
    # One GPU training step and one inference step with the installed pair.
    $vendor = if ($Accelerator -eq 'rocm') { 'AMD ROCm' } else { 'NVIDIA CUDA' }
    $failure = "$vendor PyTorch is installed, but the GPU check failed (see above)"
    $check = Join-Path $repo 'tongue_runtime_check.py'
    if (Test-Path -LiteralPath $check) {
        Invoke-Checked -Failure $failure -FilePath $python -Arguments @(
            $check, '--accelerator', $Accelerator,
            '--gate', (Join-Path $modelDir 'qpro-stereo-tongue-v8-gate.pt'),
            '--direction', (Join-Path $modelDir 'qpro-stereo-tongue-v8-direction.pt'))
    }
    else {
        Write-Warning 'tongue_runtime_check.py is missing; only checking that PyTorch can see the GPU'
        Invoke-Checked -Failure $failure -FilePath $python -Arguments @('-c', 'import torch; assert torch.cuda.is_available(); print(''GPU:'', torch.cuda.get_device_name(0))')
    }
    Write-Host "Tongue runtime ready for $vendor`: $python"
}

# VRFT uses VRFT_TONGUE_PYTHON, else the first of these that exists.
$daemonPython = if ($env:VRFT_TONGUE_PYTHON) { $env:VRFT_TONGUE_PYTHON } else {
    @('.local/tongue-python/Scripts/python.exe', '../toolchain/tongue-python/Scripts/python.exe') |
        ForEach-Object { Join-Path $repo $_ } | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}
if (-not $daemonPython -or -not (Test-Path -LiteralPath $daemonPython) -or
    (Resolve-Path -LiteralPath $daemonPython).Path -ne (Resolve-Path -LiteralPath $python).Path) {
    Write-Warning "VRFT will not pick this runtime automatically. Set VRFT_TONGUE_PYTHON=$python before starting VRFT."
}
Write-Host "Model pair ready in $modelDir"
