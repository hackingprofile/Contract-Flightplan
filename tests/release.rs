//! The release tree's SBOM is a CycloneDX 1.5 JSON document built from
//! `cargo metadata`, and it lists every direct dependency with the version
//! the lockfile pins and its licence.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Names under `[dependencies]` and `[dev-dependencies]` in Cargo.toml.
fn direct_dependencies() -> Vec<String> {
    let manifest = fs::read_to_string(root().join("Cargo.toml")).unwrap();
    let mut names = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deps = line == "[dependencies]" || line == "[dev-dependencies]";
            continue;
        }
        if in_deps && !line.is_empty() && !line.starts_with('#') {
            if let Some((name, _)) = line.split_once('=') {
                names.push(name.trim().to_string());
            }
        }
    }
    names
}

/// The version Cargo.lock pins for a package name (the first entry).
fn locked_version(name: &str) -> String {
    let lock = fs::read_to_string(root().join("Cargo.lock")).unwrap();
    let marker = format!("name = \"{name}\"\nversion = \"");
    let at = lock
        .find(&marker)
        .unwrap_or_else(|| panic!("{name} not in Cargo.lock"))
        + marker.len();
    lock[at..at + lock[at..].find('"').unwrap()].to_string()
}

#[test]
fn sbom_json_parses_and_lists_every_direct_dependency() {
    let out = std::env::temp_dir().join(format!("cfp-sbom-{}.cdx.json", std::process::id()));
    let _ = fs::remove_file(&out);
    let shell = if cfg!(windows) { "powershell" } else { "pwsh" };
    // A clean module path: a PowerShell 7 parent (the GitHub Windows step
    // shell) would otherwise hand Windows PowerShell 5.1 its own module folders.
    let run = Command::new(shell)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root().join("scripts").join("sbom.ps1"))
        .arg("-Out")
        .arg(&out)
        .env_remove("PSModulePath")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let text = fs::read_to_string(&out).unwrap();
    let sbom: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(sbom["bomFormat"], "CycloneDX");
    assert_eq!(sbom["specVersion"], "1.5");
    assert_eq!(sbom["metadata"]["component"]["name"], "contract-flightplan");
    assert_eq!(sbom["metadata"]["component"]["version"], "0.1.0");
    let components = sbom["components"].as_array().unwrap();
    let deps = direct_dependencies();
    assert!(deps.len() >= 9, "{deps:?}");
    for name in deps {
        let version = locked_version(&name);
        let found = components
            .iter()
            .find(|c| c["name"] == name.as_str() && c["version"] == version.as_str())
            .unwrap_or_else(|| panic!("{name} {version} is not in the SBOM"));
        assert_eq!(found["purl"], format!("pkg:cargo/{name}@{version}"));
        assert!(
            found["licenses"][0]["expression"]
                .as_str()
                .is_some_and(|l| !l.is_empty()),
            "{found:#}"
        );
    }
    // The root's dependency entry names every direct dependency.
    let root_ref = sbom["metadata"]["component"]["bom-ref"].as_str().unwrap();
    let edges = sbom["dependencies"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["ref"] == root_ref)
        .expect("the root's dependency entry");
    assert!(edges["dependsOn"].as_array().unwrap().len() >= 9);
}

/// The PowerShell executable on this host: `pwsh` when installed, else Windows
/// PowerShell. Returned as an absolute path so a test can narrow the child's PATH.
fn powershell_exe() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let names: &[&str] = if cfg!(windows) {
        &["pwsh.exe", "powershell.exe"]
    } else {
        &["pwsh"]
    };
    for name in names {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("no PowerShell executable on PATH")
}

/// Off Windows there is no `cmd` and no Windows tool on PATH. The SBOM script
/// must work with nothing but the Rust toolchain on PATH.
#[test]
fn sbom_runs_with_only_the_rust_toolchain_on_path() {
    let out = std::env::temp_dir().join(format!("cfp-sbom-path-{}.cdx.json", std::process::id()));
    let _ = fs::remove_file(&out);
    let cargo = PathBuf::from(std::env::var_os("CARGO").expect("cargo sets CARGO for tests"));
    let toolchain_dir = cargo.parent().unwrap().to_path_buf();
    let run = Command::new(powershell_exe())
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root().join("scripts").join("sbom.ps1"))
        .arg("-Out")
        .arg(&out)
        .env("PATH", &toolchain_dir)
        .env_remove("PSModulePath")
        .current_dir(root())
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let text = fs::read_to_string(&out).unwrap();
    let sbom: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(sbom["metadata"]["component"]["name"], "contract-flightplan");
    let _ = fs::remove_file(&out);
}

/// A Windows PowerShell 7 style module path: its Core-only
/// Microsoft.PowerShell.Utility 7.0.0.0 comes first, as when `powershell.exe`
/// inherits the module path of a `pwsh` parent. Windows PowerShell 5.1 then
/// prefers that module, cannot load it, and loses `Get-FileHash`.
#[cfg(windows)]
const PS7_UTILITY_MANIFEST: &str = "@{
GUID='1DA87E53-152B-403E-98DC-74D7B4D63D59'
ModuleVersion='7.0.0.0'
PowerShellVersion='3.0'
CompatiblePSEditions=@('Core')
NestedModules='Microsoft.PowerShell.Commands.Utility.dll'
CmdletsToExport='Get-FileHash','New-Guid','New-TemporaryFile','Format-Hex','ConvertTo-Json','ConvertFrom-Json','Out-String','Out-File','Add-Type','Select-Object','Sort-Object','Write-Output','New-Object','Get-Date'
FunctionsToExport=@()
}
";

/// The SBOM script works under Windows PowerShell 5.1 even when it is
/// handed a PowerShell 7 module path; it hashes Cargo.lock in-process.
#[cfg(windows)]
#[test]
fn sbom_runs_under_windows_powershell_with_a_powershell_7_module_path() {
    let modules = std::env::temp_dir().join(format!("cfp-ps7-modules-{}", std::process::id()));
    let utility = modules.join("Microsoft.PowerShell.Utility");
    fs::create_dir_all(&utility).unwrap();
    fs::write(
        utility.join("Microsoft.PowerShell.Utility.psd1"),
        PS7_UTILITY_MANIFEST,
    )
    .unwrap();
    let mut paths = vec![modules.clone()];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PSModulePath").unwrap_or_default(),
    ));
    let out = std::env::temp_dir().join(format!("cfp-sbom-ps7-{}.cdx.json", std::process::id()));
    let _ = fs::remove_file(&out);
    let run = Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root().join("scripts").join("sbom.ps1"))
        .arg("-Out")
        .arg(&out)
        .env("PSModulePath", std::env::join_paths(paths).unwrap())
        .output()
        .unwrap();
    let _ = fs::remove_dir_all(&modules);
    assert!(
        run.status.success(),
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    let text = fs::read_to_string(&out).unwrap();
    let sbom: Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
    assert_eq!(sbom["metadata"]["component"]["name"], "contract-flightplan");
    let _ = fs::remove_file(&out);
}
