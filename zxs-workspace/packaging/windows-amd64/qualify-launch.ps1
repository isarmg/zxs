# The caller controls the exact candidate and scratch directories for this test.
# Run on the separate, clean Windows 11 x64 desktop. No Python is needed here.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$CandidateRoot,
    [Parameter(Mandatory=$true)][string]$ExpectedReceiptSha256,
    [Parameter(Mandatory=$true)][string]$ScratchDirectory,
    [switch]$Launch
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'qualification-checks.ps1')

Require $Launch 'Use -Launch to approve starting this exact candidate in the isolated test desktop.'
Require ($ExpectedReceiptSha256 -cmatch '\A[0-9a-f]{64}\z') 'Supply the independent candidate.json SHA-256.'
$osBuild = RequireOrdinaryNativeHost

$candidate = LocalDirectory $CandidateRoot
$scratch = LocalDirectory $ScratchDirectory
Require (-not $candidate.FullName.Equals($scratch.FullName, [StringComparison]::OrdinalIgnoreCase) -and
         -not $candidate.FullName.StartsWith($scratch.FullName + '\', [StringComparison]::OrdinalIgnoreCase) -and
         -not $scratch.FullName.StartsWith($candidate.FullName + '\', [StringComparison]::OrdinalIgnoreCase) -and
         @(Get-ChildItem -LiteralPath $scratch.FullName -Force).Count -eq 0) 'Use a separate empty test directory.'
$receipt = Get-Item -LiteralPath (Join-Path $candidate.FullName 'candidate.json') -Force
Require ($receipt -is [IO.FileInfo] -and -not ($receipt.Attributes -band [IO.FileAttributes]::ReparsePoint) -and
         $receipt.Length -gt 0 -and $receipt.Length -le 2MB) 'Expected one bounded candidate receipt.'
$raw = [IO.File]::ReadAllBytes($receipt.FullName)
$sha = [Security.Cryptography.SHA256]::Create()
try { $digest = [BitConverter]::ToString($sha.ComputeHash($raw)).Replace('-', '').ToLowerInvariant() }
finally { $sha.Dispose() }
Require ($digest -ceq $ExpectedReceiptSha256) 'Candidate receipt differs from the approved SHA-256.'
$document = [Text.UTF8Encoding]::new($false, $true).GetString($raw) | ConvertFrom-Json
Require ($document.bundle -ceq 'dist/ZipWorkspace' -and $document.platform -ceq 'windows-amd64' -and
         $document.python -ceq '3.14.10') 'Unexpected portable package receipt.'
$qualified = $document.PSObject.Properties['native_provider_qualification']
$nativeItems = @($document.files.PSObject.Properties | Where-Object { [IO.Path]::GetFileName($_.Name) -imatch 'winfsp|\.sys$' })
Require (($null -eq $qualified -or $null -eq $qualified.Value) -and $nativeItems.Count -eq 0) 'Baseline only: do not launch a native/driver candidate in a smoke test that may terminate its child.'
$bundle = LocalDirectory (Join-Path $candidate.FullName 'dist\ZipWorkspace')
CheckBundle $bundle $document

$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = Join-Path $bundle.FullName 'ZipWorkspace.exe'
$start.UseShellExecute = $false
$start.WorkingDirectory = $scratch.FullName
$start.EnvironmentVariables.Clear()
foreach ($name in @('SystemRoot', 'WINDIR')) { $start.EnvironmentVariables[$name] = $env:SystemRoot }
$start.EnvironmentVariables['PATH'] = [Environment]::SystemDirectory
foreach ($name in @('TEMP', 'TMP', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA')) {
    $start.EnvironmentVariables[$name] = $scratch.FullName
}
$child = [Diagnostics.Process]::Start($start)
try {
    $null = $child.Handle
    $wait = [Diagnostics.Stopwatch]::StartNew()
    do {
        $child.Refresh()
        Require (-not $child.HasExited) 'Portable app exited before exposing its main window.'
        if ($child.MainWindowHandle -ne [IntPtr]::Zero -and $child.MainWindowTitle -ceq 'ZIP Workspace') { break }
        Start-Sleep -Milliseconds 100
    } while ($wait.ElapsedMilliseconds -lt 30000)
    Require ($child.MainWindowHandle -ne [IntPtr]::Zero -and $child.MainWindowTitle -ceq 'ZIP Workspace') 'No actual ZIP Workspace window appeared.'
    Require ($child.CloseMainWindow() -and $child.WaitForExit(15000) -and $child.ExitCode -eq 0) 'The original window did not close cleanly.'
}
finally {
    try {
        if (-not $child.HasExited) {
            $child.Kill()
            Require ($child.WaitForExit(5000)) 'Could not stop the original qualification process.'
        }
    }
    finally { $child.Dispose() }
}
CheckBundle $bundle $document
Write-Output "Windows 11 build $osBuild, x64, standard user: exact portable GUI opened and closed; package files unchanged. Receipt SHA-256: $digest"
