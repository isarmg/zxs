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

function Require($Condition, [string]$Reason) {
    if (-not $Condition) { throw $Reason }
}

function LocalDirectory([string]$Path) {
    Require ($Path -cmatch '\A[A-Za-z]:\\[^\\/]') 'Use a full local Windows path.'
    $pathName = $Path.TrimEnd('\')
    Require (([IO.Path]::GetFullPath($pathName)) -ceq $pathName) 'Use the exact absolute directory name.'
    $dir = Get-Item -LiteralPath $pathName -Force
    Require ($dir -is [IO.DirectoryInfo]) 'Expected a directory.'
    $drive = [IO.DriveInfo]::new($dir.Root.FullName)
    Require ($drive.DriveType -eq [IO.DriveType]::Fixed -and $drive.DriveFormat -ceq 'NTFS') 'Use local fixed NTFS.'
    for ($at = $dir; $null -ne $at; $at = $at.Parent) {
        Require (-not ($at.Attributes -band [IO.FileAttributes]::ReparsePoint)) 'Directory path contains a reparse point.'
    }
    return $dir
}

function FileSha256([IO.FileInfo]$File) {
    $opened = $File.Open([IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
    try { return (Get-FileHash -InputStream $opened -Algorithm SHA256).Hash.ToLowerInvariant() }
    finally { $opened.Dispose() }
}

function CheckBundle([IO.DirectoryInfo]$Bundle, $Document) {
    $expected = [Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
    foreach ($row in $Document.files.PSObject.Properties) {
        $expected.Add($row.Name, $row.Value)
    }
    foreach ($name in @('ZipWorkspace.exe', '_internal/python314.dll', '_internal/PySide6/plugins/platforms/qwindows.dll')) {
        Require ($expected.ContainsKey($name)) 'Incomplete portable package receipt.'
    }
    $todo = [Collections.Generic.Stack[IO.DirectoryInfo]]::new()
    $todo.Push($Bundle)
    $prefix = $Bundle.FullName + '\'
    $entries = 0
    while ($todo.Count -gt 0) {
        foreach ($item in (Get-ChildItem -LiteralPath ($todo.Pop().FullName) -Force)) {
            $entries += 1
            Require ($entries -le 10000 -and -not ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) 'Unexpected package inventory.'
            if ($item -is [IO.DirectoryInfo]) { $todo.Push($item); continue }
            Require ($item -is [IO.FileInfo]) 'Expected a package file.'
            $name = $item.FullName.Substring($prefix.Length).Replace('\', '/')
            Require ($expected.ContainsKey($name)) "Unlisted package file: $name"
            $row = $expected[$name]
            Require ($item.Length -eq $row.bytes -and $row.sha256 -cmatch '\A[0-9a-f]{64}\z' -and
                     (FileSha256 $item) -ceq $row.sha256) "Package file differs: $name"
            $null = $expected.Remove($name)
        }
    }
    Require ($expected.Count -eq 0) 'Missing package files.'
}

Require $Launch 'Use -Launch to approve starting this exact candidate in the isolated test desktop.'
Require ($ExpectedReceiptSha256 -cmatch '\A[0-9a-f]{64}\z') 'Supply the independent candidate.json SHA-256.'
Require ([Environment]::OSVersion.Platform -eq [PlatformID]::Win32NT -and [Environment]::Is64BitProcess) 'Use native 64-bit Windows PowerShell.'
$os = Get-CimInstance -ClassName Win32_OperatingSystem
$processors = @(Get-CimInstance -ClassName Win32_Processor)
Require ($os.ProductType -eq 1 -and [int]$os.BuildNumber -ge 22000 -and
         $processors.Count -ge 1 -and @($processors | Where-Object Architecture -NE 9).Count -eq 0) 'Use Windows 11 on a native x64 host.'
Require ([Environment]::UserInteractive -and [Diagnostics.Process]::GetCurrentProcess().SessionId -gt 0) 'Use an interactive desktop.'
$whoami = Join-Path ([Environment]::SystemDirectory) 'whoami.exe'
$groups = & $whoami /groups /fo csv /nh
Require ($LASTEXITCODE -eq 0) 'Could not check the current user token.'
$sids = @($groups | ConvertFrom-Csv -Header 'Name','Type','Sid','Flags' | ForEach-Object Sid)
Require ('S-1-16-8192' -in $sids -and 'S-1-5-32-544' -notin $sids) 'Use a standard medium-integrity account, not an Administrator account.'

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
Write-Output "Windows 11 build $($os.BuildNumber), x64, standard user: exact portable GUI opened and closed; package files unchanged. Receipt SHA-256: $digest"
