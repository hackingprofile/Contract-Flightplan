//! `cfp report` takes its verdict only from its own verification run: with
//! `--migration` it needs `--proposal` and `--fixtures` and re-runs verify, and a
//! `--verify` file is refused unless it matches that fresh run exactly.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-binding-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn pack(name: &str) -> PathBuf {
    root().join("packs").join(name)
}

/// Verify a shipped pack against its shipped approval; the verify JSON lands in `out`.
fn verify_pack(name: &str, out: &Path) {
    let p = pack(name);
    let r = run(&[
        "verify",
        "--migration",
        s(&p.join("approval.migration.json")),
        "--proposal",
        s(&p.join("proposal.json")),
        "--source",
        s(&p.join("base.openapi.yaml")),
        "--destination",
        s(&p.join("next.openapi.yaml")),
        "--fixtures",
        s(&p.join("fixtures")),
        "--out",
        s(out),
    ]);
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
}

/// Render the report for pack `name` with the given verify file. With
/// `with_migration` the pack's migration, proposal and fixtures are given.
fn report(name: &str, verify: &Path, dir: &Path, with_migration: bool) -> Output {
    let p = pack(name);
    let migration = p.join("approval.migration.json");
    let (source, destination) = (p.join("base.openapi.yaml"), p.join("next.openapi.yaml"));
    let (html, json, md) = (dir.join("r.html"), dir.join("r.json"), dir.join("r.md"));
    let mut args = vec![
        "report",
        "--source",
        s(&source),
        "--destination",
        s(&destination),
        "--verify",
        s(verify),
        "--html",
        s(&html),
        "--json",
        s(&json),
        "--md",
        s(&md),
    ];
    let (proposal, fixtures) = (p.join("proposal.json"), p.join("fixtures"));
    if with_migration {
        args.extend([
            "--migration",
            s(&migration),
            "--proposal",
            s(&proposal),
            "--fixtures",
            s(&fixtures),
        ]);
    }
    run(&args)
}

fn edit_json(path: &Path, f: impl FnOnce(&mut Value)) {
    let mut v: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    f(&mut v);
    fs::write(path, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

fn assert_refused(out: &Output, dir: &Path, needle: &str) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(3), "stderr: {err}");
    assert!(err.contains(needle), "stderr must name {needle:?}: {err}");
    assert!(!dir.join("r.html").exists(), "no report may be written");
    assert!(!dir.join("r.md").exists(), "no report may be written");
}

#[test]
fn report_refuses_a_verify_result_from_another_pack() {
    let dir = scratch("other-pack");
    let verify = dir.join("verify.json");
    // rename-user-id has 7 fixtures; default-status has its own migration.
    verify_pack("rename-user-id", &verify);
    let out = report("default-status", &verify, &dir, true);
    assert_refused(&out, &dir, "does not match a fresh verification run");
    // Without --migration there is no fresh run to compare with: refused.
    let out = report("default-status", &verify, &dir, false);
    assert_refused(&out, &dir, "--migration");
}

#[test]
fn report_refuses_a_verify_result_with_another_migration_id() {
    let dir = scratch("other-id");
    let verify = dir.join("verify.json");
    verify_pack("default-status", &verify);
    edit_json(&verify, |v| v["migration_id"] = "wallet".into());
    let out = report("default-status", &verify, &dir, true);
    assert_refused(&out, &dir, "migration id");
}

#[test]
fn report_refuses_a_verify_result_with_other_bound_digests() {
    let dir = scratch("other-digest");
    let verify = dir.join("verify.json");
    verify_pack("default-status", &verify);
    edit_json(&verify, |v| {
        v["approval"]["bound_digests"]["rules"] =
            "sha256:0000000000000000000000000000000000000000000000000000000000000000".into()
    });
    let out = report("default-status", &verify, &dir, true);
    assert_refused(&out, &dir, "rules");
}

