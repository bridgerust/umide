# Build the native Workbench and assemble a WiX v3 MSI for Windows x64.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Tag,
    [string]$WixBin
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
if ($Tag -cnotmatch '^workbench-v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-beta\.([1-9][0-9]*))?$') {
    throw "Expected a Workbench release tag such as workbench-v0.4.0-beta.1."
}
$major = [int]$Matches[1]
$minor = [int]$Matches[2]
$patch = [int]$Matches[3]
$beta = $Matches[4]
# Windows Installer compares exactly three numbers. Reserve 1..254 for betas
# of a patch and 255 for its final release, so older betas cannot downgrade it.
$rank = if ($beta) { [int]$beta } else { 255 }
if ($major -gt 255 -or $minor -gt 255 -or $patch -gt 255 -or ($beta -and $rank -gt 254)) {
    throw "Version $Tag exceeds the MSI numeric limits."
}
$msiVersion = "$major.$minor.$($patch * 256 + $rank)"
$releaseVersion = $Tag.Substring('workbench-v'.Length)
if (-not $WixBin) {
    if ($env:WIX -and (Test-Path (Join-Path $env:WIX 'bin/candle.exe'))) {
        $WixBin = Join-Path $env:WIX bin
    } else {
        $WixBin = Split-Path (Get-Command candle.exe -ErrorAction Stop).Source
    }
}
$candle = (Resolve-Path (Join-Path $WixBin candle.exe)).Path
$light = (Resolve-Path (Join-Path $WixBin light.exe)).Path

Push-Location $root
try {
    $metadata = & cargo metadata --format-version 1 --locked --no-deps
    if ($LASTEXITCODE -ne 0) { throw "Could not read the workspace version." }
    $workbench = ($metadata | ConvertFrom-Json).packages | Where-Object name -eq 'umide-workbench'
    if (-not $workbench -or $workbench.version -ne "$major.$minor.$patch") {
        throw "Tag $Tag does not match the Workbench Cargo version."
    }
    $target = 'x86_64-pc-windows-msvc'
    & cargo build --locked --release --target $target --target-dir target -p umide-workbench --bin umide-workbench
    if ($LASTEXITCODE -ne 0) { throw "The Windows x64 build failed." }
    $binary = (Resolve-Path "target/$target/release/umide-workbench.exe").Path
    $output = Join-Path $root 'dist/workbench'
    [void][IO.Directory]::CreateDirectory($output)
    $staging = Join-Path ([IO.Path]::GetTempPath()) ("umide-workbench-msi-" + [guid]::NewGuid())
    [void][IO.Directory]::CreateDirectory($staging)
    try {
        $object = Join-Path $staging workbench.wixobj
        $msi = Join-Path $staging Umide-Workbench-Windows-x64.msi
        & $candle -nologo -arch x64 "-dMsiVersion=$msiVersion" "-dReleaseVersion=$releaseVersion" "-dRoot=$root" "-dBinary=$binary" -out $object (Join-Path $PSScriptRoot 'windows/workbench.wxs')
        if ($LASTEXITCODE -ne 0) { throw "WiX could not compile the Workbench installer." }
        & $light -nologo -out $msi $object
        if ($LASTEXITCODE -ne 0) { throw "WiX could not validate and link the Workbench MSI." }
        Copy-Item -LiteralPath $msi -Destination $output -Force
        Write-Output (Join-Path $output 'Umide-Workbench-Windows-x64.msi')
    } finally {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
} finally {
    Pop-Location
}
