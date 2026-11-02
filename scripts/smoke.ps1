# Small end-to-end product check. Builds cfp, then proves the happy path and
# the two failure paths that matter most (an ill-typed proposal and a fixture
# that disagrees with its catalog). Prints one PASS/FAIL line per check and
# exits 1 if any check failed. Writes only under the system temporary folder.
# Runs under PowerShell 7 on Windows, Linux and macOS, and Windows PowerShell 5.1.
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $Root

cargo build --release --quiet
if ($LASTEXITCODE -ne 0) { Write-Error "cargo build failed"; exit 1 }
$cfp = Get-CfpBinary $Root
if (-not (Test-Path $cfp)) { Write-Error "cfp not found at $cfp"; exit 1 }

$work = New-CfpTempDir "cfp-smoke"

$results = New-Object System.Collections.ArrayList
function Check([string]$name, [bool]$ok, [string]$detail = "") {
  $status = if ($ok) { "PASS" } else { "FAIL" }
  [void]$results.Add([pscustomobject]@{ Check = $name; Result = $status; Detail = $detail })
  Write-Host ("[{0}] {1} {2}" -f $status, $name, $detail)
}
# Run cfp with its arguments passed unchanged; stderr is captured, not raised.
function Run([string[]]$Arguments) {
  $r = Invoke-CfpNative $cfp $Arguments
  return @{ Code = $r.Code; Out = $r.Out.TrimEnd() }
}
function ReadJson([string]$path) { Get-Content -Raw $path | ConvertFrom-Json }

# 1. Happy path on a shipped pack.
$pack = Join-Path $Root "packs/rename-user-id"
$src = Join-Path $pack "base.openapi.yaml"; $dst = Join-Path $pack "next.openapi.yaml"; $fx = Join-Path $pack "fixtures"
$r = Run @("review", "--proposal", (Join-Path $pack "proposal.json"), "--source", $src, "--destination", $dst, "--out", (Join-Path $work "review.json"))
$review = ReadJson (Join-Path $work "review.json")
Check "review: well-typed proposal exits 0 with typecheck ok" ($r.Code -eq 0 -and $review.typecheck -eq "ok") "diagnostics=$($review.diagnostics.Count)"

$migration = Join-Path $work "migration.json"
$r = Run @("approve", "--proposal", (Join-Path $pack "proposal.json"), "--source", $src, "--destination", $dst, "--fixtures", $fx, "--review", (Join-Path $work "review.json"), "--reviewer", "smoke", "--approved-at", "2026-09-22T00:00:00Z", "--out", $migration)
Check "approve: creates digest-bound migration from the review record" ($r.Code -eq 0 -and (Test-Path $migration))
$r = Run @("history", "--migration", $migration, "--verify-chain")
Check "history: the approval's hash chain verifies" ($r.Code -eq 0)

$verifyOut = Join-Path $work "verify.json"
$r = Run @("verify", "--migration", $migration, "--proposal", (Join-Path $pack "proposal.json"), "--source", $src, "--destination", $dst, "--fixtures", $fx, "--out", $verifyOut)
$verify = ReadJson $verifyOut
Check "verify: every catalogued fixture reproduces (stable)" ($r.Code -eq 0 -and $verify.stable -eq $true) "outcomes=$($verify.outcomes.Count)"

$sarifOk = Join-Path $work "ok.sarif"
$r = Run @("report", "--source", $src, "--destination", $dst, "--proposal", (Join-Path $pack "proposal.json"), "--migration", $migration, "--fixtures", $fx, "--verify", $verifyOut, "--html", (Join-Path $work "ok.html"), "--json", (Join-Path $work "ok.json"), "--md", (Join-Path $work "ok.md"), "--job-summary", (Join-Path $work "ok-summary.md"), "--sarif", $sarifOk)
$sarif = ReadJson $sarifOk
Check "report: html/json/md/summary/sarif written, SARIF has no results when stable" ($r.Code -eq 0 -and $sarif.runs[0].results.Count -eq 0)
$reportJson = ReadJson (Join-Path $work "ok.json")
Check "report: absolute input paths are reduced to file names" ($reportJson.diff.source.path -eq "base.openapi.yaml")

