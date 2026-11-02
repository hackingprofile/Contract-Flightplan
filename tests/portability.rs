//! The tree builds and runs the same on Windows, Linux and macOS: a pinned
//! toolchain, byte-exact checkouts of hashed files, and scripts that use only
//! PowerShell 7 and the Rust toolchain.

use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn toolchain_is_pinned_with_rustfmt_and_clippy() {
    let text = read("rust-toolchain.toml");
    let value: toml_lite::Table = toml_lite::parse(&text);
    assert_eq!(value.get("toolchain", "channel"), Some("1.98.1"), "{text}");
    let components = value.list("toolchain", "components");
    for needed in ["rustfmt", "clippy"] {
        assert!(
            components.iter().any(|c| c == needed),
            "{needed} missing: {text}"
        );
    }
}

/// `x >= a || x < b` is the form clippy's `manual_range_contains` flags on
/// rustc 1.97 (1.98 stopped reporting this one); write `!(b..a).contains(&x)`.
#[test]
fn no_manual_range_checks_that_older_clippy_rejects() {
    let pattern = regex::Regex::new(
        r"\b([A-Za-z_][A-Za-z0-9_]*) (>=|>|<=|<) [^|&;{}]+ \|\| ([A-Za-z_][A-Za-z0-9_]*) (>=|>|<=|<) ",
    )
    .unwrap();
    let mut hits = Vec::new();
    for entry in fs::read_dir(root().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let text = fs::read_to_string(&path).unwrap();
        for (n, line) in text.lines().enumerate() {
            for caps in pattern.captures_iter(line) {
                if caps[1] == caps[3] {
                    hits.push(format!("{}:{}: {}", path.display(), n + 1, line.trim()));
                }
            }
        }
    }
    assert!(hits.is_empty(), "manual range checks:\n{}", hits.join("\n"));
}

/// The few TOML shapes `rust-toolchain.toml` uses, read without a TOML crate.
mod toml_lite {
    pub struct Table(Vec<(String, String, String)>);

    pub fn parse(text: &str) -> Table {
        let mut section = String::new();
        let mut entries = Vec::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap().trim();
            if line.starts_with('[') && line.ends_with(']') {
                section = line[1..line.len() - 1].trim().to_string();
            } else if let Some((k, v)) = line.split_once('=') {
                entries.push((section.clone(), k.trim().to_string(), v.trim().to_string()));
            }
        }
        Table(entries)
    }

    impl Table {
        fn raw(&self, section: &str, key: &str) -> Option<&str> {
            self.0
                .iter()
                .find(|(s, k, _)| s == section && k == key)
                .map(|(_, _, v)| v.as_str())
        }

        pub fn get(&self, section: &str, key: &str) -> Option<&str> {
            self.raw(section, key).map(|v| v.trim_matches('"'))
        }

        pub fn list(&self, section: &str, key: &str) -> Vec<String> {
            self.raw(section, key)
                .map(|v| {
                    v.trim_start_matches('[')
                        .trim_end_matches(']')
                        .split(',')
                        .map(|s| s.trim().trim_matches('"').to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default()
        }
    }
}

fn walk(dir: &std::path::Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// Does a `.gitattributes` pattern (the subset this repository uses: `dir/**`,
/// `*.ext`, `name`) match a repository-relative path with `/` separators?
fn attr_matches(pattern: &str, rel: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return rel.starts_with(&format!("{prefix}/"));
    }
    let name = rel.rsplit('/').next().unwrap();
    if let Some(suffix) = pattern.strip_prefix('*') {
        return !suffix.contains('/') && name.ends_with(suffix);
    }
    pattern == rel || (!pattern.contains('/') && pattern == name)
}

/// Approvals bind files byte for byte, so Git must never convert their line
/// endings: every file under packs/, fixtures/ and schemas/ (contracts,
/// approvals, catalogs, fixtures, schemas) must be marked `-text`.
#[test]
fn gitattributes_keeps_hashed_files_byte_exact() {
    let text = read(".gitattributes");
    // (pattern, byte-exact?) for every line that sets the `text` attribute.
    let rules: Vec<(&str, bool)> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let pattern = parts.next()?;
            let mut state = None;
            for a in parts {
                if a == "-text" || a == "binary" {
                    state = Some(true);
                } else if a == "text" || a.starts_with("text=") {
                    state = Some(false);
                }
            }
            state.map(|exact| (pattern, exact))
        })
        .collect();
    // Git applies the last matching line, so the effective state is the last match's.
    let exact = |rel: &str| {
        rules
            .iter()
            .rev()
            .find(|(pat, _)| attr_matches(pat, rel))
            .is_some_and(|(_, exact)| *exact)
    };
    let mut files = Vec::new();
    for dir in ["packs", "fixtures", "schemas"] {
        walk(&root().join(dir), &mut files);
    }
    assert!(files.len() > 50, "expected the shipped packs and schemas");
    let uncovered: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(root())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .filter(|rel| !exact(rel))
        .collect();
    assert!(
        uncovered.is_empty(),
        "line endings could be converted in: {uncovered:?}"
    );
    // Contracts and approvals elsewhere (users' own trees copied in) too.
    for name in [
        "x.openapi.yaml",
        "x.openapi.json",
        "approval.migration.json",
    ] {
        assert!(exact(name), "{name} is not covered");
    }
    // The rule is documented for users.
    assert!(read("README.md").contains(".gitattributes"));
    // And the export ships both cross-platform files.
    let export = read("scripts/export.ps1");
    for name in ["\".gitattributes\"", "\"rust-toolchain.toml\""] {
        assert!(export.contains(name), "export allowlist lacks {name}");
    }
}

