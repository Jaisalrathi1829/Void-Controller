<#
.SYNOPSIS
    Installs the Kokoro voice model used by Void's voice feedback.

.DESCRIPTION
    Voice feedback is spoken by Kokoro, a text-to-speech model that runs
    entirely on this PC. The application contains no download code of its own
    and never fetches anything: this script is the one explicit step that
    brings the model in.

    It downloads one pinned archive from the official sherpa-onnx project,
    checks it against a known SHA-256 before unpacking anything, and extracts
    it to:

        %LOCALAPPDATA%\local.bosecontrolcenter.app\voice\kokoro-int8-en-v0_19

    After that, speech needs no internet access.

    What is installed (about 150 MB on disk):
        model.int8.onnx     the Kokoro model, 8-bit quantized
        voices.bin          the 11 English voices
        tokens.txt          the model's phoneme table
        espeak-ng-data\     pronunciation data used to turn text into phonemes

    Licences: the Kokoro model and voices are Apache-2.0; the espeak-ng data
    is GPL-3.0-or-later. See THIRD-PARTY-NOTICES.md.

.PARAMETER Force
    Reinstall even if the model is already present.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\install-voice-model.ps1
#>
[CmdletBinding()]
param([switch]$Force)

$ErrorActionPreference = 'Stop'

# Pinned. The application is built against this exact model: its voice list
# and file layout are compiled in, so a different archive would be refused.
$Name    = 'kokoro-int8-en-v0_19'
$Archive = "$Name.tar.bz2"
$Url     = "https://github.com/k2-fsa/sherpa-onnx/releases/download/tts-models/$Archive"
$Sha256  = 'c9f0dd393615805b0bab050c340834d5e684e732aec91c0e860cd30e982c08bd'
$Bytes   = 103248205

$Root = Join-Path $env:LOCALAPPDATA 'local.bosecontrolcenter.app\voice'
$Dest = Join-Path $Root $Name
$Required = @('model.int8.onnx', 'voices.bin', 'tokens.txt', 'espeak-ng-data')

function Test-Installed {
    foreach ($item in $Required) {
        if (-not (Test-Path (Join-Path $Dest $item))) { return $false }
    }
    return $true
}

if ((Test-Installed) -and -not $Force) {
    Write-Host "The voice model is already installed:`n  $Dest"
    Write-Host "Use -Force to reinstall."
    exit 0
}

New-Item -ItemType Directory -Force -Path $Root | Out-Null
$Temp = Join-Path $Root $Archive

Write-Host "Downloading $Archive ($([math]::Round($Bytes / 1MB)) MB) from"
Write-Host "  $Url"
$previous = $ProgressPreference
$ProgressPreference = 'SilentlyContinue'   # the progress bar slows large downloads badly
try {
    Invoke-WebRequest -Uri $Url -OutFile $Temp -UseBasicParsing
} finally {
    $ProgressPreference = $previous
}

Write-Host "Checking the download..."
$actual = (Get-FileHash -Path $Temp -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actual -ne $Sha256) {
    Remove-Item -Force $Temp
    throw "The download does not match the expected checksum and was deleted.`n  expected $Sha256`n  got      $actual"
}

if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }

Write-Host "Unpacking..."
# Windows' own tar (bsdtar) reads bzip2 archives.
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
& $tar -xjf $Temp -C $Root
if ($LASTEXITCODE -ne 0) { throw "Unpacking failed (tar exit code $LASTEXITCODE)." }
Remove-Item -Force $Temp

if (-not (Test-Installed)) {
    throw "The archive unpacked, but the expected files are not all in $Dest."
}

Write-Host "`nInstalled:`n  $Dest"
Write-Host "Open Settings > Voice Feedback in Void and press 'Check again'."