# 2. Failure path A: ill-typed proposal (string id renamed into integer count).
$bad = Join-Path $work "ill-typed"; New-Item -ItemType Directory -Force -Path (Join-Path $bad "fixtures") | Out-Null
function Doc([string]$field, [string]$type) {
  return "openapi: 3.0.3`ninfo:`n  title: T`n  version: '1'`npaths:`n  /x:`n    post:`n      operationId: x`n      requestBody:`n        content:`n          application/json:`n            schema:`n              type: object`n              additionalProperties: false`n              required: [$field]`n              properties:`n                ${field}:`n                  type: $type`n      responses:`n        '200':`n          description: ok`n"
}
[System.IO.File]::WriteAllText((Join-Path $bad "base.openapi.yaml"), (Doc "id" "string"))
[System.IO.File]::WriteAllText((Join-Path $bad "next.openapi.yaml"), (Doc "count" "integer"))
$proposal = @'
{"schema_version":"1","kind":"proposal","id":"ill-typed","source_contract":{"path":"base.openapi.yaml"},"destination_contract":{"path":"next.openapi.yaml"},"rules":[{"id":"r1","operation":"rename","source":{"operation":{"method":"POST","path":"/x","operation_id":"x"},"location":"request","field":"id"},"destination":{"operation":{"method":"POST","path":"/x","operation_id":"x"},"location":"request","field":"count"},"parameters":{"from":"id","to":"count"},"preconditions":[],"postconditions":[],"evidence":"smoke","lossiness":"none","reversibility":"reversible","fixture_refs":[]}]}
'@
[System.IO.File]::WriteAllText((Join-Path $bad "proposal.json"), $proposal)
[System.IO.File]::WriteAllText((Join-Path $bad "fixtures/positive.json"), '{"id":"1"}')
[System.IO.File]::WriteAllText((Join-Path $bad "fixtures/catalog.json"), '{"schema_version":"1","items":[{"id":"positive","path":"positive.json","rule_ids":["r1"],"expect":"success","description":"Request with a string id.","version":"1"}]}')
$r = Run @("review", "--proposal", (Join-Path $bad "proposal.json"), "--source", (Join-Path $bad "base.openapi.yaml"), "--destination", (Join-Path $bad "next.openapi.yaml"), "--out", (Join-Path $bad "review.json"))
$badReview = ReadJson (Join-Path $bad "review.json")
Check "review: ill-typed proposal exits 3 and still writes diagnostics" ($r.Code -eq 3 -and $badReview.typecheck -eq "failed" -and $badReview.blocking_diagnostics -ge 1) ("first=" + $badReview.diagnostics[0].message)
$r = Run @("approve", "--proposal", (Join-Path $bad "proposal.json"), "--source", (Join-Path $bad "base.openapi.yaml"), "--destination", (Join-Path $bad "next.openapi.yaml"), "--fixtures", (Join-Path $bad "fixtures"), "--review", (Join-Path $bad "review.json"), "--reviewer", "smoke", "--out", (Join-Path $bad "migration.json"))
Check "approve: refuses ill-typed proposal (exit 3, no migration file)" ($r.Code -eq 3 -and -not (Test-Path (Join-Path $bad "migration.json")))

