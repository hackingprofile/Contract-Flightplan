//! Hostile inputs cannot exhaust memory or time: `$ref` fan-out, special
//! files, regular-expression cost and YAML nesting are bounded, and every
//! bound fails closed with exit 3 and the bound's name.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cfp"))
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cfp-bounds-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn s(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Run `cfp` and kill it after `limit`; `None` when it did not finish in time.
fn run_within(args: &[&str], limit: Duration) -> Option<Output> {
    let mut child = Command::new(bin())
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            return Some(child.wait_with_output().unwrap());
        }
        if start.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A JSON contract whose `POST /u` request body is `#/components/schemas/S0`.
fn contract_with_components(dir: &Path, name: &str, components: Map<String, Value>) -> PathBuf {
    let doc = json!({
        "openapi": "3.0.3",
        "info": {"title": "t", "version": "1"},
        "paths": {"/u": {"post": {
            "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/S0"}}}},
            "responses": {"200": {"description": "ok"}}
        }}},
        "components": {"schemas": components}
    });
    let path = dir.join(name);
    fs::write(&path, serde_json::to_string(&doc).unwrap()).unwrap();
    path
}

/// `levels` components, each with `width` properties that all `$ref` the
/// next one; the last component is `leaf`. The body expands to width^levels leaves.
fn fanout(dir: &Path, name: &str, width: usize, levels: usize, leaf: Value) -> PathBuf {
    let mut components = Map::new();
    for level in 0..levels {
        let mut properties = Map::new();
        for p in 0..width {
            properties.insert(
                format!("p{p}"),
                json!({"$ref": format!("#/components/schemas/S{}", level + 1)}),
            );
        }
        components.insert(
            format!("S{level}"),
            json!({"type": "object", "properties": properties}),
        );
    }
    components.insert(format!("S{levels}"), leaf);
    contract_with_components(dir, name, components)
}

/// Refused by the node or the byte budget, whichever is reached first.
fn refused_by_a_schema_budget(out: &Output) -> bool {
    let text = stderr(out);
    text.contains("resource bound exceeded (schema_nodes)")
        || text.contains("resource bound exceeded (schema_bytes)")
}

fn compare_within(contract: &Path, limit: Duration) -> Option<Output> {
    run_within(
        &[
            "compare",
            "--source",
            s(contract),
            "--destination",
            s(contract),
        ],
        limit,
    )
}

#[test]
fn ref_fanout_beyond_the_node_budget_is_refused_with_exit_3() {
    let dir = scratch("nodes");
    // 10^5 leaves (111,111 schema nodes) from a contract of a few KB.
    let contract = fanout(&dir, "wide.json", 10, 5, json!({"type": "string"}));
    assert!(fs::metadata(&contract).unwrap().len() < 4096);
    let out = compare_within(&contract, Duration::from_secs(120)).expect("compare finished");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("schema_nodes"), "{}", stderr(&out));
}

#[test]
fn ref_fanout_beyond_the_byte_budget_is_refused_with_exit_3() {
    let dir = scratch("bytes");
    // 65,536 leaves (under the node budget), each a copy of a 2 KB schema.
    let leaf = json!({"type": "string", "description": "d".repeat(2048)});
    let contract = fanout(&dir, "heavy.json", 256, 2, leaf);
    assert!(fs::metadata(&contract).unwrap().len() < 64 * 1024);
    let out = compare_within(&contract, Duration::from_secs(120)).expect("compare finished");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("schema_bytes"), "{}", stderr(&out));
}

/// A 2.3 KB contract with properties `a` and `b` at each of 16 levels expands
/// to 65,536 leaves; it is refused within seconds, before any diff is written.
#[test]
fn two_wide_sixteen_deep_ref_fanout_is_refused_quickly() {
    let dir = scratch("deep");
    let mut components = Map::new();
    for level in 0..16 {
        let next = json!({"$ref": format!("#/components/schemas/S{}", level + 1)});
        components.insert(
            format!("S{level}"),
            json!({"type": "object", "properties": {"a": next.clone(), "b": next}}),
        );
    }
    components.insert("S16".into(), json!({"type": "string"}));
    let contract = contract_with_components(&dir, "deep.json", components);
    let out = compare_within(&contract, Duration::from_secs(60))
        .expect("compare must finish within 60 s");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(refused_by_a_schema_budget(&out), "{}", stderr(&out));
}

