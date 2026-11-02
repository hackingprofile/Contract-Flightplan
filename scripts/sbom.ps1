# Minimal CycloneDX 1.5 JSON SBOM, built by this script from `cargo metadata`
# (no SBOM tool is installed or downloaded). It lists every package in the
# locked build and test graph with its version, purl and declared licence,
# and the dependency edges between them. Components are identified by purl,
# so no local path is recorded. Runs under Windows PowerShell 5.1 and
# PowerShell 7 on Windows, Linux and macOS; it needs only cargo on PATH.
#
#   pwsh -File scripts/sbom.ps1 [-Out out/sbom.cdx.json]
Param(
  [string]$Out = $(Join-Path (Join-Path (Join-Path $PSScriptRoot "..") "out") "sbom.cdx.json")
)

$ErrorActionPreference = "Stop"
$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$Out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Out)
Push-Location $Root
try {
  # cargo's progress messages go to stderr; Windows PowerShell turns redirected
  # native stderr into errors under "Stop", so relax it for this one call.
  $ErrorActionPreference = "Continue"
  $json = (& cargo metadata --format-version 1 --locked 2>$null) | Out-String
  $code = $LASTEXITCODE
  $ErrorActionPreference = "Stop"
  if ($code -ne 0) { Write-Error "cargo metadata failed (exit $code)"; exit 1 }
  # Hashed in-process: Get-FileHash lives in a module that Windows PowerShell 5.1
  # cannot load when it inherits a PowerShell 7 module path.
  $sha = [System.Security.Cryptography.SHA256]::Create()
  try { $digest = $sha.ComputeHash([System.IO.File]::ReadAllBytes((Join-Path $Root "Cargo.lock"))) } finally { $sha.Dispose() }
  $lockHash = ([System.BitConverter]::ToString($digest) -replace "-", "").ToLowerInvariant()
} finally {
  Pop-Location
}

# Windows PowerShell's ConvertFrom-Json stops at 2 MB; read with a larger limit.
if ($PSVersionTable.PSEdition -eq "Core") {
  $meta = $json | ConvertFrom-Json -AsHashtable
} else {
  Add-Type -AssemblyName System.Web.Extensions
  $serializer = New-Object System.Web.Script.Serialization.JavaScriptSerializer
  $serializer.MaxJsonLength = [int]::MaxValue
  $meta = $serializer.DeserializeObject($json)
}

$purlById = @{}
$packages = @{}
foreach ($p in $meta["packages"]) {
  $purlById[$p["id"]] = "pkg:cargo/{0}@{1}" -f $p["name"], $p["version"]
  $packages[$p["id"]] = $p
}
$rootId = $meta["resolve"]["root"]

function Component($p, [string]$type) {
  $c = [ordered]@{
    "type" = $type
    "bom-ref" = $purlById[$p["id"]]
    "name" = $p["name"]
    "version" = $p["version"]
    "purl" = $purlById[$p["id"]]
  }
  if ($p["license"]) { $c["licenses"] = @(@{ "expression" = $p["license"] }) }
  return $c
}

$components = New-Object System.Collections.ArrayList
foreach ($id in ($packages.Keys | Sort-Object { $purlById[$_] })) {
  if ($id -ne $rootId) { [void]$components.Add((Component $packages[$id] "library")) }
}
$dependencies = New-Object System.Collections.ArrayList
foreach ($node in ($meta["resolve"]["nodes"] | Sort-Object { $purlById[$_["id"]] })) {
  $on = @($node["deps"] | ForEach-Object { $purlById[$_["pkg"]] } | Sort-Object -Unique)
  [void]$dependencies.Add([ordered]@{ "ref" = $purlById[$node["id"]]; "dependsOn" = $on })
}

$h = $lockHash
$serial = "urn:uuid:{0}-{1}-4{2}-8{3}-{4}" -f $h.Substring(0, 8), $h.Substring(8, 4), $h.Substring(13, 3), $h.Substring(17, 3), $h.Substring(20, 12)
$bom = [ordered]@{
  "bomFormat" = "CycloneDX"
  "specVersion" = "1.5"
  "serialNumber" = $serial
  "version" = 1
  "metadata" = [ordered]@{
    "tools" = [ordered]@{
      "components" = @([ordered]@{
        "type" = "application"
        "name" = "scripts/sbom.ps1"
        "description" = "minimal CycloneDX 1.5 document built from cargo metadata --locked; covers the build and test graph"
      })
    }
    "component" = (Component $packages[$rootId] "application")
  }
  "components" = $components
  "dependencies" = $dependencies
}

$parent = Split-Path -Parent $Out
if ($parent) { New-Item -ItemType Directory -Force -Path $parent | Out-Null }
[System.IO.File]::WriteAllText($Out, ($bom | ConvertTo-Json -Depth 20), (New-Object System.Text.UTF8Encoding $false))
Write-Output ("sbom written {0} ({1} components)" -f $Out, $components.Count)
exit 0
