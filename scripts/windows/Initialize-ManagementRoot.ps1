#Requires -Version 5.1
param(
    [Parameter(Mandatory = $true)]
    [string] $OwnerSid
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class AccountSidLookup {
    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool LookupAccountSid(
        string systemName, byte[] sid, StringBuilder name, ref uint nameLength,
        StringBuilder domain, ref uint domainLength, out int accountType);

    public static bool IsUser(byte[] sid) {
        uint nameLength = 0, domainLength = 0;
        int accountType;
        LookupAccountSid(null, sid, null, ref nameLength, null, ref domainLength, out accountType);
        var name = new StringBuilder((int)nameLength);
        var domain = new StringBuilder((int)domainLength);
        return LookupAccountSid(null, sid, name, ref nameLength, domain, ref domainLength, out accountType)
            && accountType == 1; // SidTypeUser
    }
}
'@

function Resolve-ManagementRoot {
    $programData = [Environment]::GetFolderPath([Environment+SpecialFolder]::CommonApplicationData)
    if ([string]::IsNullOrWhiteSpace($programData)) {
        throw 'The ProgramData known folder could not be resolved.'
    }
    return [IO.Path]::Combine($programData, 'ComposeNest')
}

function New-ManagementSecurity([Security.Principal.SecurityIdentifier] $owner) {
    $security = New-Object Security.AccessControl.DirectorySecurity
    $security.SetAccessRuleProtection($true, $false)
    $security.SetOwner($owner)
    $system = New-Object Security.Principal.SecurityIdentifier('S-1-5-18')
    $administrators = New-Object Security.Principal.SecurityIdentifier('S-1-5-32-544')
    $inheritance = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
    foreach ($identity in @($system, $administrators, $owner)) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            $identity,
            [Security.AccessControl.FileSystemRights]::FullControl,
            $inheritance,
            [Security.AccessControl.PropagationFlags]::None,
            [Security.AccessControl.AccessControlType]::Allow
        )
        $security.AddAccessRule($rule)
    }
    return $security
}

function Assert-ManagedDirectory([string] $path, [Security.Principal.SecurityIdentifier] $owner) {
    $item = Get-Item -LiteralPath $path -Force
    if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing a file or reparse point: $path"
    }
    $security = Get-Acl -LiteralPath $path
    if (-not $security.AreAccessRulesProtected -or $security.GetOwner([Security.Principal.SecurityIdentifier]).Value -ne $owner.Value) {
        throw "Unexpected owner or inherited ACL: $path"
    }
    $rules = @($security.GetAccessRules($true, $false, [Security.Principal.SecurityIdentifier]))
    $expected = @('S-1-5-18', 'S-1-5-32-544', $owner.Value)
    $inheritance = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [Security.AccessControl.InheritanceFlags]::ObjectInherit
    if ($rules.Count -ne 3) {
        throw "Unexpected ACL entries: $path"
    }
    foreach ($rule in $rules) {
        if ($rule.IdentityReference.Value -notin $expected -or
            $rule.AccessControlType -ne [Security.AccessControl.AccessControlType]::Allow -or
            $rule.FileSystemRights -ne [Security.AccessControl.FileSystemRights]::FullControl -or
            $rule.InheritanceFlags -ne $inheritance -or
            $rule.PropagationFlags -ne [Security.AccessControl.PropagationFlags]::None) {
            throw "Unexpected ACL entries: $path"
        }
        $expected = @($expected | Where-Object { $_ -ne $rule.IdentityReference.Value })
    }
    if ($expected.Count -ne 0) {
        throw "Missing ACL entries: $path"
    }
}

function Test-ExistingItem([string] $path) {
    # Get-Item also reports a dangling reparse point, which Test-Path can miss.
    return $null -ne (Get-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue)
}

function Initialize-ManagementRoot([string] $root, [string] $sid) {
    if ($sid -notmatch '^S-1-5-21-(\d+-){3}\d+$') {
        throw 'Specify an existing individual user SID (S-1-5-21-...).'
    }
    $owner = New-Object Security.Principal.SecurityIdentifier($sid)
    try {
        $account = $owner.Translate([Security.Principal.NTAccount])
    } catch {
        throw "The selected SID does not resolve to an account: $sid"
    }
    $sidBytes = New-Object byte[] $owner.BinaryLength
    $owner.GetBinaryForm($sidBytes, 0)
    if (-not [AccountSidLookup]::IsUser($sidBytes)) {
        throw "The selected SID is not an individual user: $sid"
    }

    $directories = @('locks', 'state', 'templates', 'templates\local', 'instances', 'data', 'ownership', 'staging', 'diagnostics')
    # Inspect the existing tree before creating anything; never repair it recursively.
    if (Test-ExistingItem $root) {
        Assert-ManagedDirectory $root $owner
    }
    foreach ($relativePath in $directories) {
        $path = [IO.Path]::Combine($root, $relativePath)
        if (Test-ExistingItem $path) {
            Assert-ManagedDirectory $path $owner
        }
    }

    $security = New-ManagementSecurity $owner
    foreach ($path in @($root) + @($directories | ForEach-Object { [IO.Path]::Combine($root, $_) })) {
        if (-not (Test-ExistingItem $path)) {
            # Windows PowerShell 5.1 creates the directory with its ACL in one call.
            [IO.Directory]::CreateDirectory($path, $security) | Out-Null
        }
        Assert-ManagedDirectory $path $owner
    }
    Write-Output "Initialized $root for $account ($sid)"
}

if ($MyInvocation.InvocationName -ne '.') {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = New-Object Security.Principal.WindowsPrincipal($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Run initial setup in an elevated Windows PowerShell session.'
    }
    Initialize-ManagementRoot (Resolve-ManagementRoot) $OwnerSid
}
