<#
.SYNOPSIS
  Builds the unsigned MSIX package for the Microsoft Store.

.DESCRIPTION
  The Store re-signs MSIX packages, so this package is not signed. The three identity values must be copied from
  Partner Center (Product management > Product identity): there is no default, and a missing value stops the build.

.EXAMPLE
  .\store\build-msix.ps1 -IdentityName "<Package/Identity/Name>" -Publisher "CN=<Package/Identity/Publisher>" -PublisherDisplayName "<Publisher display name>"
#>
param(
  [Parameter(Mandatory)][string]$IdentityName,
  [Parameter(Mandatory)][string]$Publisher,
  [Parameter(Mandatory)][string]$PublisherDisplayName,
  [string]$ExePath,
  [string]$OutDir
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if (-not $OutDir) { $OutDir = Join-Path $root 'dist' }

if ($Publisher -notmatch '^CN=') { throw "Publisher must be the full subject from Partner Center, starting with CN= (got '$Publisher')." }
if ($IdentityName -notmatch '^[A-Za-z0-9][A-Za-z0-9.\-]{2,49}$') { throw "IdentityName '$IdentityName' is not a valid package name (3-50 letters, digits, '.' or '-')." }

# Version: Cargo.toml x.y.z -> x.y.z.0. The fourth part is reserved by the Store and must be 0.
$m = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"' | Select-Object -First 1
if (-not $m) { throw 'Cargo.toml has no x.y.z version.' }
$g = $m.Matches[0].Groups
if ([int]$g[1].Value -eq 0) { throw 'The Store does not accept a first version part of 0: raise the version (e.g. 1.0.0) before submitting.' }
$version = '{0}.{1}.{2}.0' -f $g[1].Value, $g[2].Value, $g[3].Value

# The executable: given, or a fresh release build.
if (-not $ExePath) {
  Push-Location $root
  # cargo writes progress to stderr, which Windows PowerShell 5.1 would turn into an error: judge by the exit code.
  $prev = $ErrorActionPreference; $ErrorActionPreference = 'Continue'
  try { cargo build --release; $code = $LASTEXITCODE } finally { $ErrorActionPreference = $prev; Pop-Location }
  if ($code) { throw "cargo build failed ($code)" }
  $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
  $ExePath = Join-Path $target 'release\nanoarch-app.exe'
}
if (-not (Test-Path $ExePath)) { throw "Executable not found: $ExePath" }

$sdk = Get-ChildItem 'C:\Program Files (x86)\Windows Kits\10\bin\*\x64\makeappx.exe' -ErrorAction SilentlyContinue | Sort-Object FullName | Select-Object -Last 1
if (-not $sdk) { throw 'makeappx.exe not found: install the Windows 10/11 SDK.' }

# Staging layout.
$stage = Join-Path $OutDir 'msix-stage'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path "$stage\Assets" -Force | Out-Null
Copy-Item $ExePath (Join-Path $stage 'NanoArch.exe')

# Tile and logo images drawn from assets\icon.png (512 x 512).
Add-Type -AssemblyName System.Drawing
$src = [System.Drawing.Image]::FromFile((Join-Path $root 'assets\icon.png'))
function New-Logo([string]$name, [int]$w, [int]$h) {
  $bmp = New-Object System.Drawing.Bitmap $w, $h, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $gfx = [System.Drawing.Graphics]::FromImage($bmp)
  $gfx.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $gfx.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
  $side = [Math]::Min($w, $h)
  $gfx.DrawImage($src, [int](($w - $side) / 2), [int](($h - $side) / 2), $side, $side)
  $gfx.Dispose(); $bmp.Save((Join-Path "$stage\Assets" $name), [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}
New-Logo 'Square44x44Logo.png' 44 44
New-Logo 'Square150x150Logo.png' 150 150
New-Logo 'StoreLogo.png' 50 50
New-Logo 'Wide310x150Logo.png' 310 150
$src.Dispose()

# Manifest.
$xml = Get-Content (Join-Path $PSScriptRoot 'AppxManifest.xml') -Raw -Encoding UTF8
$esc = { param($s) [System.Security.SecurityElement]::Escape($s) }
$xml = $xml.Replace('@IDENTITY_NAME@', (& $esc $IdentityName)).Replace('@PUBLISHER@', (& $esc $Publisher)).Replace('@PUBLISHER_DISPLAY@', (& $esc $PublisherDisplayName)).Replace('@VERSION@', $version)
if ($xml -match '@[A-Z_]+@') { throw "Unfilled token left in the manifest: $($Matches[0])" }
[System.IO.File]::WriteAllText((Join-Path $stage 'AppxManifest.xml'), $xml, (New-Object System.Text.UTF8Encoding($false)))

# Pack, then read the package back.
$out = Join-Path $OutDir ("NanoArch-{0}.msix" -f $version)
& $sdk.FullName pack /d $stage /p $out /o /nv
if ($LASTEXITCODE) { throw "makeappx pack failed ($LASTEXITCODE)" }
$check = Join-Path $OutDir 'msix-check'
if (Test-Path $check) { Remove-Item $check -Recurse -Force }
& $sdk.FullName unpack /p $out /d $check /o /nv
if ($LASTEXITCODE) { throw "makeappx unpack failed ($LASTEXITCODE)" }
foreach ($f in 'NanoArch.exe', 'AppxManifest.xml', 'Assets\StoreLogo.png', 'Assets\Square150x150Logo.png', 'Assets\Square44x44Logo.png', 'Assets\Wide310x150Logo.png') {
  if (-not (Test-Path (Join-Path $check $f))) { throw "Package is missing $f" }
}
Remove-Item $check -Recurse -Force
"{0}  ({1:N1} MB)  version {2}" -f $out, ((Get-Item $out).Length / 1MB), $version
