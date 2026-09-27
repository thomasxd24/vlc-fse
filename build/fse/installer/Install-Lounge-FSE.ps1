<#
  Installs Lounge as a Windows "Full screen experience" home app.
  Installe Lounge comme application d'accueil de l'« Expérience plein écran » de Windows.

  What it does / Ce que fait ce script :
    1. Trusts the package certificate (Lounge-FSE.cer) for app installs only (LocalMachine\TrustedPeople).
    2. Turns on Developer Mode just long enough to install (the home-app capability needs it), then
       puts it back the way it was.
    3. Installs (or updates) Lounge-FSE.msix.
    4. Optionally sets Lounge as the full screen experience home app.
#>
param([switch]$SetHomeApp, [switch]$Quiet, [switch]$Update, [switch]$Launch, [string]$Log)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

function Say($en, $fr) {
  Write-Host "$en" -ForegroundColor Cyan; Write-Host "  $fr" -ForegroundColor DarkGray
  if ($Log) { Add-Content -Path $Log -Value ("{0:u} {1}" -f (Get-Date), $en) -ErrorAction SilentlyContinue }
}
# Lounge's updater runs this hidden: any failure goes to its update log.
trap {
  if ($Log) { Add-Content -Path $Log -Value ("{0:u} installer failed: {1}" -f (Get-Date), $_) -ErrorAction SilentlyContinue }
  if (-not $Quiet) { Write-Host "$_" -ForegroundColor Red; Read-Host 'Press Enter to close / Entrée pour fermer' | Out-Null }
  exit 1
}

# Re-launch elevated if needed (certificate store and Developer Mode are machine-wide).
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
  $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$($MyInvocation.MyCommand.Path)`"")
  if ($SetHomeApp) { $argList += '-SetHomeApp' }
  if ($Quiet) { $argList += '-Quiet' }
  if ($Update) { $argList += '-Update' }
  if ($Launch) { $argList += '-Launch' }
  if ($Log) { $argList += @('-Log', "`"$Log`"") }
  Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList $argList
  exit
}

$msix = Join-Path $here 'Lounge-FSE.msix'
$cer = Join-Path $here 'Lounge-FSE.cer'
if (-not (Test-Path $msix) -or -not (Test-Path $cer)) { throw 'Lounge-FSE.msix / Lounge-FSE.cer not found next to this script.' }

Say 'Installing Lounge for the full screen experience…' 'Installation de Lounge pour l''expérience plein écran…'

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
Say "Installed Lounge $($pkg.Version)." "Lounge $($pkg.Version) installé."

# 4. Home app (left as it is when updating)
if (-not $SetHomeApp -and -not $Quiet -and -not $Update) {
  $answer = Read-Host 'Make Lounge the full screen experience home app? / Faire de Lounge l''application d''accueil ? [Y/n / O/n]'
  $SetHomeApp = ($answer -eq '' -or $answer -match '^[yYoO]')
}
if ($SetHomeApp) {
  $gc = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GamingConfiguration'
  if (-not (Test-Path $gc)) { New-Item -Path $gc -Force | Out-Null }
  Set-ItemProperty -Path $gc -Name GamingHomeApp -Value $aumid -Type String
  Say 'Lounge is now the home app. You can change it in Settings > Gaming > Full screen experience.' 'Lounge est maintenant l''application d''accueil. Modifiable dans Paramètres > Jeux > Expérience plein écran.'
} elseif (-not $Update) {
  Say 'Choose Lounge in Settings > Gaming > Full screen experience > Home app.' 'Choisissez Lounge dans Paramètres > Jeux > Expérience plein écran > Application d''accueil.'
}

if (-not $Update) {
$oem = (Get-ItemProperty -Path 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\OEM' -Name DeviceForm -ErrorAction SilentlyContinue).DeviceForm
if ($oem -ne 46) {
  Say 'Note: if you don''t see "Full screen experience" in Settings > Gaming, this Windows build hasn''t enabled it for your device yet.' 'Remarque : si « Expérience plein écran » n''apparaît pas dans Paramètres > Jeux, cette version de Windows ne l''a pas encore activée pour votre appareil.'
}
}
if ($Launch) {
  # Start Lounge through Explorer so it runs as the signed-in user, not with this script's admin rights.
  Start-Process -FilePath 'explorer.exe' -ArgumentList "shell:AppsFolder\$aumid"
}
if (-not $Quiet) { Read-Host 'Press Enter to close / Entrée pour fermer' | Out-Null }
