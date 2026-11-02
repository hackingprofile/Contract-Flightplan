# Helpers shared by the scripts, so each one runs the same under Windows PowerShell 5.1 and
# PowerShell 7 on Windows, Linux and macOS. Dot-source it:
#   . (Join-Path $PSScriptRoot "common.ps1")

# $IsWindows does not exist in Windows PowerShell 5.1, which only runs on Windows.
$CfpOnWindows = ($PSVersionTable.PSEdition -eq "Desktop") -or [bool]$IsWindows
$CfpExe = if ($CfpOnWindows) { ".exe" } else { "" }

# The system temporary folder ($env:TEMP exists only on Windows; Linux and macOS use TMPDIR or /tmp).
function Get-CfpTempRoot { return [System.IO.Path]::GetTempPath() }

# A fresh, unique folder under the temporary folder.
function New-CfpTempDir([string]$Prefix) {
  $name = "{0}-{1}-{2}-{3}" -f $Prefix, (Get-Date -Format "yyyyMMdd-HHmmss"), $PID, ([guid]::NewGuid().ToString("N").Substring(0, 8))
  $dir = Join-Path (Get-CfpTempRoot) $name
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  return $dir
}

# Join any number of path segments with the platform's separator.
function Join-CfpPath([string]$Base, [string[]]$Parts) {
  $path = $Base
  foreach ($part in $Parts) { $path = Join-Path $path $part }
  return $path
}

# The PowerShell executable running this script, for starting a child script the same way
# (pwsh on Linux and macOS, pwsh or powershell on Windows).
function Get-CfpShell { return [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName }

# The release cfp binary that `cargo build --release` produced for this tree.
function Get-CfpBinary([string]$Root) {
  $target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root "target" }
  return (Join-CfpPath $target @("release", "cfp$CfpExe"))
}

# Quote one argument so the child process sees it unchanged. .NET splits a command line with the
# same rules on every OS (the MSVC runtime's), so one quoting works everywhere.
function ConvertTo-CfpArg([string]$Value) {
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

# Run a program found on PATH (or by path) and capture its exit code, stdout and stderr without
# PowerShell turning stderr into errors. Optional working folder.
function Invoke-CfpNative([string]$File, [string[]]$Arguments = @(), [string]$WorkDir = "") {
  $utf8 = New-Object System.Text.UTF8Encoding $false
  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = $File
  $psi.Arguments = (@($Arguments) | ForEach-Object { ConvertTo-CfpArg $_ }) -join " "
  $psi.UseShellExecute = $false
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.StandardOutputEncoding = $utf8
  $psi.StandardErrorEncoding = $utf8
  $psi.CreateNoWindow = $true
  if ($WorkDir) { $psi.WorkingDirectory = $WorkDir } else { $psi.WorkingDirectory = (Get-Location).ProviderPath }
  $p = [System.Diagnostics.Process]::Start($psi)
  $out = $p.StandardOutput.ReadToEndAsync()
  $err = $p.StandardError.ReadToEndAsync()
  $p.WaitForExit()
  return [pscustomobject]@{ Code = $p.ExitCode; Out = $out.Result; Err = $err.Result; All = $out.Result + $err.Result }
}

# Run another script of this repository in a child PowerShell of the same kind.
function Invoke-CfpScript([string]$Script, [string[]]$Arguments = @()) {
  return (Invoke-CfpNative (Get-CfpShell) (@("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $Script) + @($Arguments)))
}

# Create a directory link: a junction on Windows (no privilege needed), a symbolic link elsewhere.
function New-CfpDirLink([string]$Link, [string]$Target) {
  if ($CfpOnWindows) { New-Item -ItemType Junction -Path $Link -Target $Target | Out-Null }
  else { New-Item -ItemType SymbolicLink -Path $Link -Target $Target | Out-Null }
}

# Remove a directory link without touching what it points to.
function Remove-CfpDirLink([string]$Link) {
  if (-not (Test-Path -LiteralPath $Link)) { return }
  # Directory.Delete removes a junction or a directory symlink itself, never its target.
  [System.IO.Directory]::Delete($Link)
}
