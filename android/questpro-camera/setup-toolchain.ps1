# Installs the headset app's build toolchain into <repo>/.local/toolchain:
# JDK 17, the Android SDK (cmdline-tools, platform-tools, platform 34,
# build-tools 34.0.0) and, with -Ndk, NDK 26.1.10909125 for build-native.ps1.
# Gradle itself comes from the wrapper on the first build.ps1 run.
param([switch]$Ndk)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$repo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$tools = Join-Path $repo '.local\toolchain'
$downloads = Join-Path $tools 'downloads'
New-Item -ItemType Directory -Force -Path $tools, $downloads | Out-Null

function Expand-Download([string]$Url, [string]$Name, [string]$Destination) {
    $archive = Join-Path $downloads $Name
    Write-Host "Downloading $Url"
    Invoke-WebRequest -Uri $Url -OutFile $archive
    Expand-Archive -LiteralPath $archive -DestinationPath $Destination -Force
    Remove-Item -LiteralPath $archive
}

# JDK 17 (Eclipse Temurin), unpacked as jdk/jdk-17.x.y+z.
$jdkRoot = Join-Path $tools 'jdk'
$jdk = Get-ChildItem -LiteralPath $jdkRoot -Directory -Filter 'jdk-17*' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $jdk) {
    Expand-Download 'https://api.adoptium.net/v3/binary/latest/17/ga/windows/x64/jdk/hotspot/normal/eclipse' 'jdk17.zip' $jdkRoot
    $jdk = Get-ChildItem -LiteralPath $jdkRoot -Directory -Filter 'jdk-17*' | Select-Object -First 1
}
Write-Host "JDK: $($jdk.FullName)"

# Android command-line tools, which sdkmanager needs at cmdline-tools/latest.
# Pinned to 19.0: from 23.0 sdkmanager hands off to the new Android CLI and
# no longer installs packages non-interactively.
$sdk = Join-Path $tools 'android-sdk'
$sdkmanager = Join-Path $sdk 'cmdline-tools\latest\bin\sdkmanager.bat'
if (-not (Test-Path -LiteralPath $sdkmanager)) {
    $zip = 'commandlinetools-win-13114758_latest.zip'
    $staging = Join-Path $downloads 'cmdline-tools'
    Expand-Download "https://dl.google.com/android/repository/$zip" $zip $staging
    New-Item -ItemType Directory -Force -Path (Join-Path $sdk 'cmdline-tools') | Out-Null
    Move-Item -LiteralPath (Join-Path $staging 'cmdline-tools') -Destination (Join-Path $sdk 'cmdline-tools\latest')
    Remove-Item -Recurse -Force -LiteralPath $staging
}

$env:JAVA_HOME = $jdk.FullName
$env:ANDROID_HOME = $sdk
$env:ANDROID_SDK_ROOT = $sdk
$env:ANDROID_USER_HOME = Join-Path $tools 'android-user-home'

$packages = @('platform-tools', 'platforms;android-34', 'build-tools;34.0.0')
if ($Ndk) { $packages += 'ndk;26.1.10909125' }

# Accept the SDK licences, then install the packages.
(1..20 | ForEach-Object { 'y' }) | & $sdkmanager --sdk_root=$sdk --licenses | Out-Null
& $sdkmanager --sdk_root=$sdk @packages
if ($LASTEXITCODE -ne 0) { throw 'sdkmanager failed.' }

Remove-Item -Recurse -Force -LiteralPath $downloads
Write-Host ''
Write-Host "Toolchain ready in $tools"
Write-Host "adb: $(Join-Path $sdk 'platform-tools\adb.exe')"
