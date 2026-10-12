<#
.SYNOPSIS
    Builds the Android application (an APK for arm64 phones), with the voice model inside.

.DESCRIPTION
    Does three things, in order:

      1. Points the build at the Android toolchain, unless the environment already does:
         JAVA_HOME (Android Studio's bundled JDK), ANDROID_HOME (the Android SDK) and
         NDK_HOME (the Android NDK).
      2. Packs the installed Kokoro voice model into the Android project as one archive,
         so that the application can speak without downloading anything. The model is the
         copy installed by tools\install-voice-model.ps1; nothing is fetched here.
      3. Runs `tauri android build`.

    The result is signed with the Android debug key, which is enough to install it on your
    own phone. An application meant for other people needs a key of its own.

.PARAMETER NoVoice
    Build without the voice model. Voice feedback then reports "Not installed" on the phone.

.PARAMETER Target
    The processor to build for: aarch64 (the default - phones) or x86_64 (the Android
    emulator on a PC, which cannot run the speech runtime's ARM build).

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\build-android.ps1
#>
[CmdletBinding()]
param(
    [switch]$NoVoice,
    [ValidateSet('aarch64', 'x86_64')]
    [string]$Target = 'aarch64'
)

$ErrorActionPreference = 'Stop'
$Root  = Resolve-Path (Join-Path $PSScriptRoot '..')
$Model = 'kokoro-int8-en-v0_19'
# What Android and Gradle call each processor.
$Abi    = @{ aarch64 = 'arm64-v8a'; x86_64 = 'x86_64' }[$Target]
$Flavor = @{ aarch64 = 'Arm64';     x86_64 = 'X86_64' }[$Target]

function Use-FirstExisting([string]$Name, [string[]]$Candidates) {
    if ((Get-Item "env:$Name" -ErrorAction SilentlyContinue).Value) { return }
    $found = $Candidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
    if (-not $found) { throw "$Name is not set and none of these exist: $($Candidates -join ', ')" }
    Set-Item "env:$Name" $found
}

Use-FirstExisting 'JAVA_HOME' @(
    "$env:ProgramFiles\Android\Android Studio\jbr"
)
Use-FirstExisting 'ANDROID_HOME' @(
    "$env:LOCALAPPDATA\Android\Sdk"
)
$ndks = @("D:\Android\ndk", "$env:ANDROID_HOME\ndk") |
    Where-Object { Test-Path $_ } |
    ForEach-Object { Get-ChildItem $_ -Directory } |
    Sort-Object Name -Descending |
    ForEach-Object { $_.FullName }
Use-FirstExisting 'NDK_HOME' $ndks

Write-Host "JAVA_HOME    $env:JAVA_HOME"
Write-Host "ANDROID_HOME $env:ANDROID_HOME"
Write-Host "NDK_HOME     $env:NDK_HOME"

$assets  = Join-Path $Root 'app\src-tauri\gen\android\app\src\main\assets'
$archive = Join-Path $assets 'voice-model.zip'
New-Item -ItemType Directory -Force -Path $assets | Out-Null
if (Test-Path $archive) { Remove-Item -Force $archive }

if ($NoVoice) {
    Write-Host 'Building without the voice model.'
} else {
    $voice = Join-Path $env:LOCALAPPDATA 'local.bosecontrolcenter.app\voice'
    if (-not (Test-Path (Join-Path $voice "$Model\model.int8.onnx"))) {
        throw "The voice model is not installed at $voice\$Model. Run tools\install-voice-model.ps1 first, or pass -NoVoice."
    }
    # One archive, with the model's own folder at its top level. Windows' tar writes
    # zip files with the forward-slash names Android expects.
    & "$env:SystemRoot\System32\tar.exe" -a -c -f $archive -C $voice $Model
    if ($LASTEXITCODE -ne 0) { throw "tar could not pack the voice model (exit code $LASTEXITCODE)." }
    Write-Host ("Packed the voice model: {0:N0} MB" -f ((Get-Item $archive).Length / 1MB))
}

Push-Location $Root
try {
    # Through cmd, so that the build's progress on stderr is ordinary output here.
    cmd /c "npx tauri android build --apk --target $Target 2>&1" | Tee-Object -Variable cliOutput
    $cliFailed = $LASTEXITCODE -ne 0
} finally {
    Pop-Location
}

if ($cliFailed) {
    # The Tauri CLI places the compiled library in the Android project with a symbolic
    # link, and Windows only lets an ordinary user create one with Developer Mode on.
    # If that is all that went wrong - the library compiled, the link was refused - the
    # same thing is done here with a copy, and Gradle is run directly.
    $text    = $cliOutput -join "`n"
    $android = Join-Path $Root 'app\src-tauri\gen\android'
    $library = Join-Path $Root "app\src-tauri\target\$Target-linux-android\release\libbose_qc_lib.so"
    $compiled = $text -match 'Finished\S* `release` profile'
    $refused  = $text -match 'symbolic link'
    if (-not ($compiled -and $refused -and (Test-Path $library))) {
        throw 'The Android build failed.'
    }
    Write-Host ''
    Write-Host 'Windows refused the symbolic link (Developer Mode is off). Packaging with a copy instead.'

    $jni = Join-Path $android "app\src\main\jniLibs\$Abi"
    New-Item -ItemType Directory -Force -Path $jni | Out-Null
    Copy-Item -Force $library $jni

    # The CLI would also have written the application's configuration here for plugins
    # that read it on the Android side. None of this application's plugins is configured,
    # so the part they look at is empty.
    $conf = Join-Path $assets 'tauri.conf.json'
    if (-not (Test-Path $conf)) {
        [System.IO.File]::WriteAllText($conf, '{"plugins":{}}')
    }

    Push-Location $android
    try {
        # The Rust step is left out: it has just been done, and it is the step that links.
        cmd /c ".\gradlew.bat assemble${Flavor}Release -x rustBuild${Flavor}Release 2>&1"
        if ($LASTEXITCODE -ne 0) { throw "Gradle failed (exit code $LASTEXITCODE)." }
    } finally {
        Pop-Location
    }
}

Get-ChildItem (Join-Path $Root 'app\src-tauri\gen\android\app\build\outputs\apk') -Recurse -Filter *.apk |
    ForEach-Object { Write-Host ("Built {0}  ({1:N0} MB)" -f $_.FullName, ($_.Length / 1MB)) }

# The phone package goes to Setups\ at the top of the project, where it is easy to find.
# The emulator's build is for testing on this machine and is not a setup.
if ($Target -eq 'aarch64') {
    & (Join-Path $PSScriptRoot 'collect-setups.ps1')
}