# 3. Failure path B: catalog expectation that the fixture does not reproduce.
$copy = Join-Path $work "pack-copy"; Copy-Item -Recurse $pack $copy
$catalogPath = Join-Path $copy "fixtures/catalog.json"
$catalog = ReadJson $catalogPath
$catalog.items[0].expect = "collision"
[System.IO.File]::WriteAllText($catalogPath, ($catalog | ConvertTo-Json -Depth 10))
$m2 = Join-Path $copy "migration.json"
$r2 = Join-Path $copy "review.json"
$r = Run @("review", "--proposal", (Join-Path $copy "proposal.json"), "--source", (Join-Path $copy "base.openapi.yaml"), "--destination", (Join-Path $copy "next.openapi.yaml"), "--out", $r2)
$r = Run @("approve", "--review", $r2, "--proposal", (Join-Path $copy "proposal.json"), "--source", (Join-Path $copy "base.openapi.yaml"), "--destination", (Join-Path $copy "next.openapi.yaml"), "--fixtures", (Join-Path $copy "fixtures"), "--reviewer", "smoke", "--approved-at", "2026-09-22T00:00:00Z", "--out", $m2)
$v2 = Join-Path $copy "verify.json"
$r = Run @("verify", "--migration", $m2, "--proposal", (Join-Path $copy "proposal.json"), "--source", (Join-Path $copy "base.openapi.yaml"), "--destination", (Join-Path $copy "next.openapi.yaml"), "--fixtures", (Join-Path $copy "fixtures"), "--out", $v2)
$failing = ReadJson $v2
$failed = @($failing.outcomes | Where-Object { -not $_.passed })
Check "verify: wrong catalog expectation exits 1 and the full report is still written" ($r.Code -eq 1 -and $failing.stable -eq $false -and $failed.Count -eq 1 -and $failing.outcomes.Count -gt 1) ("expect=" + $failed[0].expect + " actual=" + $failed[0].actual)
$r = Run @("report", "--source", (Join-Path $copy "base.openapi.yaml"), "--destination", (Join-Path $copy "next.openapi.yaml"), "--html", (Join-Path $copy "x.html"), "--json", (Join-Path $copy "x.json"), "--sarif", (Join-Path $copy "x.sarif"))
Check "report: --sarif without a verification run (--migration, --proposal, --fixtures) is refused (exit 3)" ($r.Code -eq 3 -and -not (Test-Path (Join-Path $copy "x.sarif")))
$s2 = Join-Path $copy "fail.sarif"
$r = Run @("report", "--source", (Join-Path $copy "base.openapi.yaml"), "--destination", (Join-Path $copy "next.openapi.yaml"), "--proposal", (Join-Path $copy "proposal.json"), "--migration", $m2, "--fixtures", (Join-Path $copy "fixtures"), "--verify", $v2, "--html", (Join-Path $copy "fail.html"), "--json", (Join-Path $copy "fail.json"), "--sarif", $s2)
$sarifFail = ReadJson $s2
$result0 = $sarifFail.runs[0].results[0]
Check "report: failing verify becomes one SARIF result with rule id and fixture location" ($r.Code -eq 0 -and $sarifFail.runs[0].results.Count -eq 1 -and $result0.ruleId -like "cfp.fixture.*" -and $result0.locations[0].physicalLocation.artifactLocation.uri -eq "request.positive.json")

# 4. Provider boundary.
$r = Run @("provider", "--preview", "openai", "--select", (Join-Path $pack "fixtures/request.positive.json"))
$preview = $r.Out | ConvertFrom-Json
$leaks = ($r.Out -match '"user-1"') -or ($r.Out -match '"Ada"')
Check "provider: preview stays blocked and shows only redacted shape" ($r.Code -eq 0 -and $preview.blocked -eq $true -and $preview.would_send.Count -eq 1 -and -not $leaks)

# 5. Generated Node suite drives the binary (skipped when node is absent).
$node = Get-Command node -ErrorAction SilentlyContinue
if ($node) {
  $gen = Join-Path $work "generated"
  $r = Run @("generate", "--kind", "jest", "--pack", $pack, "--out", $gen)
  $env:CFP_PACK = $pack; $env:CFP_BIN = $cfp
  & node --test (Join-Path $gen "pack.test.js") *> (Join-Path $work "node-test.log")
  $nodeCode = $LASTEXITCODE
  Remove-Item Env:CFP_PACK; Remove-Item Env:CFP_BIN
  Check "generate: node --test suite passes against the binary" ($r.Code -eq 0 -and $nodeCode -eq 0) ("log=" + (Join-Path $work "node-test.log"))
} else {
  Check "generate: node --test suite" $true "skipped (node not installed)"
}

Write-Host ""
$results | Format-Table -AutoSize | Out-String | Write-Host
$failures = @($results | Where-Object { $_.Result -eq "FAIL" }).Count
Write-Host ("smoke: {0} checks, {1} failed, artifacts in {2}" -f $results.Count, $failures, $work)
if ($failures -gt 0) { exit 1 }
