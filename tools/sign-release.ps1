<#
.SYNOPSIS
    Build, code-sign and package a release of OBR Music Tool.

.DESCRIPTION
    Signs target\release\obr-music-tool.exe with Azure Artifact Signing (formerly
    Trusted Signing) using the Computer Works certificate profile, verifies the
    signature, and writes dist\OBR-Music-Tool-v<version>-windows-x64.zip.

    Requirements (all already present on the dev machine):
      * Windows 10/11 SDK (signtool.exe)
      * Azure CLI, logged in (`az login`) as an account that holds the
        "Artifact Signing Certificate Profile Signer" role on the signing account.
      * .NET 8+ runtime (the signing dlib is a .NET component).
    The Microsoft.Trusted.Signing.Client package (the signtool dlib) is downloaded
    from nuget.org into %LOCALAPPDATA%\TrustedSigningClient on first use.

.EXAMPLE
    .\tools\sign-release.ps1            # build, sign, verify, zip
    .\tools\sign-release.ps1 -SkipBuild # sign whatever is already in target\release
    .\tools\sign-release.ps1 -ToolsOnly # just fetch/locate signtool + dlib
#>
[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$SkipZip,
    [switch]$ToolsOnly,
    [string]$Exe = 'target\release\obr-music-tool.exe'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
Set-Location $repo

# ---- Signing identity -------------------------------------------------------
# Azure Artifact Signing account owned by Computer Works (the developer's company).
# The certificate subject is fixed by the identity validation; these only select it.
$Endpoint       = 'https://eus.codesigning.azure.net/'
$AccountName    = 'computerworksrmmsign'
$ProfileName    = 'ComputerWorksRMM'
$TimestampUrl   = 'http://timestamp.acs.microsoft.com'
# Shown next to the signer in the file's Digital Signatures tab / SmartScreen.
$Description    = 'OBR Music Tool (free, open-source)'
$DescriptionUrl = 'https://github.com/LorexValkin/OBR-Music-Tool'

# ---- Tooling ----------------------------------------------------------------
function Get-SignTool {
    $kits = 'C:\Program Files (x86)\Windows Kits\10\bin'
    $candidates = Get-ChildItem -Path $kits -Directory -Filter '10.*' -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
        Where-Object { Test-Path $_ }
    if (-not $candidates) { throw "signtool.exe not found under $kits. Install the Windows 10/11 SDK." }
    return @($candidates)[0]
}

function Get-SigningDlib {
    $cache = Join-Path $env:LOCALAPPDATA 'TrustedSigningClient'
    $existing = Get-ChildItem -Path $cache -Recurse -Filter 'Azure.CodeSigning.Dlib.dll' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -like '*\x64\*' } | Select-Object -First 1
    if ($existing) { return $existing.FullName }

    New-Item -ItemType Directory -Force -Path $cache | Out-Null
    $zip = Join-Path $cache 'Microsoft.Trusted.Signing.Client.zip'
    Write-Host 'Downloading Microsoft.Trusted.Signing.Client from nuget.org ...'
    Invoke-WebRequest -Uri 'https://www.nuget.org/api/v2/package/Microsoft.Trusted.Signing.Client' -OutFile $zip
    Expand-Archive -Path $zip -DestinationPath (Join-Path $cache 'Microsoft.Trusted.Signing.Client') -Force
    Remove-Item $zip -Force

    $dlib = Get-ChildItem -Path $cache -Recurse -Filter 'Azure.CodeSigning.Dlib.dll' |
        Where-Object { $_.FullName -like '*\x64\*' } | Select-Object -First 1
    if (-not $dlib) { throw 'Azure.CodeSigning.Dlib.dll (x64) not found in the downloaded package.' }
    return $dlib.FullName
}

$signtool = Get-SignTool
$dlib     = Get-SigningDlib
Write-Host "signtool : $signtool"
Write-Host "dlib     : $dlib"
if ($ToolsOnly) { return }

# ---- Build ------------------------------------------------------------------
if (-not $SkipBuild) {
    Write-Host 'Building release ...'
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
}
if (-not (Test-Path $Exe)) { throw "Executable not found: $Exe" }

# ---- Sign -------------------------------------------------------------------
$metadata = Join-Path $env:TEMP 'obr-music-tool-signing.json'
@{
    Endpoint               = $Endpoint
    CodeSigningAccountName = $AccountName
    CertificateProfileName = $ProfileName
    # Skip credential sources that only make sense inside Azure; they add slow timeouts.
    ExcludeCredentials     = @('ManagedIdentityCredential', 'WorkloadIdentityCredential')
} | ConvertTo-Json | Set-Content -Path $metadata -Encoding ascii

Write-Host "Signing $Exe as $ProfileName ..."
& $signtool sign /v /fd SHA256 /tr $TimestampUrl /td SHA256 `
    /dlib $dlib /dmdf $metadata `
    /d $Description /du $DescriptionUrl `
    $Exe
if ($LASTEXITCODE -ne 0) { throw "signtool sign failed ($LASTEXITCODE)" }

& $signtool verify /pa /v $Exe
if ($LASTEXITCODE -ne 0) { throw "signtool verify failed ($LASTEXITCODE)" }

$sig = Get-AuthenticodeSignature $Exe
Write-Host "Signature : $($sig.Status) - $($sig.SignerCertificate.Subject)"

# ---- Package ----------------------------------------------------------------
if (-not $SkipZip) {
    $version = (Select-String -Path 'Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
    $dist = Join-Path $repo 'dist'
    New-Item -ItemType Directory -Force -Path $dist | Out-Null
    $zipPath = Join-Path $dist "OBR-Music-Tool-v$version-windows-x64.zip"
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }
    Compress-Archive -Path $Exe, 'LICENSE', 'README.md' -DestinationPath $zipPath -CompressionLevel Optimal
    Write-Host "Packaged  : $zipPath"
    Get-FileHash -Algorithm SHA256 $Exe, $zipPath | Format-Table Hash, Path -AutoSize
}
