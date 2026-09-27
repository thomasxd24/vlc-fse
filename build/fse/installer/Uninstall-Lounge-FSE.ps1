<#
  Removes the Lounge FSE package, its certificate, and the home-app setting if it pointed at Lounge.
  Supprime le paquet Lounge FSE, son certificat et le réglage d'application d'accueil s'il désignait Lounge.
  Your Lounge settings and library (%APPDATA%\Lounge) are kept.
#>
$ErrorActionPreference = 'Stop'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
  Start-Process -FilePath 'powershell.exe' -Verb RunAs -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$($MyInvocation.MyCommand.Path)`"")
  exit
}
$pkg = Get-AppxPackage -Name 'Foyer.Launcher' | Select-Object -First 1
if ($pkg) {
  $gc = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\GamingConfiguration'
  $current = (Get-ItemProperty -Path $gc -Name GamingHomeApp -ErrorAction SilentlyContinue).GamingHomeApp
  if ($current -eq "$($pkg.PackageFamilyName)!App") { Remove-ItemProperty -Path $gc -Name GamingHomeApp -ErrorAction SilentlyContinue }
  Remove-AppxPackage -Package $pkg.PackageFullName
  Write-Host 'Lounge FSE package removed. / Paquet Lounge FSE supprimé.' -ForegroundColor Cyan
}
Get-ChildItem 'Cert:\LocalMachine\TrustedPeople' | Where-Object { $_.Subject -eq 'CN=Foyer Launcher' } | Remove-Item
Read-Host 'Press Enter to close / Entrée pour fermer' | Out-Null
