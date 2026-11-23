# Final frozen GUI exercise. Never stop a driver, kill an editor, synthesize a receipt or send test data.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$SignedBundle,
    [Parameter(Mandatory=$true)][string]$SignedInventory,
    [Parameter(Mandatory=$true)][string]$ExpectedInventorySha256,
    [Parameter(Mandatory=$true)][string]$ExpectedAppSignerCertSha256,
    [Parameter(Mandatory=$true)][string]$ExpectedProviderSignerCertSha256,
    [Parameter(Mandatory=$true)][string]$ScratchDirectory,
    [switch]$LaunchIsolatedNativeCampaign
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'qualification-checks.ps1')
Require $LaunchIsolatedNativeCampaign 'Explicit approval to start this final candidate in a disposable Windows test desktop is required.'
$osBuild = RequireOrdinaryNativeHost
foreach ($pin in @($ExpectedInventorySha256,$ExpectedAppSignerCertSha256,$ExpectedProviderSignerCertSha256)) {
    Require ($pin -cmatch '\A[0-9a-f]{64}\z') 'Supply independent SHA-256 pins (not copied from the untrusted candidate).'
}
$bundle = LocalDirectory $SignedBundle
$scratch = LocalDirectory $ScratchDirectory
Require (-not $bundle.FullName.Equals($scratch.FullName,[StringComparison]::OrdinalIgnoreCase) -and
         -not $scratch.FullName.StartsWith($bundle.FullName+'\',[StringComparison]::OrdinalIgnoreCase) -and
         -not $bundle.FullName.StartsWith($scratch.FullName+'\',[StringComparison]::OrdinalIgnoreCase) -and
         @(Get-ChildItem -LiteralPath $scratch.FullName -Force).Count -eq 0) 'Separate empty ordinary local scratch required.'
$parent = LocalDirectory ([IO.Path]::GetDirectoryName($SignedInventory))
$receipt = Get-Item -LiteralPath $SignedInventory -Force
Require ($receipt -is [IO.FileInfo] -and $receipt.Directory.FullName -ceq $parent.FullName -and
         -not ($receipt.Attributes -band [IO.FileAttributes]::ReparsePoint) -and $receipt.Length -gt 0 -and
         $receipt.Length -le 2MB) 'Expected a bounded final signed-tree inventory.'
function ShaBytes([byte[]]$Bytes) {
    $h = [Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($h.ComputeHash($Bytes)).Replace('-','').ToLowerInvariant() }
    finally { $h.Dispose() }
}
$raw = [IO.File]::ReadAllBytes($receipt.FullName)
Require ((ShaBytes $raw) -ceq $ExpectedInventorySha256) 'Final inventory differs from independent release evidence.'
$document = [Text.UTF8Encoding]::new($false,$true).GetString($raw) | ConvertFrom-Json
$native = '_internal/zipworkspace/_workspace_winfsp_draft_host.pyd'
Require ($document.format -ceq 'workspace-signed-windows-candidate/1' -and $document.platform -ceq 'windows-amd64' -and
         $document.attested_build_commit -cmatch '\A[0-9a-f]{40}\z' -and $document.native_fixture_sha256 -cmatch '\A[0-9a-f]{64}\z' -and
         $document.app_signer_cert_sha256 -ceq $ExpectedAppSignerCertSha256 -and
         $document.provider_signer_cert_sha256 -ceq $ExpectedProviderSignerCertSha256 -and
         $document.files.$native.sha256 -ceq $document.provider_sha256) 'Wrong native final candidate.'
CheckBundle $bundle $document
function RequireSigner([string]$File, [string]$Pin) {
    # Extract pinned publisher certificate from the exact inventory file, without a trust download.
    # Its signature was verified under WinVerifyTrust at final inventory prep; the frozen loader
    # independently requires WinVerifyTrust against the held original on this host before use.
    $cert = [Security.Cryptography.X509Certificates.X509Certificate]::CreateFromSignedFile($File)
    try { Require ((ShaBytes ($cert.GetRawCertData())) -ceq $Pin) "Wrong publisher certificate: $File" }
    finally { $cert.Dispose() }
}
$entry = Join-Path $bundle.FullName 'ZipWorkspace.exe'
RequireSigner $entry $ExpectedAppSignerCertSha256
RequireSigner (Join-Path $bundle.FullName $native.Replace('/','\')) $ExpectedProviderSignerCertSha256
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = $entry
$start.UseShellExecute = $false
$start.WorkingDirectory = $scratch.FullName
$start.EnvironmentVariables.Clear()
foreach ($name in @('SystemRoot','WINDIR')) { $start.EnvironmentVariables[$name] = $env:SystemRoot }
$start.EnvironmentVariables['PATH'] = [Environment]::SystemDirectory
foreach ($name in @('TEMP','TMP','USERPROFILE','APPDATA','LOCALAPPDATA')) { $start.EnvironmentVariables[$name] = $scratch.FullName }
$child = [Diagnostics.Process]::Start($start)
try {
    $null = $child.Handle
    $wait = [Diagnostics.Stopwatch]::StartNew()
    do {
        $child.Refresh()
        Require (-not $child.HasExited) 'Frozen GUI exited before start of the isolated campaign.'
        if ($child.MainWindowHandle -ne [IntPtr]::Zero -and $child.MainWindowTitle -ceq 'ZIP Workspace') { break }
        Start-Sleep -Milliseconds 100
    } while ($wait.ElapsedMilliseconds -lt 30000)
    Require ($child.MainWindowHandle -ne [IntPtr]::Zero -and $child.MainWindowTitle -ceq 'ZIP Workspace') 'No actual signed Workspace window.'
    Write-Host "Started only the checked final executable, PID $($child.Id). No mount or save has been certified."
    Write-Host 'Follow the separate native test plan. Stop the mount in the GUI and close the application yourself.'
    $child.WaitForExit()
    Require ($child.ExitCode -eq 0) 'Signed GUI did not exit cleanly.'
}
finally { $child.Dispose() } # Tracking handle only. NEVER kill a process that may own a live mount.
CheckBundle $bundle $document
Write-Host "Windows 11 build $osBuild, native x64 standard user. The final GUI closed with unchanged inventory $ExpectedInventorySha256. Verify the original receipt and bytes separately."
