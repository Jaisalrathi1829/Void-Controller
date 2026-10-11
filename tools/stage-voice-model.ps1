<#
.SYNOPSIS
    Copies the installed Kokoro voice model to where the installer build picks it up.

.DESCRIPTION
    `npm run tauri:build:voice` builds an installer that carries the voice model,
    so the installed application can speak without a separate download. The
    bundle configuration (app/src-tauri/tauri.bundle.voice.conf.json) takes the
    model from app/src-tauri/target/voice-model, which this script fills from the
    copy installed by tools/install-voice-model.ps1. Nothing is downloaded.
#>
$ErrorActionPreference = 'Stop'
$Model  = 'kokoro-int8-en-v0_19'
$Source = Join-Path $env:LOCALAPPDATA "local.bosecontrolcenter.app\voice\$Model"
$Dest   = Join-Path $PSScriptRoot "..\app\src-tauri\target\voice-model\$Model"

if (-not (Test-Path (Join-Path $Source 'model.int8.onnx'))) {
    throw "The voice model is not installed at $Source. Run tools\install-voice-model.ps1 first."
}
if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
New-Item -ItemType Directory -Force -Path (Split-Path $Dest) | Out-Null
Copy-Item -Recurse -Path $Source -Destination $Dest
$files = Get-ChildItem -Recurse -File $Dest
Write-Host ("Staged {0} files, {1:N0} MB, at {2}" -f $files.Count, (($files | Measure-Object Length -Sum).Sum / 1MB), (Resolve-Path $Dest))
