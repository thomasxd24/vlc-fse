<#
  Builds dist\Foyer-FSE-<version>.zip: Foyer packaged as an MSIX that Windows offers as a
  Full screen experience home app, plus a certificate and an installer.

  Run after `electron-builder --win --dir` (needs dist\win-unpacked). Requires the Windows SDK
  (makeappx.exe, signtool.exe), which GitHub's windows-latest runners include.

  The package is signed with a fresh self-signed certificate. Only its public part (.cer) is
  shipped; the private key never leaves this machine and is deleted at the end.
#>
param(
  [Parameter(Mandatory = $true)][string]$Version,
  [string]$Root = (Resolve-Path "$PSScriptRoot\..").Path
)
$ErrorActionPreference = 'Stop'

$Publisher = 'CN=Foyer Launcher'
$dist = Join-Path $Root 'dist'
$unpacked = Join-Path $dist 'win-unpacked'
$stage = Join-Path $dist 'fse-stage'
$out = Join-Path $dist 'fse'
if (-not (Test-Path (Join-Path $unpacked 'Foyer.exe'))) { throw "Build the app first: $unpacked\Foyer.exe not found" }

# MSIX versions are four numbers; drop any pre-release suffix.
$v = ($Version -replace '^v', '') -replace '[-+].*$', ''
$parts = @($v.Split('.') | ForEach-Object { [int]$_ })
while ($parts.Count -lt 4) { $parts += 0 }
$msixVersion = ($parts[0..3] -join '.')

function Find-SdkTool([string]$name) {
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $hit = Get-ChildItem -Path $kits -Recurse -Filter $name -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } |
    Sort-Object { [version]($_.FullName -replace '.*\\bin\\([\d.]+)\\.*', '$1') } -Descending |
    Select-Object -First 1
  if (-not $hit) { throw "$name not found under $kits" }
  return $hit.FullName
}
$makeappx = Find-SdkTool 'makeappx.exe'
$signtool = Find-SdkTool 'signtool.exe'
Write-Host "makeappx: $makeappx"
Write-Host "signtool: $signtool"

# Stage: the unpacked app + manifest + assets + capability descriptor.
Remove-Item -Recurse -Force $stage, $out -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage, $out | Out-Null
Copy-Item -Recurse -Path (Join-Path $unpacked '*') -Destination $stage
Copy-Item -Recurse -Path (Join-Path $Root 'build\fse\Assets') -Destination (Join-Path $stage 'Assets')
Copy-Item -Path (Join-Path $Root 'build\fse\CustomCapability.SCCD') -Destination $stage
(Get-Content -Raw (Join-Path $Root 'build\fse\AppxManifest.xml')).
  Replace('{VERSION}', $msixVersion).
  Replace('{PUBLISHER}', $Publisher) |
  Set-Content -Encoding UTF8 (Join-Path $stage 'AppxManifest.xml')

$msix = Join-Path $out 'Foyer-FSE.msix'
& $makeappx pack /d $stage /p $msix /o
if ($LASTEXITCODE -ne 0) { throw "makeappx failed ($LASTEXITCODE)" }

# One-off code-signing certificate whose subject matches the manifest's Publisher.
$cert = New-SelfSignedCertificate -Type Custom -Subject $Publisher -KeyUsage DigitalSignature `
  -FriendlyName 'Foyer FSE package' -CertStoreLocation 'Cert:\CurrentUser\My' `
  -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}') -NotAfter (Get-Date).AddYears(5)
try {
  $plain = [guid]::NewGuid().ToString()
  $pfxPassword = ConvertTo-SecureString -String $plain -Force -AsPlainText
  $tmp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { $env:TEMP }
  $pfx = Join-Path $tmp ('foyer-' + [guid]::NewGuid() + '.pfx')
  Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $pfxPassword | Out-Null
  & $signtool sign /fd SHA256 /f $pfx /p $plain $msix
  if ($LASTEXITCODE -ne 0) { throw "signtool failed ($LASTEXITCODE)" }
  Export-Certificate -Cert $cert -FilePath (Join-Path $out 'Foyer-FSE.cer') | Out-Null
} finally {
  if ($pfx -and (Test-Path $pfx)) { Remove-Item -Force $pfx }
  Remove-Item -Force "Cert:\CurrentUser\My\$($cert.Thumbprint)" -ErrorAction SilentlyContinue
}

Copy-Item -Path (Join-Path $Root 'build\fse\installer\*') -Destination $out
$zip = Join-Path $dist "Foyer-FSE-$v.zip"
Remove-Item -Force $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $out '*') -DestinationPath $zip
Write-Host "Built $zip"
Remove-Item -Recurse -Force $stage
