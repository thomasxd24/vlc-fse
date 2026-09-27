# Foyer 2.0.0's updater looks for this file name in an FSE update and runs it with -Quiet -Update -Launch.
# It hands everything to the Lounge installer next to it (the package upgrades Foyer in place).
param([switch]$SetHomeApp, [switch]$Quiet, [switch]$Update, [switch]$Launch)
& (Join-Path $PSScriptRoot 'Install-Lounge-FSE.ps1') @PSBoundParameters
