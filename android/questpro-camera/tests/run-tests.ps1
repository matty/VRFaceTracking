# Compile and run the plain-JDK unit tests for the Android-free classes.
# Uses the workspace JDK and compiles only the pure-Java sources (no Android).
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$tools = [IO.Path]::GetFullPath((Join-Path $root '..\..\.local\toolchain'))
if (-not (Test-Path -LiteralPath $tools)) { $tools = [IO.Path]::GetFullPath((Join-Path $root '..\..\..\toolchain')) }
$jdk = Get-ChildItem -LiteralPath (Join-Path $tools 'jdk') -Directory -Filter 'jdk-17*' -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $jdk) { throw 'JDK 17 is missing from the toolchain; run setup-toolchain.ps1.' }
$javac = Join-Path $jdk.FullName 'bin\javac.exe'
$java = Join-Path $jdk.FullName 'bin\java.exe'

$src = Join-Path $root 'app\src\main\java\io\github\matty\vrft\questprocamera'
$pure = @('Json.java', 'ModelPatcher.java', 'TraceParser.java', 'GazePackets.java', 'CameraFrames.java') |
    ForEach-Object { Join-Path $src $_ }
$test = Join-Path $PSScriptRoot 'PureTests.java'
$out = Join-Path $PSScriptRoot 'out'
if (Test-Path -LiteralPath $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force -Path $out | Out-Null

& $javac -Xlint:all -d $out @pure $test
if ($LASTEXITCODE -ne 0) { throw 'Test compilation failed.' }

& $java -cp $out io.github.matty.vrft.questprocamera.PureTests
exit $LASTEXITCODE