/// The stored flags are compared too: a verify file whose only edit is a
/// `passed` flag or the run's `stable` is refused, not silently corrected.
#[test]
fn report_refuses_a_verify_result_with_edited_flags() {
    for (case, edit) in [
        (
            "passed",
            (|v: &mut Value| v["outcomes"][0]["passed"] = false.into()) as fn(&mut Value),
        ),
        ("stable", |v: &mut Value| v["stable"] = false.into()),
    ] {
        let dir = scratch(case);
        let verify = dir.join("verify.json");
        verify_pack("rename-user-id", &verify);
        edit_json(&verify, edit);
        let out = report("rename-user-id", &verify, &dir, true);
        assert_refused(&out, &dir, "does not match a fresh verification run");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains(case), "{case}: {err}");
    }
}

/// Setup: a copy of rename-user-id whose catalog expects
/// `request-positive` to collide, approved and verified. The honest verify
/// file fails one fixture; two forgeries keep every bound digest intact:
/// (a) the failing outcome's `actual` set to its `expect`, (b) the failing
/// outcome deleted.
struct FailingRun {
    dir: PathBuf,
    pack: PathBuf,
    migration: PathBuf,
    honest: PathBuf,
    forged_actual: PathBuf,
    forged_drop: PathBuf,
}

fn failing_run(name: &str) -> FailingRun {
    let dir = scratch(name);
    let p = dir.join("P");
    copy_dir(&pack("rename-user-id"), &p);
    edit_json(&p.join("fixtures").join("catalog.json"), |v| {
        let items = v["items"].as_array_mut().unwrap();
        let item = items
            .iter_mut()
            .find(|i| i["id"] == "request-positive")
            .unwrap();
        item["expect"] = "collision".into();
    });
    let (review, migration, honest) = (
        dir.join("review.json"),
        dir.join("m.json"),
        dir.join("v.json"),
    );
    let common = |cmd: &'static str| {
        vec![
            cmd.to_string(),
            "--proposal".into(),
            s(&p.join("proposal.json")).into(),
            "--source".into(),
            s(&p.join("base.openapi.yaml")).into(),
            "--destination".into(),
            s(&p.join("next.openapi.yaml")).into(),
        ]
    };
    let mut args = common("review");
    args.extend(["--out".into(), s(&review).into()]);
    let r = Command::new(bin()).args(&args).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let mut args = common("approve");
    args.extend([
        "--fixtures".into(),
        s(&p.join("fixtures")).into(),
        "--review".into(),
        s(&review).into(),
        "--reviewer".into(),
        "reviewer".into(),
        "--out".into(),
        s(&migration).into(),
    ]);
    let r = Command::new(bin()).args(&args).output().unwrap();
    assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
    let mut args = common("verify");
    args.extend([
        "--migration".into(),
        s(&migration).into(),
        "--fixtures".into(),
        s(&p.join("fixtures")).into(),
        "--out".into(),
        s(&honest).into(),
    ]);
    let r = Command::new(bin()).args(&args).output().unwrap();
    assert_eq!(r.status.code(), Some(1), "the honest run fails one fixture");
    let forged_actual = dir.join("forged-actual.json");
    fs::copy(&honest, &forged_actual).unwrap();
    edit_json(&forged_actual, |v| {
        for o in v["outcomes"].as_array_mut().unwrap() {
            if o["passed"] == false {
                o["actual"] = o["expect"].clone();
                o["passed"] = true.into();
            }
        }
        v["stable"] = true.into();
    });
    let forged_drop = dir.join("forged-drop.json");
    fs::copy(&honest, &forged_drop).unwrap();
    edit_json(&forged_drop, |v| {
        v["outcomes"]
            .as_array_mut()
            .unwrap()
            .retain(|o| o["passed"] == true);
        v["stable"] = true.into();
    });
    FailingRun {
        dir,
        pack: p,
        migration,
        honest,
        forged_actual,
        forged_drop,
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `cfp report` for a [`FailingRun`], writing into `out`; `fixtures` adds
/// `--fixtures`, `verify` adds `--verify`.
fn report_run(run: &FailingRun, out: &Path, fixtures: bool, verify: Option<&Path>) -> Output {
    fs::create_dir_all(out).unwrap();
    let p = &run.pack;
    let mut args: Vec<String> = vec![
        "report".into(),
        "--source".into(),
        s(&p.join("base.openapi.yaml")).into(),
        "--destination".into(),
        s(&p.join("next.openapi.yaml")).into(),
        "--proposal".into(),
        s(&p.join("proposal.json")).into(),
        "--migration".into(),
        s(&run.migration).into(),
        "--html".into(),
        s(&out.join("r.html")).into(),
        "--json".into(),
        s(&out.join("r.json")).into(),
        "--md".into(),
        s(&out.join("r.md")).into(),
        "--job-summary".into(),
        s(&out.join("s.md")).into(),
    ];
    if fixtures {
        args.extend(["--fixtures".into(), s(&p.join("fixtures")).into()]);
    }
    if let Some(v) = verify {
        args.extend(["--verify".into(), s(v).into()]);
    }
    Command::new(bin()).args(&args).output().unwrap()
}

fn assert_honest_failing_summary(out: &Output, dir: &Path) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary = fs::read_to_string(dir.join("s.md")).unwrap();
    assert!(summary.contains("- stable: no"), "{summary}");
    assert!(summary.contains("- fixtures: 6/7 passed"), "{summary}");
    assert!(
        summary.contains("- failing fixtures: request-positive"),
        "{summary}"
    );
}