/// The reason for the rule: a CRLF checkout of a pack is a different set of
/// bytes, so its shipped approval is stale and verify refuses it.
#[test]
fn crlf_converted_pack_fails_verify_as_stale() {
    let dir = std::env::temp_dir().join(format!("cfp-crlf-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let pack = root().join("packs").join("default-status");
    let mut files = Vec::new();
    walk(&pack, &mut files);
    for file in &files {
        let rel = file.strip_prefix(&pack).unwrap();
        let to = dir.join(rel);
        fs::create_dir_all(to.parent().unwrap()).unwrap();
        let text = fs::read_to_string(file).unwrap();
        assert!(
            !text.contains('\r'),
            "{} must be LF in the tree",
            rel.display()
        );
        fs::write(&to, text.replace('\n', "\r\n")).unwrap();
    }
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cfp"))
        .arg("verify")
        .arg("--migration")
        .arg(dir.join("approval.migration.json"))
        .arg("--proposal")
        .arg(dir.join("proposal.json"))
        .arg("--source")
        .arg(dir.join("base.openapi.yaml"))
        .arg("--destination")
        .arg(dir.join("next.openapi.yaml"))
        .arg("--fixtures")
        .arg(dir.join("fixtures"))
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(1), "{all}");
    assert!(all.contains("stale"), "{all}");
    let _ = fs::remove_dir_all(&dir);
}

/// The scripts the export ships.
fn shipped_scripts() -> Vec<PathBuf> {
    let mut scripts: Vec<PathBuf> = fs::read_dir(root().join("scripts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "ps1"))
        .filter(|p| {
            !p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("emit-")
        })
        .collect();
    scripts.sort();
    scripts
}

/// Every shipped script runs under PowerShell 7 on Linux and macOS as well as
/// on Windows: no `cmd`, no Windows PowerShell executable, no `$env:TEMP`, no
/// hard-coded `cfp.exe`, no backslash paths, and real symlinks wherever a
/// junction is used on Windows.
#[test]
fn shipped_scripts_avoid_windows_only_constructs() {
    let backslash_path = regex::Regex::new(
        r"(?i)\b(packs|out|target|scripts|fixtures|negatives|dist|release|src|docs|schemas|bin|web|node|python|export|checksums|v1)\\",
    )
    .unwrap();
    let windows_shell = regex::Regex::new(r"(?i)(^|[\s&(])powershell(\.exe)?(\s|$)").unwrap();
    let mut hits = Vec::new();
    for script in shipped_scripts() {
        let name = script.file_name().unwrap().to_string_lossy().to_string();
        let text = fs::read_to_string(&script).unwrap();
        for (n, line) in text.lines().enumerate() {
            let at = format!("{name}:{}: {}", n + 1, line.trim());
            if backslash_path.is_match(line) {
                hits.push(format!("backslash path   {at}"));
            }
            if line.trim_start().starts_with('#') {
                continue;
            }
            let lower = line.to_lowercase();
            if lower.contains("cmd /c") || lower.contains("cmd.exe") {
                hits.push(format!("cmd              {at}"));
            }
            if windows_shell.is_match(line) {
                hits.push(format!("powershell exe   {at}"));
            }
            if line.contains("$env:TEMP") {
                hits.push(format!("$env:TEMP        {at}"));
            }
            if lower.contains("cfp.exe") {
                hits.push(format!("cfp.exe          {at}"));
            }
            // `$args` is automatic: a parameter of that name never receives its arguments.
            if lower.contains("]$args)") || lower.contains("]$args,") {
                hits.push(format!("$args parameter  {at}"));
            }
        }
        if text.contains("ItemType Junction") && !text.contains("SymbolicLink") {
            hits.push(format!("junction only    {name}"));
        }
    }
    assert!(hits.is_empty(), "not portable:\n{}", hits.join("\n"));
}

/// The 26 mutants in `scripts/mutation-check.ps1`, read from its table.
fn mutants() -> Vec<(String, String, String)> {
    let text = read("scripts/mutation-check.ps1");
    let row = regex::Regex::new(r#"^\s*@\("([^"]*)", "([^"]*)", "([^"]*)"\),?\s*$"#).unwrap();
    text.lines()
        .filter_map(|l| row.captures(l))
        .map(|c| (c[1].to_string(), c[2].to_string(), c[3].to_string()))
        .collect()
}

/// Every mutant's anchor text is found inside its function in src/mapping.rs,
/// using the script's own rule (from `fn name(` to the next top-level `fn `),
/// so the mutation score covers all 26 and is reproducible on every OS.
#[test]
fn every_mutation_anchor_applies_to_the_mapping_source() {
    let source = read("src/mapping.rs").replace("\r\n", "\n");
    let mutants = mutants();
    assert_eq!(mutants.len(), 26, "the script defines 26 mutants");
    let mut missing = Vec::new();
    for (n, (function, find, replace)) in mutants.iter().enumerate() {
        let Some(start) = source.find(&format!("fn {function}(")) else {
            missing.push(format!("{}: no function {function}", n + 1));
            continue;
        };
        let end = source[start + 1..]
            .find("\nfn ")
            .map_or(source.len(), |i| start + 1 + i);
        match source[start..].find(find.as_str()).map(|i| start + i) {
            Some(at) if at < end => {
                let mutated = format!("{}{replace}{}", &source[..at], &source[at + find.len()..]);
                assert_ne!(mutated, source, "mutant {} changes nothing", n + 1);
            }
            _ => missing.push(format!("{}: {function} does not contain {find:?}", n + 1)),
        }
    }
    assert!(
        missing.is_empty(),
        "anchors that no longer match:\n{}",
        missing.join("\n")
    );
}

/// The PowerShell executable on this host: `pwsh` when installed, else
/// Windows PowerShell, as an absolute path so the child's PATH can be narrowed.
fn powershell_exe() -> PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let names: &[&str] = if cfg!(windows) {
        &["pwsh.exe", "powershell.exe"]
    } else {
        &["pwsh"]
    };
    for name in names {
        for dir in std::env::split_paths(&path) {
            if dir.join(name).is_file() {
                return dir.join(name);
            }
        }
    }
    panic!("no PowerShell executable on PATH")
}

fn dir_of(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let names = if cfg!(windows) {
        vec![format!("{program}.exe"), format!("{program}.cmd")]
    } else {
        vec![program.to_string()]
    };
    std::env::split_paths(&path).find(|d| names.iter().any(|n| d.join(n).is_file()))
}

/// The generated-suite runner works with only node and the Rust toolchain on
/// PATH (no `cmd`, as off Windows), runs the Node suite and reports Python
/// honestly when no interpreter is reachable.
#[test]
fn generated_suite_runner_needs_no_cmd() {
    let Some(node_dir) = dir_of("node") else {
        eprintln!("node is not installed; skipping");
        return;
    };
    let cargo = PathBuf::from(std::env::var_os("CARGO").expect("cargo sets CARGO for tests"));
    let path = std::env::join_paths([cargo.parent().unwrap().to_path_buf(), node_dir]).unwrap();
    let run = std::process::Command::new(powershell_exe())
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(root().join("scripts").join("run-generated-tests.ps1"))
        .arg("-Cfp")
        .arg(env!("CARGO_BIN_EXE_cfp"))
        .env("PATH", path)
        .env_remove("PSModulePath")
        .output()
        .unwrap();
    let all = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(run.status.success(), "{all}");
    assert!(all.contains("node suite: PASS"), "{all}");
    assert!(
        all.contains("python suite: PASS") || all.contains("python suite: UNEXECUTED"),
        "{all}"
    );
}

/// Every shipped script is valid PowerShell (a parse error stops a script
/// before its first line runs).
#[test]
fn every_shipped_script_parses() {
    let list = shipped_scripts()
        .iter()
        .map(|p| format!("'{}'", p.display().to_string().replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(",");
    let command = format!(
        "$bad = 0; foreach ($f in @({list})) {{ $e = $null; [void][System.Management.Automation.Language.Parser]::ParseFile($f, [ref]$null, [ref]$e); foreach ($x in $e) {{ $bad++; Write-Output (\"{{0}}:{{1}}: {{2}}\" -f (Split-Path -Leaf $f), $x.Extent.StartLineNumber, $x.Message) }} }}; exit $bad"
    );
    let run = std::process::Command::new(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", &command])
        .env_remove("PSModulePath")
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "parse errors:\n{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
}
