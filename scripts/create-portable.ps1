[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Executable,
    [Parameter(Mandatory)][string]$NodeExecutable,
    [Parameter(Mandatory)][string]$FridaDirectory,
    [string]$OutputDirectory = 'dist/ci'
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectRoot
try {
    $exePath = (Resolve-Path -LiteralPath $Executable).Path
    $nodePath = (Resolve-Path -LiteralPath $NodeExecutable).Path
    $fridaPath = (Resolve-Path -LiteralPath $FridaDirectory).Path
    $outputPath = [System.IO.Path]::GetFullPath($OutputDirectory, $projectRoot)
    # Fresh staging prevents previously collected account logs/config from leaking.
    # Do not recursively delete or reuse an existing output directory.
    if (Test-Path -LiteralPath $outputPath) {
        if (@(Get-ChildItem -LiteralPath $outputPath -Force).Count -gt 0) {
            throw "Output directory must be empty: $outputPath"
        }
    } else {
        New-Item -ItemType Directory -Path $outputPath | Out-Null
    }
    $packagePath = Join-Path $outputPath 'LeiGodLauncher'
    & $nodePath ./package-portable.cjs $exePath $nodePath $fridaPath $packagePath
    if ($LASTEXITCODE -ne 0) { throw 'Portable assembly failed' }
    & $nodePath ./scripts/verify-portable.cjs $packagePath
    if ($LASTEXITCODE -ne 0) { throw 'Portable verification failed' }

    $archivePath = Join-Path $outputPath 'LeiGodLauncher-portable-windows-x64.zip'
    Compress-Archive -LiteralPath $packagePath -DestinationPath $archivePath -CompressionLevel Optimal
    $hash = (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $([System.IO.Path]::GetFileName($archivePath))" |
        Set-Content -LiteralPath (Join-Path $outputPath 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Portable package: $archivePath"
} finally {
    Pop-Location
}