/// A 35 KB contract, 256 wide and 3 levels deep, expands to 16.7 million
/// leaves; it is refused before it can exhaust memory.
#[test]
fn wide_three_level_ref_fanout_is_refused_without_exhausting_memory() {
    let dir = scratch("wide3");
    let contract = fanout(&dir, "wide3.json", 256, 3, json!({"type": "string"}));
    let out = compare_within(&contract, Duration::from_secs(60))
        .expect("compare must finish within 60 s");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(refused_by_a_schema_budget(&out), "{}", stderr(&out));
}

/// A contract that reuses components moderately stays well inside the budget.
#[test]
fn moderate_ref_reuse_still_compares() {
    let dir = scratch("moderate");
    let contract = fanout(&dir, "ok.json", 256, 2, json!({"type": "string"}));
    let out = compare_within(&contract, Duration::from_secs(120)).expect("compare finished");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

/// The null device: `NUL` on Windows, `/dev/null` elsewhere.
fn null_device() -> &'static str {
    if cfg!(windows) {
        "NUL"
    } else {
        "/dev/null"
    }
}

/// Refused without reading. Windows cannot describe a device (`NUL` fails with
/// "Incorrect function"), so there the refusal is that I/O error naming it.
fn refused_as_irregular(out: &Output) -> bool {
    let text = stderr(out);
    out.status.code() == Some(3)
        && (text.contains("not a regular file")
            || (cfg!(windows) && text.contains("I/O error at NUL")))
}

/// Every flag that reads a file refuses a device instead of reading it.
/// The endless devices themselves are Linux and macOS cases, below.
#[test]
fn every_input_refuses_a_device_file() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let pack = root.join("packs").join("rename-user-id");
    let p = |name: &str| pack.join(name).to_str().unwrap().to_string();
    let dir = scratch("device");
    let out = |name: &str| dir.join(name).to_str().unwrap().to_string();
    let dev = null_device().to_string();
    let base = p("base.openapi.yaml");
    let next = p("next.openapi.yaml");
    let cases: Vec<Vec<String>> = vec![
        vec![
            "compare".into(),
            "--source".into(),
            dev.clone(),
            "--destination".into(),
            next.clone(),
        ],
        vec![
            "compare".into(),
            "--source".into(),
            base.clone(),
            "--destination".into(),
            dev.clone(),
        ],
        vec![
            "compare".into(),
            "--source".into(),
            base.clone(),
            "--destination".into(),
            next.clone(),
            "--oasdiff".into(),
            dev.clone(),
        ],
        vec![
            "review".into(),
            "--proposal".into(),
            dev.clone(),
            "--source".into(),
            base.clone(),
            "--destination".into(),
            next.clone(),
        ],
        vec![
            "verify".into(),
            "--migration".into(),
            dev.clone(),
            "--proposal".into(),
            p("proposal.json"),
            "--source".into(),
            base.clone(),
            "--destination".into(),
            next.clone(),
            "--fixtures".into(),
            p("fixtures"),
        ],
        vec![
            "report".into(),
            "--source".into(),
            base.clone(),
            "--destination".into(),
            next.clone(),
            "--verify".into(),
            dev.clone(),
            "--html".into(),
            out("r.html"),
            "--json".into(),
            out("r.json"),
        ],
        vec![
            "validate".into(),
            "--schema-of".into(),
            base.clone(),
            "--method".into(),
            "POST".into(),
            "--path".into(),
            "/users".into(),
            "--location".into(),
            "request".into(),
            "--payload".into(),
            dev.clone(),
        ],
        vec![
            "propose".into(),
            "--mode".into(),
            "file".into(),
            "--proposal".into(),
            dev.clone(),
        ],
        vec![
            "provider".into(),
            "--preview".into(),
            "anthropic".into(),
            "--select".into(),
            dev.clone(),
        ],
    ];
    for args in cases {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let result = run_within(&refs, Duration::from_secs(30)).expect("finished in time");
        assert!(
            refused_as_irregular(&result),
            "{args:?}: exit {:?}, {}",
            result.status.code(),
            stderr(&result)
        );
    }
}

