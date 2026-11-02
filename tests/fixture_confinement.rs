//! A fixture catalog can only name files inside the fixtures folder, and every
//! byte verify reads must be covered by a digest the approval bound. Paths that
//! leave the folder, and links (junctions, symlinks) whose targets are not
//! bound, are refused at approve and at verify.

use std::fs;
use std::path::{Path, PathBuf};

use contract_flightplan::approve::{approval_digest, approve, load_migration, write_json};
use contract_flightplan::digest::file_digest;
use contract_flightplan::verify::verify_paths_report;

/// The review record `cfp review --out` writes for a pack's proposal, saved
/// to a fresh temporary file (approve requires one).
fn review_record(pack: &Path) -> PathBuf {
    let review = contract_flightplan::verify::review_proposal(
        &pack.join("proposal.json"),
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
    )
    .unwrap();
    let path = std::env::temp_dir().join(format!(
        "cfp-review-record-{}-{}.json",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
            + "-"
            + &{
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_string()
            }
    ));
    contract_flightplan::approve::write_json(&path, &review).unwrap();
    path
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A private scratch folder holding a copy of the rename pack under `pack/`.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "cfp-confine-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
            .to_string()
            + "-"
            + &{
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    .to_string()
            }
    ));
    let _ = fs::remove_dir_all(&dir);
    copy_dir(&root().join("packs/rename-user-id"), &dir.join("pack"));
    dir
}

fn add_catalog_item(fixtures: &Path, item_path: &str) {
    let catalog_path = fixtures.join("catalog.json");
    let mut catalog: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&catalog_path).unwrap()).unwrap();
    let mut item = catalog["items"][0].clone();
    item["id"] = serde_json::json!("outside-item");
    item["path"] = serde_json::json!(item_path);
    catalog["items"].as_array_mut().unwrap().insert(0, item);
    fs::write(
        &catalog_path,
        serde_json::to_string_pretty(&catalog).unwrap(),
    )
    .unwrap();
}

fn approve_to(pack: &Path, out: &Path) -> contract_flightplan::Result<()> {
    let migration = approve(
        &pack.join("proposal.json"),
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        &pack.join("fixtures"),
        &review_record(pack),
        "local-reviewer",
        None,
        Some("2026-09-30T00:00:00Z"),
    )?;
    write_json(out, &migration)
}

fn verify(pack: &Path, migration: &Path) -> contract_flightplan::Result<String> {
    let report = verify_paths_report(
        migration,
        &pack.join("proposal.json"),
        &pack.join("base.openapi.yaml"),
        &pack.join("next.openapi.yaml"),
        &pack.join("fixtures"),
    )?;
    Ok(serde_json::to_string(&report).unwrap())
}

/// Edit the catalog after approval and re-bind its digest inside the
/// migration, recomputing the approved event too, as anyone holding the
/// migration file can do. Only the folder checks at verify stand in the way.
fn add_item_and_rebind(fixtures: &Path, migration: &Path, item_path: &str) {
    let before = file_digest(&fixtures.join("catalog.json")).unwrap();
    add_catalog_item(fixtures, item_path);
    let after = file_digest(&fixtures.join("catalog.json")).unwrap();
    let text = fs::read_to_string(migration).unwrap();
    assert!(text.contains(&before));
    fs::write(migration, text.replace(&before, &after)).unwrap();
    let mut forged = load_migration(migration).unwrap();
    let committed = approval_digest(&forged.approval.bound_digests).unwrap();
    for event in forged.history.iter_mut().filter(|e| e.action == "approved") {
        event.digest = committed.clone();
    }
    write_json(migration, &forged).unwrap();
}

