# Verify a local release tree.
#
#   pwsh -File scripts/verify-local-release.ps1 [-Tree dist/release]
#
# For a release tree with a binary (one with bin/): every checksum matches, nothing is
# unlisted, the binary runs and reports the source version, the SBOM is
# CycloneDX 1.5 naming this package, SIGNATURES.md says the tree is unsigned,
# and the README quickstart runs from the exported source. For the earlier
# source-only layout (dist/v1), the checksums of export/ are verified.
# Exits 1 on any failure. This is a local integrity check, not a signature.
Param(
  [string]$Tree = $(Join-Path (Join-Path (Join-Path $PSScriptRoot "..") "dist") "release")
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
if (-not [System.IO.Path]::IsPathRooted($Tree)) { $Tree = Join-Path $Root $Tree }
$Tree = (Resolve-Path $Tree).Path.TrimEnd([char]0x5c, [char]'/')
$Sums = Join-CfpPath $Tree @("checksums", "SHA256SUMS.txt")
if (-not (Test-Path $Sums)) { Write-Error "missing $Sums"; exit 1 }
$Candidate = Test-Path (Join-Path $Tree "bin")
$Base = if ($Candidate) { $Tree } else { (Resolve-Path (Join-Path $Tree "export")).Path.TrimEnd([char]0x5c, [char]'/') }

$failed = 0
$listed = @{}
foreach ($line in (Get-Content $Sums)) {
  if (-not $line) { continue }
  $parts = $line -split "  ", 2
  if ($parts.Count -ne 2) { continue }
  # Older trees list Windows separators; compare with "/" on every OS.
  $rel = $parts[1].Replace([char]0x5c, [char]'/')
  $listed[$rel] = $true
  $path = Join-CfpPath $Base ($rel -split "/")
  if (-not (Test-Path -LiteralPath $path)) { Write-Output "missing $rel"; $failed++; continue }
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath $path).Hash -ne $parts[0]) { Write-Output "mismatch $rel"; $failed++ }
}
if ($Candidate) {
  $sumsDir = (Join-Path $Tree "checksums") + [System.IO.Path]::DirectorySeparatorChar
  Get-ChildItem -Recurse -File $Tree | Where-Object { -not $_.FullName.StartsWith($sumsDir) } | ForEach-Object {
    $rel = $_.FullName.Substring($Tree.Length + 1).Replace([System.IO.Path]::DirectorySeparatorChar, [char]'/')
    if (-not $listed.ContainsKey($rel)) { Write-Output "unlisted $rel"; $script:failed++ }
  }
}
if ($failed -ne 0) { Write-Error "$failed checksum failures"; exit 1 }
Write-Output ("checksums: {0} files verified in {1}" -f $listed.Count, $Base)
if (-not $Candidate) {
  Write-Output "verified SHA256SUMS for $Base (unsigned local check; source-only layout)"
  exit 0
}

# The one platform folder under bin/ (bin/<os>-<arch>/cfp[.exe]).
$platforms = @(Get-ChildItem -Directory (Join-Path $Tree "bin"))
if ($platforms.Count -ne 1) { Write-Error "expected one platform folder under bin/, found $($platforms.Count)"; exit 1 }
$cfp = Join-Path $platforms[0].FullName "cfp$CfpExe"
if (-not (Test-Path -LiteralPath $cfp)) { Write-Error "no binary for this platform at $cfp"; exit 1 }
$manifest = [System.IO.File]::ReadAllText((Join-CfpPath $Tree @("export", "Cargo.toml")))
$version = ([regex]::Match($manifest, '(?m)^version = "([^"]+)"')).Groups[1].Value
$reported = ((& $cfp --version) | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $reported -ne "cfp $version") { Write-Error "binary reports '$reported', expected 'cfp $version'"; exit 1 }
Write-Output "binary: $reported"

# The binary must not carry the build machine's private paths (user profile,
# home directory, cargo registry checkout), e.g. in panic locations.
$bytes = [System.IO.File]::ReadAllBytes($cfp)
$text = [System.Text.Encoding]::GetEncoding(28591).GetString($bytes)
# Generic user-profile and home directory names, matched inside the binary.
$profileWord = "Us" + "ers"
$homeWord = "ho" + "me"
$pattern = '[A-Za-z]:\\' + $profileWord + '\\[^\x00\s"]{1,40}|[A-Za-z]:/' + $profileWord + '/[^\x00\s"]{1,40}|/' + $homeWord + '/[^\x00\s"/]{1,40}|\.cargo[\\/]registry'
$private = [regex]::Matches($text, $pattern)
if ($private.Count -gt 0) {
  $shown = @($private | Select-Object -First 3 | ForEach-Object { $_.Value }) -join "; "
  Write-Error ("binary embeds {0} private path(s), e.g. {1}" -f $private.Count, $shown)
  exit 1
}
Write-Output "binary: no private build paths embedded"

$sbom = Get-Content -Raw (Join-CfpPath $Tree @("sbom", "cfp.cdx.json")) | ConvertFrom-Json
if ($sbom.bomFormat -ne "CycloneDX" -or $sbom.specVersion -ne "1.5" -or $sbom.metadata.component.name -ne "contract-flightplan") {
  Write-Error "sbom is not a CycloneDX 1.5 document for contract-flightplan"; exit 1
}
Write-Output ("sbom: CycloneDX {0}, {1} components" -f $sbom.specVersion, @($sbom.components).Count)

if ((Get-Content -Raw (Join-Path $Tree "SIGNATURES.md")) -notmatch "Unsigned local") { Write-Error "SIGNATURES.md must state the tree is unsigned"; exit 1 }

# The README quickstart, from a copy of the exported source, on a fresh target.
$copy = Join-Path (New-CfpTempDir "cfp-verify-release") "export"
Copy-Item -Recurse (Join-Path $Tree "export") $copy
$env:CARGO_TARGET_DIR = Join-Path $copy "target"
$run = Invoke-CfpScript (Join-CfpPath $copy @("scripts", "quickstart.ps1"))
Remove-Item Env:\CARGO_TARGET_DIR
if ($run.Code -ne 0) { Write-Output $run.All; Write-Error "quickstart failed from the exported source"; exit 1 }
Write-Output "quickstart: passed from a copy of export/ ($copy)"
Write-Output "verified $Tree (unsigned local check; not a signature)"
exit 0
