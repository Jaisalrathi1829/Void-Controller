<#
.SYNOPSIS
    Copies the newest installer and phone package into Setups\ at the top of the project.

.DESCRIPTION
    Each build leaves what it makes several folders down inside its own build tree. This
    puts a copy of each where it is easy to find:

      Setups\Bose QC Control Center_<version>_x64-setup.exe      the Windows installer
      Setups\Bose-QC-Control-Center-<version>-android-arm64.apk  the Android phone package

    and writes Setups\README.txt saying what each file is, when it was built and its
    checksum. A new build replaces the file of the same name.

    Nothing is built here and nothing is fetched. A build that has not been made is
    simply left out. It runs by itself at the end of `npm run tauri:build`,
    `npm run tauri:build:voice` and tools\build-android.ps1.

    Setups\ is not part of the repository: what is in it is made from the source.

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File tools\collect-setups.ps1
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
$Root   = Resolve-Path (Join-Path $PSScriptRoot '..')
$Setups = Join-Path $Root 'Setups'
New-Item -ItemType Directory -Force -Path $Setups | Out-Null

$version = (Get-Content -Raw (Join-Path $Root 'app\src-tauri\tauri.conf.json') | ConvertFrom-Json).version

# Where each build leaves its result, and what the copy is called. The installer keeps
# the name it was built with; the phone package's own name says nothing about what it is.
$builds = @(
    @{
        What   = 'Windows installer'
        From   = Join-Path $Root 'app\src-tauri\target\release\bundle\nsis'
        Filter = '*-setup.exe'
        Name   = $null
    },
    @{
        What   = 'Android phone package (arm64)'
        From   = Join-Path $Root 'app\src-tauri\gen\android\app\build\outputs\apk\arm64\release'
        Filter = 'app-arm64-release.apk'
        Name   = "Bose-QC-Control-Center-$version-android-arm64.apk"
    }
)

$note = @(
    'Bose QC Control Center - latest builds',
    '',
    'Copied here by tools\collect-setups.ps1 at the end of each build. A new build',
    'replaces the file of the same name. Both install over the version already there.',
    ''
)

foreach ($build in $builds) {
    $newest = $null
    if (Test-Path $build.From) {
        $newest = Get-ChildItem -Path $build.From -Filter $build.Filter -File |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
    }
    if (-not $newest) {
        Write-Host ("{0}: not built yet, nothing to copy." -f $build.What)
        continue
    }

    $name   = if ($build.Name) { $build.Name } else { $newest.Name }
    $target = Join-Path $Setups $name
    Copy-Item -Force -Path $newest.FullName -Destination $target
    $copy = Get-Item $target
    $hash = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower()

    $note += $name
    $note += ('  {0}' -f $build.What)
    $note += ('  built   {0:yyyy-MM-dd HH:mm}' -f $copy.LastWriteTime)
    $note += ('  size    {0:N1} MB' -f ($copy.Length / 1MB))
    $note += ('  sha256  {0}' -f $hash)
    $note += ''

    Write-Host ("{0}: {1}  ({2:N0} MB, built {3:yyyy-MM-dd HH:mm})" -f $build.What, $target, ($copy.Length / 1MB), $copy.LastWriteTime)
}

Set-Content -Path (Join-Path $Setups 'README.txt') -Value $note -Encoding ASCII
