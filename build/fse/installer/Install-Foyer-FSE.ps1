<#
  Installs Foyer as a Windows "Full screen experience" home app.
  Installe Foyer comme application d'accueil de l'« Expérience plein écran » de Windows.

  What it does / Ce que fait ce script :
    1. Trusts the package certificate (Foyer-FSE.cer) for app installs only (LocalMachine\TrustedPeople).
    2. Turns on Developer Mode just long enough to install (the home-app capability needs it), then
       puts it back the way it was.
    3. Installs (or updates) Foyer-FSE.msix.
    4. Optionally sets Foyer as the full screen experience home app.
#>
param([switch]$SetHomeApp, [switch]$Quiet)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

function Say($en, $fr) { Write-Host "$en" -ForegroundColor Cyan; Write-Host "  $fr" -ForegroundColor DarkGray }

# Re-launch elevated if needed (certificate store and Developer Mode are machine-wide).
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
  $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$($MyInvocation.MyCommand.Path)`"")
  if ($SetHomeApp) { $argList += '-SetHomeApp' }
  if ($Quiet) { $argList += '-Quiet' }
  Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList
  exit
}

$msix = Join-Path $here 'Foyer-FSE.msix'
$cer = Join-Path $here 'Foyer-FSE.cer'
if (-not (Test-Path $msix) -or -not (Test-Path $cer)) { throw 'Foyer-FSE.msix / Foyer-FSE.cer not found next to this script.' }

Say 'Installing Foyer for the full screen experience…' 'Installation de Foyer pour l''expérience plein écran…'

# 1. Certificate
Import-Certificate -FilePath $cer -CertStoreLocation 'Cert:\LocalMachine\TrustedPeople' | Out-Null

# 2. Developer Mode, temporarily
$devKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock'
if (-not (Test-Path $devKey)) { New-Item -Path $devKey -Force | Out-Null }
$prevDev = (Get-ItemProperty -Path $devKey -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue).AllowDevelopmentWithoutDevLicense
Set-ItemProperty -Path $devKey -Name AllowDevelopmentWithoutDevLicense -Value 1 -Type DWord

try {
  # 3. Install / update
  Add-AppxPackage -Path $msix -ForceApplicationShutdown -ForceUpdateFromAnyVersion
} finally {
  if ($null -eq $prevDev) { Remove-ItemProperty -Path $devKey -Name AllowDevelopmentWithoutDevLicense -ErrorAction SilentlyContinue }
  else { Set-ItemProperty -Path $devKey -Name AllowDevelopmentWithoutDevLicense -Value $prevDev -Type DWord }
}

$pkg = Get-AppxPackage -Name 'Foyer.Launcher' | Select-Object -First 1
if (-not $pkg) { throw 'Installation failed: package not found after install.' }
$aumid = "$($pkg.PackageFamilyName)!App"
Say "Installed Foyer $($pkg.Version)." "Foyer $($pkg.Version) installé."

# 4. Home app
if (-not $SetHomeApp -and -not $Quiet) {
  $answer = Read-Host 'Make Foyer the full screen experience home app? / Faire de Foyer l''application d''accueil ? [Y/n / O/n]'
  $SetHomeApp = ($answer -eq '' -or $answer -match '^[yYoO]')
}
if ($SetHomeApp) {
  $gc = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GamingConfiguration'
  if (-not (Test-Path $gc)) { New-Item -Path $gc -Force | Out-Null }
  Set-ItemProperty -Path $gc -Name GamingHomeApp -Value $aumid -Type String
  Say 'Foyer is now the home app. You can change it in Settings > Gaming > Full screen experience.' 'Foyer est maintenant l''application d''accueil. Modifiable dans Paramètres > Jeux > Expérience plein écran.'
} else {
  Say 'Choose Foyer in Settings > Gaming > Full screen experience > Home app.' 'Choisissez Foyer dans Paramètres > Jeux > Expérience plein écran > Application d''accueil.'
}

$oem = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\OEM' -Name DeviceForm -ErrorAction SilentlyContinue).DeviceForm
if ($oem -ne 46) {
  Say 'Note: if you don''t see "Full screen experience" in Settings > Gaming, this Windows build hasn''t enabled it for your device yet.' 'Remarque : si « Expérience plein écran » n''apparaît pas dans Paramètres > Jeux, cette version de Windows ne l''a pas encore activée pour votre appareil.'
}
if (-not $Quiet) { Read-Host 'Press Enter to close / Entrée pour fermer' | Out-Null }
