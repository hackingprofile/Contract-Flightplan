# The README quickstart: build cfp, then compare, review, approve, verify and report on the
# rename pack. Writes into out/. Runs under PowerShell 7 on Windows, Linux and macOS, and under
# Windows PowerShell 5.1.
#
#   pwsh -File scripts/quickstart.ps1
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = Split-Path -Parent $PSScriptRoot
Set-Location $Root

cargo build --release --locked
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$cfp = Get-CfpBinary $Root
$pack = Join-Path "packs" "rename-user-id"
$base = Join-Path $pack "base.openapi.yaml"
$next = Join-Path $pack "next.openapi.yaml"
$proposal = Join-Path $pack "proposal.json"
$fixtures = Join-Path $pack "fixtures"
$out = "out"
New-Item -ItemType Directory -Force -Path $out | Out-Null
function Out-File-Path([string]$Name) { return (Join-Path $out $Name) }

& $cfp compare --source $base --destination $next --out (Out-File-Path "diff.json")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cfp review --proposal $proposal --source $base --destination $next --out (Out-File-Path "review.json")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cfp approve --proposal $proposal --source $base --destination $next --fixtures $fixtures --review (Out-File-Path "review.json") --reviewer "local-reviewer" --approved-at "2026-09-18T00:00:00Z" --out (Out-File-Path "migration.json")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cfp verify --migration (Out-File-Path "migration.json") --proposal $proposal --source $base --destination $next --fixtures $fixtures --out (Out-File-Path "verify.json")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& $cfp report --source $base --destination $next --proposal $proposal --migration (Out-File-Path "migration.json") --fixtures $fixtures --verify (Out-File-Path "verify.json") --html (Out-File-Path "report.html") --json (Out-File-Path "report.json") --md (Out-File-Path "report.md") --sarif (Out-File-Path "report.sarif") --job-summary (Out-File-Path "job-summary.md")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Output "quickstart wrote $(Join-Path $Root $out)"
