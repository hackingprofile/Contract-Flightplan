# Assemble a local, unsigned release tree:
#
#   export/                     the public source tree (scripts/export.ps1)
#   bin/<os>-<arch>/cfp[.exe]   built from export/ with a clean target dir, for this host
#   schemas/                    the published JSON schemas
#   sbom/cfp.cdx.json           CycloneDX 1.5, built from cargo metadata
#   FIXTURE-DIGEST.txt          SHA-256 of every file under export/packs, and
#                               one digest over that list
#   SUPPORT-MATRIX.md           supported constructs and executed hosts
#   SIGNATURES.md               states that nothing is signed or attested
#   checksums/SHA256SUMS.txt    every file above, relative to the tree
#
#   pwsh -File scripts/assemble-v1.ps1 [-Destination dist/release]
#
# Nothing is published, signed, tagged or uploaded.
Param(
  [string]$Destination = $(Join-Path (Join-Path (Join-Path $PSScriptRoot "..") "dist") "release")
)

$ErrorActionPreference = "Stop"
. (Join-Path $PSScriptRoot "common.ps1")
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)
$Destination = [System.IO.Path]::GetFullPath($Destination)
$Utf8 = New-Object System.Text.UTF8Encoding $false
. (Join-Path $PSScriptRoot "destination-guard.ps1")
try { Assert-SafeDestination $Root $Destination } catch { Write-Error $_; exit 1 }
if (Test-Path $Destination) { Remove-Item -Recurse -Force $Destination }
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

$Export = Join-Path $Destination "export"
$exported = Invoke-CfpScript (Join-CfpPath $Root @("scripts", "export.ps1")) @("-Destination", $Export)
if ($exported.Code -ne 0) { Write-Output $exported.All; Write-Error "export failed"; exit 1 }

# Build from the exported tree with a target dir nobody else uses.
$Target = New-CfpTempDir "cfp-assemble-target"
$env:CARGO_TARGET_DIR = $Target
# Panic locations embed source paths; remap the build machine's cargo and
# rustup homes and the export folder so no private path ends up in the binary.
# CARGO_ENCODED_RUSTFLAGS separates flags with 0x1F, so paths may contain spaces.
$profileDir = [Environment]::GetFolderPath("UserProfile")
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $profileDir ".cargo" }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $profileDir ".rustup" }
$env:CARGO_ENCODED_RUSTFLAGS = @(
  "--remap-path-prefix=$cargoHome=/cargo",
  "--remap-path-prefix=$rustupHome=/rustup",
  "--remap-path-prefix=$Export=/contract-flightplan"
) -join [char]0x1f
$build = Invoke-CfpNative "cargo" @("build", "--release", "--locked", "--quiet", "--manifest-path", (Join-Path $Export "Cargo.toml"))
Remove-Item Env:\CARGO_TARGET_DIR
Remove-Item Env:\CARGO_ENCODED_RUSTFLAGS
if ($build.Code -ne 0) { Write-Output $build.All; Write-Error "release build failed"; exit 1 }
# The platform folder names the OS and CPU this binary was built and run on.
$os = if ($CfpOnWindows) { "windows" } elseif ($IsMacOS) { "macos" } else { "linux" }
$arch = ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture).ToString().ToLowerInvariant()
$Platform = "$os-$arch"
$Bin = Join-CfpPath $Destination @("bin", $Platform)
New-Item -ItemType Directory -Force -Path $Bin | Out-Null
Copy-Item (Join-CfpPath $Target @("release", "cfp$CfpExe")) (Join-Path $Bin "cfp$CfpExe")

Copy-Item -Recurse (Join-Path $Export "schemas") (Join-Path $Destination "schemas")

$sbom = Invoke-CfpScript (Join-CfpPath $Export @("scripts", "sbom.ps1")) @("-Out", (Join-CfpPath $Destination @("sbom", "cfp.cdx.json")))
if ($sbom.Code -ne 0) { Write-Output $sbom.All; Write-Error "sbom failed"; exit 1 }

# Fixture digest: one line per pack file, then a digest over those lines.
$packs = Join-Path $Export "packs"
$lines = Get-ChildItem -Recurse -File $packs | Sort-Object FullName | ForEach-Object {
  $rel = $_.FullName.Substring($packs.Length + 1).Replace([System.IO.Path]::DirectorySeparatorChar, [char]'/')
  "{0}  {1}" -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant(), $rel
}
$listing = ($lines -join "`n") + "`n"
$sha = [System.Security.Cryptography.SHA256]::Create()
$total = ([System.BitConverter]::ToString($sha.ComputeHash($Utf8.GetBytes($listing))) -replace "-", "").ToLowerInvariant()
[System.IO.File]::WriteAllText((Join-Path $Destination "FIXTURE-DIGEST.txt"),
  "# SHA-256 of every file under packs/, then of this listing (the lines below, LF-joined).`n" +
  "fixture-corpus sha256:$total`n`n" + $listing, $Utf8)

$support = "# Support matrix (unsigned local build)`n`n" +
  [System.IO.File]::ReadAllText((Join-CfpPath $Export @("docs", "supported-matrix.md"))).Replace("# Supported / unsupported matrix", "## Constructs") + "`n" +
  [System.IO.File]::ReadAllText((Join-CfpPath $Export @("docs", "HOST-MATRIX.md"))).Replace("# Host matrix", "## Hosts") +
  "`nThe binary in bin/$Platform was built and executed on this host only. No other platform binary is included.`n"
[System.IO.File]::WriteAllText((Join-Path $Destination "SUPPORT-MATRIX.md"), $support, $Utf8)

[System.IO.File]::WriteAllText((Join-Path $Destination "SIGNATURES.md"),
  "# Signatures`n`nUnsigned local build. No signature, no attestation, no provenance statement and no publication. " +
  "checksums/SHA256SUMS.txt lets a reader check that files are unchanged since assembly; it does not show who assembled them.`n", $Utf8)

# Checksums over every file in the tree, relative to it, with "/" separators on every OS.
$sumsDir = Join-Path $Destination "checksums"
New-Item -ItemType Directory -Force -Path $sumsDir | Out-Null
$entries = Get-ChildItem -Recurse -File $Destination | Where-Object { $_.FullName -notlike "$sumsDir*" } | Sort-Object FullName | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash, $_.FullName.Substring($Destination.Length + 1).Replace([System.IO.Path]::DirectorySeparatorChar, [char]'/')
}
[System.IO.File]::WriteAllText((Join-Path $sumsDir "SHA256SUMS.txt"), ($entries -join "`n") + "`n", $Utf8)

Write-Output ("assembled local unsigned release tree at {0} ({1} files checksummed; not published)" -f $Destination, $entries.Count)
exit 0
