//! The corpus and fixture-generator seed is a real u64 that changes the
//! generated samples; the same seed always gives the same output.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn cfp(args: &[&str]) -> Output {
    Command::new(bin()).args(args).output().unwrap()
}

fn json_out(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn fixtures(seed: &str) -> Value {
    let source = root().join("packs/rename-user-id/base.openapi.yaml");
    json_out(&cfp(&[
        "fixtures",
        "--source",
        source.to_str().unwrap(),
        "--method",
        "POST",
        "--path",
        "/users",
        "--location",
        "request",
        "--seed",
        seed,
    ]))
}

fn corpus(seed: &str) -> Value {
    json_out(&cfp(&[
        "corpus",
        "--packs",
        root().join("packs").to_str().unwrap(),
        "--seed",
        seed,
    ]))
}

#[test]
fn fixture_generator_uses_the_seed() {
    let first = fixtures("1");
    assert_eq!(first["seed"], 1, "the seed is a number, not a label");
    assert_eq!(first, fixtures("1"), "same seed, same output");
    let other = fixtures("2");
    assert_eq!(other["seed"], 2);
    assert_ne!(
        first["items"][0]["body"], other["items"][0]["body"],
        "a different seed gives different samples"
    );
    // The expectations do not depend on the seed.
    let expects = |v: &Value| -> Vec<Value> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["expect"].clone())
            .collect()
    };
    assert_eq!(expects(&first), expects(&other));
}

#[test]
fn corpus_two_runs_identical() {
    let first = corpus("7");
    assert_eq!(first["seed"], 7);
    assert_eq!(first, corpus("7"), "same seed, identical corpus report");
    assert_eq!(first["stable"], true, "{first:#}");
    let other = corpus("8");
    let digests = |v: &Value| -> Vec<Value> {
        v["packs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["generated_digest"].clone())
            .collect()
    };
    assert!(
        digests(&first).iter().all(|d| d.is_string()),
        "every pack records what the seeded generator produced: {first:#}"
    );
    assert_ne!(digests(&first), digests(&other), "the corpus uses the seed");
    assert_eq!(other["stable"], true, "{other:#}");
}
