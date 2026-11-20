# Run on a clean Windows x64 machine. Exercises the MSI itself and removes it.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Msi,
    [switch]$RequireWindow
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
if (-not [Environment]::Is64BitProcess) { throw 'Run this smoke test from 64-bit PowerShell.' }
$msiPath = (Resolve-Path -LiteralPath $Msi).Path
$install = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Umide Workbench'
$shortcut = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Umide Workbench.lnk'
if ((Test-Path -LiteralPath $install) -or (Test-Path -LiteralPath $shortcut)) {
    throw 'Use a clean test machine. An existing Workbench installation or shortcut was found.'
}
$logs = Join-Path ([IO.Path]::GetTempPath()) ("umide-install-test-" + [guid]::NewGuid())
[void][IO.Directory]::CreateDirectory($logs)
function Invoke-Installer([string]$operation, [string]$logName) {
    $log = Join-Path $logs $logName
    $installer = Start-Process -FilePath (Join-Path $env:SystemRoot 'System32/msiexec.exe') -ArgumentList @(
        $operation, ('"' + $msiPath + '"'), '/qn', '/norestart', '/l*v', ('"' + $log + '"')
    ) -Wait -PassThru
    if ($installer.ExitCode -notin @(0, 3010)) {
        Get-Content -LiteralPath $log -Tail 50 -ErrorAction SilentlyContinue | Write-Host
        throw "msiexec $operation failed with exit code $($installer.ExitCode). See $log."
    }
}
$binary = Join-Path $install 'umide-workbench.exe'
function Start-Workbench([string]$arguments) {
    $info = [Diagnostics.ProcessStartInfo]::new()
    $info.FileName = $binary
    $info.WorkingDirectory = $logs
    $info.UseShellExecute = $false
    $info.Arguments = $arguments
    $info.RedirectStandardError = $true
    $info.EnvironmentVariables['UMIDE_WORKBENCH_DATA_DIR'] = Join-Path $logs 'empty-workbench'
    $info.EnvironmentVariables['CLAUDE_CONFIG_DIR'] = Join-Path $logs 'empty-claude'
    $info.EnvironmentVariables['CODEX_HOME'] = Join-Path $logs 'empty-codex'
    # First-run probing in this test must not open a tester's own agent sessions.
    $info.EnvironmentVariables['UMIDE_CLAUDE_PATH'] = Join-Path $logs 'absent-claude.exe'
    $info.EnvironmentVariables['UMIDE_CODEX_PATH'] = Join-Path $logs 'absent-codex.exe'
    return [Diagnostics.Process]::Start($info)
}
function Stop-SmokeProcess([Diagnostics.Process]$process) {
    if ($null -eq $process) { return }
    if (-not $process.HasExited) {
        [void]$process.CloseMainWindow()
        if (-not $process.WaitForExit(3000)) { $process.Kill(); $process.WaitForExit() }
    }
    $process.Dispose()
}

$installed = $false
try {
    Invoke-Installer '/i' 'install.log'
    $installed = $true
    foreach ($file in @('umide-workbench.exe', 'Licenses/LICENSE', 'Licenses/NOTICE', 'Licenses/THIRD-PARTY-NOTICES', 'Licenses/Codicons-LICENSE')) {
        if (-not (Test-Path -LiteralPath (Join-Path $install $file) -PathType Leaf)) { throw "Missing installed file: $file" }
    }
    if (-not (Test-Path -LiteralPath $shortcut -PathType Leaf)) { throw "Missing Start menu shortcut: $shortcut" }
    $pe = [IO.BinaryReader]::new([IO.File]::OpenRead($binary))
    try {
        if ($pe.ReadUInt16() -ne 0x5a4d) { throw 'Installed file is not a Windows executable.' }
        $pe.BaseStream.Position = 0x3c
        $offset = $pe.ReadUInt32()
        $pe.BaseStream.Position = $offset
        if ($pe.ReadUInt32() -ne 0x00004550 -or $pe.ReadUInt16() -ne 0x8664) { throw 'Expected an x64 PE binary.' }
    } finally { $pe.Dispose() }
    $relay = Start-Workbench '--umide-verify'
    try {
        $diagnostics = $relay.StandardError.ReadToEndAsync()
        if (-not $relay.WaitForExit(15000)) { throw 'Installed verification relay did not exit.' }
        if ($relay.ExitCode -ne 1 -or $diagnostics.Result -notmatch 'Expected checkout and session directory') {
            throw "The installed Workbench verification entry point failed: $($diagnostics.Result)"
        }
    } finally { Stop-SmokeProcess $relay }

    $app = Start-Workbench ''
    try {
        $stderr = $app.StandardError.ReadToEndAsync()
        $deadline = [DateTime]::UtcNow.AddSeconds(30)
        $visible = $false
        do {
            $app.Refresh()
            if ($app.HasExited) { throw "Workbench exited during startup: $($stderr.Result)" }
            $visible = $app.MainWindowHandle -ne [IntPtr]::Zero -and $app.MainWindowTitle -eq 'Umide Workbench'
            if (-not $visible) { Start-Sleep -Milliseconds 250 }
        } while (-not $visible -and [DateTime]::UtcNow -lt $deadline)
        if ($visible) { Write-Host "Verified the installed window: $($app.MainWindowTitle)" }
        elseif ($RequireWindow -or [Environment]::UserInteractive) { throw 'The Workbench window did not appear.' }
        else { Write-Host 'Workbench started in a noninteractive desktop session. Its window could not be observed.' }
    } finally { Stop-SmokeProcess $app }
} finally {
    if ($installed) {
        Invoke-Installer '/x' 'uninstall.log'
        if ((Test-Path -LiteralPath $install) -or (Test-Path -LiteralPath $shortcut)) {
            throw 'Uninstall left Workbench files or its Start menu shortcut.'
        }
    }
    Write-Host "Windows Installer smoke logs: $logs"
}
