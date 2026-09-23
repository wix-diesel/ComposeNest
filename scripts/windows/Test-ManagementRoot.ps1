#Requires -Version 5.1
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
. "$PSScriptRoot\Initialize-ManagementRoot.ps1" -OwnerSid $sid

$tempPath = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$testParent = [IO.Path]::Combine($tempPath, "ComposeNest-$(New-Guid)")
$testRoot = [IO.Path]::Combine($testParent, '日本語 path', 'ComposeNest')
try {
    Initialize-ManagementRoot $testRoot $sid | Out-Null
    Initialize-ManagementRoot $testRoot $sid | Out-Null
    $stateFile = [IO.Path]::Combine($testRoot, 'state', 'probe.txt')
    [IO.File]::WriteAllText($stateFile, 'private')
    if ([IO.File]::ReadAllText($stateFile) -ne 'private') {
        throw 'The selected user could not read the state file.'
    }
    $fileAcl = Get-Acl -LiteralPath $stateFile
    $fileSids = @($fileAcl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]) |
        ForEach-Object { $_.IdentityReference.Value })
    if ('S-1-5-32-545' -in $fileSids -or 'S-1-1-0' -in $fileSids) {
        throw 'A created file grants access to Users or Everyone.'
    }

    $dataChild = [IO.Path]::Combine($testRoot, 'data', 'image-owned')
    [IO.Directory]::CreateDirectory($dataChild) | Out-Null
    $otherSid = 'S-1-5-18'
    $denied = $false
    try {
        Initialize-ManagementRoot $testRoot $otherSid | Out-Null
    } catch {
        $denied = $true
    }
    if (-not $denied) {
        throw 'Setup accepted a different owner SID.'
    }
    $staging = [IO.Path]::Combine($testRoot, 'staging')
    & icacls.exe $staging /grant '*S-1-5-32-545:(R)' | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not add the unexpected test ACL entry.'
    }
    $denied = $false
    try {
        Initialize-ManagementRoot $testRoot $sid | Out-Null
    } catch {
        $denied = $true
    }
    if (-not $denied) {
        throw 'Setup accepted an unexpected ACL entry.'
    }
    $stagingAcl = Get-Acl -LiteralPath $staging
    if (-not (@($stagingAcl.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]) |
        Where-Object { $_.IdentityReference.Value -eq 'S-1-5-32-545' }).Count -eq 1)) {
        throw 'Setup changed the existing ACL while rejecting it.'
    }
    Assert-ManagedDirectory ([IO.Path]::Combine($testRoot, 'data')) (New-Object Security.Principal.SecurityIdentifier($sid))
    Write-Output 'Windows management root checks passed.'
} finally {
    if (-not ([IO.Path]::GetFullPath($testParent).StartsWith($tempPath, [StringComparison]::OrdinalIgnoreCase))) {
        throw 'Refusing to remove a test directory outside the temporary directory.'
    }
    if (Test-Path -LiteralPath $testParent) {
        Remove-Item -LiteralPath $testParent -Recurse -Force
    }
}
