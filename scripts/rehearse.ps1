# Local dry run of install, provider-disabled use, Action use, upgrade and
# uninstall. Everything is built and removed inside a sandbox under the
# system temporary folder; nothing is published and no remote workflow is dispatched.
#
#   pwsh -File scripts/rehearse.ps1
#
# install   - a copy of the earlier local assembly (dist/v1/export, when it is
#             present and the host is Windows, where it was built; otherwise
#             this tree's export) is built and its
#             quickstart runs.
# upgrade   - this tree is exported next to it, built, and swapped in place of
#             the installed one; the quickstart runs again from the swapped tree.
# uninstall - the installed tree, the previous tree and their build output are
#             deleted, and their absence is checked.
$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Sandbox = New-CfpTempDir "cfp-dry-run"
$Install = Join-Path $Sandbox "install"
$Next = Join-Path $Sandbox "next"
$Previous = Join-Path $Sandbox "install.previous"

function Build-And-Quickstart([string]$Tree, [string]$Label) {
  $env:CARGO_TARGET_DIR = Join-Path $Tree "target"
  Push-Location $Tree
  try {
    $run = Invoke-CfpScript (Join-CfpPath $Tree @("scripts", "quickstart.ps1"))
    if ($run.Code -ne 0) { Write-Output $run.All; throw "$Label quickstart failed" }
    $cfp = Join-CfpPath $env:CARGO_TARGET_DIR @("release", "cfp$CfpExe")
    $version = (& $cfp --version) | Out-String
    Write-Host ("{0}: quickstart passed with {1}" -f $Label, $version.Trim())
    return $cfp
  } finally {
    Pop-Location
    Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  }
}

# Install.
$earlier = Join-CfpPath $Root @("dist", "v1", "export")
if ($CfpOnWindows -and (Test-Path (Join-Path $earlier "Cargo.toml"))) {
  Copy-Item -Recurse $earlier $Install
  Write-Output "install: copied the earlier local assembly dist/v1/export"
} else {
  $exported = Invoke-CfpScript (Join-CfpPath $Root @("scripts", "export.ps1")) @("-Destination", $Install)
  if ($exported.Code -ne 0) { Write-Output $exported.All; Write-Error "export failed"; exit 1 }
  Write-Output "install: no earlier assembly; installed this tree's export"
}
$cfp = Build-And-Quickstart $Install "install"

# Provider-disabled use: `none` never generates, hosted ids stay blocked.
if ((Invoke-CfpNative $cfp @("propose", "--mode", "none")).Code -eq 0) { Write-Error "propose --mode none must not generate"; exit 1 }
if ((Invoke-CfpNative $cfp @("propose", "--mode", "openai")).Code -ne 2) { Write-Error "a hosted provider must stay unsupported (exit 2)"; exit 1 }
Write-Output "provider-disabled: none refuses to generate; hosted ids exit 2"

# Action use, as a consumer workspace would run it.
$action = Invoke-CfpScript (Join-Path $PSScriptRoot "rehearse-action.ps1")
Write-Output $action.All.TrimEnd()
if ($action.Code -ne 0) { exit $action.Code }

# Upgrade: build the new tree beside the installed one, then swap.
$exported = Invoke-CfpScript (Join-CfpPath $Root @("scripts", "export.ps1")) @("-Destination", $Next)
if ($exported.Code -ne 0) { Write-Error "export of the upgrade failed"; exit 1 }
Rename-Item $Install $Previous
Rename-Item $Next $Install
$cfp = Build-And-Quickstart $Install "upgrade"
if (-not ((& $cfp approve --help) -match "--review")) { Write-Error "the upgraded binary is not the new build"; exit 1 }
Write-Output "upgrade: swapped in the new tree; its approve requires --review"

# Uninstall: delete both trees and their build output, then check.
Remove-Item -Recurse -Force $Install, $Previous
foreach ($path in @($Install, $Previous, (Join-Path $Install "target"), (Join-Path $Previous "target"))) {
  if (Test-Path $path) { Write-Error "uninstall left $path"; exit 1 }
}
Write-Output "uninstall: removed the installed and previous trees and their build output"

if (Test-Path (Join-CfpPath $Root @("dist", "v1", "checksums", "SHA256SUMS.txt"))) {
  $verified = Invoke-CfpScript (Join-Path $PSScriptRoot "verify-local-release.ps1") @("-Tree", (Join-CfpPath $Root @("dist", "v1")))
  Write-Output $verified.All.TrimEnd()
  if ($verified.Code -ne 0) { exit $verified.Code }
}
Write-Output "dry run passed: install, provider-disabled use, Action use (local), upgrade and uninstall in $Sandbox"
exit 0
