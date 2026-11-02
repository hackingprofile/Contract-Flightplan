# Regression check for the export destination guard: export.ps1 must never remove its own source
# root, a folder that contains it, or a protected folder inside it, and an alias (a directory link
# ABOVE the destination, outside the root or nested below dist/) must not address a protected
# folder either. Directory links are junctions on Windows and symbolic links elsewhere.
# Runs export.ps1 from a disposable copy in the system temporary folder (never the real tree):
# every unsafe destination must fail with a sentinel file still present; the default dist/
# destination and an external folder must still export. Exits 0 when every case holds.
# Runs under PowerShell 7 on Windows, Linux and macOS, and Windows PowerShell 5.1.
#   pwsh -NoProfile -File scripts/destination-guard-check.ps1
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
. (Join-Path $here "common.ps1")
$work = New-CfpTempDir "cfp-guard-check"
# The real path, so the temp folder itself is not reached through a link (macOS /var -> /private/var).
if (-not $CfpOnWindows) { $work = (& realpath -- $work).Trim() }
$root = Join-Path $work "root"
New-Item -ItemType Directory -Force -Path (Join-Path $root "scripts") | Out-Null
Copy-Item (Join-Path $here "export.ps1"), (Join-Path $here "destination-guard.ps1"), (Join-Path $here "common.ps1") (Join-Path $root "scripts")
foreach ($f in ".gitignore", ".gitattributes", "rust-toolchain.toml", "Cargo.toml", "Cargo.lock", "LICENSE", "NOTICE", "README.md", "CONTRIBUTING.md", "DCO", "rustfmt.toml", "action.yml") {
  Set-Content -Path (Join-Path $root $f) -Value "x"
}
foreach ($d in "src", "schemas", "packs", "docs", "tests", "fixtures", "web", ".github", "dist") {
  New-Item -ItemType Directory -Force -Path (Join-Path $root $d) | Out-Null
  Set-Content -Path (Join-CfpPath $root @($d, "keep.txt")) -Value "x"
}
$sentinels = @((Join-CfpPath $root @("src", "keep.txt")), (Join-CfpPath $root @("scripts", "export.ps1")), (Join-CfpPath $root @("dist", "keep.txt")))
$export = Join-CfpPath $root @("scripts", "export.ps1")
New-Item -ItemType Directory -Force -Path (Join-CfpPath $root @("dist", "link-parent")) | Out-Null
New-CfpDirLink (Join-CfpPath $root @("dist", "junction")) (Join-Path $root "src")
New-CfpDirLink (Join-CfpPath $root @("dist", "nest")) (Join-Path $root "scripts")
New-CfpDirLink (Join-Path $work "outside-link") $root
$links = @((Join-CfpPath $root @("dist", "junction")), (Join-CfpPath $root @("dist", "nest")), (Join-Path $work "outside-link"))

$fail = 0
function Run($dest) {
  $r = Invoke-CfpScript $export @("-Destination", $dest)
  return @{ code = $r.Code; out = $r.All }
}
$unsafe = [ordered]@{
  "source root"        = $root
  "ancestor"           = $work
  "protected src"      = (Join-Path $root "src")
  "protected scripts"  = (Join-Path $root "scripts")
  "dist itself"        = (Join-Path $root "dist")
  "link into src"      = (Join-CfpPath $root @("dist", "junction"))
  "relative to root"   = (Join-CfpPath $root @("dist", "..", "src"))
  "outside link to src" = (Join-CfpPath $work @("outside-link", "src"))
  "outside link, root" = (Join-Path $work "outside-link")
  "nest below dist"    = (Join-CfpPath $root @("dist", "nest", "deeper"))
}
# a short 8.3 name of the root, when the volume keeps them (Windows only)
if ($CfpOnWindows) {
  $fso = New-Object -ComObject Scripting.FileSystemObject
  $short = $fso.GetFolder($root).ShortPath
  if ($short -and -not $short.Equals($root, [System.StringComparison]::OrdinalIgnoreCase)) {
    $unsafe["short name of src"] = (Join-Path $short "src")
  }
}
foreach ($name in $unsafe.Keys) {
  $r = Run $unsafe[$name]
  $kept = @($sentinels | Where-Object { -not (Test-Path $_) }).Count -eq 0
  $ok = $r.code -ne 0 -and $kept -and $r.out -match "refusing\s+destination"
  "{0,-20} {1}" -f $name, $(if ($ok) { "refused, nothing removed" } else { "FAILED (exit $($r.code), sentinels kept: $kept)" })
  if (-not $ok) { $fail++ }
}
foreach ($case in @(@("default dist/ path", (Join-CfpPath $root @("dist", "contract-flightplan"))), @("external folder", (Join-CfpPath $work @("out", "export"))))) {
  $r = Run $case[1]
  $ok = $r.code -eq 0 -and (Test-Path (Join-CfpPath $case[1] @("src", "keep.txt")))
  "{0,-20} {1}" -f $case[0], $(if ($ok) { "exported" } else { "FAILED (exit $($r.code)) $($r.out)" })
  if (-not $ok) { $fail++ }
}
foreach ($link in $links) { Remove-CfpDirLink $link }
Remove-Item -Recurse -Force $work
if ($fail) { "destination guard: $fail case(s) failed"; exit 1 }
"destination guard: all cases hold"
