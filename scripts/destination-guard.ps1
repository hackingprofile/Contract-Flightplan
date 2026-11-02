# Shared by export.ps1 and assemble-v1.ps1, which recursively remove their destination before
# writing it. A destination is refused before anything is removed when it is:
#   - the source root itself, or any folder that contains the source root (an ancestor);
#   - any other folder inside the source root except below dist/ (src, tests, scripts, docs and the
#     private working records are all protected; dist/ itself holds every earlier release tree);
#   - a drive root;
#   - a junction or symbolic link, or a path whose existing parent folders include one inside the
#     source root (a removal could follow it).
# The rules hold for the path as written AND for the real path the filesystem resolves it to, so an
# alias (a junction or link anywhere above the destination, a subst drive, a short 8.3 name) cannot
# address a protected folder. When the real path cannot be resolved, the destination is refused.
# Usage: . (Join-Path $PSScriptRoot "destination-guard.ps1"); Assert-SafeDestination $Root $Destination

if ($IsWindows -ne $false -and -not ('CfpFinalPath' -as [type])) {
  Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;
public static class CfpFinalPath {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  static extern SafeFileHandle CreateFileW(string name, uint access, uint share, IntPtr security, uint disposition, uint flags, IntPtr template);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  static extern uint GetFinalPathNameByHandleW(SafeFileHandle handle, StringBuilder path, uint length, uint flags);
  // Opens the existing file or folder (following every junction and link) and asks Windows where it is.
  public static string Get(string path) {
    using (SafeFileHandle handle = CreateFileW(path, 0, 7, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero)) {
      if (handle.IsInvalid) throw new Win32Exception();
      StringBuilder buffer = new StringBuilder(32768);
      uint length = GetFinalPathNameByHandleW(handle, buffer, (uint)buffer.Capacity, 0);
      if (length == 0 || length >= buffer.Capacity) throw new Win32Exception();
      string real = buffer.ToString();
      if (real.StartsWith(@"\\?\UNC\")) return @"\\" + real.Substring(8);
      if (real.StartsWith(@"\\?\")) return real.Substring(4);
      return real;
    }
  }
}
"@
}

# The real path of $Path: its deepest existing folder resolved by the filesystem, plus the part
# that does not exist yet (which cannot be an alias).
function Resolve-RealPath([string]$Path) {
  $existing = [System.IO.Path]::GetFullPath($Path).TrimEnd('\', '/')
  $rest = @()
  while (-not (Test-Path -LiteralPath $existing)) {
    $parent = [System.IO.Path]::GetDirectoryName($existing)
    if (-not $parent) { throw "no existing parent folder" }
    $rest = @([System.IO.Path]::GetFileName($existing)) + $rest
    $existing = $parent
  }
  if ($IsWindows -ne $false) { $real = [CfpFinalPath]::Get($existing) }
  else { $real = (& realpath -- $existing); if ($LASTEXITCODE -ne 0 -or -not $real) { throw "realpath failed" } }
  foreach ($name in $rest) { $real = Join-Path $real $name }
  return $real.TrimEnd('\', '/')
}

function Assert-SafeDestination([string]$Root, [string]$Destination) {
  $sep = [System.IO.Path]::DirectorySeparatorChar
  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
  $destFull = [System.IO.Path]::GetFullPath($Destination).TrimEnd('\', '/')
  $cmp = [System.StringComparison]::OrdinalIgnoreCase
  $refuse = { param($why) throw "refusing destination '$destFull': $why (nothing was removed)" }

  try { $rootReal = Resolve-RealPath $rootFull; $destReal = Resolve-RealPath $destFull }
  catch { & $refuse "its real path could not be resolved ($($_.Exception.Message))" }
  foreach ($pair in @(@($rootFull, $destFull, ""), @($rootReal, $destReal, " (its real path is '$destReal')"))) {
    $rootP = $pair[0]; $destP = $pair[1]; $via = $pair[2]
    if ([System.IO.Path]::GetPathRoot($destP).TrimEnd('\', '/') -ieq $destP) { & $refuse "a drive root$via" }
    if ($destP.Equals($rootP, $cmp)) { & $refuse "it is the source root$via" }
    if (($rootP + $sep).StartsWith($destP + $sep, $cmp)) { & $refuse "it contains the source root$via" }
    if (($destP + $sep).StartsWith($rootP + $sep, $cmp)) {
      $dist = $rootP + $sep + "dist"
      if (-not $destP.StartsWith($dist + $sep, $cmp)) {
        & $refuse "inside the source root only folders below dist/ may be replaced$via"
      }
    }
  }
  # a junction or link at the destination, or among its existing parents inside the root
  $p = $destFull
  while ($true) {
    if (Test-Path -LiteralPath $p) {
      $item = Get-Item -LiteralPath $p -Force
      if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) { & $refuse "'$p' is a junction or link" }
    }
    $parent = [System.IO.Path]::GetDirectoryName($p)
    if (-not $parent -or $parent.Equals($rootFull, $cmp) -or -not ($parent + $sep).StartsWith($rootFull + $sep, $cmp)) { break }
    $p = $parent
  }
}