#[cfg(unix)]
#[test]
fn endless_devices_are_refused_without_reading() {
    for dev in ["/dev/zero", "/dev/urandom"] {
        let out = run_within(
            &["compare", "--source", dev, "--destination", dev],
            Duration::from_secs(10),
        )
        .expect("a device must be refused at once");
        assert!(refused_as_irregular(&out), "{dev}: {}", stderr(&out));
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_device_is_refused() {
    let dir = scratch("dev-link");
    let link = dir.join("base.openapi.yaml");
    std::os::unix::fs::symlink("/dev/zero", &link).unwrap();
    let out = compare_within(&link, Duration::from_secs(10)).expect("refused at once");
    assert!(refused_as_irregular(&out), "{}", stderr(&out));
}

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking() {
    let dir = scratch("fifo");
    let fifo = dir.join("f.yaml");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    let out = compare_within(&fifo, Duration::from_secs(10)).expect("a FIFO must not block");
    assert!(refused_as_irregular(&out), "{}", stderr(&out));
}

/// A symlink flipped between a regular contract and a FIFO while compare
/// runs again and again never makes compare hang. The file is opened without
/// blocking and its type is taken from the open handle, so a FIFO swapped in
/// after any path check is refused instead of waiting for a writer.
#[cfg(unix)]
#[test]
fn a_fifo_swapped_in_after_the_type_check_never_hangs() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let dir = scratch("fifo-swap");
    let fifo = dir.join("f.fifo");
    let made = Command::new("mkfifo").arg(&fifo).status().unwrap();
    assert!(made.success());
    let regular = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("packs")
        .join("rename-user-id")
        .join("base.openapi.yaml");
    let link = dir.join("base.openapi.yaml");
    std::os::unix::fs::symlink(&regular, &link).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let flipper = {
        let (stop, dir, link) = (stop.clone(), dir.clone(), link.clone());
        std::thread::spawn(move || {
            let tmp = dir.join("next-link.tmp");
            let mut i = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let target = if i.is_multiple_of(2) { &fifo } else { &regular };
                let _ = fs::remove_file(&tmp);
                if std::os::unix::fs::symlink(target, &tmp).is_ok() {
                    let _ = fs::rename(&tmp, &link);
                }
                i += 1;
            }
        })
    };
    let (mut ran, mut hung, mut finished) = (0, 0, 0);
    let deadline = Instant::now() + Duration::from_secs(120);
    while ran < 200 && Instant::now() < deadline {
        ran += 1;
        match compare_within(&link, Duration::from_secs(5)) {
            Some(_) => finished += 1,
            None => hung += 1,
        }
    }
    stop.store(true, Ordering::Relaxed);
    flipper.join().unwrap();
    assert_eq!(hung, 0, "{hung} of {ran} runs hung ({finished} finished)");
}

/// A refused proposal over the size bound is not copied into quarantine: the
/// copy would read and write the whole file, however large.
#[test]
fn quarantine_does_not_copy_an_input_beyond_the_size_bound() {
    let dir = scratch("quarantine-size");
    let big = dir.join("big.json");
    fs::write(&big, vec![b' '; 2 * 1024 * 1024]).unwrap();
    let quarantine = dir.join("quarantine");
    let out = run_within(
        &[
            "propose",
            "--mode",
            "file",
            "--proposal",
            s(&big),
            "--quarantine",
            s(&quarantine),
        ],
        Duration::from_secs(30),
    )
    .expect("finished");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("file_size"), "{}", stderr(&out));
    let copies = fs::read_dir(&quarantine).map(|d| d.count()).unwrap_or(0);
    assert_eq!(copies, 0, "nothing may be copied into quarantine");
}

/// A contract whose `POST /u` request body is `{"m": [[<string schema>]]}` and
/// a payload of 16 x 256 strings: one schema, 4,096 strings to check.
fn nested_strings(dir: &Path, string_schema: Value) -> (PathBuf, PathBuf) {
    let mut components = Map::new();
    components.insert(
        "S0".into(),
        json!({"type": "object", "properties": {"m": {"type": "array", "items": {"type": "array", "items": string_schema}}}}),
    );
    let contract = contract_with_components(dir, "nest.json", components);
    let payload = dir.join("payload.json");
    fs::write(&payload, json!({"m": vec![vec!["a"; 256]; 16]}).to_string()).unwrap();
    (contract, payload)
}

