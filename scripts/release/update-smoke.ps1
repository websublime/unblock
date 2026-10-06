# update-smoke.ps1 — the LIVE `unblock update` end-to-end smoke, Windows half (FR-25 / NFR-17; spec:
# docs/plans/ci-cd-and-distribution.md §4; tracked as ub-lp9.26). Mirrors update-smoke.sh step for step
# against the REAL dist powershell installer; read that file's header for what is proved, the non-goals
# (dist's own SHA256 code, attestations off the update path) and the network/isolation posture.
#
# PRECONDITION: <ToTag> must be the LATEST STABLE release — `unblock update` always targets latest.
#
# A REFUSED QUERY. Two binaries query GitHub here. The <FromTag> binary runs `update --dry-run` and
# `update`, and the swapped-in <ToTag> binary runs the post-swap `update --dry-run`; the version of the
# binary that ran the failing step decides how a refusal reads. v1.0.2 and later (PRD D55) send the token
# and give RATE_LIMITED, exit 2, for a 403 or 429, and CONFIG_ERROR, exit 7, for a 401; v1.0.0 and
# v1.0.1 send none and give INTERNAL_ERROR, exit 1, for a 403 or a 429. So a v1.0.1 -> v1.0.2 run can
# already show RATE_LIMITED at the post-swap step. Key on the step, the code and the exit code, never on
# the reason text.
#
# Usage:  pwsh -File scripts/release/update-smoke.ps1 -FromTag v1.0.0 -ToTag v1.0.1
# Env:    UNBLOCK_SMOKE_REPO (default websublime/unblock) · AXOUPDATER_GITHUB_TOKEN (optional; only a
#         binary that reads it, ub-jh5, sends it — v1.0.0 and v1.0.1 ignore it; ci-cd §4)
#         UNBLOCK_SMOKE_KEEP=1 keeps the temp dir.
# Exit:   0 = every step passed · 1 = a smoke assertion failed.
param(
    [Parameter(Mandatory = $true)][string]$FromTag,
    [Parameter(Mandatory = $true)][string]$ToTag
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false

function Say([string]$Message) { [Console]::Error.WriteLine("update-smoke: $Message") }
function Fail([string]$Message) { Say "FAIL: $Message"; exit 1 }

$FromVer = $FromTag.TrimStart('v')
$ToVer = $ToTag.TrimStart('v')
$Repo = if ($env:UNBLOCK_SMOKE_REPO) { $env:UNBLOCK_SMOKE_REPO } else { 'websublime/unblock' }

$Root = Join-Path ([IO.Path]::GetTempPath()) ("unblock-update-smoke." + [Guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $Root

$env:UNBLOCK_CLI_INSTALL_DIR = Join-Path $Root 'prefix'
$env:UNBLOCK_CLI_NO_MODIFY_PATH = '1'
# Both the powershell installer and axoupdater consult XDG_CONFIG_HOME before LOCALAPPDATA.
$env:XDG_CONFIG_HOME = Join-Path $Root 'config'
Remove-Item Env:UNBLOCK_DIR, Env:UNBLOCK_ACTOR, Env:UNBLOCK_OUTPUT_FORMAT -ErrorAction SilentlyContinue
$Bin = Join-Path $env:UNBLOCK_CLI_INSTALL_DIR 'bin\unblock.exe'
$Receipt = Join-Path $env:XDG_CONFIG_HOME 'unblock-cli\unblock-cli-receipt.json'
$Ws = Join-Path $Root 'ws'
$Out = Join-Path $Root 'out'
$Err = Join-Path $Root 'err'

# Run — echo the command, run it with stdout/stderr captured, replay both, return its exit code. The
# transcript goes to the HOST stream (Write-Host), never the pipeline, so the function's only pipeline
# output is the exit code its callers compare.
function Run([string]$Exe, [string[]]$Arguments = @()) {
    Write-Host ''
    Write-Host ('$ ' + $Exe + ' ' + ($Arguments -join ' '))
    # Start-Process joins -ArgumentList with bare spaces, so quote any argument that contains one.
    $quoted = @($Arguments | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } })
    $p = Start-Process -FilePath $Exe -ArgumentList $quoted -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $Out -RedirectStandardError $Err
    Get-Content $Out | ForEach-Object { Write-Host "  [stdout] $_" }
    Get-Content $Err | ForEach-Object { Write-Host "  [stderr] $_" }
    Write-Host "  [exit] $($p.ExitCode)"
    return $p.ExitCode
}
function StdoutText { "$(Get-Content -Raw $Out)".Trim() }
function StderrHas([string]$Needle) { "$(Get-Content -Raw $Err)" -like "*$Needle*" }
function Sha256([string]$Path) { (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant() }

try {
    Say "repo=$Repo from=$FromTag to=$ToTag host=$([Runtime.InteropServices.RuntimeInformation]::OSDescription) $([Runtime.InteropServices.RuntimeInformation]::OSArchitecture)"

    # 1. Install <FromTag> with the REAL dist powershell installer, so a genuine receipt exists.
    $Installer = Join-Path $Root 'installer.ps1'
    $InstallerUrl = "https://github.com/$Repo/releases/download/$FromTag/unblock-cli-installer.ps1"
    Invoke-WebRequest -UseBasicParsing -Uri $InstallerUrl -OutFile $Installer
    $pwsh = (Get-Process -Id $PID).Path
    if ((Run $pwsh @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $Installer)) -ne 0) { Fail "the $FromTag installer exited non-zero" }
    if (-not (Test-Path $Bin)) { Fail "installer did not place $Bin" }
    if (-not (Test-Path $Receipt)) { Fail "installer wrote no receipt at $Receipt" }
    Write-Host ''
    Write-Host "receipt ($Receipt):"
    Get-Content $Receipt | ForEach-Object { Write-Host "  $_" }

    if ((Run $Bin @('version')) -ne 0) { Fail "installed $FromTag cannot run 'version'" }
    if ((Run $Bin @('version', '--short')) -ne 0) { Fail "installed $FromTag cannot run 'version --short'" }
    if ((StdoutText) -ne $FromVer) { Fail "installed binary reports '$(StdoutText)', expected $FromVer" }

    # A workspace created by the OLD binary, so the post-swap binary is exercised against real old state.
    $null = New-Item -ItemType Directory -Path $Ws
    Push-Location $Ws
    try {
        if ((Run $Bin @('init')) -ne 0) { Fail "$FromTag cannot 'init' a workspace" }
        if ((Run $Bin @('doctor')) -ne 0) { Fail "$FromTag 'doctor' failed on its own fresh workspace" }
    } finally { Pop-Location }

    $ShaBefore = Sha256 $Bin
    Say "sha256 before: $ShaBefore"

    # 2. --dry-run: resolves the real release source, reports <ToTag>, swaps NOTHING.
    if ((Run $Bin @('update', '--dry-run')) -ne 0) { Fail "'update --dry-run' exited non-zero" }
    if (-not (StderrHas "update available: $ToVer")) { Fail "'update --dry-run' did not report $ToVer (is $ToTag the latest STABLE release of $Repo?)" }
    if ((Sha256 $Bin) -ne $ShaBefore) { Fail "'update --dry-run' modified the binary" }
    Say "dry-run: reported $ToVer, binary byte-identical"

    # 3. The real update: download + dist installer SHA256 verify + swap.
    if ((Run $Bin @('update')) -ne 0) { Fail "'update' exited non-zero" }
    if (-not (StderrHas "updated to $ToTag")) { Fail "'update' did not report 'updated to $ToTag'" }
    $ShaAfter = Sha256 $Bin
    Say "sha256 after:  $ShaAfter"
    if ($ShaAfter -eq $ShaBefore) { Fail "'update' reported success but the binary is unchanged" }
    if (-not ((Get-Content -Raw $Receipt) -like "*`"version`":`"$ToVer`"*")) { Fail "receipt does not record version $ToVer after the swap" }

    # 4. The swapped-in binary is <ToTag> and still serves real commands.
    if ((Run $Bin @('version')) -ne 0) { Fail "swapped binary cannot run 'version'" }
    if ((Run $Bin @('version', '--short')) -ne 0) { Fail "swapped binary cannot run 'version --short'" }
    if ((StdoutText) -ne $ToVer) { Fail "swapped binary reports '$(StdoutText)', expected $ToVer" }
    Push-Location $Ws
    try {
        if ((Run $Bin @('migrate')) -ne 0) { Fail "$ToTag 'migrate' failed on the $FromTag workspace" }
        if ((Run $Bin @('doctor')) -ne 0) { Fail "$ToTag 'doctor' failed on the $FromTag workspace" }
    } finally { Pop-Location }
    if ((Run $Bin @('update', '--dry-run')) -ne 0) { Fail "post-swap 'update --dry-run' exited non-zero" }
    if (-not (StderrHas 'already up to date')) { Fail "post-swap 'update --dry-run' does not report up to date" }

    Say "PASS: $FromTag -> $ToTag via the real dist installer on Windows"
} finally {
    if ($env:UNBLOCK_SMOKE_KEEP -eq '1') { Say "keeping $Root" } else { Remove-Item -Recurse -Force $Root -ErrorAction SilentlyContinue }
}