/// Create a directory junction on Windows, a directory symlink elsewhere.
fn link_dir(link: &Path, target: &Path) {
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .arg("/J")
            .arg(link.to_string_lossy().replace('/', "\\"))
            .arg(target.to_string_lossy().replace('/', "\\"))
            .output()
            .unwrap();
        assert!(
            status.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn catalog_path_with_parent_traversal_is_refused_at_approve() {
    let dir = scratch("approve-traversal");
    let pack = dir.join("pack");
    fs::write(
        dir.join("outside-secret.json"),
        r#"{"id": "secret", "name": "x"}"#,
    )
    .unwrap();
    add_catalog_item(&pack.join("fixtures"), "../../outside-secret.json");
    let out = dir.join("migration.json");
    let err = approve_to(&pack, &out).expect_err("a path leaving the fixtures folder");
    assert!(err.to_string().contains("outside-secret.json"), "{err}");
    assert!(err.to_string().contains("fixtures folder"), "{err}");
    assert!(!out.exists());
}

#[test]
fn catalog_path_with_parent_traversal_is_refused_at_verify_and_leaks_nothing() {
    let dir = scratch("verify-traversal");
    let pack = dir.join("pack");
    fs::write(
        dir.join("outside-secret.json"),
        r#"{"id": "outside-secret-value", "name": "x"}"#,
    )
    .unwrap();
    let migration = dir.join("migration.json");
    approve_to(&pack, &migration).unwrap();
    add_item_and_rebind(
        &pack.join("fixtures"),
        &migration,
        "../../outside-secret.json",
    );
    match verify(&pack, &migration) {
        Ok(report) => panic!("verify read outside the fixtures folder: {report}"),
        Err(err) => {
            assert!(!err.to_string().contains("outside-secret-value"), "{err}");
            assert!(err.to_string().contains("fixtures folder"), "{err}");
        }
    }
}

#[test]
fn file_outside_the_fixtures_folder_changed_after_approval_cannot_pass_verify() {
    let dir = scratch("outside-change");
    let pack = dir.join("pack");
    let outside = dir.join("outside.json");
    fs::write(&outside, r#"{"id": "before", "name": "x"}"#).unwrap();
    let migration = dir.join("migration.json");
    approve_to(&pack, &migration).unwrap();
    add_item_and_rebind(&pack.join("fixtures"), &migration, "../../outside.json");
    fs::write(&outside, r#"{"id": "changed-after-approval", "name": "x"}"#).unwrap();
    assert!(
        verify(&pack, &migration).is_err(),
        "content outside the bound digests passed verify"
    );
}

#[test]
fn absolute_and_drive_relative_catalog_paths_are_refused_at_approve() {
    let dir = scratch("absolute");
    let pack = dir.join("pack");
    let outside = dir.join("outside.json");
    fs::write(&outside, r#"{"id": "x", "name": "x"}"#).unwrap();
    let absolute = outside.display().to_string();
    let mut candidates = vec![
        absolute.clone(),
        absolute.replace('\\', "/"),
        "sub/../request.positive.json".to_string(),
        "./request.positive.json".to_string(),
        "request.positive.json:stream".to_string(),
    ];
    if cfg!(windows) {
        candidates.push("C:request.positive.json".into());
        // A device path to a system file, built so no drive path is written here.
        candidates.push(format!(r"\\?\{}:\Windows\win.ini", 'C'));
    }
    for candidate in candidates {
        let fresh = scratch("absolute-case");
        let fresh_pack = fresh.join("pack");
        add_catalog_item(&fresh_pack.join("fixtures"), &candidate);
        let out = fresh.join("migration.json");
        let result = approve_to(&fresh_pack, &out);
        assert!(result.is_err(), "catalog path {candidate:?} was accepted");
        assert!(!out.exists());
    }
    let _ = pack;
}

#[test]
fn junction_inside_the_fixtures_folder_is_refused_at_approve() {
    let dir = scratch("junction-approve");
    let pack = dir.join("pack");
    let target = dir.join("junction-target");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        target.join("j.json"),
        r#"{"id": "through-link", "name": "x"}"#,
    )
    .unwrap();
    link_dir(&pack.join("fixtures").join("linked"), &target);
    add_catalog_item(&pack.join("fixtures"), "linked/j.json");
    let out = dir.join("migration.json");
    let err = approve_to(&pack, &out).expect_err("a link inside the fixtures folder");
    assert!(err.to_string().contains("linked"), "{err}");
    assert!(!out.exists());
}

#[test]
fn junction_added_after_approval_is_refused_at_verify() {
    let dir = scratch("junction-verify");
    let pack = dir.join("pack");
    let migration = dir.join("migration.json");
    approve_to(&pack, &migration).unwrap();
    let target = dir.join("junction-target");
    fs::create_dir_all(&target).unwrap();
    fs::write(
        target.join("j.json"),
        r#"{"id": "through-link", "name": "x"}"#,
    )
    .unwrap();
    link_dir(&pack.join("fixtures").join("linked"), &target);
    add_item_and_rebind(&pack.join("fixtures"), &migration, "linked/j.json");
    let err = verify(&pack, &migration).expect_err("a link inside the fixtures folder");
    assert!(err.to_string().contains("linked"), "{err}");
}

/// On Linux and macOS a file may be named `sub\x.json`; such a name must not
/// alias the nested file `sub/x.json` in the digest keys, so it is refused.
/// (Windows cannot create such a name; there the unit test
/// `approve::tests::fixture_digest_keys_are_unambiguous` covers the case.)
#[cfg(unix)]
#[test]
fn a_backslash_file_name_cannot_share_a_digest_key() {
    let dir = scratch("backslash-name");
    let fixtures = dir.join("fixtures");
    fs::create_dir_all(fixtures.join("sub")).unwrap();
    fs::write(fixtures.join("sub").join("x.json"), "{}").unwrap();
    fs::write(fixtures.join("sub\\x.json"), "{}").unwrap();
    let err = contract_flightplan::approve::digest_fixture_dir(&fixtures).unwrap_err();
    assert!(err.to_string().contains("backslash"), "{err}");
}