fn validate_within(contract: &Path, payload: &Path, limit: Duration) -> Option<Output> {
    run_within(
        &[
            "validate",
            "--schema-of",
            s(contract),
            "--method",
            "POST",
            "--path",
            "/u",
            "--location",
            "request",
            "--payload",
            s(payload),
        ],
        limit,
    )
}

/// The pattern is compiled once for the contract, not once per string checked.
#[test]
fn a_pattern_is_compiled_once_not_per_string() {
    let dir = scratch("pattern-once");
    let (contract, payload) =
        nested_strings(&dir, json!({"type": "string", "pattern": r"^\p{L}{1,8}$"}));
    let out = validate_within(&contract, &payload, Duration::from_secs(30))
        .expect("4,096 strings with one pattern must validate within 30 s");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

/// A pattern whose compiled form exceeds the size limit is reported as
/// unsupported (exit 2) at once, not compiled at full size.
#[test]
fn a_pattern_beyond_the_compile_limit_is_unsupported_quickly() {
    let dir = scratch("pattern-big");
    let (contract, payload) = nested_strings(
        &dir,
        json!({"type": "string", "pattern": r"^\p{L}{1,200}$"}),
    );
    let out = validate_within(&contract, &payload, Duration::from_secs(30))
        .expect("an oversized pattern must be refused within 30 s");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("pattern"), "{}", stderr(&out));
}

/// More distinct patterns than the bound in one contract fail closed.
#[test]
fn distinct_patterns_beyond_the_bound_are_refused() {
    let dir = scratch("pattern-count");
    let mut components = Map::new();
    let mut outer = Map::new();
    for group in 0..2 {
        let mut inner = Map::new();
        for i in 0..150 {
            let n = group * 150 + i + 1;
            inner.insert(
                format!("f{i}"),
                json!({"type": "string", "pattern": format!("^a{{{n}}}$")}),
            );
        }
        outer.insert(
            format!("g{group}"),
            json!({"type": "object", "properties": inner}),
        );
    }
    components.insert("S0".into(), json!({"type": "object", "properties": outer}));
    let contract = contract_with_components(&dir, "patterns.json", components);
    let out = compare_within(&contract, Duration::from_secs(60)).expect("finished");
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("resource bound exceeded (patterns)"),
        "{}",
        stderr(&out)
    );
}

fn yaml_contract(dir: &Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, text).unwrap();
    path
}

/// Deep YAML flow nesting is refused before the YAML parser, whose cost per
/// token grows with flow depth, can spend minutes on it.
#[test]
fn deep_yaml_flow_nesting_is_refused_quickly() {
    let dir = scratch("yaml-deep");
    let unclosed = yaml_contract(
        &dir,
        "unclosed.yaml",
        &format!("a: {}\n", "[".repeat(1_048_000)),
    );
    let balanced = yaml_contract(
        &dir,
        "balanced.yaml",
        &format!("a: {}{}\n", "[".repeat(100_000), "]".repeat(100_000)),
    );
    for contract in [unclosed, balanced] {
        let out = compare_within(&contract, Duration::from_secs(30))
            .expect("deep YAML nesting must be refused within 30 s");
        assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
        assert!(
            stderr(&out).contains("resource bound exceeded (depth)"),
            "{}",
            stderr(&out)
        );
    }
}

/// A YAML contract written with many short flow collections still parses.
#[test]
fn flow_heavy_yaml_within_the_depth_bound_still_compares() {
    let dir = scratch("yaml-flow");
    let mut text = String::from(
        "openapi: 3.0.3\ninfo: {title: t, version: '1'}\npaths:\n  /u:\n    post:\n      requestBody:\n        content:\n          application/json:\n            schema: {type: object, required: [p0], properties: {p0: {type: string, enum: ['a', \"b\", c]}}}\n      responses: {'200': {description: ok}}\nx-lists:\n",
    );
    for i in 0..5_000 {
        text.push_str(&format!("  - [\"x{i}\", 'y', [z, {{k: [1, 2]}}]]\n"));
    }
    let contract = yaml_contract(&dir, "flow.yaml", &text);
    let out = compare_within(&contract, Duration::from_secs(60)).expect("finished");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}
