//! Outputs never replace the command's own inputs, never collide with each
//! other, and are never written through a link at the output path.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-outputs-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

fn pack_copy(name: &str) -> PathBuf {
    let dir = scratch(name).join("pack");
    copy_dir(&root().join("packs").join("rename-user-id"), &dir);
    dir
}

fn run(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn refused_unchanged(out: &Output, file: &Path, before: &[u8]) {
    assert_eq!(out.status.code(), Some(3), "{}", stderr(out));
    assert_eq!(
        fs::read(file).unwrap(),
        before,
        "{} was changed",
        file.display()
    );
}

#[test]
fn verify_refuses_to_write_its_report_over_the_migration() {
    let pack = pack_copy("verify-over-migration");
    let migration = pack.join("approval.migration.json");
    let before = fs::read(&migration).unwrap();
    let out = run(&[
        "verify",
        "--migration",
        s(&migration),
        "--proposal",
        s(&pack.join("proposal.json")),
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--fixtures",
        s(&pack.join("fixtures")),
        "--out",
        s(&migration),
    ]);
    refused_unchanged(&out, &migration, &before);
    assert!(stderr(&out).contains("input"), "{}", stderr(&out));
}

#[test]
fn compare_refuses_to_write_over_its_source() {
    let pack = pack_copy("compare-over-source");
    let source = pack.join("base.openapi.yaml");
    let before = fs::read(&source).unwrap();
    // The same file named another way.
    let other_spelling = pack.join(".").join("base.openapi.yaml");
    let out = run(&[
        "compare",
        "--source",
        s(&source),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--out",
        s(&other_spelling),
    ]);
    refused_unchanged(&out, &source, &before);
}

#[test]
fn report_refuses_two_outputs_at_one_path() {
    let pack = pack_copy("report-twice");
    let target = pack.parent().unwrap().join("r.out");
    let out = run(&[
        "report",
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--html",
        s(&target),
        "--json",
        s(&target),
    ]);
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(!target.exists(), "nothing may be written");
}

/// A link at the output path is refused; the file it points to is untouched.
/// Needs a file symlink (Linux and macOS; Windows only with that privilege).
#[test]
fn outputs_are_not_written_through_a_link() {
    let pack = pack_copy("link");
    let victim = pack.parent().unwrap().join("victim.txt");
    fs::write(&victim, b"keep me").unwrap();
    let link = pack.parent().unwrap().join("report.html");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&victim, &link).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_file(&victim, &link).is_ok();
    if !made {
        eprintln!("skipping: this host cannot create a file symlink");
        return;
    }
    let out = run(&[
        "report",
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--html",
        s(&link),
        "--json",
        s(&pack.parent().unwrap().join("r.json")),
    ]);
    refused_unchanged(&out, &victim, b"keep me");
    assert!(stderr(&out).contains("link"), "{}", stderr(&out));
}

/// `cfp compare` on the shipped rename pack, writing the diff to `out`.
fn compare_out(out: &Path) -> Output {
    let pack = root().join("packs").join("rename-user-id");
    run(&[
        "compare",
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--out",
        s(out),
    ])
}

/// Special files at an output path are written through or refused, never
/// replaced by renaming a temporary file over them.
#[cfg(unix)]
mod unix_special_files {
    use super::*;
    use std::os::unix::fs::FileTypeExt;
    use std::time::Duration;

    fn kind(path: &Path) -> fs::FileType {
        fs::symlink_metadata(path).unwrap().file_type()
    }

