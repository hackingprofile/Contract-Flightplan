# Generate the Node and Python suites for one pack and run them.
#
#   pwsh -File scripts/run-generated-tests.ps1 [-Pack packs/rename-user-id] [-Cfp <path to cfp>] [-RequirePython]
#
# Node runs with `node --test`. Python runs with `python3 -m unittest` (or `python`) when a
# working interpreter is on PATH; otherwise the Python suite is generated but reported as
# UNEXECUTED, and the script exits 0 unless -RequirePython is set (then it exits 2). Exits 1 if
# a suite that ran failed. Runs under PowerShell 7 on Windows, Linux and macOS, and under
# Windows PowerShell 5.1.
param(
  [string]$Pack = "",
  [string]$Cfp = "",
  [switch]$RequirePython
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not $Pack) { $Pack = Join-CfpPath $Root @("packs", "rename-user-id") }
$Pack = (Resolve-Path $Pack).Path
if (-not $Cfp) {
  $built = Invoke-CfpNative "cargo" @("build", "--release", "--locked", "--quiet") $Root
  if ($built.Code -ne 0) { Write-Output $built.All; Write-Error "cargo build failed"; exit 1 }
  $Cfp = Get-CfpBinary $Root
}
$Cfp = (Resolve-Path $Cfp).Path

$Work = New-CfpTempDir "cfp-generated-run"
$env:CFP_PACK = $Pack
$env:CFP_BIN = $Cfp
$failed = $false

& $Cfp generate --kind jest --pack $Pack --out (Join-Path $Work "node") | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error "generate jest failed"; exit 1 }
& $Cfp generate --kind pytest --pack $Pack --out (Join-Path $Work "python") | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Error "generate pytest failed"; exit 1 }

$nodeVersion = Invoke-CfpNative "node" @("--version")
Write-Output ("node: {0}" -f $nodeVersion.All.Trim())
$node = Invoke-CfpNative "node" @("--test", "pack.test.js") (Join-Path $Work "node")
Write-Output $node.All
if ($node.Code -eq 0) { Write-Output "node suite: PASS" } else { Write-Output "node suite: FAIL"; $failed = $true }

# The first interpreter on PATH that answers `--version` with "Python 3.x".
$python = $null
$pythonVersion = "no python3 or python on PATH"
foreach ($candidate in @("python3", "python")) {
  if (-not (Get-Command $candidate -CommandType Application -ErrorAction SilentlyContinue)) { continue }
  try { $v = Invoke-CfpNative $candidate @("--version") } catch { $pythonVersion = $_.Exception.Message; continue }
  if ($v.Code -eq 0 -and $v.All.Trim() -match '^Python 3\.') { $python = $candidate; $pythonVersion = $v.All.Trim(); break }
  $pythonVersion = $v.All.Trim()
}
if ($python) {
  Write-Output ("python: {0}" -f $pythonVersion)
  $py = Invoke-CfpNative $python @("-m", "unittest", "-v", "test_pack") (Join-Path $Work "python")
  Write-Output $py.All
  if ($py.Code -eq 0) { Write-Output "python suite: PASS" } else { Write-Output "python suite: FAIL"; $failed = $true }
} else {
  Write-Output ("python: unavailable ({0})" -f $pythonVersion)
  Write-Output "python suite: UNEXECUTED (generated at $(Join-CfpPath $Work @('python', 'test_pack.py')); no working Python 3 on PATH)"
}

if ($failed) { exit 1 }
if (-not $python -and $RequirePython) { exit 2 }
exit 0
