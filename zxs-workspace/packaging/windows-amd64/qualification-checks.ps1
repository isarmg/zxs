# Read-only helpers shared by the separate frozen GUI acceptance campaigns.
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


function RequireOrdinaryNativeHost {
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
    return [int]$os.BuildNumber
}
