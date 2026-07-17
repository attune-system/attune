<#
.SYNOPSIS
Materializes and validates the Chocolatey package exactly as the publish workflow does.

.EXAMPLE
.\tests\test_chocolatey_package.ps1 -Version 0.1.1
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^\d+\.\d+\.\d+$')]
    [string]$Version,

    [string]$Repository = 'attune-system/attune',

    # A valid placeholder is sufficient for `choco pack`; use the release checksum
    # when inspecting the generated installation script.
    [ValidatePattern('^[a-fA-F0-9]{64}$')]
    [string]$Checksum = '0000000000000000000000000000000000000000000000000000000000000000',

    [switch]$KeepPackage
)

$ErrorActionPreference = 'Stop'

if (-not (Get-Command choco -ErrorAction SilentlyContinue)) {
    throw 'Chocolatey is required. Install it first: https://chocolatey.org/install'
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$sourceDir = Join-Path $repositoryRoot 'packaging\chocolatey'
$packageDir = Join-Path $env:TEMP "attune-chocolatey-$Version"

Remove-Item -Recurse -Force $packageDir -ErrorAction SilentlyContinue
Copy-Item -Recurse $sourceDir $packageDir

$nuspecPath = Join-Path $packageDir 'attune-cli.nuspec'
$installTemplatePath = Join-Path $packageDir 'tools\chocolateyInstall.ps1.in'
$installScriptPath = Join-Path $packageDir 'tools\chocolateyInstall.ps1'

$installScript = Get-Content -Path $installTemplatePath -Raw
$installScript = $installScript.Replace('__REPOSITORY__', $Repository)
$installScript = $installScript.Replace('__VERSION__', $Version)
$installScript = $installScript.Replace('__SHA256__', $Checksum)
Set-Content -Path $installScriptPath -Value $installScript -Encoding utf8
Remove-Item $installTemplatePath

$nuspec = (Get-Content -Path $nuspecPath -Raw).Replace('__VERSION__', $Version)
Set-Content -Path $nuspecPath -Value $nuspec -Encoding utf8

Push-Location $packageDir
try {
    choco pack attune-cli.nuspec --limit-output
    if ($LASTEXITCODE -ne 0) {
        throw 'Unable to pack Chocolatey package'
    }
} finally {
    Pop-Location
}

$packagePath = Join-Path $packageDir "attune-cli.$Version.nupkg"
if (-not (Test-Path -LiteralPath $packagePath)) {
    throw "Chocolatey completed without creating $packagePath"
}

Write-Host "Chocolatey package validated: $packagePath"

if (-not $KeepPackage) {
    Remove-Item -Recurse -Force $packageDir
    Write-Host 'Temporary package files removed. Use -KeepPackage to retain them.'
}
