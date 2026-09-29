# Builds the debug APK, or with -Release the release APK: unsigned unless the
# VRFT_APK_KEYSTORE* variables name a signing key, as the release workflow sets.
param([switch]$Release)
$ErrorActionPreference = 'Stop'
# The toolchain from setup-toolchain.ps1 in <repo>/.local, else a sibling
# toolchain folder beside the repo.
$tools = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\.local\toolchain'))
if (-not (Test-Path -LiteralPath $tools)) { $tools = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..\..\toolchain')) }
$jdk = Get-ChildItem -LiteralPath (Join-Path $tools 'jdk') -Directory -Filter 'jdk-17*' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $jdk) { throw 'JDK 17 is missing from the toolchain; run setup-toolchain.ps1.' }
$env:JAVA_HOME = $jdk.FullName
$env:ANDROID_HOME = Join-Path $tools 'android-sdk'
$env:ANDROID_SDK_ROOT = $env:ANDROID_HOME
$env:GRADLE_USER_HOME = Join-Path $tools 'gradle-home'
$env:ANDROID_USER_HOME = Join-Path $tools 'android-user-home'
$env:PATH = (Join-Path $env:JAVA_HOME 'bin') + ';' + $env:PATH
$localGradle = Join-Path $tools 'gradle-8.7\bin\gradle.bat'
$gradle = if (Test-Path -LiteralPath $localGradle) { $localGradle } else { Join-Path $PSScriptRoot 'gradlew.bat' }
$variant = if ($Release) { 'release' } else { 'debug' }
$task = if ($Release) { ':app:assembleRelease' } else { ':app:assembleDebug' }
& $gradle -p $PSScriptRoot $task --no-daemon
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
Get-ChildItem -LiteralPath (Join-Path $PSScriptRoot "app\build\outputs\apk\$variant") -Filter '*.apk' |
    ForEach-Object { Write-Host $_.FullName }
