# Hand-written mutation check for the mapping interpreter.
#
# Exports a clean copy of the tree to the system temporary folder, checks that its test suite
# passes, then applies one mutation at a time to an `apply_*` function in
# src/mapping.rs (two per operation, 26 in all), runs `cargo test --locked`,
# and records whether a test failed ("killed") or every test still passed
# ("survived"). Exits 1 when the kill rate is below -Threshold percent, and
# exits 1 before building anything when a mutant's anchor text is not found.
# Runs under PowerShell 7 on Windows, Linux and macOS, and Windows PowerShell 5.1.
#
#   pwsh -File scripts/mutation-check.ps1 [-Threshold 85]
param(
  [int]$Threshold = 85
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Work = New-CfpTempDir "cfp-mutation"
$Target = New-CfpTempDir "cfp-mutation-target"

$exported = Invoke-CfpScript (Join-CfpPath $Root @("scripts", "export.ps1")) @("-Destination", $Work)
if ($exported.Code -ne 0) { Write-Output $exported.All; Write-Error "export failed"; exit 1 }
$Mapping = Join-CfpPath $Work @("src", "mapping.rs")
$Original = [System.IO.File]::ReadAllText($Mapping)
$Utf8 = New-Object System.Text.UTF8Encoding $false

# Function, text to find inside that function, replacement.
$Mutants = @(
  @("apply_rename", "if obj.contains_key(to) {", "if false && obj.contains_key(to) {"),
  @("apply_rename", "obj.insert(to.to_string(), value);", "obj.insert(from.to_string(), value);"),
  @("apply_copy", "if obj.contains_key(to) {", "if false && obj.contains_key(to) {"),
  @("apply_copy", "obj.get(from).cloned()", "obj.remove(from)"),
  @("apply_default", "Ok(true)", "Ok(false)"),
  @("apply_default", "Some(_) => Ok(false),", "Some(_) => { obj.insert(field.to_string(), value.clone()); Ok(true) }"),
  @("apply_enum_map", "if current.is_null() {", "if false {"),
  @("apply_enum_map", "let many_to_one = map.iter().any(|(k, v)| k != &key && v == mapped);", "let many_to_one = false;"),
  @("apply_drop", "if obj.remove(field).is_none() {", "if obj.get(field).is_none() {"),
  @("apply_drop", "if obj.remove(field).is_none() {", "if obj.remove(field).is_none() && false {"),
  @("apply_required", "Some(Value::Null) => Err(Error::ExplicitNull {", "Some(Value::Null) => Err(Error::MissingField {"),
  @("apply_required", "None => Err(Error::MissingField {", "None => Err(Error::ExplicitNull {"),
  @("apply_safe_cast", "if current.is_null() {", "if false {"),
  @("apply_safe_cast", "n.fract() != 0.0 ||", "false && n.fract() != 0.0 ||"),
  @("apply_unit", "let inexact = p % d != 0;", "let inexact = false;"),
  @("apply_unit", "p.rem_euclid(d) * 2 >= d", "p.rem_euclid(d) * 2 > d"),
  @("apply_split", "if parts.len() != into.len() {", "if false {"),
  @("apply_split", "obj.remove(field);", ""),
  @("apply_merge", "if obj.contains_key(into) {", "if false {"),
  @("apply_merge", "Value::String(parts.join(separator))", "Value::String(parts.concat())"),
  @("apply_array_map", "lossy = true;", "lossy = false;"),
  @("apply_array_map", "if items.len() > MAX_ARRAY {", "if items.len() > MAX_ARRAY + 1 {"),
  @("apply_array_filter", ".filter(|item| !item.is_null())", ".filter(|item| item.is_null() || true)"),
  @("apply_array_filter", "if items.len() > MAX_ARRAY {", "if items.len() > MAX_ARRAY + 1 {"),
  @("apply_pagination", "apply_rename(obj, from_token, to_cursor, location)", "Ok(())"),
  @("apply_pagination", "apply_rename(obj, from_items, to_data, location)?;", "apply_copy(obj, from_items, to_data, location)?;")
)

function Mutate([string]$Text, [string]$Function, [string]$Find, [string]$Replace) {
  $start = $Text.IndexOf("fn $Function(")
  if ($start -lt 0) { throw "no function $Function" }
  $end = $Text.IndexOf("`nfn ", $start + 1)
  if ($end -lt 0) { $end = $Text.Length }
  $at = $Text.IndexOf($Find, $start)
  if ($at -lt 0 -or $at -ge $end) { throw "$Function does not contain: $Find" }
  return $Text.Substring(0, $at) + $Replace + $Text.Substring($at + $Find.Length)
}

# Run the suite with stdout and stderr captured, not raised. A run
# that failed only because another process briefly held a freshly written
# temp file (Windows sharing violation, os error 32) is repeated, so a host
# flake never counts as a killed mutant.
function Run-Tests {
  Push-Location $Work
  try {
    $env:CARGO_TARGET_DIR = $Target
    for ($attempt = 1; $attempt -le 3; $attempt++) {
      $run = Invoke-CfpNative "cargo" @("test", "--locked", "-q") $Work
      $output = $run.All
      $code = $run.Code
      if ($code -eq 0 -or $output -notmatch 'os error 32|being used by another process') { break }
      Write-Host "  (sharing violation on attempt $attempt; repeating the run)"
    }
    return [pscustomobject]@{ Code = $code; Output = $output }
  } finally {
    Pop-Location
    Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
  }
}

Write-Output "work: $Work"
# Every anchor must match before anything is built, so the score always covers all mutants.
foreach ($m in $Mutants) {
  try { [void](Mutate $Original $m[0] $m[1] $m[2]) }
  catch { Write-Output "mutant anchor does not match src/mapping.rs: $($_.Exception.Message)"; exit 1 }
}
Write-Output ("anchors: all {0} mutants apply" -f $Mutants.Count)
$baseline = Run-Tests
if ($baseline.Code -ne 0) {
  Write-Output $baseline.Output
  Write-Error "the unmutated copy does not pass its tests"
  exit 1
}
Write-Output "baseline: tests pass"

# The tests that failed in a quiet run: the names listed under "failures:",
# or the first compile error.
function Failing-Tests([string]$Output) {
  $names = New-Object System.Collections.ArrayList
  $inList = $false
  foreach ($line in ($Output -split "`r?`n")) {
    if ($line -eq "failures:") { $inList = $true; continue }
    if ($inList -and $line -match '^    (\S+)$') { [void]$names.Add($Matches[1]); continue }
    if ($inList -and $line.Trim() -eq "") { continue }
    if ($inList) { $inList = $false }
  }
  if ($names.Count -eq 0) {
    $err = ($Output -split "`r?`n" | Where-Object { $_ -match '^error(\[|:)' } | Select-Object -First 1)
    if ($err) { [void]$names.Add($err.Trim()) }
  }
  return (@($names | Select-Object -Unique) -join ", ")
}

$killed = 0
$rows = New-Object System.Collections.ArrayList
$n = 0
foreach ($m in $Mutants) {
  $n++
  [System.IO.File]::WriteAllText($Mapping, (Mutate $Original $m[0] $m[1] $m[2]), $Utf8)
  # A mutant is killed only when the suite fails twice; when two runs
  # disagree a third decides, so one flaky failure never counts as a kill.
  $runs = @(Run-Tests)
  if ($runs[0].Code -ne 0) {
    $runs += (Run-Tests)
    if ($runs[1].Code -eq 0) { $runs += (Run-Tests) }
  }
  $failures = @($runs | Where-Object { $_.Code -ne 0 })
  if ($failures.Count -ge 2) {
    $killed++
    $by = Failing-Tests $failures[-1].Output
    if (-not $by) { $by = "non-zero exit" }
    $status = "killed"
  } else {
    $by = $(if ($failures.Count -eq 1) { "one unrepeated failure: " + (Failing-Tests $failures[0].Output) } else { "" })
    $status = "SURVIVED"
  }
  [void]$rows.Add([pscustomobject]@{ N = $n; Function = $m[0]; Status = $status; By = $by.Trim(); Mutation = "$($m[1]) -> $($m[2])" })
  Write-Output ("[{0}] {1,2} {2}: {3} {4}" -f $status, $n, $m[0], $m[1], $(if ($by) { "| " + $by.Trim() } else { "" }))
}
[System.IO.File]::WriteAllText($Mapping, $Original, $Utf8)

$score = [math]::Round(100 * $killed / $Mutants.Count, 1)
Write-Output ("killed {0} of {1} mutants ({2}%); threshold {3}%" -f $killed, $Mutants.Count, $score, $Threshold)
if ($score -lt $Threshold) { exit 1 }
exit 0
