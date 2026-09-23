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

    $otherUser = Get-LocalUser | Where-Object { $_.SID.Value -ne $sid } | Select-Object -First 1
    if ($null -eq $otherUser) {
        throw 'A second local user is required for the owner-mismatch test.'
    }
    $ownerMismatch = $null
    try {
        Initialize-ManagementRoot $testRoot $otherUser.SID.Value | Out-Null
    } catch {
        $ownerMismatch = $_.Exception.Message
    }
    if ($ownerMismatch -notlike 'Unexpected owner or inherited ACL:*') {
        throw "Setup did not reject the existing owner's mismatch: $ownerMismatch"
    }

    $dataChild = [IO.Path]::Combine($testRoot, 'data', 'image-owned')
    [IO.Directory]::CreateDirectory($dataChild) | Out-Null
    $childFile = [IO.Path]::Combine($dataChild, 'image-data.txt')
    [IO.File]::WriteAllText($childFile, 'container data')
    & icacls.exe $dataChild /deny "*$($sid):(OI)(CI)(R)" | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not restrict the image data directory.'
    }
    try {
        $readDenied = $false
        try {
            [IO.File]::ReadAllText($childFile) | Out-Null
        } catch [UnauthorizedAccessException] {
            $readDenied = $true
        }
        if (-not $readDenied) {
            throw 'The GUI user could still read restricted image data.'
        }
        Assert-ManagedDirectory ([IO.Path]::Combine($testRoot, 'data')) (New-Object Security.Principal.SecurityIdentifier($sid))
        if ([IO.File]::ReadAllText($stateFile) -ne 'private') {
            throw 'Restricting image data affected state access.'
        }
    } finally {
        & icacls.exe $dataChild /remove:d "*$sid" | Out-Null
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not restore access to the test image data directory.'
        }
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
    Write-Output 'Windows management root checks passed.'
} finally {
    if (-not ([IO.Path]::GetFullPath($testParent).StartsWith($tempPath, [StringComparison]::OrdinalIgnoreCase))) {
        throw 'Refusing to remove a test directory outside the temporary directory.'
    }
    if (Test-Path -LiteralPath $testParent) {
        Remove-Item -LiteralPath $testParent -Recurse -Force
    }
}
