Param(
  [string]$Destination = $(Join-Path $PSScriptRoot "../dist/contract-flightplan")
)

$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
$Allow = @(
  ".gitignore",
  ".gitattributes",
  "rust-toolchain.toml",
  "Cargo.toml",
  "Cargo.lock",
  "LICENSE",
  "NOTICE",
  "README.md",
  "CONTRIBUTING.md",
  "DCO",
  "rustfmt.toml",
  "src",
  "schemas",
  "packs",
  "docs",
  "scripts",
  "tests",
  "fixtures",
  "web",
  ".github",
  "action.yml"
)

. (Join-Path $PSScriptRoot "destination-guard.ps1")
try { Assert-SafeDestination $Root $Destination } catch { Write-Error $_; exit 1 }
if (Test-Path $Destination) {
  Remove-Item -Recurse -Force $Destination
}
New-Item -ItemType Directory -Force -Path $Destination | Out-Null

foreach ($item in $Allow) {
  $from = Join-Path $Root $item
  if (-not (Test-Path $from)) {
    Write-Error "allowlisted path missing: $item"
    exit 1
  }
  $to = Join-Path $Destination $item
  Copy-Item -Recurse -Force $from $to
  if ($item -eq "web") {
    $nm = Join-Path $to "node_modules"
    if (Test-Path $nm) { Remove-Item -Recurse -Force $nm }
  }
}

# Maintenance helpers (scripts/emit-*.ps1) stay out of the export.
Get-ChildItem -File (Join-Path $Destination "scripts") -Filter "emit-*.ps1" | Remove-Item -Force

# Only allowlisted entries may sit at the top of the export.
$top = @(Get-ChildItem -Force -Name $Destination)
foreach ($name in $top) {
  if ($Allow -notcontains $name) {
    Write-Error "export has an entry that is not allowlisted: $name"
    exit 1
  }
}

# Local working records never belong in the public tree: no dated,
# upper-case, underscore-joined Markdown names (the shape of private planning
# notes) and no local evidence or run folders below the allowlisted entries.
$records = @(Get-ChildItem -Recurse -Force -File $Destination | Where-Object { $_.Name -cmatch '^[A-Z0-9]+(_[A-Z0-9]+)+_[0-9]{8}[.]md$' })
$folders = @(Get-ChildItem -Recurse -Force -Directory $Destination | Where-Object { @("proof", "skills", "target", "node_modules", "out", "dist") -contains $_.Name })
foreach ($leak in @($records) + @($folders)) {
  Write-Error "export contains a local working record: $($leak.FullName.Substring($Destination.Length))"
  exit 1
}

Write-Output "exported $Destination"