/// A verdict needs a fresh verification run, so a report that carries a
/// migration without `--fixtures` is refused instead of reading a verdict
/// from a file.
#[test]
fn report_with_a_migration_but_no_fixtures_is_refused() {
    let run = failing_run("no-fixtures");
    let out_dir = run.dir.join("out");
    let out = report_run(&run, &out_dir, false, Some(&run.forged_actual));
    assert_refused(&out, &out_dir, "--fixtures");
    let out = report_run(&run, &out_dir, false, None);
    assert_refused(&out, &out_dir, "--fixtures");
}

/// The verdict comes from the fresh run; a verify file only has to agree.
#[test]
fn report_takes_the_verdict_from_a_fresh_run() {
    let run = failing_run("fresh-run");
    let without = run.dir.join("without-verify");
    assert_honest_failing_summary(&report_run(&run, &without, true, None), &without);
    let honest = run.dir.join("honest");
    assert_honest_failing_summary(&report_run(&run, &honest, true, Some(&run.honest)), &honest);
}

/// Forgery (a): the failing outcome's `actual` set to its `expect`.
#[test]
fn report_refuses_a_verify_file_whose_actual_was_edited() {
    let run = failing_run("forged-actual");
    let out_dir = run.dir.join("out");
    let out = report_run(&run, &out_dir, true, Some(&run.forged_actual));
    assert_refused(&out, &out_dir, "does not match a fresh verification run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("request-positive"), "{err}");
    assert!(!out_dir.join("s.md").exists());
}

/// Forgery (b): the failing outcome deleted.
#[test]
fn report_refuses_a_verify_file_with_a_deleted_outcome() {
    let run = failing_run("deleted-outcome");
    let out_dir = run.dir.join("out");
    let out = report_run(&run, &out_dir, true, Some(&run.forged_drop));
    assert_refused(&out, &out_dir, "does not match a fresh verification run");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("7") && err.contains("6"), "{err}");
    assert!(!out_dir.join("s.md").exists());
}

/// A hand-edited verify result whose outcome path is a URL: it differs from the
/// fresh run, so the report is refused before anything is written and the URL
/// never reaches SARIF.
#[test]
fn report_refuses_an_outcome_path_that_is_not_a_bound_fixture() {
    let dir = scratch("outcome-url");
    let verify = dir.join("verify.json");
    verify_pack("rename-user-id", &verify);
    edit_json(&verify, |v| {
        v["outcomes"][0]["path"] = "https://evil.example/x".into();
    });
    let p = pack("rename-user-id");
    let sarif = dir.join("r.sarif");
    let out = run(&[
        "report",
        "--source",
        s(&p.join("base.openapi.yaml")),
        "--destination",
        s(&p.join("next.openapi.yaml")),
        "--migration",
        s(&p.join("approval.migration.json")),
        "--proposal",
        s(&p.join("proposal.json")),
        "--fixtures",
        s(&p.join("fixtures")),
        "--verify",
        s(&verify),
        "--html",
        s(&dir.join("r.html")),
        "--json",
        s(&dir.join("r.json")),
        "--md",
        s(&dir.join("r.md")),
        "--sarif",
        s(&sarif),
    ]);
    assert_refused(&out, &dir, "has path \"https://evil.example/x\"");
    assert!(!sarif.exists(), "no SARIF may be written");
}
