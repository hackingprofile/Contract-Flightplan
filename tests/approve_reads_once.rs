//! Approve reads each input once and binds the digest of the bytes it parsed
//! and checked, so a file swapped while approve runs cannot be bound unseen.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use contract_flightplan::approve::{approve, load_review_record, write_json};
use contract_flightplan::verify::review_proposal;

/// How many approvals were tried and how many succeeded.
#[derive(Debug, Default, PartialEq)]
struct Tally {
    attempts: usize,
    approved: usize,
}

/// At least this many approvals run under the race.
const MIN_ATTEMPTS: usize = 400;
/// After that, approving goes on until one succeeds, up to this long in all.
const TIME_BOUND: Duration = Duration::from_secs(20);

/// Run `attempt` (one approval; `Some(bound_unseen)` when it succeeded) under
/// the race: at least `MIN_ATTEMPTS` times, then on until one approval has
/// succeeded or `TIME_BOUND` has passed. So the test cannot fail because the
/// scheduler happened to swap the file during every one of a fixed number of
/// approvals, and every success still checks that nothing unseen was bound.
fn race_attempts(mut attempt: impl FnMut() -> Option<bool>, bound_unseen: &mut usize) -> Tally {
    let mut tally = Tally::default();
    let start = Instant::now();
    while (tally.attempts < MIN_ATTEMPTS || tally.approved == 0) && start.elapsed() < TIME_BOUND {
        tally.attempts += 1;
        if let Some(unseen) = attempt() {
            tally.approved += 1;
            if unseen {
                *bound_unseen += 1;
            }
        }
    }
    tally
}

/// The race loop cannot end without a successful approval just because
/// the scheduler was unlucky; here every approval before the 450th is refused.
#[test]
fn the_race_loop_keeps_going_until_one_approval_succeeds() {
    let mut calls = 0;
    let mut unseen = 0;
    let tally = race_attempts(
        || {
            calls += 1;
            (calls >= 450).then_some(false)
        },
        &mut unseen,
    );
    assert!(tally.approved > 0, "{tally:?}");
    assert_eq!(unseen, 0);
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

/// A race: the source contract is swapped between the reviewed
/// text A and A plus a comment line (B) with atomic renames while approve
/// runs again and again. An approval may be refused, but it never binds B
/// under a review record that saw A.
#[test]
fn a_contract_swapped_during_approve_is_never_bound_unseen() {
    let dir = std::env::temp_dir().join(format!("cfp-reads-once-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    let pack = dir.join("pack");
    copy_dir(&root().join("packs").join("rename-user-id"), &pack);
    let source = pack.join("base.openapi.yaml");
    let a = fs::read_to_string(&source).unwrap();
    let b = format!("{a}# swapped\n");
    let record = dir.join("review.json");
    let review = review_proposal(
        &pack.join("proposal.json"),
        &source,
        &pack.join("next.openapi.yaml"),
    )
    .unwrap();
    write_json(&record, &review).unwrap();
    let reviewed = load_review_record(&record).unwrap().diff.source.digest;

    let stop = Arc::new(AtomicBool::new(false));
    let swapper = {
        let (stop, source, dir) = (stop.clone(), source.clone(), dir.clone());
        std::thread::spawn(move || {
            let staged = [dir.join("a.tmp"), dir.join("b.tmp")];
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let text = if i.is_multiple_of(2) { &a } else { &b };
                let tmp = &staged[i % 2];
                if fs::write(tmp, text).is_ok() {
                    let _ = fs::rename(tmp, &source);
                }
                i += 1;
            }
        })
    };
    let mut bound_unseen = 0;
    let tally = race_attempts(
        || {
            approve(
                &pack.join("proposal.json"),
                &source,
                &pack.join("next.openapi.yaml"),
                &pack.join("fixtures"),
                &record,
                "local-reviewer",
                None,
                Some("2026-09-30T00:00:00Z"),
            )
            .ok()
            .map(|m| m.approval.bound_digests.source_contract != reviewed)
        },
        &mut bound_unseen,
    );
    let approved = tally.approved;
    eprintln!("race: {} attempts, {approved} approved", tally.attempts);
    stop.store(true, Ordering::Relaxed);
    swapper.join().unwrap();
    assert!(
        approved > 0,
        "no approval succeeded; the race was not exercised"
    );
    assert_eq!(
        bound_unseen, 0,
        "{bound_unseen} of {approved} approvals bound a source the review record did not see"
    );
}
