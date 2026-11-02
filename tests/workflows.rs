//! Lint for the shipped GitHub Actions surface. These files are not executed
//! from this tree (no hosted run is made); the lint keeps the
//! properties the docs advertise true: SHA-pinned actions, least-privilege
//! permissions, no expression injection inside `run:` blocks, and an action
//! that resolves its own binary instead of assuming the consumer's checkout.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load_yaml(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap();
    yaml_serde::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn workflow_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(root().join(".github/workflows"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    files
}

fn steps(job: &Value) -> Vec<&Value> {
    job["steps"]
        .as_array()
        .map(|s| s.iter().collect())
        .unwrap_or_default()
}

fn assert_sha_pinned(uses: &str, where_: &str) {
    if uses.starts_with("./") {
        return;
    }
    let (_, reference) = uses
        .split_once('@')
        .unwrap_or_else(|| panic!("{where_}: `{uses}` has no version"));
    assert!(
        reference.len() == 40 && reference.chars().all(|c| c.is_ascii_hexdigit()),
        "{where_}: `{uses}` must be pinned to a full commit SHA"
    );
}

fn assert_run_has_no_expressions(step: &Value, where_: &str) {
    if let Some(run) = step["run"].as_str() {
        assert!(
            !run.contains("${{"),
            "{where_}: `run:` must not interpolate expressions; pass values through `env:`"
        );
    }
}

#[test]
fn every_workflow_pins_actions_and_declares_read_permissions() {
    for file in workflow_files() {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let doc = load_yaml(&file);
        assert!(doc.get("on").is_some(), "{name}: missing triggers");
        let top = doc.get("permissions");
        assert!(top.is_some(), "{name}: declare top-level permissions");
        if let Some(map) = top.and_then(Value::as_object) {
            for (scope, level) in map {
                assert!(
                    level == "read" || level == "none",
                    "{name}: top-level permission {scope} must be read or none"
                );
            }
        } else {
            assert_eq!(
                top.unwrap(),
                "read-all",
                "{name}: unexpected permissions shape"
            );
        }
        let jobs = doc["jobs"]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: no jobs"));
        for (job_name, job) in jobs {
            assert!(
                job.get("timeout-minutes").is_some(),
                "{name}/{job_name}: set timeout-minutes"
            );
            for (index, step) in steps(job).iter().enumerate() {
                let where_ = format!("{name}/{job_name}/step {index}");
                if let Some(uses) = step["uses"].as_str() {
                    assert_sha_pinned(uses, &where_);
                    if uses.starts_with("actions/checkout@") {
                        assert_eq!(
                            step["with"]["persist-credentials"],
                            Value::Bool(false),
                            "{where_}: checkout must not persist credentials"
                        );
                    }
                }
                assert_run_has_no_expressions(step, &where_);
            }
        }
    }
}

#[test]
fn no_workflow_reaches_a_model_provider_or_publishes() {
    for file in workflow_files() {
        let text = fs::read_to_string(&file).unwrap().to_ascii_lowercase();
        for forbidden in [
            "openai",
            "anthropic",
            "api.github.com/repos",
            "gh release",
            "cargo publish",
            "npm publish",
            "docker push",
        ] {
            assert!(
                !text.contains(forbidden),
                "{}: contains `{forbidden}`",
                file.display()
            );
        }
    }
}

#[test]
fn action_resolves_its_own_checkout_and_passes_inputs_through_env() {
    let action = load_yaml(&root().join("action.yml"));
    assert_eq!(action["runs"]["using"], "composite");
    for required in ["source", "destination"] {
        assert_eq!(
            action["inputs"][required]["required"],
            Value::Bool(true),
            "input {required} must be required"
        );
    }
    let steps = action["runs"]["steps"].as_array().unwrap();
    assert!(!steps.is_empty());
    let mut saw_action_path = false;
    for (index, step) in steps.iter().enumerate() {
        let where_ = format!("action.yml/step {index}");
        assert_eq!(
            step["shell"], "bash",
            "{where_}: composite steps must set shell"
        );
        assert_run_has_no_expressions(step, &where_);
        if let Some(env) = step["env"].as_object() {
            for value in env.values() {
                if value
                    .as_str()
                    .is_some_and(|v| v.contains("github.action_path"))
                {
                    saw_action_path = true;
                }
            }
        }
        let run = step["run"].as_str().unwrap_or_default();
        assert!(
            !run.contains("target/release/cfp\"") || run.contains("CFP_ACTION_PATH"),
            "{where_}: the binary must come from the action checkout"
        );
    }
    assert!(saw_action_path, "action must build from github.action_path");
}

#[test]
fn workflow_that_uses_the_local_action_forwards_inputs() {
    let doc = load_yaml(&root().join(".github/workflows/contract-flightplan.yml"));
    let job = &doc["jobs"]["review"];
    let local = steps(job)
        .into_iter()
        .find(|s| s["uses"] == "./")
        .expect("workflow exercises the checked-in action");
    for key in ["source", "destination", "out-dir"] {
        assert!(
            local["with"].get(key).is_some(),
            "action input {key} not forwarded"
        );
    }
    let dispatch = &doc["on"]["workflow_dispatch"]["inputs"];
    assert!(dispatch.get("source").is_some() && dispatch.get("destination").is_some());
}

#[test]
fn dependabot_covers_every_ecosystem_in_the_tree() {
    let doc = load_yaml(&root().join(".github/dependabot.yml"));
    let ecosystems: Vec<String> = doc["updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["package-ecosystem"].as_str().unwrap().to_string())
        .collect();
    for expected in ["cargo", "github-actions", "npm"] {
        assert!(
            ecosystems.contains(&expected.to_string()),
            "dependabot misses {expected}"
        );
    }
}

// The YAML lints above do not look at what a step runs, so a step could call a file that does not
// exist. Every `node <file>` in a workflow must name a file in the tree, and the web job runs the locked npm
// install and the package's own type check.
#[test]
fn workflow_commands_name_files_that_exist_and_web_job_type_checks() {
    for file in workflow_files() {
        let wf = load_yaml(&file);
        for (name, job) in wf["jobs"].as_object().unwrap() {
            for step in steps(job) {
                let Some(run) = step["run"].as_str() else {
                    continue;
                };
                let dir = root().join(step["working-directory"].as_str().unwrap_or("."));
                for word in run.split_whitespace().collect::<Vec<_>>().windows(2) {
                    if word[0] == "node" && !word[1].starts_with('-') {
                        assert!(
                            dir.join(word[1]).is_file(),
                            "{}: job {name} runs `node {}`, which is not in the tree",
                            file.display(),
                            word[1]
                        );
                    }
                }
            }
        }
    }
    let ci = load_yaml(&root().join(".github/workflows/ci.yml"));
    let runs: Vec<&str> = steps(&ci["jobs"]["web"])
        .iter()
        .filter(|s| s["working-directory"].as_str() == Some("web"))
        .filter_map(|s| s["run"].as_str())
        .collect();
    assert!(
        runs.iter().any(|r| r.starts_with("npm ci")),
        "web job must install from the lockfile: {runs:?}"
    );
    assert!(
        runs.contains(&"npm test"),
        "web job must run the package's type check: {runs:?}"
    );
    assert!(root().join("web/package-lock.json").is_file());
}

// Cargo honours a caller's CARGO_TARGET_DIR, so running a fixed `$CFP_ACTION_PATH/target/release/cfp`
// could pick a stale binary. The build must name its target directory explicitly and the run must use the binary it reports.
#[test]
fn action_builds_and_runs_from_one_target_directory() {
    let action = load_yaml(&root().join("action.yml"));
    let steps = action["runs"]["steps"].as_array().unwrap();
    let build = steps
        .iter()
        .find(|s| s["id"] == "build")
        .expect("a build step with id build");
    let run = build["run"].as_str().unwrap();
    assert!(
        run.contains("CARGO_TARGET_DIR"),
        "the build must honour a caller's CARGO_TARGET_DIR"
    );
    assert!(
        run.contains("--target-dir \"$CFP_TARGET\""),
        "the build must name its target directory"
    );
    assert!(
        run.contains("bin=$CFP_TARGET/release/cfp"),
        "the build must report the binary it made"
    );
    let report = steps.iter().find(|s| s["id"] == "report").unwrap();
    assert_eq!(report["env"]["CFP_BIN"], "${{ steps.build.outputs.bin }}");
    assert!(
        !report["run"].as_str().unwrap().contains("target/release"),
        "the run must not assume a target path"
    );
}

/// Scorecard needs these read permissions on its job (without them it fails
/// with "Resource not accessible by integration").
#[test]
fn scorecard_job_has_the_read_permissions_it_needs() {
    let workflow = load_yaml(&root().join(".github/workflows/scorecard.yml"));
    let permissions = &workflow["jobs"]["analysis"]["permissions"];
    for (scope, level) in [
        ("contents", "read"),
        ("security-events", "write"),
        ("id-token", "write"),
        ("issues", "read"),
        ("pull-requests", "read"),
        ("checks", "read"),
        ("actions", "read"),
    ] {
        assert_eq!(
            permissions[scope], level,
            "scorecard job {scope}: {permissions}"
        );
    }
}
