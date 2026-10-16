<#
.SYNOPSIS
    Copies the newest installer and phone package into Setups\ at the top of the project.

.DESCRIPTION
    Each build leaves what it makes several folders down inside its own build tree. This
    puts a copy of each where it is easy to find:

      Setups\Void_<version>_x64-setup.exe      the Windows installer
      Setups\Void-<version>-android-arm64.apk  the Android phone package

    and writes Setups\README.txt saying what each file is, when it was built and its
    checksum. A new build replaces the file of the same name.

    The application was called Bose QC Control Center until 2026-10-16. Copies under
    that name are taken out of Setups\ once one under the new name is there, so that
    nothing old is left looking like the latest.

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
        Name   = "Void-$version-android-arm64.apk"
        Legacy = 'Bose-QC-Control-Center-*-android-arm64.apk'
        # Its file name says nothing of which application it is, so one built
        # before the rename is told by its date.
        Since  = [datetime]'2026-10-16'
    }
)
$builds[0].Legacy = 'Bose QC Control Center_*_x64-setup.exe'

$note = @(
    'Void - latest builds',
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
    # Only a build made under the new name goes to Setups; an old-named one
    # still lying in the build tree is not the latest.
    if ($build.Legacy -and ($newest.Name -like $build.Legacy)) {
        Write-Host ("{0}: the newest build still has the old name; nothing copied." -f $build.What)
        continue
    }
    if ($build.Since -and ($newest.LastWriteTime -lt $build.Since)) {
        Write-Host ("{0}: the newest build was made before the rename to Void; nothing copied." -f $build.What)
        $old = Get-ChildItem -Path $Setups -Filter $build.Legacy -File -ErrorAction SilentlyContinue |
            Select-Object -First 1
        if ($old) {
            $note += $old.Name
            $note += ('  {0}, built before the rename: still Bose QC Control Center' -f $build.What)
            $note += ('  built   {0:yyyy-MM-dd HH:mm}' -f $old.LastWriteTime)
            $note += ''
        }
        continue
    }
    $target = Join-Path $Setups $name
    Copy-Item -Force -Path $newest.FullName -Destination $target
    $copy = Get-Item $target
    $hash = (Get-FileHash -Algorithm SHA256 -Path $target).Hash.ToLower()

    if ($build.Legacy) {
        Get-ChildItem -Path $Setups -Filter $build.Legacy -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -ne $name } |
            ForEach-Object { Remove-Item -Force $_.FullName; Write-Host ("removed the old-named copy {0}" -f $_.Name) }
    }

    $note += $name
    $note += ('  {0}' -f $build.What)
    $note += ('  built   {0:yyyy-MM-dd HH:mm}' -f $copy.LastWriteTime)
    $note += ('  size    {0:N1} MB' -f ($copy.Length / 1MB))
    $note += ('  sha256  {0}' -f $hash)
    $note += ''

    Write-Host ("{0}: {1}  ({2:N0} MB, built {3:yyyy-MM-dd HH:mm})" -f $build.What, $target, ($copy.Length / 1MB), $copy.LastWriteTime)
}

Set-Content -Path (Join-Path $Setups 'README.txt') -Value $note -Encoding ASCII
