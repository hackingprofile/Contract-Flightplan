//! Keeps public documentation in step with the binary: every advertised
//! operation, command, module, pack and test owner must exist in the tree, and
//! statements that no longer match the implementation must not come back.

use contract_flightplan::typecheck::{LOSSINESS_VOCABULARY, REVERSIBILITY_VOCABULARY};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn schema_operations() -> BTreeSet<String> {
    let schema: serde_json::Value =
        serde_json::from_str(&read("schemas/v1/migration.schema.json")).unwrap();
    schema["$defs"]["rule"]["properties"]["operation"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn rust_sources() -> String {
    let mut all = String::new();
    for dir in ["src", "tests"] {
        for entry in fs::read_dir(root().join(dir)).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|e| e == "rs") {
                all.push_str(&fs::read_to_string(path).unwrap());
                all.push('\n');
            }
        }
    }
    all
}

#[test]
fn readme_and_mapping_doc_list_exactly_the_implemented_operations() {
    let ops = schema_operations();
    let readme = read("README.md");
    let line = readme
        .lines()
        .find(|l| l.starts_with("Implemented mapping operations:"))
        .expect("README names the operations");
    let listed: BTreeSet<String> = line
        .trim_start_matches("Implemented mapping operations:")
        .trim_end_matches('.')
        .split(',')
        .map(|s| s.trim().to_string())
        .collect();
    assert_eq!(listed, ops, "README operation list drifted from the schema");

    let mapping = read("docs/mapping.md");
    for op in &ops {
        assert!(
            mapping.contains(&format!("## {op}")),
            "docs/mapping.md misses `## {op}`"
        );
    }
    let headings = mapping
        .lines()
        .filter(|l| l.starts_with("## "))
        .map(|l| l[3..].trim().to_string())
        .filter(|h| h.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .collect::<BTreeSet<_>>();
    for heading in headings {
        assert!(
            ops.contains(&heading),
            "docs/mapping.md documents unknown op `{heading}`"
        );
    }
}

#[test]
fn vocabularies_in_docs_match_the_type_checker() {
    let mapping = read("docs/mapping.md");
    let uncertainty = read("docs/UNCERTAINTY.md");
    for word in LOSSINESS_VOCABULARY
        .iter()
        .chain(REVERSIBILITY_VOCABULARY.iter())
    {
        assert!(
            mapping.contains(word),
            "docs/mapping.md misses vocabulary `{word}`"
        );
        assert!(
            uncertainty.contains(word),
            "docs/UNCERTAINTY.md misses vocabulary `{word}`"
        );
    }
}

#[test]
fn cli_doc_and_readme_cover_every_subcommand() {
    let help = Command::new(env!("CARGO_BIN_EXE_cfp"))
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&help.stdout).to_string();
    let commands: Vec<&str> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|c| *c != "help")
        .collect();
    assert!(commands.len() >= 12, "{commands:?}");
    let cli = read("docs/CLI.md");
    let readme = read("README.md");
    for command in commands {
        assert!(
            cli.contains(&format!("cfp {command}")),
            "docs/CLI.md misses `cfp {command}`"
        );
        assert!(
            readme.contains(&format!("`{command}`")),
            "README misses `{command}`"
        );
    }
}

#[test]
fn architecture_table_names_every_module() {
    let architecture = read("docs/architecture.md");
    for entry in fs::read_dir(root().join("src")).unwrap() {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        if stem == "lib" || stem == "main" {
            continue;
        }
        assert!(
            architecture.contains(&format!("| `{stem}` |")),
            "docs/architecture.md misses module `{stem}`"
        );
    }
}

#[test]
fn readme_pack_table_matches_the_packs_directory() {
    let readme = read("README.md");
    let mut count = 0;
    for entry in fs::read_dir(root().join("packs")).unwrap() {
        let dir = entry.unwrap().path();
        if !dir.join("proposal.json").is_file() {
            continue;
        }
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            readme.contains(&format!("`packs/{name}`")),
            "README misses pack {name}"
        );
        count += 1;
    }
    assert!(count >= 12);
}

