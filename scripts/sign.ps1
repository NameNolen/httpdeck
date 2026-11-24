# Authenticode code-sign the given executable files.
# Certificate sources (highest priority first):
#   1. -CertFile <pfx> + -Password <secret>      explicit pfx file
#   2. env SIGNING_CERT_BASE64 + SIGNING_CERT_PASSWORD   base64 pfx injected by CI
#   3. -Thumbprint <thumb> or env SIGNING_CERT_THUMBPRINT cert in the local store
# If no certificate is configured the script exits 0 without signing, so it
# never blocks the build pipeline.
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Files,

    [string]$CertFile,
    [string]$Password,
    [string]$Thumbprint,

    [switch]$SkipVerify,

    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"

$targets = @($Files | Where-Object { Test-Path $_ })
if ($targets.Count -eq 0) {
    Write-Host "No files to sign, skipping."
    exit 0
}

if (-not $CertFile) { $CertFile = $env:SIGNING_CERT_FILE }
if (-not $Password) { $Password = $env:SIGNING_CERT_PASSWORD }
if (-not $Thumbprint) { $Thumbprint = $env:SIGNING_CERT_THUMBPRINT }

$certBase64 = $env:SIGNING_CERT_BASE64
$hasCert = [bool]$CertFile -or [bool]$certBase64 -or [bool]$Thumbprint

if (-not $hasCert) {
    Write-Host "No signing certificate configured (SIGNING_CERT_BASE64 / SIGNING_CERT_THUMBPRINT / -CertFile), skipping."
    exit 0
}

$signtool = $null
$sdkTool = Get-ChildItem "C:\Program Files (x86)\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName -Descending | Select-Object -First 1 -ExpandProperty FullName
$candidates = @("signtool", $sdkTool) | Where-Object { $_ }
foreach ($c in $candidates) {
    if (Test-Path $c) { $signtool = $c; break }
    $cmd = Get-Command $c -ErrorAction SilentlyContinue
    if ($cmd) { $signtool = $cmd.Source; break }
}
if (-not $signtool) { throw "signtool not found; install the Windows SDK." }
Write-Host "Using signtool: $signtool"

# signtool writes progress to stderr; under Stop that surfaces as an exception.
# Switch to Continue for native calls and rely on $LASTEXITCODE instead.
$ErrorActionPreference = "Continue"

$tempPfx = $null
if ($certBase64) {
    $tempPfx = Join-Path $env:TEMP ("signing-cert-" + [guid]::NewGuid().ToString("N") + ".pfx")
    [IO.File]::WriteAllBytes($tempPfx, [Convert]::FromBase64String($certBase64))
    if (-not (Test-Path $tempPfx)) { throw "Failed to write base64 pfx to disk." }
    $CertFile = $tempPfx
}

try {
    $commonArgs = @("/fd", "sha256")
    if ($CertFile) {
        if (-not $Password) { throw "A password is required when signing with a pfx file (SIGNING_CERT_PASSWORD or -Password)." }
        $args = @("sign", "/f", $CertFile, "/p", $Password) + $commonArgs
    } else {
        $args = @("sign", "/sha1", $Thumbprint) + $commonArgs
    }

    $ok = $false
    foreach ($ts in @($TimestampUrl, $null)) {
        $runArgs = $args.Clone()
        if ($ts) { $runArgs += @("/tr", $ts, "/td", "sha256") }
        $runArgs += $targets
        Write-Host ">> $signtool $($runArgs -join ' ')"
        & $signtool @runArgs
        if ($LASTEXITCODE -eq 0) { $ok = $true; break }
        Write-Warning "sign failed (exit $LASTEXITCODE); $(if ($ts) {'retrying without timestamp'} else {'giving up'})"
    }
    if (-not $ok) { throw "Code signing failed." }

    if ($SkipVerify) {
        Write-Host "SkipVerify specified, skipping signature verification."
        foreach ($f in $targets) { Write-Host "Signed (unverified): $f" }
        return
    }

    foreach ($f in $targets) {
        & $signtool verify /pa /v $f > $null 2>&1
        if ($LASTEXITCODE -ne 0) { throw "Signature verification failed: $f" }
        Write-Host "Signed: $f"
    }
} finally {
    if ($tempPfx -and (Test-Path $tempPfx)) { Remove-Item $tempPfx -Force -ErrorAction SilentlyContinue }
}
