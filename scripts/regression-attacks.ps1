# Regression attacks against a built cfp binary.
#
# Each check reproduces one known attack or false-verdict path on copies of the
# shipped packs under the system temporary folder and prints PASS when the binary resists it, FAIL
# when the attack still works. The script exits 1 if any check fails. Runs under
# PowerShell 7 on Windows, Linux and macOS, and Windows PowerShell 5.1. The
# directory-link checks use a junction on Windows and a symbolic link elsewhere.
#
#   pwsh -File scripts/regression-attacks.ps1 -Cfp target/release/cfp
param(
  [Parameter(Mandatory = $true)][string]$Cfp
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Cfp = (Resolve-Path $Cfp).Path
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Packs = Join-Path $Root "packs"
$Negatives = Join-CfpPath $Root @("fixtures", "negatives")
$Work = New-CfpTempDir "cfp-attacks"
$Utf8 = New-Object System.Text.UTF8Encoding $false
$Junctions = New-Object System.Collections.ArrayList
$Results = New-Object System.Collections.ArrayList

function Check([string]$Name, [bool]$Ok, [string]$Detail = "") {
  $status = if ($Ok) { "PASS" } else { "FAIL" }
  [void]$Results.Add([pscustomobject]@{ Check = $Name; Result = $status })
  if ($Detail) { Write-Output ("[{0}] {1} ({2})" -f $status, $Name, $Detail) }
  else { Write-Output ("[{0}] {1}" -f $status, $Name) }
}

# Quote one argument for CreateProcess the way the MSVC runtime parses it.
function Quote-Arg([string]$Value) {
  if ($Value -ne "" -and $Value -notmatch '[\s"]') { return $Value }
  $sb = New-Object System.Text.StringBuilder
  [void]$sb.Append('"')
  $slashes = 0
  foreach ($ch in $Value.ToCharArray()) {
    if ($ch -eq '\') { $slashes++; continue }
    if ($ch -eq '"') {
      [void]$sb.Append('\' * (2 * $slashes + 1)); [void]$sb.Append('"'); $slashes = 0; continue
    }
    if ($slashes -gt 0) { [void]$sb.Append('\' * $slashes); $slashes = 0 }
    [void]$sb.Append($ch)
  }
  [void]$sb.Append('\' * (2 * $slashes))
  [void]$sb.Append('"')
  return $sb.ToString()
}

function Start-Cfp([string[]]$Arguments) {
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $Cfp
  $psi.Arguments = (($Arguments | ForEach-Object { Quote-Arg $_ }) -join " ")
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.StandardOutputEncoding = $Utf8
  $psi.StandardErrorEncoding = $Utf8
  $psi.CreateNoWindow = $true
  $p = [System.Diagnostics.Process]::Start($psi)
  return [pscustomobject]@{ Process = $p; Out = $p.StandardOutput.ReadToEndAsync(); Err = $p.StandardError.ReadToEndAsync() }
}

function Invoke-Cfp([string[]]$Arguments) {
  $s = Start-Cfp $Arguments
  $s.Process.WaitForExit()
  return [pscustomobject]@{ Code = $s.Process.ExitCode; Out = $s.Out.Result; Err = $s.Err.Result }
}

# Run cfp, killing it after $Seconds or once it holds more than $MaxBytes of
# memory. Code is $null and Killed says why when it was killed.
function Invoke-CfpLimited([string[]]$Arguments, [int]$Seconds, [long]$MaxBytes = 1GB) {
  $s = Start-Cfp $Arguments
  $p = $s.Process
  $clock = [System.Diagnostics.Stopwatch]::StartNew()
  $killed = ""
  while (-not $p.WaitForExit(100)) {
    $p.Refresh()
    if ($clock.Elapsed.TotalSeconds -gt $Seconds) { $killed = "time" }
    elseif ($p.WorkingSet64 -gt $MaxBytes) { $killed = "memory" }
    if ($killed) {
      try { $p.Kill() } catch { }
      $p.WaitForExit()
      break
    }
  }
  $code = if ($killed) { $null } else { $p.ExitCode }
  return [pscustomobject]@{ Code = $code; Killed = $killed; Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 1); Out = $s.Out.Result; Err = $s.Err.Result }
}

function Limited-Detail($r) {
  if ($r.Killed) { return ("killed for {0} after {1} s" -f $r.Killed, $r.Seconds) }
  return ("exit={0} after {1} s" -f $r.Code, $r.Seconds)
}

# A JSON contract whose POST /u body is S0; S0..S(levels-1) each have $Width
# properties that $ref the next component, and the last one is a string.
function Fanout-Contract([int]$Width, [int]$Levels, [string[]]$Names = @()) {
  $components = @()
  for ($level = 0; $level -lt $Levels; $level++) {
    $props = @()
    for ($i = 0; $i -lt $Width; $i++) {
      $name = if ($Names.Count -gt $i) { $Names[$i] } else { "p$i" }
      $props += ('"{0}":{{"$ref":"#/components/schemas/S{1}"}}' -f $name, ($level + 1))
    }
    $components += ('"S{0}":{{"type":"object","properties":{{{1}}}}}' -f $level, ($props -join ","))
  }
  $components += ('"S{0}":{{"type":"string"}}' -f $Levels)
  return ('{{"openapi":"3.0.3","info":{{"title":"t","version":"1"}},"paths":{{"/u":{{"post":{{"requestBody":{{"content":{{"application/json":{{"schema":{{"$ref":"#/components/schemas/S0"}}}}}}}},"responses":{{"200":{{"description":"ok"}}}}}}}}}},"components":{{"schemas":{{{0}}}}}}}' -f ($components -join ","))
}

function Write-Utf8([string]$Path, [string]$Text) { [System.IO.File]::WriteAllText($Path, $Text, $Utf8) }
function Read-Text([string]$Path) { if (Test-Path $Path) { [System.IO.File]::ReadAllText($Path) } else { "" } }
function Read-Json([string]$Path) {
  if (-not (Test-Path $Path)) { return $null }
  try { return (Get-Content -Raw -Encoding UTF8 $Path | ConvertFrom-Json) } catch { return $null }
}
function Sha256([string]$Path) { "sha256:" + (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLowerInvariant() }

function Copy-Pack([string]$Name, [string]$Label) {
  $dest = Join-Path $Work $Label
  Copy-Item -Recurse (Join-Path $Packs $Name) $dest
  return $dest
}

$ReviewRecords = (Invoke-Cfp @("approve", "--help")).Out -match "--review(?![a-z-])"
# Binaries that check the reviewed proposal at verify also commit the approved
# event to every bound digest.
$ProposalAtVerify = (Invoke-Cfp @("verify", "--help")).Out -match "--proposal"

# Approve a pack directory. When the binary requires a review record, write one
# with `cfp review --out` first, exactly as a reviewer would.
function Approve-Pack([string]$Dir, [string]$Out, [string]$Reviewer = "attack-reviewer", [string]$Note = "") {
  $common = @("--proposal", (Join-Path $Dir "proposal.json"), "--source", (Join-Path $Dir "base.openapi.yaml"),
    "--destination", (Join-Path $Dir "next.openapi.yaml"))
  $arguments = @("approve") + $common + @("--fixtures", (Join-Path $Dir "fixtures"), "--reviewer", $Reviewer,
    "--approved-at", "2026-09-30T00:00:00Z", "--out", $Out)
  if ($Note) { $arguments += @("--note", $Note) }
  if ($ReviewRecords) {
    $record = Join-Path $Dir "review.record.json"
    $review = Invoke-Cfp (@("review") + $common + @("--out", $record))
    if ($review.Code -ne 0) { return $review }
    $arguments += @("--review", $record)
  }
  return Invoke-Cfp $arguments
}

function Verify-Pack([string]$Dir, [string]$Migration, [string]$Out) {
  $proposal = @()
  if ($ProposalAtVerify) { $proposal = @("--proposal", (Join-Path $Dir "proposal.json")) }
  return Invoke-Cfp (@("verify", "--migration", $Migration) + $proposal + @("--source", (Join-Path $Dir "base.openapi.yaml"),
    "--destination", (Join-Path $Dir "next.openapi.yaml"), "--fixtures", (Join-Path $Dir "fixtures"), "--out", $Out))
}

# Binaries whose report re-runs verification itself take --fixtures (and need
# --proposal with --migration); the verdict then never comes from a file.
$ReportVerifies = (Invoke-Cfp @("report", "--help")).Out -match "--fixtures"

# The arguments that give a report its verdict for pack $Dir and migration $Migration.
function Verdict-Args([string]$Dir, [string]$Migration) {
  $arguments = @("--migration", $Migration)
  if ($ReportVerifies) { $arguments += @("--proposal", (Join-Path $Dir "proposal.json"), "--fixtures", (Join-Path $Dir "fixtures")) }
  return $arguments
}

# Prepend one catalog item that points at $ItemPath. The item is a copy of the
# first existing item, so it carries whatever metadata the catalog requires.
function Add-CatalogItem([string]$Dir, [string]$ItemPath) {
  $catalogPath = Join-CfpPath $Dir @("fixtures", "catalog.json")
  $doc = Read-Json $catalogPath
  $item = $doc.items[0].PSObject.Copy()
  $item.id = "attack-item"
  $item.path = $ItemPath
  $item.expect = "success"
  $doc.items = @($item) + @($doc.items)
  Write-Utf8 $catalogPath ($doc | ConvertTo-Json -Depth 20)
}

# serde_json's compact form with object keys in byte order: the canonical JSON
# the approved event's digest is computed over (strings and objects only).
function Canonical-Json($Node) {
  if ($Node -is [string]) {
    $sb = New-Object System.Text.StringBuilder
    [void]$sb.Append('"')
    foreach ($ch in $Node.ToCharArray()) {
      $code = [int]$ch
      if ($ch -eq [char]'"') { [void]$sb.Append('\"') }
      elseif ($ch -eq [char]'\') { [void]$sb.Append('\\') }
      elseif ($code -eq 10) { [void]$sb.Append('\n') }
      elseif ($code -eq 13) { [void]$sb.Append('\r') }
      elseif ($code -eq 9) { [void]$sb.Append('\t') }
      elseif ($code -eq 8) { [void]$sb.Append('\b') }
      elseif ($code -eq 12) { [void]$sb.Append('\f') }
      elseif ($code -lt 0x20) { [void]$sb.Append(('\u{0:x4}' -f $code)) }
      else { [void]$sb.Append($ch) }
    }
    [void]$sb.Append('"')
    return $sb.ToString()
  }
  if ($Node -is [System.Management.Automation.PSCustomObject]) {
    $names = [string[]]@($Node.PSObject.Properties | ForEach-Object { $_.Name })
    [Array]::Sort($names, [StringComparer]::Ordinal)
    $parts = @(foreach ($name in $names) { (Canonical-Json $name) + ":" + (Canonical-Json $Node.$name) })
    return "{" + ($parts -join ",") + "}"
  }
  throw "Canonical-Json handles strings and objects only"
}

function Sha256-Text([string]$Text) {
  $sha = [System.Security.Cryptography.SHA256]::Create()
  $hash = $sha.ComputeHash($Utf8.GetBytes($Text))
  return "sha256:" + (($hash | ForEach-Object { $_.ToString("x2") }) -join "")
}

# Add one catalog item and re-bind the approval to the edited catalog by digest,
# recomputing the approved event's digest too when the binary commits it to the
# bound digests: everything anyone holding the migration file can do (digests are
# not signatures). Only the fixture-folder checks at verify stand in the way.
function Add-CatalogItem-And-Rebind([string]$Dir, [string]$Migration, [string]$ItemPath) {
  $catalogPath = Join-CfpPath $Dir @("fixtures", "catalog.json")
  $before = Sha256 $catalogPath
  $committed = Sha256-Text (Canonical-Json (Read-Json $Migration).approval.bound_digests)
  Add-CatalogItem $Dir $ItemPath
  $after = Sha256 $catalogPath
  Write-Utf8 $Migration ((Read-Text $Migration).Replace($before, $after))
  if ($ProposalAtVerify) {
    $approved = @((Read-Json $Migration).history | Where-Object { $_.action -eq "approved" })[-1].digest
    if ($approved -ne $committed) { throw "the approval digest computed here ($committed) is not the approved event's ($approved)" }
    $recomputed = Sha256-Text (Canonical-Json (Read-Json $Migration).approval.bound_digests)
    Write-Utf8 $Migration ((Read-Text $Migration).Replace($approved, $recomputed))
  }
}

function Json-Objects($Node, [scriptblock]$Predicate, [System.Collections.ArrayList]$Found) {
  if ($null -eq $Node) { return }
  if ($Node -is [System.Array]) {
    foreach ($child in $Node) { Json-Objects $child $Predicate $Found }
    return
  }
  if ($Node -is [System.Management.Automation.PSCustomObject]) {
    if (& $Predicate $Node) { [void]$Found.Add($Node) }
    foreach ($prop in $Node.PSObject.Properties) { Json-Objects $prop.Value $Predicate $Found }
  }
}

function Contract([string]$Path, [string]$Schema, [string]$Components = "") {
  $text = "openapi: 3.0.3`ninfo:`n  title: Attack`n  version: '1'`npaths:`n" + $Path +
    "`n    post:`n      operationId: createUser`n      requestBody:`n        content:`n          application/json:`n            schema:`n" +
    $Schema + "`n      responses:`n        '200':`n          description: ok`n"
  if ($Components) { $text += "components:`n  schemas:`n" + $Components + "`n" }
  return $text
}

Write-Output "cfp: $Cfp"
Write-Output "work: $Work"
Write-Output ("review records required by approve: {0}" -f $ReviewRecords)

# --- Nested property changes are classified, not reported as "no change" -----
$d = Join-Path $Work "nested"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$nested = "              type: object`n              properties:`n                address:`n                  type: object`n                  properties:`n                    zip:`n                      type: {0}"
Write-Utf8 (Join-Path $d "base.yaml") (Contract "  /users:" ($nested -f "string"))
Write-Utf8 (Join-Path $d "next.yaml") (Contract "  /users:" ($nested -f "integer"))
$r = Invoke-Cfp @("compare", "--source", (Join-Path $d "base.yaml"), "--destination", (Join-Path $d "next.yaml"))
$found = New-Object System.Collections.ArrayList
if ($r.Code -eq 0) {
  Json-Objects ($r.Out | ConvertFrom-Json) { param($o) $o.pointer -is [string] -and $o.pointer.EndsWith("/zip") -and $o.compatibility -eq "breaking" } $found
}
Check "compare reports a nested property type change as breaking with its pointer" ($found.Count -ge 1) ("exit={0}" -f $r.Code)

# --- Fixture catalog paths stay inside the fixtures folder -------------------
$d = Copy-Pack "rename-user-id" "traversal-approve"
Write-Utf8 (Join-Path $Work "outside-secret.json") '{"id": "outside-secret-value", "name": "x"}'
Add-CatalogItem $d "../../outside-secret.json"
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "approve refuses a catalog path that leaves the fixtures folder" ($r.Code -ne 0 -and -not (Test-Path $m)) ("exit={0}" -f $r.Code)

$d = Copy-Pack "rename-user-id" "traversal-verify"
Write-Utf8 (Join-Path $Work "outside-secret.json") '{"id": "outside-secret-value", "name": "x"}'
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
$v = Join-Path $d "verify.json"
if ($a.Code -eq 0) {
  Add-CatalogItem-And-Rebind $d $m "../../outside-secret.json"
  $r = Verify-Pack $d $m $v
  $leak = ((Read-Text $v) + $r.Out).Contains("outside-secret-value")
  Check "verify refuses a catalog path that leaves the fixtures folder and leaks nothing" ($r.Code -ne 0 -and -not $leak) ("exit={0} leak={1}" -f $r.Code, $leak)
  Write-Utf8 (Join-Path $Work "outside-secret.json") '{"id": "changed-after-approval", "name": "x"}'
  $r = Verify-Pack $d $m (Join-Path $d "verify2.json")
  Check "a file changed outside the fixtures folder after approval cannot pass verify" ($r.Code -ne 0) ("exit={0}" -f $r.Code)
} else {
  Check "verify refuses a catalog path that leaves the fixtures folder and leaks nothing" $false ("setup approve failed: exit={0} {1}" -f $a.Code, $a.Err.Trim())
  Check "a file changed outside the fixtures folder after approval cannot pass verify" $false "setup approve failed"
}

# --- Directory links (junctions on Windows, symlinks elsewhere) in fixtures ---
# Each check first asserts the link really exists (a check that ran without a
# link would pass vacuously) and that the refusal names a link.
function Test-DirLink([string]$Path) {
  $item = Get-Item -LiteralPath $Path -Force -ErrorAction SilentlyContinue
  return ($null -ne $item -and @("Junction", "SymbolicLink") -contains [string]$item.LinkType)
}
$target = Join-Path $Work "junction-target"; New-Item -ItemType Directory -Force -Path $target | Out-Null
Write-Utf8 (Join-Path $target "j.json") '{"id": "through-junction", "name": "x"}'
$d = Copy-Pack "rename-user-id" "junction-approve"
$link = Join-CfpPath $d @("fixtures", "linked")
New-CfpDirLink $link $target
[void]$Junctions.Add($link)
$linkMade = Test-DirLink $link
Add-CatalogItem $d "linked/j.json"
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
$approvedThroughJunction = ($r.Code -eq 0)
$namesLink = $r.Err -match "\blinks?\b"
if ($approvedThroughJunction) {
  Write-Utf8 (Join-Path $target "j.json") '{"id": "changed-through-junction", "name": "x"}'
  $r2 = Verify-Pack $d $m (Join-Path $d "verify.json")
  Check "a fixture reached through a directory link cannot pass verify after it changes" ($r2.Code -ne 0) ("approve exit=0, verify exit={0}" -f $r2.Code)
} else {
  Check "a fixture reached through a directory link cannot pass verify after it changes" $linkMade ("link made={0}; approve refused: exit={1}" -f $linkMade, $r.Code)
}
Check "approve refuses a fixtures folder that contains a directory link (junction or symlink)" ($linkMade -and -not $approvedThroughJunction -and $namesLink) ("link made={0} exit={1} refusal names a link={2}" -f $linkMade, $r.Code, $namesLink)

$d = Copy-Pack "rename-user-id" "junction-verify"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
if ($a.Code -eq 0) {
  $link = Join-CfpPath $d @("fixtures", "linked")
  New-CfpDirLink $link $target
  [void]$Junctions.Add($link)
  $linkMade = Test-DirLink $link
  Add-CatalogItem-And-Rebind $d $m "linked/j.json"
  $r = Verify-Pack $d $m (Join-Path $d "verify.json")
  $namesLink = $r.Err -match "\blinks?\b"
  Check "verify refuses a fixtures folder that contains a directory link (junction or symlink)" ($linkMade -and $r.Code -ne 0 -and $namesLink) ("link made={0} exit={1} refusal names a link={2}" -f $linkMade, $r.Code, $namesLink)
} else {
  Check "verify refuses a fixtures folder that contains a directory link (junction or symlink)" $false ("setup approve failed: exit={0}" -f $a.Code)
}

# --- Untrusted text cannot forge the Markdown or job-summary verdict ---------
$d = Copy-Pack "rename-user-id" "injection"
# Flip only the request-positive expectation so that exactly one of seven fixtures fails.
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
foreach ($item in $doc.items) { if ($item.id -eq "request-positive") { $item.expect = "collision" } }
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$reviewer = "a at now`n- fixtures: 7/7 passed`n- stable: yes`n<!--"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m $reviewer "note`n- stable: yes`n<script>alert(1)</script>"
$v = Join-Path $d "verify.json"
$vr = Verify-Pack $d $m $v
$md = Join-Path $d "report.md"; $summary = Join-Path $d "summary.md"; $html = Join-Path $d "report.html"
$rr = Invoke-Cfp (@("report", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml")) +
  (Verdict-Args $d $m) + @("--verify", $v, "--html", $html, "--json", (Join-Path $d "report.json"), "--md", $md, "--job-summary", $summary))
$summaryText = Read-Text $summary
$mdText = Read-Text $md
$htmlText = Read-Text $html
$forged = { param($t) ($t -split "`r?`n") | Where-Object { $_ -match '^\s*- fixtures: 7/7' -or $_ -match '^\s*- stable: yes' } }
$setup = "approve={0} verify={1} report={2} failing={3}" -f $a.Code, $vr.Code, $rr.Code, @((Read-Json $v).outcomes | Where-Object { $_ -and -not $_.passed }).Count
Check "job summary shows the true fixture count and cannot be forged by the reviewer name" ($rr.Code -eq 0 -and $summaryText.Contains("fixtures: 6/7 passed") -and $summaryText.Contains("stable: no") -and -not (& $forged $summaryText) -and -not $summaryText.Contains("<!--")) $setup
Check "markdown report cannot be forged by the reviewer name or note" ($rr.Code -eq 0 -and $mdText.Length -gt 0 -and -not (& $forged $mdText) -and -not $mdText.Contains("<!--") -and -not $mdText.Contains("<script")) $setup
$verdictAt = $htmlText.IndexOf("6 of 7")
$approvalAt = $htmlText.IndexOf("Approval state")
Check "html report opens with the verdict before any approval text" ($rr.Code -eq 0 -and $verdictAt -ge 0 -and $approvalAt -gt $verdictAt -and $htmlText.Contains("not stable") -and $htmlText.Contains("request-positive")) ("verdict at {0}, approval at {1}" -f $verdictAt, $approvalAt)
$mdVerdictAt = $mdText.IndexOf("6 of 7")
$mdApprovalAt = $mdText.IndexOf("Approval state")
Check "markdown report opens with the verdict before any approval text" ($rr.Code -eq 0 -and $mdVerdictAt -ge 0 -and $mdApprovalAt -gt $mdVerdictAt) ("verdict at {0}, approval at {1}" -f $mdVerdictAt, $mdApprovalAt)

# --- Duplicate keys and unknown fields in inputs that carry approval ----------
$d = Copy-Pack "rename-user-id" "duplicate-proposal"
$p = Join-Path $d "proposal.json"
Write-Utf8 $p ((Read-Text $p).Replace('"id": "rename-user-id",', '"id": "rename-user-id", "id": "shadow-id",'))
$r = Invoke-Cfp @("review", "--proposal", $p, "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"))
Check "a proposal with a duplicate key is a parse error naming the key" ($r.Code -eq 3 -and $r.Err -match 'duplicate' -and $r.Err -match '\bid\b') ("exit={0}" -f $r.Code)

$d = Copy-Pack "rename-user-id" "duplicate-catalog"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
Write-Utf8 $catalog ((Read-Text $catalog).Replace('"schema_version": "1",', '"schema_version": "1", "schema_version": "1",'))
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "a fixture catalog with a duplicate key is refused at approve" ($r.Code -eq 3 -and -not (Test-Path $m) -and $r.Err -match 'duplicate') ("exit={0}" -f $r.Code)

$d = Copy-Pack "rename-user-id" "duplicate-migration"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
if ($a.Code -eq 0) {
  Write-Utf8 $m ((Read-Text $m).Replace('"kind": "migration",', '"kind": "migration", "kind": "migration",'))
  $r = Verify-Pack $d $m (Join-Path $d "verify.json")
  Check "a migration with a duplicate key is refused at verify" ($r.Code -eq 3 -and $r.Err -match 'duplicate') ("exit={0}" -f $r.Code)
} else {
  Check "a migration with a duplicate key is refused at verify" $false ("setup approve failed: exit={0}" -f $a.Code)
}
if ($ReviewRecords) {
  $d = Copy-Pack "rename-user-id" "duplicate-review"
  $common = @("--proposal", (Join-Path $d "proposal.json"), "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"))
  $record = Join-Path $d "review.record.json"
  [void](Invoke-Cfp (@("review") + $common + @("--out", $record)))
  Write-Utf8 $record ((Read-Text $record).Replace('"kind": "review",', '"kind": "review", "kind": "review",'))
  $m = Join-Path $d "migration.json"
  $r = Invoke-Cfp (@("approve") + $common + @("--fixtures", (Join-Path $d "fixtures"), "--reviewer", "r", "--review", $record, "--out", $m))
  Check "a review record with a duplicate key is refused at approve" ($r.Code -eq 3 -and -not (Test-Path $m) -and $r.Err -match 'duplicate') ("exit={0}" -f $r.Code)
} else {
  Check "a review record with a duplicate key is refused at approve" $false "approve takes no review record"
}

$d = Copy-Pack "rename-user-id" "unknown-field"
$p = Join-Path $d "proposal.json"
Write-Utf8 $p ((Read-Text $p).Replace('"kind": "proposal",', '"kind": "proposal", "execute_on_load": true,'))
$r = Invoke-Cfp @("review", "--proposal", $p, "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"))
Check "a proposal with an unknown field is refused" ($r.Code -eq 3 -and $r.Err -match 'execute_on_load') ("exit={0}" -f $r.Code)

# --- Operation identity, pointers, notes and imported evidence ----------------
$d = Join-Path $Work "moved"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$flat = "              type: object`n              properties:`n                name:`n                  type: string"
Write-Utf8 (Join-Path $d "base.yaml") (Contract "  /users:" $flat)
Write-Utf8 (Join-Path $d "next.yaml") (Contract "  /v2/users:" $flat)
$r = Invoke-Cfp @("compare", "--source", (Join-Path $d "base.yaml"), "--destination", (Join-Path $d "next.yaml"))
$ok = $false
if ($r.Code -eq 0) {
  $diff = $r.Out | ConvertFrom-Json
  $moved = @($diff.moved_operations | Where-Object { $_.from.path -eq "/users" -and $_.to.path -eq "/v2/users" })
  $ok = ($moved.Count -eq 1 -and @($diff.removed_operations).Count -eq 0 -and @($diff.added_operations).Count -eq 0)
}
Check "an operation that moved path with the same operationId is classified as moved" $ok ("exit={0}" -f $r.Code)

$pack = Join-Path $Packs "rename-user-id"
$r = Invoke-Cfp @("compare", "--source", (Join-Path $pack "base.openapi.yaml"), "--destination", (Join-Path $pack "next.openapi.yaml"))
Check "diff pointers are RFC 6901 escaped" ($r.Code -eq 0 -and -not $r.Out.Contains("/paths//") -and $r.Out.Contains("/paths/~1users/post")) ("exit={0}" -f $r.Code)

$recursive = Join-Path $Negatives "recursive.openapi.yaml"
$r = Invoke-Cfp @("compare", "--source", $recursive, "--destination", $recursive)
$notes = New-Object System.Collections.ArrayList
if ($r.Code -eq 0) {
  Json-Objects ($r.Out | ConvertFrom-Json) { param($o) $o.construct -is [string] -and $o.message -is [string] -and ($o.construct + $o.message) -match 'recursive' } $notes
}
$keys = @($notes | ForEach-Object { $_.construct + "|" + $_.location })
$distinct = @($keys | Sort-Object -Unique)
Check "a recursive-reference note is emitted once per location" ($r.Code -eq 0 -and $keys.Count -ge 1 -and $keys.Count -eq $distinct.Count) ("notes={0} distinct={1}" -f $keys.Count, $distinct.Count)

$d = Join-Path $Work "allof"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$flat2 = "              type: object`n              properties:`n                a:`n                  type: string`n                b:`n                  type: string"
$wrapped = "              allOf:`n                - `$ref: '#/components/schemas/Base'"
$base = "    Base:`n      type: object`n      properties:`n        a:`n          type: string`n        b:`n          type: string"
Write-Utf8 (Join-Path $d "base.yaml") (Contract "  /users:" $flat2)
Write-Utf8 (Join-Path $d "next.yaml") (Contract "  /users:" $wrapped $base)
$r = Invoke-Cfp @("compare", "--source", (Join-Path $d "base.yaml"), "--destination", (Join-Path $d "next.yaml"))
$ok = $false
if ($r.Code -eq 0) {
  $req = ($r.Out | ConvertFrom-Json).operations[0].request
  $ok = (@($req.removed_properties).Count -eq 0 -and $req.compatibility -eq "unknown")
}
Check "an allOf body reports unknown compatibility and derives no removals" $ok ("exit={0}" -f $r.Code)

$r = Invoke-Cfp @("compare", "--source", (Join-Path $pack "base.openapi.yaml"), "--destination", (Join-Path $pack "next.openapi.yaml"), "--oasdiff", (Join-Path $Negatives "oasdiff-array.json"))
$ok = $false
if ($r.Code -eq 0) {
  $ev = @(($r.Out | ConvertFrom-Json).imported_evidence)[0]
  $ok = ($null -ne $ev -and $ev.level -eq 3 -and $ev.operation -eq "POST" -and $ev.path -eq "/users")
}
Check "imported oasdiff evidence keeps level, operation and path" $ok ("exit={0}" -f $r.Code)

# --- Approval needs a review record and its history is checked ----------------
$d = Copy-Pack "rename-user-id" "no-review"
$m = Join-Path $d "migration.json"
$r = Invoke-Cfp @("approve", "--proposal", (Join-Path $d "proposal.json"), "--source", (Join-Path $d "base.openapi.yaml"),
  "--destination", (Join-Path $d "next.openapi.yaml"), "--fixtures", (Join-Path $d "fixtures"), "--reviewer", "r", "--out", $m)
Check "approve without a review record is refused" ($r.Code -ne 0 -and -not (Test-Path $m)) ("exit={0}" -f $r.Code)

$d = Copy-Pack "rename-user-id" "history-edit"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
if ($a.Code -eq 0) {
  $doc = Read-Json $m
  $doc.approval.reviewer = "someone-else"
  $doc.history = @()
  Write-Utf8 $m ($doc | ConvertTo-Json -Depth 100)
  $r = Verify-Pack $d $m (Join-Path $d "verify.json")
  Check "a migration whose reviewer was edited and history emptied is refused at verify" ($r.Code -ne 0 -and $r.Err -match '(?i)history') ("exit={0}" -f $r.Code)
} else {
  Check "a migration whose reviewer was edited and history emptied is refused at verify" $false ("setup approve failed: exit={0}" -f $a.Code)
}
$claims = @()
foreach ($command in @("approve", "reject", "history", "verify")) {
  $h = Invoke-Cfp @($command, "--help")
  if ($h.Out -match '(?i)append-only') { $claims += $command }
}
Check "no command help claims an append-only history" ($claims.Count -eq 0) ("claims in: {0}" -f ($claims -join ", "))

# --- Approve refuses fixtures it cannot parse ---------------------------------
$d = Copy-Pack "rename-user-id" "utf16"
$fixture = Join-CfpPath $d @("fixtures", "request.positive.json")
[System.IO.File]::WriteAllText($fixture, (Read-Text $fixture), [System.Text.Encoding]::Unicode)
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "approve refuses a fixture that is not UTF-8 JSON and names it" ($r.Code -ne 0 -and -not (Test-Path $m) -and $r.Err.Contains("request.positive.json")) ("exit={0}" -f $r.Code)

# --- Hostile contracts cannot exhaust memory or time ---------------------------
$d = Join-Path $Work "fanout"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$deep = Join-Path $d "deep.json"
Write-Utf8 $deep (Fanout-Contract 2 16 @("a", "b"))
$r = Invoke-CfpLimited @("compare", "--source", $deep, "--destination", $deep) 30
Check "a 2 KB contract whose refs fan out 2 wide and 16 deep is refused within 30 s" ($r.Code -eq 3 -and $r.Err -match 'resource bound exceeded \(schema_(nodes|bytes)\)') (Limited-Detail $r)
$wide = Join-Path $d "wide3.json"
Write-Utf8 $wide (Fanout-Contract 256 3)
$r = Invoke-CfpLimited @("compare", "--source", $wide, "--destination", $wide) 30
Check "a 35 KB contract whose refs fan out 256 wide and 3 deep is refused within 1 GB of memory" ($r.Code -eq 3 -and $r.Err -match 'resource bound exceeded \(schema_(nodes|bytes)\)') (Limited-Detail $r)

# --- An approval cannot be rewritten by recomputing its digests ----------------
# The tool itself re-approves an edited proposal under the original reviewer and
# time: the rules and rules digest a forger needs, computed exactly.
$d = Copy-Pack "default-status" "reviewed"
$forger = Copy-Pack "default-status" "forger"
$p = Join-Path $forger "proposal.json"
Write-Utf8 $p ((Read-Text $p).Replace('"value": "active"', '"value": "suspended-by-attacker"'))
$reapproved = Join-Path $forger "reapproved.migration.json"
# The reviewed approval is made by the binary under test, so its own scheme applies.
$approvedHere = Join-Path $d "migration.json"
$a0 = Approve-Pack $d $approvedHere "local-reviewer"
$a = Approve-Pack $forger $reapproved "local-reviewer"
if ($a0.Code -eq 0 -and $a.Code -eq 0) {
  $r = Invoke-Cfp @("history", "--migration", $approvedHere, "--verify-chain")
  if ($r.Code -ne 0) { throw "the untouched approval must pass the history chain: $($r.Err)" }
  $doc = Read-Json $approvedHere
  $re = Read-Json $reapproved
  $doc.rules = $re.rules
  $doc.approval.bound_digests.rules = $re.approval.bound_digests.rules
  $forged = Join-Path $d "forged.migration.json"
  Write-Utf8 $forged ($doc | ConvertTo-Json -Depth 100)
  $r = Invoke-Cfp @("history", "--migration", $forged, "--verify-chain")
  Check "an approval whose rules were edited and whose rules digest was recomputed fails the history chain" ($r.Code -ne 0) ("exit={0}" -f $r.Code)
  $v = Join-Path $d "verify-reapproved.json"
  $r = Verify-Pack $d $reapproved $v
  $leak = ((Read-Text $v) + $r.Out).Contains("suspended-by-attacker")
  Check "a migration re-approved from an edited proposal under the original reviewer is refused against the reviewed proposal" ($r.Code -ne 0 -and -not $leak) ("exit={0} attacker value in output={1}" -f $r.Code, $leak)
} else {
  Check "an approval whose rules were edited and whose rules digest was recomputed fails the history chain" $false ("setup approve failed: exit={0}/{1}" -f $a0.Code, $a.Code)
  Check "a migration re-approved from an edited proposal under the original reviewer is refused against the reviewed proposal" $false "setup approve failed"
}

# --- A hand-edited verify result cannot steer the report ----------------------
$d = Copy-Pack "rename-user-id" "forged-verify"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
$v = Join-Path $d "verify.json"
$vr = Verify-Pack $d $m $v
if ($a.Code -eq 0 -and $vr.Code -eq 0) {
  $doc = Read-Json $v
  $doc.outcomes[0].passed = $false
  $doc.outcomes[0].actual = "malformed"
  $doc.outcomes[0].path = "https://evil.example/x"
  $forged = Join-Path $d "forged-verify.json"
  Write-Utf8 $forged ($doc | ConvertTo-Json -Depth 100)
  $sarif = Join-Path $d "forged.sarif"
  $r = Invoke-Cfp (@("report", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml")) +
    (Verdict-Args $d $m) + @("--verify", $forged, "--html", (Join-Path $d "forged.html"), "--json", (Join-Path $d "forged.json"),
    "--job-summary", (Join-Path $d "forged-summary.md"), "--sarif", $sarif))
  $url = (Read-Text $sarif).Contains("evil.example")
  Check "a verify outcome path edited to a URL never reaches the SARIF report" ($r.Code -ne 0 -and -not $url) ("exit={0} url in sarif={1}" -f $r.Code, $url)
} else {
  Check "a verify outcome path edited to a URL never reaches the SARIF report" $false ("setup failed: approve={0} verify={1}" -f $a.Code, $vr.Code)
}

# --- A failed run cannot be turned green by editing its verify file ----------
# One of seven fixtures fails honestly; two forgeries keep every bound digest:
# (a) the failing outcome's actual set to its expect, (b) that outcome deleted.
$d = Copy-Pack "rename-user-id" "forged-outcomes"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
foreach ($item in $doc.items) { if ($item.id -eq "request-positive") { $item.expect = "collision" } }
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
$v = Join-Path $d "verify.json"
$vr = Verify-Pack $d $m $v
if ($a.Code -eq 0 -and $vr.Code -eq 1) {
  $doc = Read-Json $v
  foreach ($o in $doc.outcomes) { if (-not $o.passed) { $o.actual = $o.expect; $o.passed = $true } }
  $doc.stable = $true
  Write-Utf8 (Join-Path $d "forged-actual.json") ($doc | ConvertTo-Json -Depth 100)
  $doc = Read-Json $v
  $doc.outcomes = @($doc.outcomes | Where-Object { $_.passed })
  $doc.stable = $true
  Write-Utf8 (Join-Path $d "forged-drop.json") ($doc | ConvertTo-Json -Depth 100)
  foreach ($case in @("actual", "drop")) {
    $summary = Join-Path $d "$case-summary.md"
    $r = Invoke-Cfp (@("report", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml")) +
      (Verdict-Args $d $m) + @("--verify", (Join-Path $d "forged-$case.json"), "--html", (Join-Path $d "$case.html"),
      "--json", (Join-Path $d "$case.json"), "--job-summary", $summary))
    $text = Read-Text $summary
    $green = $text.Contains("stable: yes")
    Check "a failed run's verify file with the failing outcome's $case edited is refused, never reported stable" ($r.Code -eq 3 -and -not $green -and -not (Test-Path $summary)) ("exit={0} stable yes in summary={1}" -f $r.Code, $green)
  }
  # Without the fixtures folder there is no fresh run, so no verdict is shown at all.
  $summary = Join-Path $d "nofixtures-summary.md"
  $r = Invoke-Cfp @("report", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"),
    "--proposal", (Join-Path $d "proposal.json"), "--migration", $m, "--verify", (Join-Path $d "forged-actual.json"),
    "--html", (Join-Path $d "nf.html"), "--json", (Join-Path $d "nf.json"), "--job-summary", $summary)
  Check "a report with a migration but no --fixtures is refused instead of reading a verdict" ($r.Code -eq 3 -and -not (Test-Path $summary)) ("exit={0}" -f $r.Code)
} else {
  Check "a failed run's verify file with the failing outcome's actual edited is refused, never reported stable" $false ("setup failed: approve={0} verify={1}" -f $a.Code, $vr.Code)
  Check "a failed run's verify file with the failing outcome's drop edited is refused, never reported stable" $false ("setup failed: approve={0} verify={1}" -f $a.Code, $vr.Code)
  Check "a report with a migration but no --fixtures is refused instead of reading a verdict" $false "setup failed"
}

# --- An output never replaces the null device ---------------------------------
# Windows: NUL. Elsewhere: /dev/null, only when this user cannot create files in
# /dev, so a regressed binary fails instead of replacing the system device.
$pack = Join-Path $Packs "rename-user-id"
$compareArgs = @("compare", "--source", (Join-Path $pack "base.openapi.yaml"), "--destination", (Join-Path $pack "next.openapi.yaml"), "--out")
if ($CfpOnWindows) {
  $r = Invoke-Cfp ($compareArgs + @("NUL"))
  Check "compare --out NUL writes to the null device instead of renaming a file over it" ($r.Code -eq 0) ("exit={0} {1}" -f $r.Code, $r.Err.Trim())
} else {
  $mayWriteDev = $false
  try { [System.IO.File]::Open("/dev/.cfp-probe-$PID", "CreateNew").Dispose(); Remove-Item -Force "/dev/.cfp-probe-$PID"; $mayWriteDev = $true } catch {}
  if ($mayWriteDev) {
    Write-Output "[SKIP] compare --out /dev/null (running as root: a regressed binary would replace /dev/null; the cfg(unix) test uses a private node)"
  } else {
    $r = Invoke-Cfp ($compareArgs + @("/dev/null"))
    & test -c /dev/null; $stillDevice = ($LASTEXITCODE -eq 0)
    Check "compare --out /dev/null works and /dev/null stays a character device" ($r.Code -eq 0 -and $stillDevice) ("exit={0} device={1} {2}" -f $r.Code, $stillDevice, $r.Err.Trim())
  }
}

# --- An output never lands inside an input folder -----------------------------
# Three cases: verify --out over the catalog, over a bound fixture, and as a new
# file in the fixtures folder.
$snapshot = { param($dir) (Get-ChildItem -Recurse -File $dir | Sort-Object FullName | ForEach-Object { $_.FullName + "=" + (Sha256 $_.FullName) }) -join ";" }
foreach ($target in @("catalog.json", "request.positive.json", "new.json")) {
  # A fresh copy each time: on a vulnerable binary one replacement makes the next verify stale.
  $d = Copy-Pack "rename-user-id" ("into-fixtures-" + $target)
  $m = Join-Path $d "migration.json"
  $a = Approve-Pack $d $m
  $fx = Join-Path $d "fixtures"
  if ($a.Code -eq 0) {
    $before = & $snapshot $fx
    $r = Verify-Pack $d $m (Join-Path $fx $target)
    $after = & $snapshot $fx
    Check "verify --out $target inside the fixtures folder is refused and the folder is unchanged" ($r.Code -eq 3 -and $before -eq $after) ("exit={0} folder changed={1}" -f $r.Code, ($before -ne $after))
  } else {
    Check "verify --out $target inside the fixtures folder is refused and the folder is unchanged" $false ("setup approve failed: exit={0}" -f $a.Code)
  }
}

# --- The guide's verdict is its own line --------------------------------------
$d = Copy-Pack "rename-user-id" "guide-verdict"
$r = Invoke-Cfp @("guide", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"),
  "--fixtures", (Join-Path $d "fixtures"), "--proposal", (Join-Path $d "proposal.json"), "--out-dir", (Join-Path $Work "guide-verdict-out"),
  "--reviewer", "attack-reviewer", "--yes")
$lines = @(($r.Out -split "`r?`n") | Where-Object { $_.Trim() })
$last = if ($lines.Count -gt 0) { $lines[-1] } else { "" }
Check "the guide prints the verdict on its own line, never after an escaped \n" ($r.Code -eq 0 -and $last -eq "Verdict: stable." -and -not $r.Out.Contains("\nVerdict")) ("exit={0} last line={1}" -f $r.Code, $last)

# --- A pattern costs one compile per contract, not one per string -------------
$d = Join-Path $Work "pattern-cost"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$nest = Join-Path $d "nest.json"
Write-Utf8 $nest '{"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{"/u":{"post":{"requestBody":{"content":{"application/json":{"schema":{"type":"object","properties":{"m":{"type":"array","items":{"type":"array","items":{"type":"string","pattern":"\\w{1,200}"}}}}}}}},"responses":{"200":{"description":"ok"}}}}}}'
$row = '[' + ((1..256 | ForEach-Object { '"a"' }) -join ",") + ']'
$payload = Join-Path $d "n16.json"
Write-Utf8 $payload ('{"m":[' + ((1..16 | ForEach-Object { $row }) -join ",") + ']}')
$r = Invoke-CfpLimited @("validate", "--schema-of", $nest, "--method", "POST", "--path", "/u", "--location", "request", "--payload", $payload) 30
Check "4,096 strings under one costly pattern are checked or refused within 30 s" ($null -ne $r.Code -and ($r.Code -eq 0 -or $r.Code -eq 2)) (Limited-Detail $r)

# --- Deep YAML flow nesting is refused before the parser spends minutes on it --
$d = Join-Path $Work "yaml-deep"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$deepYaml = Join-Path $d "o1m.yaml"
Write-Utf8 $deepYaml ("a: " + ("[" * 1048000) + "`n")
$r = Invoke-CfpLimited @("compare", "--source", $deepYaml, "--destination", $deepYaml) 30
Check "a 1 MiB YAML contract of unclosed flow sequences is refused within 30 s" ($r.Code -eq 3 -and $r.Err -match 'resource bound exceeded \(depth\)') (Limited-Detail $r)

# --- A catalog id is a name, never a path --------------------------------------
$d = Copy-Pack "rename-user-id" "catalog-id-path"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
$doc.items[0].id = "../../../escape-from-generated-suite"
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$r = Invoke-Cfp @("generate", "--kind", "jest", "--pack", $d, "--out", (Join-Path $d "generated"))
Check "a catalog id that walks out of the generated suite's folder is refused" ($r.Code -eq 3) ("exit={0}" -f $r.Code)

# --- Untrusted text cannot drive the terminal or the Actions log ---------------
$d = Join-Path $Work "workflow-command"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$wf = Join-Path $d "wf.json"
Write-Utf8 $wf ('{"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{"/u":{"post":{"requestBody":{"content":{"application/json":{"schema":{"$ref":"https://x/\n::error file=README.md,line=1::approved by security\n::add-mask::stable\u001b[2J"}}}},"responses":{"200":{"description":"ok"}}}}}}')
$r = Invoke-Cfp @("compare", "--source", $wf, "--destination", (Join-Path $pack "next.openapi.yaml"))
$commandLines = @($r.Err -split "`r?`n" | Where-Object { $_.TrimStart().StartsWith("::") }).Count
$raw = $r.Err.Contains([string][char]27)
Check "error output carries no workflow command line and no raw escape sequence" ($r.Code -eq 2 -and $commandLines -eq 0 -and -not $raw) ("exit={0} workflow lines={1} raw ESC={2}" -f $r.Code, $commandLines, $raw)

# --- Numeric bounds hold beyond 2^53 ----------------------------------------
$d = Join-Path $Work "exact-bounds"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$maxContract = Join-Path $d "max.json"
Write-Utf8 $maxContract '{"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{"/u":{"post":{"requestBody":{"content":{"application/json":{"schema":{"type":"object","properties":{"v":{"type":"integer","maximum":9007199254740992}}}}}},"responses":{"200":{"description":"ok"}}}}}}'
$over = Join-Path $d "p.json"
Write-Utf8 $over '{"v":9007199254740993}'
$r = Invoke-Cfp @("validate", "--schema-of", $maxContract, "--method", "POST", "--path", "/u", "--location", "request", "--payload", $over)
Check "a value one above a maximum of 2^53 is refused" ($r.Code -eq 1 -and $r.Err -match 'above maximum') ("exit={0}" -f $r.Code)

# --- Supersede revokes the old migration ------------------------------------
$d = Copy-Pack "rename-user-id" "supersede-revokes"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
$next = Join-Path $d "next.proposal.json"
Write-Utf8 $next ((Read-Text (Join-Path $d "proposal.json")).Replace('"kind": "proposal",', '"kind": "proposal", "assumptions": ["successor"],'))
$nextReview = Join-Path $d "next.review.json"
$rv = Invoke-Cfp @("review", "--proposal", $next, "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml"), "--out", $nextReview)
$sp = Invoke-Cfp @("supersede", "--migration", $m, "--proposal", $next, "--source", (Join-Path $d "base.openapi.yaml"),
  "--destination", (Join-Path $d "next.openapi.yaml"), "--fixtures", (Join-Path $d "fixtures"), "--review", $nextReview,
  "--reviewer", "attack-reviewer", "--approved-at", "2026-10-01T00:00:00Z", "--out", (Join-Path $d "successor.migration.json"),
  "--archive", (Join-Path $d "archived.migration.json"))
if ($a.Code -eq 0 -and $rv.Code -eq 0 -and $sp.Code -eq 0) {
  $r = Verify-Pack $d $m (Join-Path $d "verify-old.json")
  Check "a superseded migration is refused while its successor is beside it" ($r.Code -ne 0) ("exit={0}" -f $r.Code)
} else {
  Check "a superseded migration is refused while its successor is beside it" $false ("setup failed: approve={0} review={1} supersede={2} {3}" -f $a.Code, $rv.Code, $sp.Code, $sp.Err.Trim())
}

# --- An output never replaces an input ---------------------------------------
$d = Copy-Pack "rename-user-id" "output-over-input"
$m = Join-Path $d "migration.json"
$a = Approve-Pack $d $m
if ($a.Code -eq 0) {
  $before = Sha256 $m
  $r = Verify-Pack $d $m $m
  $after = Sha256 $m
  Check "verify refuses to write its report over the migration it reads" ($r.Code -eq 3 -and $before -eq $after) ("exit={0} migration changed={1}" -f $r.Code, ($before -ne $after))
} else {
  Check "verify refuses to write its report over the migration it reads" $false ("setup approve failed: exit={0}" -f $a.Code)
}

# --- A closed standard output is an error, not a crash ------------------------
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = $Cfp
$psi.Arguments = (@("compare", "--source", (Join-Path $pack "base.openapi.yaml"), "--destination", (Join-Path $pack "next.openapi.yaml")) | ForEach-Object { Quote-Arg $_ }) -join " "
$psi.UseShellExecute = $false
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.CreateNoWindow = $true
$p = [System.Diagnostics.Process]::Start($psi)
$p.StandardOutput.Close()
$errText = $p.StandardError.ReadToEnd()
$p.WaitForExit()
Check "a closed standard output exits 3 without a panic" ($p.ExitCode -eq 3 -and -not $errText.Contains("panicked")) ("exit={0}" -f $p.ExitCode)

# --- Invisible characters cannot make two reviewer names look alike -----------
$d = Copy-Pack "rename-user-id" "invisible-reviewer"
$m = Join-Path $d "migration.json"
$invisible = "alice" + [char]0x200B + [char]0x061C + [char]0x2060 + [char]0xFEFF + [char]0x034F
$a = Approve-Pack $d $m $invisible
$v = Join-Path $d "verify.json"
$vr = Verify-Pack $d $m $v
$summary = Join-Path $d "summary.md"
$rr = Invoke-Cfp (@("report", "--source", (Join-Path $d "base.openapi.yaml"), "--destination", (Join-Path $d "next.openapi.yaml")) +
  (Verdict-Args $d $m) + @("--verify", $v, "--html", (Join-Path $d "r.html"), "--json", (Join-Path $d "r.json"), "--job-summary", $summary))
$summaryText = Read-Text $summary
$raw = @([char]0x200B, [char]0x061C, [char]0x2060, [char]0xFEFF, [char]0x034F | Where-Object { $summaryText.Contains([string]$_) }).Count
Check "zero-width and bidi characters in the reviewer name are shown as escapes in the job summary" ($rr.Code -eq 0 -and $summaryText.Length -gt 0 -and $raw -eq 0) ("approve={0} report={1} raw characters={2}" -f $a.Code, $rr.Code, $raw)

# --- The validator follows ECMA-262 classes and JSON Schema equality ---------
$d = Join-Path $Work "ecma"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$ecma = Join-Path $d "ecma.json"
Write-Utf8 $ecma '{"openapi":"3.1.0","info":{"title":"t","version":"1"},"paths":{"/u":{"post":{"requestBody":{"content":{"application/json":{"schema":{"type":"object","properties":{"d":{"type":"string","pattern":"^\\d+$"},"s":{"type":"array","uniqueItems":true,"items":{"type":"number"}}}}}}},"responses":{"200":{"description":"ok"}}}}}}'
$arabic = Join-Path $d "arabic.json"
Write-Utf8 $arabic ('{"d":"' + [char]0x0661 + [char]0x0662 + [char]0x0663 + '"}')
$dup = Join-Path $d "dup.json"
Write-Utf8 $dup '{"s":[1,1.0]}'
$validate = @("validate", "--schema-of", $ecma, "--method", "POST", "--path", "/u", "--location", "request", "--payload")
$r1 = Invoke-Cfp ($validate + @($arabic))
$r2 = Invoke-Cfp ($validate + @($dup))
Check "Arabic-Indic digits fail ^\d+$ and [1, 1.0] fails uniqueItems" ($r1.Code -eq 1 -and $r2.Code -eq 1) ("exit={0}/{1}" -f $r1.Code, $r2.Code)

# --- Catalog ids are unique ----------------------------------------------------
$d = Copy-Pack "rename-user-id" "duplicate-ids"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
foreach ($item in $doc.items) { $item.id = "same" }
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "a catalog whose items share one id is refused at approve" ($r.Code -eq 3 -and -not (Test-Path $m)) ("exit={0}" -f $r.Code)

# --- A catalog that proves nothing about a rule is refused ---------------------
$d = Copy-Pack "rename-user-id" "empty-catalog"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
$doc.items = @()
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "an empty fixture catalog is refused at approve" ($r.Code -eq 3 -and -not (Test-Path $m)) ("exit={0}" -f $r.Code)
$d = Copy-Pack "rename-user-id" "rule-without-fixture"
$catalog = Join-CfpPath $d @("fixtures", "catalog.json")
$doc = Read-Json $catalog
$doc.items = @($doc.items | Where-Object { $_.rule_ids -contains "rename-user-request" })
Write-Utf8 $catalog ($doc | ConvertTo-Json -Depth 20)
$m = Join-Path $d "migration.json"
$r = Approve-Pack $d $m
Check "a catalog with no fixture for one rule is refused at approve" ($r.Code -eq 3 -and -not (Test-Path $m)) ("exit={0}" -f $r.Code)

# --- Inputs are read within their bound, quarantine included -----------------
$d = Join-Path $Work "quarantine-size"; New-Item -ItemType Directory -Force -Path $d | Out-Null
$big = Join-Path $d "big.json"
[System.IO.File]::WriteAllBytes($big, (New-Object byte[] (2MB)))
$q = Join-Path $d "quarantine"
$r = Invoke-Cfp @("propose", "--mode", "file", "--proposal", $big, "--quarantine", $q)
$copies = if (Test-Path $q) { @(Get-ChildItem $q).Count } else { 0 }
Check "a refused proposal over the size bound is not copied into quarantine" ($r.Code -eq 3 -and $copies -eq 0) ("exit={0} files in quarantine={1}" -f $r.Code, $copies)

foreach ($j in $Junctions) { Remove-CfpDirLink $j }

$failed = @($Results | Where-Object { $_.Result -eq "FAIL" }).Count
Write-Output ""
Write-Output ("regression attacks: {0} checks, {1} failed, work in {2}" -f $Results.Count, $failed, $Work)
if ($failed -gt 0) { exit 1 }
exit 0