    /// Whether this process may create files in /dev (it runs as root). Then
    /// a regression would replace the system's /dev/null, so the tests use a
    /// private node with its device numbers instead.
    fn may_write_dev() -> bool {
        let probe = PathBuf::from(format!("/dev/.cfp-probe-{}", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
        {
            Ok(_) => {
                let _ = fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    fn mknod(path: &Path, kind: &str, major: &str, minor: &str) -> bool {
        Command::new("mknod")
            .arg(path)
            .args([kind, major, minor])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// The null device to write to: /dev/null for a normal user, a private
    /// device node as root (so a test can never replace the real one).
    fn null_device() -> Option<PathBuf> {
        if !may_write_dev() {
            return Some(PathBuf::from("/dev/null"));
        }
        if !cfg!(target_os = "linux") {
            return None;
        }
        let node = scratch("fakenull").join("fakenull");
        mknod(&node, "c", "1", "3").then_some(node)
    }

    #[test]
    fn dev_null_as_out_still_works_and_stays_a_device() {
        let Some(null) = null_device() else {
            eprintln!("skipping: root on a host where a private null device cannot be made");
            return;
        };
        let out = compare_out(&null);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(
            kind(&null).is_char_device(),
            "{} must stay a character device",
            null.display()
        );
        // Several outputs may go to the null device in one command.
        let pack = root().join("packs").join("rename-user-id");
        let out = run(&[
            "report",
            "--source",
            s(&pack.join("base.openapi.yaml")),
            "--destination",
            s(&pack.join("next.openapi.yaml")),
            "--html",
            s(&null),
            "--json",
            s(&null),
        ]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(kind(&null).is_char_device());
    }

    #[test]
    fn a_fifo_as_out_stays_a_fifo_and_receives_the_output() {
        let fifo = scratch("fifo").join("out.fifo");
        let made = Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            eprintln!("skipping: mkfifo is not available");
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let reader = fifo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fs::read_to_string(&reader));
        });
        let out = compare_out(&fifo);
        assert!(out.status.success(), "{}", stderr(&out));
        assert!(kind(&fifo).is_fifo(), "the FIFO must not be replaced");
        let text = rx
            .recv_timeout(Duration::from_secs(20))
            .expect("the reader received the output")
            .unwrap();
        assert!(text.contains("\"kind\""), "{text}");
    }

    #[test]
    fn a_socket_as_out_is_refused_and_stays_a_socket() {
        let sock = scratch("sock").join("o.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let out = compare_out(&sock);
        assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
        assert!(stderr(&out).contains("socket"), "{}", stderr(&out));
        assert!(kind(&sock).is_socket(), "the socket must not be replaced");
    }

    #[test]
    fn a_block_device_as_out_is_refused_and_stays_a_device() {
        if !may_write_dev() || !cfg!(target_os = "linux") {
            eprintln!("skipping: making a block device node needs root on Linux");
            return;
        }
        // A private node for a loop device minor that is normally absent.
        let node = scratch("block").join("fakeblock");
        if !mknod(&node, "b", "7", "250") {
            eprintln!("skipping: mknod failed");
            return;
        }
        let out = compare_out(&node);
        assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
        assert!(
            kind(&node).is_block_device(),
            "the node must not be replaced"
        );
    }
}

/// On Windows the NUL device is written to, never renamed over.
#[cfg(windows)]
#[test]
fn nul_as_out_works_on_windows() {
    for nul in ["NUL", "nul", r"\\.\NUL"] {
        let out = compare_out(Path::new(nul));
        assert!(out.status.success(), "{nul}: {}", stderr(&out));
    }
    let pack = root().join("packs").join("rename-user-id");
    let out = run(&[
        "report",
        "--source",
        s(&pack.join("base.openapi.yaml")),
        "--destination",
        s(&pack.join("next.openapi.yaml")),
        "--html",
        "NUL",
        "--json",
        "NUL",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
}

/// Other DOS device names (the console, ports) are refused with a clear message.
#[cfg(windows)]
#[test]
fn other_windows_devices_as_out_are_refused() {
    for device in ["CON", "COM1", "LPT1", "AUX"] {
        let out = compare_out(Path::new(device));
        assert_eq!(out.status.code(), Some(3), "{device}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("device"),
            "{device}: {}",
            stderr(&out)
        );
    }
}

/// Every file under `dir` (relative path, bytes), sorted: a folder is unchanged
/// when two snapshots are equal.
fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(base: &Path, dir: &Path, files: &mut Vec<(String, Vec<u8>)>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, files);
            } else {
                let rel = path.strip_prefix(base).unwrap().display().to_string();
                files.push((rel, fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();
    files
}

fn assert_refused_inside(out: &Output, folder: &Path, before: &[(String, Vec<u8>)], what: &str) {
    assert_eq!(out.status.code(), Some(3), "{what}: {}", stderr(out));
    assert!(stderr(out).contains("inside"), "{what}: {}", stderr(out));
    assert!(
        snapshot(folder) == before,
        "{what}: {} was changed",
        folder.display()
    );
}

/// Outputs inside the fixtures folder, plus a new subfolder: `verify --out`
/// inside `--fixtures` is refused and the folder stays byte-identical.
#[test]
fn verify_refuses_outputs_inside_its_fixtures_folder() {
    let pack = pack_copy("verify-into-fixtures");
    let fixtures = pack.join("fixtures");
    let before = snapshot(&fixtures);
    for target in [
        "catalog.json",
        "request.positive.json",
        "new.json",
        "sub/new.json",
    ] {
        let out = run(&[
            "verify",
            "--migration",
            s(&pack.join("approval.migration.json")),
            "--proposal",
            s(&pack.join("proposal.json")),
            "--source",
            s(&pack.join("base.openapi.yaml")),
            "--destination",
            s(&pack.join("next.openapi.yaml")),
            "--fixtures",
            s(&fixtures),
            "--out",
            s(&fixtures.join(target)),
        ]);
        assert_refused_inside(&out, &fixtures, &before, target);
    }
    assert!(!fixtures.join("sub").exists(), "no folder may be created");
}

/// The same for the other commands that read a fixtures folder.
#[test]
fn approve_report_and_guide_refuse_outputs_inside_the_fixtures_folder() {
    let pack = pack_copy("others-into-fixtures");
    let fixtures = pack.join("fixtures");
    let before = snapshot(&fixtures);
    let (proposal, base, next) = (
        pack.join("proposal.json"),
        pack.join("base.openapi.yaml"),
        pack.join("next.openapi.yaml"),
    );
    let record = pack.join("review.json");
    let r = run(&[
        "review",
        "--proposal",
        s(&proposal),
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--out",
        s(&record),
    ]);
    assert!(r.status.success(), "{}", stderr(&r));
    let out = run(&[
        "approve",
        "--proposal",
        s(&proposal),
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--fixtures",
        s(&fixtures),
        "--review",
        s(&record),
        "--reviewer",
        "r",
        "--out",
        s(&fixtures.join("m.json")),
    ]);
    assert_refused_inside(&out, &fixtures, &before, "approve");
    let out = run(&[
        "report",
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--proposal",
        s(&proposal),
        "--migration",
        s(&pack.join("approval.migration.json")),
        "--fixtures",
        s(&fixtures),
        "--html",
        s(&fixtures.join("r.html")),
        "--json",
        s(&pack.join("r.json")),
    ]);
    assert_refused_inside(&out, &fixtures, &before, "report");
    let out = run(&[
        "guide",
        "--source",
        s(&base),
        "--destination",
        s(&next),
        "--proposal",
        s(&proposal),
        "--fixtures",
        s(&fixtures),
        "--out-dir",
        s(&fixtures.join("g")),
        "--reviewer",
        "r",
        "--yes",
    ]);
    assert_refused_inside(&out, &fixtures, &before, "guide");
}

/// The same for `--packs` and `--pack` folders.
#[test]
fn corpus_and_generate_refuse_outputs_inside_their_pack_folders() {
    let packs = scratch("into-packs").join("packs");
    copy_dir(
        &root().join("packs").join("rename-user-id"),
        &packs.join("rename-user-id"),
    );
    let before = snapshot(&packs);
    for (flag, target) in [
        ("--matrix", packs.join("matrix.md")),
        ("--out", packs.join("rename-user-id").join("corpus.json")),
    ] {
        let out = run(&["corpus", "--packs", s(&packs), flag, s(&target)]);
        assert_refused_inside(&out, &packs, &before, flag);
    }
    let pack = packs.join("rename-user-id");
    for (kind, target) in [
        ("jest", pack.join("fixtures").join("gen")),
        ("pytest", pack.join("gen")),
        ("lossiness", pack.join("fixtures").join("witness.json")),
    ] {
        let out = run(&[
            "generate",
            "--kind",
            kind,
            "--pack",
            s(&pack),
            "--out",
            s(&target),
        ]);
        assert_refused_inside(&out, &packs, &before, kind);
    }
}