#[test]
fn threat_model_test_owners_exist() {
    let threat = read("docs/THREAT-MODEL.md");
    let sources = rust_sources();
    let mut checked = 0;
    for line in threat.lines().filter(|l| l.starts_with('|')) {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if cells.len() < 5 {
            continue;
        }
        for token in cells[3].split(',') {
            let token = token.trim().trim_matches('`');
            let name = token.rsplit("::").next().unwrap_or(token);
            let looks_like_test = name.contains('_')
                && !name.contains(' ')
                && !name.contains('/')
                && !name.contains('.')
                && name != "*";
            if looks_like_test {
                assert!(
                    sources.contains(&format!("fn {name}(")),
                    "docs/THREAT-MODEL.md names missing test `{name}`"
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 15,
        "expected many named test owners, found {checked}"
    );
}

#[test]
fn stale_claims_do_not_return() {
    let banned: &[(&str, &str)] = &[
        ("docs/PROVIDER-BOUNDARY.md", "would_send: []"),
        ("docs/UNCERTAINTY.md", "three packs"),
        (
            "docs/architecture.md",
            "rename / default / enum_map execution",
        ),
        ("README.md", "not_reversible"),
        ("docs/mapping.md", "not_reversible"),
    ];
    for (file, phrase) in banned {
        assert!(!read(file).contains(phrase), "{file} still says `{phrase}`");
    }
    // The docs must keep the disabled-provider and host-coverage statements.
    let readme = read("README.md");
    assert!(readme.contains("not_enabled") || readme.contains("cannot send data"));
    assert!(
        read("docs/HOST-MATRIX.md").contains("unexecuted"),
        "docs/HOST-MATRIX.md must keep the coverage limits visible"
    );
}

#[test]
fn control_documents_stay_out_of_the_export_allowlist() {
    let export = read("scripts/export.ps1");
    let start = export
        .find("$Allow = @(")
        .expect("export.ps1 has an allowlist");
    let end = start + export[start..].find(')').unwrap();
    let allowed: Vec<&str> = export[start..end].split('"').skip(1).step_by(2).collect();
    assert_eq!(
        allowed,
        [
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
            "action.yml",
        ],
        "the export allowlist is exactly the public entries"
    );
    for generic in ["-notcontains $name", "proof", "skills", "node_modules"] {
        assert!(
            export.contains(generic),
            "export.ps1 lacks the generic check for {generic}"
        );
    }
    let ignore = read(".gitignore");
    for path in ["/target", "/dist", "/proof", "/out"] {
        assert!(
            ignore.lines().any(|l| l.trim() == path),
            ".gitignore misses {path}"
        );
    }
    let _ = Path::new("");
}

#[test]
fn diff_depth_and_change_classes_are_documented() {
    let schema: serde_json::Value =
        serde_json::from_str(&read("schemas/v1/diff.schema.json")).unwrap();
    let cli = read("docs/CLI.md");
    for pointer in [
        "/$defs/change/properties/kind/enum",
        "/$defs/compatibility/enum",
    ] {
        let values = schema
            .pointer(pointer)
            .and_then(serde_json::Value::as_array)
            .unwrap_or_else(|| panic!("diff schema has no enum at {pointer}"));
        for value in values {
            let word = format!("`{}`", value.as_str().unwrap());
            assert!(cli.contains(&word), "docs/CLI.md does not explain {word}");
        }
    }
    let matrix = read("docs/supported-matrix.md");
    let row = matrix
        .lines()
        .find(|line| line.starts_with("| Diff depth"))
        .expect("docs/supported-matrix.md has no `Diff depth` row");
    for needed in ["properties", "items", "$ref", "unknown"] {
        assert!(row.contains(needed), "Diff depth row omits {needed}: {row}");
    }
}

#[test]
fn diff_traceability_fields_are_documented() {
    let schema: serde_json::Value =
        serde_json::from_str(&read("schemas/v1/diff.schema.json")).unwrap();
    let cli = read("docs/CLI.md");
    for field in ["moved_operations", "affected_schemas", "imported_evidence"] {
        assert!(
            schema["properties"].get(field).is_some(),
            "diff schema lacks {field}"
        );
        assert!(
            cli.contains(&format!("`{field}`")),
            "docs/CLI.md does not explain `{field}`"
        );
    }
    for word in [
        "`source_line`",
        "`destination_line`",
        "`json_key_path`",
        "`line_unavailable`",
        "`level`",
    ] {
        assert!(cli.contains(word), "docs/CLI.md does not explain {word}");
    }
    let matrix = read("docs/supported-matrix.md");
    for row in [
        "| Moved operations",
        "| Affected component schemas",
        "| oasdiff JSON import",
        "| Line numbers",
    ] {
        assert!(
            matrix.lines().any(|l| l.starts_with(row)),
            "docs/supported-matrix.md has no row starting {row:?}"
        );
    }
}

#[test]
fn diff_notes_and_unreadable_bodies_are_documented() {
    let schema: serde_json::Value =
        serde_json::from_str(&read("schemas/v1/diff.schema.json")).unwrap();
    assert!(
        schema["$defs"]["bodyDiff"]["properties"]
            .get("unsupported")
            .is_none(),
        "bodies must not repeat the document's notes"
    );
    let cli = read("docs/CLI.md");
    for needed in [
        "each note is emitted once",
        "`recursive_reference`",
        "derives no `added_properties`, `removed_properties` or `changed_properties`",
    ] {
        assert!(cli.contains(needed), "docs/CLI.md lacks {needed:?}");
    }
}

#[test]
fn review_record_history_chain_and_supersession_are_documented() {
    let cli = read("docs/CLI.md");
    for needed in [
        "--review <file>",
        "--verify-chain",
        "`cfp supersede",
        "`prev`",
        "`superseded`",
        "not signatures",
    ] {
        assert!(cli.contains(needed), "docs/CLI.md lacks {needed:?}");
    }
    let approval = read("docs/approval.md");
    for needed in ["review record", "hash-chained history", "## Supersession"] {
        assert!(
            approval.contains(needed),
            "docs/approval.md lacks {needed:?}"
        );
    }
    let quickstart = read("scripts/quickstart.ps1");
    assert!(
        quickstart.contains("--review"),
        "the README quickstart must pass the review record to approve"
    );
}

/// The quoted strings of a TypeScript union or array between two markers.
fn quoted_between(text: &str, start: &str, end: &str) -> Vec<String> {
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("{start} not found"))
        + start.len();
    let to = from + text[from..].find(end).unwrap();
    text[from..to]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

#[test]
fn web_types_match_the_schema_and_the_html_report() {
    let ts = read("web/src/report.ts");
    let schema: serde_json::Value =
        serde_json::from_str(&read("schemas/v1/migration.schema.json")).unwrap();
    let lifecycle: Vec<String> = schema["properties"]["lifecycle"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        quoted_between(&ts, "export type Lifecycle =", ";"),
        lifecycle
    );

    let pack = root().join("packs/rename-user-id");
    let report = contract_flightplan::report::build_report(
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let html = contract_flightplan::report::render_html(&report);
    let sections: Vec<String> = html
        .split("<section id=\"")
        .skip(1)
        .map(|s| s[..s.find('"').unwrap()].to_string())
        .collect();
    assert_eq!(quoted_between(&ts, "export const NAV = [", "]"), sections);
}

/// The `| code | meaning |` rows of the first table under `## Exit codes`.
fn exit_code_rows(doc: &str, name: &str) -> Vec<(String, String)> {
    let start = doc
        .find("## Exit codes")
        .unwrap_or_else(|| panic!("{name} has no exit-code section"));
    doc[start..]
        .lines()
        .skip(1)
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| {
            let cells: Vec<&str> = l.trim_matches('|').split('|').map(str::trim).collect();
            (cells.len() == 2 && cells[0].parse::<u8>().is_ok())
                .then(|| (cells[0].to_string(), cells[1].to_string()))
        })
        .collect()
}

#[test]
fn readme_exit_codes_match_the_cli_doc() {
    let readme = exit_code_rows(&read("README.md"), "README.md");
    let cli = exit_code_rows(&read("docs/CLI.md"), "docs/CLI.md");
    assert_eq!(
        readme, cli,
        "README.md and docs/CLI.md disagree on exit codes"
    );
    let codes: Vec<&str> = cli.iter().map(|(c, _)| c.as_str()).collect();
    assert_eq!(codes, ["0", "1", "2", "3"]);
    // The classes the code maps (src/error.rs exit_code) are all named.
    for (code, word) in [
        ("1", "ambiguity"),
        ("1", "history"),
        ("1", "digest"),
        ("3", "type-check"),
        ("3", "parse"),
        ("3", "I/O"),
    ] {
        let meaning = &cli.iter().find(|(c, _)| c == code).unwrap().1;
        assert!(
            meaning.contains(word),
            "exit {code} does not mention {word}: {meaning}"
        );
    }
}

#[test]
fn every_cli_flag_is_documented_on_its_command_line() {
    let bin = env!("CARGO_BIN_EXE_cfp");
    let help = Command::new(bin).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&help.stdout).to_string();
    let commands: Vec<String> = help
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|c| *c != "help")
        .map(str::to_string)
        .collect();
    let cli = read("docs/CLI.md");
    let mut missing = Vec::new();
    for command in &commands {
        let out = Command::new(bin)
            .args([command, "--help"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        // The documented usage lines of this command: backticked `cfp <command> ...`.
        let usage: Vec<&str> = cli
            .split('`')
            .filter(|span| span.starts_with(&format!("cfp {command} ")))
            .collect();
        for flag in text
            .split_whitespace()
            .filter(|w| w.starts_with("--") && w.len() > 2)
            .map(|w| w.trim_end_matches([',', '.', ':', '`', ')']))
            .filter(|w| *w != "--help" && *w != "--version")
        {
            let flag = flag.split('=').next().unwrap().split('<').next().unwrap();
            if !usage.iter().any(|u| u.contains(flag)) {
                missing.push(format!("cfp {command} {flag}"));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(
        missing.is_empty(),
        "flags not on a documented usage line: {missing:#?}"
    );
}

#[test]
fn uncertainty_doc_lists_every_outcome_name() {
    let source = read("src/error.rs");
    let start = source
        .find("pub fn outcome_name")
        .expect("outcome_name in src/error.rs");
    let body = &source[start..start + source[start..].find("\n    }\n").unwrap()];
    let names: Vec<&str> = body
        .lines()
        .filter_map(|l| l.split("=> \"").nth(1))
        .map(|rest| rest.trim_end_matches(['"', ',']))
        .collect();
    assert!(names.len() >= 20, "{names:?}");
    let doc = read("docs/UNCERTAINTY.md");
    let missing: Vec<&&str> = names
        .iter()
        .filter(|n| !doc.contains(&format!("`{n}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/UNCERTAINTY.md does not name: {missing:?}"
    );
}

#[test]
fn threat_model_names_an_owner_for_every_listed_threat() {
    let threat = read("docs/THREAT-MODEL.md");
    let owners = [
        "nested_property_type_change_is_detected_with_pointer",
        "catalog_path_with_parent_traversal_is_refused_at_verify_and_leaks_nothing",
        "file_outside_the_fixtures_folder_changed_after_approval_cannot_pass_verify",
        "junction_added_after_approval_is_refused_at_verify",
        "job_summary_counts_failures_and_cannot_be_forged_by_the_reviewer",
        "html_report_opens_with_the_verdict",
        "duplicate_key_in_proposal_is_a_parse_error_naming_the_key",
        "unknown_top_level_proposal_field_is_refused",
        "moved_operation_is_classified_not_added_removed",
        "pointers_are_rfc6901_escaped",
        "recursive_reference_note_is_emitted_once_with_its_location",
        "allof_body_reports_unknown_and_derives_no_removals",
        "oasdiff_array_shape_is_ingested_with_level",
        "readme_exit_codes_match_the_cli_doc",
        "approve_without_review_record_fails",
        "edited_reviewer_or_emptied_history_is_refused_at_verify",
        "approve_refuses_a_utf16_fixture_and_names_it",
        "corpus_two_runs_identical",
        "malformed_proposal_is_quarantined_with_diagnostics",
        "schema_validation_enforces_numeric_and_string_bounds",
    ];
    let missing: Vec<&&str> = owners
        .iter()
        .filter(|o| !threat.contains(&format!("`{o}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "docs/THREAT-MODEL.md names no row for: {missing:#?}"
    );
}
