#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$Runtime,
    [Parameter(Mandatory=$true)][string]$Helper,
    [Parameter(Mandatory=$true)][ValidatePattern('^[A-Za-z0-9-]{1,64}$')][string]$Name
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
# Explicit local installation of user-selected build artifacts. Nothing starts.
$root = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
$destination = Join-Path $root "MixyardRuntime-$Name"
if (Test-Path -LiteralPath $destination) { throw 'Refusing to replace an existing installation' }
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
$sddl = "O:${sid}D:P(A;;FA;;;${sid})"
$directorySecurity = [Security.AccessControl.DirectorySecurity]::new()
$directorySecurity.SetSecurityDescriptorSddlForm($sddl)
$directory = [IO.DirectoryInfo]::new($destination)
[IO.FileSystemAclExtensions]::Create($directory, $directorySecurity)
if (($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Installation must not be a reparse point' }
$actual = [IO.FileSystemAclExtensions]::GetAccessControl($directory)
$rules = $actual.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier])
if ($actual.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $sid -or -not $actual.AreAccessRulesProtected -or $rules.Count -ne 1 -or $rules[0].IdentityReference.Value -ne $sid -or $rules[0].AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or $rules[0].FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or $rules[0].InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None -or $rules[0].IsInherited) { throw 'Installation ACL did not match the current SID' }
$fileSecurity = [Security.AccessControl.FileSecurity]::new()
$fileSecurity.SetSecurityDescriptorSddlForm($sddl)
foreach ($item in @(@($Runtime, 'mixyard-runtime.exe'), @($Helper, 'mixyard-windows-inventory.exe'))) {
    $source = Get-Item -LiteralPath $item[0]
    if ($source.PSIsContainer -or ($source.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) { throw 'Source must be a regular local build artifact' }
    $file = [IO.FileInfo]::new((Join-Path $destination $item[1]))
    $output = [IO.FileSystemAclExtensions]::Create($file, [IO.FileMode]::CreateNew, [Security.AccessControl.FileSystemRights]::FullControl, [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough, $fileSecurity)
    try {
        $inputFile = $source.OpenRead()
        try { $inputFile.CopyTo($output) } finally { $inputFile.Dispose() }
        $output.Flush($true)
    } finally { $output.Dispose() }
}
Write-Output (Join-Path $destination 'mixyard-runtime.exe')
