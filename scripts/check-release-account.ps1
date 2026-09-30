# The release build's account guard (launch checklist E3/E4, session 78).
# Ported word for word in its rules from the local release-build8.ps1 (S6,
# SIGNIN-DESIGN 8.33 step 2b: "a release-build guard that fails when they are
# absent"), so the GitHub release build refuses exactly what the local one does.
#
# The seven account settings are read from the environment (ZAAHEEN_*), the
# names the build bakes in with option_env!. For -Account production, the
# sandbox values are read from SANDBOX_ZAAHEEN_* to prove nothing is shared
# (8.31: a sandbox lease, paid with a test card, must never verify in a
# production app). VALUES ARE NEVER PRINTED: only setting names.
#
#   pwsh scripts/check-release-account.ps1 -Account sandbox
#   pwsh scripts/check-release-account.ps1 -Account production
#
# Exit 0 = CHECKS PASSED; exit 1 = "ABORTED: <reason>".
# (pure ASCII on purpose: Windows PowerShell 5.1 misreads UTF-8 without a BOM)

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('production', 'sandbox')]
    [string]$Account
)

$ErrorActionPreference = 'Stop'

function Stop-Check($reason) {
    Write-Host "ABORTED: $reason"
    exit 1
}

$names = @(
    'ZAAHEEN_ACCOUNT_ISSUER', 'ZAAHEEN_ACCOUNT_CLIENT_ID', 'ZAAHEEN_ACCOUNT_API',
    'ZAAHEEN_LEASE_PRIMARY_KID', 'ZAAHEEN_LEASE_PRIMARY_KEY',
    'ZAAHEEN_LEASE_BACKUP_KID', 'ZAAHEEN_LEASE_BACKUP_KEY'
)

function Read-Settings($prefix) {
    $v = @{}
    foreach ($n in $names) {
        $x = [Environment]::GetEnvironmentVariable("$prefix$n")
        if ($x) { $v[$n] = $x.Trim() }
    }
    return $v
}

# Case-sensitive checks (-cmatch): the issuer is compared by exact string (RFC 9207).
$rules = [ordered]@{
    'ZAAHEEN_ACCOUNT_ISSUER'    = '^https://[a-z0-9-]+\.clerk\.accounts\.dev$'
    'ZAAHEEN_ACCOUNT_CLIENT_ID' = '^[A-Za-z0-9_-]{1,128}$'
    'ZAAHEEN_ACCOUNT_API'       = '^https://api-sandbox\.zaaheen\.com$'
    'ZAAHEEN_LEASE_PRIMARY_KID' = '^[a-z0-9-]{1,32}$'
    'ZAAHEEN_LEASE_PRIMARY_KEY' = '^[0-9a-f]{64}$'
    'ZAAHEEN_LEASE_BACKUP_KID'  = '^[a-z0-9-]{1,32}$'
    'ZAAHEEN_LEASE_BACKUP_KEY'  = '^[0-9a-f]{64}$'
}
if ($Account -eq 'production') {
    $rules['ZAAHEEN_ACCOUNT_ISSUER']    = '^https://clerk\.zaaheen\.com$'
    $rules['ZAAHEEN_ACCOUNT_API']       = '^https://api\.zaaheen\.com$'
    $rules['ZAAHEEN_LEASE_PRIMARY_KID'] = '^(?!sandbox)[a-z0-9-]{1,32}$'
    $rules['ZAAHEEN_LEASE_BACKUP_KID']  = '^(?!sandbox)[a-z0-9-]{1,32}$'
}

$vals = Read-Settings ''
$bad = @()
foreach ($k in $rules.Keys) {
    if (-not $vals.ContainsKey($k) -or ($vals[$k] -cnotmatch $rules[$k])) { $bad += $k }
}
if ($bad.Count -gt 0) {
    Stop-Check "account setting(s) missing or malformed for $($Account): $($bad -join ', ')"
}
if ($vals['ZAAHEEN_LEASE_PRIMARY_KID'] -ceq $vals['ZAAHEEN_LEASE_BACKUP_KID'] -or
    $vals['ZAAHEEN_LEASE_PRIMARY_KEY'] -ceq $vals['ZAAHEEN_LEASE_BACKUP_KEY']) {
    Stop-Check 'the primary and backup lease keys must be two different keys with two different ids'
}

if ($Account -eq 'production') {
    # Nothing may be shared with the sandbox pair (8.31). The API and issuer
    # rules above already exclude the sandbox hosts; this catches the rest.
    $sb = Read-Settings 'SANDBOX_'
    if ($sb.Count -lt 5) {
        Stop-Check 'cannot compare with the sandbox settings: SANDBOX_ZAAHEEN_* are missing'
    }
    $cmp = @('ZAAHEEN_ACCOUNT_CLIENT_ID', 'ZAAHEEN_LEASE_PRIMARY_KID', 'ZAAHEEN_LEASE_PRIMARY_KEY', 'ZAAHEEN_LEASE_BACKUP_KID', 'ZAAHEEN_LEASE_BACKUP_KEY')
    $shared = @()
    foreach ($k in $cmp) {
        foreach ($s in $cmp) {
            if ($sb.ContainsKey($s) -and ($vals[$k] -ceq $sb[$s])) { $shared += $k; break }
        }
    }
    if ($shared.Count -gt 0) {
        Stop-Check "production setting(s) equal to a SANDBOX value: $($shared -join ', ')"
    }
    # The issuer must be a live OIDC issuer that names itself exactly so: the
    # app compares it by exact string at sign-in. Three tries: one slow answer
    # (session 76 saw a 20 s timeout) is not a wrong issuer.
    $disc = $null
    for ($try = 1; $try -le 3 -and -not $disc; $try++) {
        try {
            $disc = Invoke-RestMethod -Uri "$($vals['ZAAHEEN_ACCOUNT_ISSUER'])/.well-known/openid-configuration" -TimeoutSec 30
        } catch {
            Write-Host "issuer discovery attempt $try failed"
            if ($try -lt 3) { Start-Sleep -Seconds 10 }
        }
    }
    if (-not $disc) { Stop-Check 'could not read the issuer discovery document after 3 tries' }
    if ($disc.issuer -cne $vals['ZAAHEEN_ACCOUNT_ISSUER']) {
        Stop-Check 'the issuer discovery document names a different issuer'
    }
}

Write-Host "CHECKS PASSED: account $($Account.ToUpper()), 7 of 7 well-formed (values not printed)"
exit 0
