# Local dry run of action.yml as a consumer would use it: the action is a
# separate checkout (its "action_path"), the job runs in the consumer's
# workspace, and the inputs are paths relative to that workspace. Everything
# happens under the system temporary folder; no hosted runner, nothing is published.
#
#   pwsh -File scripts/rehearse-action.ps1
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Base = New-CfpTempDir "cfp-action-dry-run"
$ActionPath = Join-Path $Base "action"
$Workspace = Join-Path $Base "consumer"

$exported = Invoke-CfpScript (Join-CfpPath $Root @("scripts", "export.ps1")) @("-Destination", $ActionPath)
if ($exported.Code -ne 0) { Write-Output $exported.All; Write-Error "export of the action tree failed"; exit 1 }

# The consumer repository: its own contracts, nothing of the action's.
New-Item -ItemType Directory -Force -Path (Join-Path $Workspace "api") | Out-Null
Copy-Item (Join-CfpPath $Root @("packs", "rename-user-id", "base.openapi.yaml")) (Join-CfpPath $Workspace @("api", "base.openapi.yaml"))
Copy-Item (Join-CfpPath $Root @("packs", "rename-user-id", "next.openapi.yaml")) (Join-CfpPath $Workspace @("api", "next.openapi.yaml"))

Push-Location $Workspace
try {
 # Twice, as action.yml resolves its target directory: the default (action checkout's target/)
 # and a caller's own CARGO_TARGET_DIR, which the build honours and the run must use too.
 foreach ($custom in @($null, (Join-Path $Base "caller-target"))) {
  # Step 1 of action.yml: build from the action checkout into one explicit target directory.
  if ($custom) { $env:CARGO_TARGET_DIR = $custom } else { Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue }
  $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $ActionPath "target" }
  $build = Invoke-CfpNative "cargo" @("build", "--release", "--locked", "--quiet", "--manifest-path", (Join-Path $ActionPath "Cargo.toml"), "--target-dir", $target)
  if ($build.Code -ne 0) { Write-Output $build.All; Write-Error "build from action_path failed"; exit 1 }
  $bin = Join-CfpPath $target @("release", "cfp$CfpExe")
  $defaultBin = Join-CfpPath $ActionPath @("target", "release", "cfp$CfpExe")
  if (-not (Test-Path $bin)) { Write-Error "cfp was not built in the resolved target directory $target"; exit 1 }
  if (Test-Path (Join-Path $Workspace "target")) { Write-Error "the build wrote into the consumer workspace"; exit 1 }
  if ($custom -and (Test-Path $defaultBin) -and ((Get-Item $bin).LastWriteTime -lt (Get-Item $defaultBin).LastWriteTime)) {
    Write-Error "the run would select a stale default-path binary"; exit 1
  }

  # Step 2: compare and report with workspace-relative inputs, with the binary step 1 reported.
  $out = if ($custom) { "contract-flightplan-out-custom-target" } else { "contract-flightplan-out" }
  New-Item -ItemType Directory -Force -Path $out | Out-Null
  & $bin compare --source "api/base.openapi.yaml" --destination "api/next.openapi.yaml" --out "$out/diff.json"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  & $bin report --source "api/base.openapi.yaml" --destination "api/next.openapi.yaml" `
    --html "$out/report.html" --json "$out/report.json" --md "$out/report.md" --job-summary "$out/summary.md"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  foreach ($file in @("diff.json", "report.html", "report.json", "report.md", "summary.md")) {
    if (-not (Test-Path (Join-Path $out $file))) { Write-Error "review pack lacks $file"; exit 1 }
  }
  $summary = Get-Content -Raw (Join-Path $out "summary.md")
  if ($summary -notmatch "## Contract Flightplan") { Write-Error "job summary is not the cfp summary"; exit 1 }
  $report = Get-Content -Raw (Join-Path $out "report.json") | ConvertFrom-Json
  if ($report.diff.source.path -ne "api/base.openapi.yaml") { Write-Error ("recorded path is {0}" -f $report.diff.source.path); exit 1 }
  Write-Output "action dry run ($(if ($custom) { 'caller CARGO_TARGET_DIR' } else { 'default target' })): built in $target, ran that binary in the consumer workspace, wrote the review pack to $(Join-Path $Workspace $out) (no remote workflow)"
 }
} finally {
  Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  Pop-Location
}
exit 0
