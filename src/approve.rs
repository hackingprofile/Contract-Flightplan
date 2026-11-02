use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::digest::{canonical_digest, file_digest, now_rfc3339_utc};
use crate::error::{Error, Result};
use crate::parse::load_openapi;
use crate::proposal::{load_proposal, proposal_digest, rules_digest, PathHint, Rule};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Migration {
    pub schema_version: String,
    pub kind: String,
    pub id: String,
    pub source_contract: DigestedFile,
    pub destination_contract: DigestedFile,
    pub rules: Vec<Rule>,
    pub fixtures: BTreeMap<String, String>,
    pub lifecycle: crate::state::Lifecycle,
    pub approval: Approval,
    #[serde(default)]
    pub history: Vec<crate::history::HistoryEvent>,
    /// The migration this one replaced (`cfp supersede`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<Supersedes>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Supersedes {
    pub id: String,
    /// Canonical digest of the replaced migration document.
    pub digest: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DigestedFile {
    pub path: String,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Approval {
    pub reviewer: String,
    pub approved_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub bound_digests: BoundDigests,
    pub identity_boundary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BoundDigests {
    pub proposal: String,
    pub rules: String,
    pub source_contract: String,
    pub destination_contract: String,
    pub fixtures: BTreeMap<String, String>,
    /// SHA-256 of the review record file `approve --review` consumed.
    pub review_record: String,
}

/// The digest the `approved` event carries: the canonical digest of every bound
/// digest (proposal, rules, both contracts, every fixture file and the review
/// record), so the history chain commits to all of them, not only the proposal.
pub fn approval_digest(bound: &BoundDigests) -> Result<String> {
    let value = serde_json::to_value(bound).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    canonical_digest(&value)
}

/// Read a review record (the document `cfp review --out` writes) strictly:
/// duplicate keys and unknown fields are refused.
pub fn load_review_record(path: &Path) -> Result<crate::verify::ReviewDocument> {
    Ok(load_review_record_with_digest(path)?.0)
}

/// `load_review_record`, with the SHA-256 of the very bytes it parsed.
pub fn load_review_record_with_digest(
    path: &Path,
) -> Result<(crate::verify::ReviewDocument, String)> {
    let bytes = crate::digest::read_bounded_file(path)?;
    let digest = crate::digest::sha256_prefixed(&bytes);
    let text = crate::digest::utf8_text(path, &bytes)?;
    let value = crate::strict_json::from_str(&text).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if kind != "review" {
        return Err(Error::Invalid {
            message: format!(
                "--review must be a review record written by `cfp review --out`; got kind={kind}"
            ),
        });
    }
    let record = serde_json::from_value(value).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    Ok((record, digest))
}

/// The review record must be for exactly this proposal and these contracts,
/// and its type-check must have passed.
fn check_review_record(
    record: &crate::verify::ReviewDocument,
    proposal: &str,
    source: &str,
    destination: &str,
) -> Result<()> {
    for (what, expected, actual) in [
        (
            "review record proposal",
            proposal,
            record.proposal_digest.as_str(),
        ),
        (
            "review record source contract",
            source,
            record.diff.source.digest.as_str(),
        ),
        (
            "review record destination contract",
            destination,
            record.diff.destination.digest.as_str(),
        ),
    ] {
        if expected != actual {
            return Err(Error::StaleDigest {
                what: format!("{what} (review it again with `cfp review --out`)"),
                expected: expected.into(),
                actual: actual.into(),
            });
        }
    }
    if record.typecheck != "ok" {
        return Err(Error::Invalid {
            message: "the review record's type-check did not pass; approve refuses it".into(),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn approve(
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
    review_path: &Path,
    reviewer: &str,
    note: Option<&str>,
    approved_at: Option<&str>,
) -> Result<Migration> {
    if reviewer.trim().is_empty() {
        return Err(Error::Invalid {
            message: "reviewer name is required".into(),
        });
    }
    let proposal = load_proposal(proposal_path)?;
    let source = load_openapi(source_path)?;
    let dest = load_openapi(dest_path)?;
    crate::typecheck::typecheck_proposal(&proposal, &source, &dest)?;
    // Each input is read once: every digest bound below is the digest of the
    // bytes that were parsed, type-checked and compared with the review record.
    let (record, review_hash) = load_review_record_with_digest(review_path)?;
    let proposal_hash = proposal_digest(&proposal)?;
    check_review_record(&record, &proposal_hash, &source.digest, &dest.digest)?;
    let fixtures = digest_fixture_dir(fixtures_dir)?;
    let catalog = check_catalog_paths(fixtures_dir, &fixtures)?;
    crate::verify::check_fixture_coverage(&proposal.rules, &catalog)?;
    check_fixtures_are_json(fixtures_dir, &catalog, &fixtures)?;
    let rules_hash = rules_digest(&proposal.rules)?;
    let source_hash = source.digest.clone();
    let dest_hash = dest.digest.clone();
    if let Some(at) = approved_at {
        if !crate::digest::is_rfc3339_utc_seconds(at) {
            return Err(Error::Invalid {
                message: format!(
                    "--approved-at {at:?} is not an RFC 3339 UTC timestamp with whole seconds, such as 2026-10-30T09:15:00Z"
                ),
            });
        }
    }
    let approved_at = approved_at
        .map(str::to_string)
        .unwrap_or_else(now_rfc3339_utc);
    let bound = BoundDigests {
        proposal: proposal_hash,
        rules: rules_hash,
        source_contract: source_hash.clone(),
        destination_contract: dest_hash.clone(),
        fixtures: fixtures.clone(),
        review_record: review_hash.clone(),
    };
    let approved_digest = approval_digest(&bound)?;
    let mut history = Vec::new();
    crate::history::append(
        &mut history,
        crate::history::event(
            "reviewed",
            reviewer.trim(),
            &review_hash,
            Some("review record bound"),
            Some(&approved_at),
        ),
    )?;
    crate::history::append(
        &mut history,
        crate::history::event(
            "approved",
            reviewer.trim(),
            &approved_digest,
            note,
            Some(&approved_at),
        ),
    )?;
    Ok(Migration {
        schema_version: "1".into(),
        kind: "migration".into(),
        id: proposal.id.clone(),
        source_contract: DigestedFile {
            path: crate::parse::portable_path(source_path),
            digest: source_hash,
        },
        destination_contract: DigestedFile {
            path: crate::parse::portable_path(dest_path),
            digest: dest_hash,
        },
        rules: proposal.rules,
        fixtures,
        lifecycle: crate::state::transition(
            crate::state::Lifecycle::Reviewed,
            crate::state::Lifecycle::Approved,
        )?,
        approval: Approval {
            reviewer: reviewer.trim().to_string(),
            approved_at: approved_at.clone(),
            note: note.map(str::to_string),
            bound_digests: bound,
            identity_boundary:
                "Local reviewer names and content digests are an auditable workflow, not remote identity authentication or tamper-proof signatures."
                    .into(),
        },
        history,
        supersedes: None,
    })
}

/// Approve a successor proposal and retire `old_path`: the new migration
/// records `supersedes` (id and digest of the old document), and the old one
/// is returned as an archive copy with lifecycle `superseded` and a chained
/// `superseded` event pointing at the new migration's digest.
#[allow(clippy::too_many_arguments)]
pub fn supersede(
    old_path: &Path,
    proposal_path: &Path,
    source_path: &Path,
    dest_path: &Path,
    fixtures_dir: &Path,
    review_path: &Path,
    reviewer: &str,
    note: Option<&str>,
    approved_at: Option<&str>,
) -> Result<(Migration, Migration)> {
    let old_value = crate::proposal::load_json_value(old_path)?;
    let old = parse_migration_value(&old_value)?;
    crate::state::must_not_execute(old.lifecycle)?;
    crate::history::check_executable_history(&old)?;
    let mut new = approve(
        proposal_path,
        source_path,
        dest_path,
        fixtures_dir,
        review_path,
        reviewer,
        note,
        approved_at,
    )?;
    new.supersedes = Some(Supersedes {
        id: old.id.clone(),
        digest: canonical_digest(&old_value)?,
    });
    let new_digest =
        canonical_digest(&serde_json::to_value(&new).map_err(|e| Error::Invalid {
            message: e.to_string(),
        })?)?;
    let mut archive = old;
    archive.lifecycle =
        crate::state::transition(archive.lifecycle, crate::state::Lifecycle::Superseded)?;
    let at = new.approval.approved_at.clone();
    crate::history::append(
        &mut archive.history,
        crate::history::event(
            "superseded",
            reviewer.trim(),
            &new_digest,
            Some(&format!("superseded by migration {}", new.id)),
            Some(&at),
        ),
    )?;
    Ok((new, archive))
}

pub fn load_migration(path: &Path) -> Result<Migration> {
    let value = crate::proposal::load_json_value(path)?;
    parse_migration_value(&value)
}

pub fn parse_migration_value(value: &serde_json::Value) -> Result<Migration> {
    let kind = value
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if kind == "proposal" {
        return Err(Error::Unapproved);
    }
    if kind != "migration" {
        return Err(Error::Invalid {
            message: format!("expected kind=migration, got {kind}"),
        });
    }
    if value.get("approval").is_none() {
        return Err(Error::Unapproved);
    }
    if value.get("lifecycle").is_none() {
        return Err(Error::Invalid {
            message: "migration must declare lifecycle; a missing field is not treated as approved"
                .into(),
        });
    }
    serde_json::from_value(value.clone()).map_err(|e| Error::Parse {
        path: "migration".into(),
        message: e.to_string(),
    })
}

pub fn digest_fixture_dir(dir: &Path) -> Result<BTreeMap<String, String>> {
    if !dir.is_dir() {
        return Err(Error::Invalid {
            message: format!("fixtures directory not found: {}", dir.display()),
        });
    }
    let mut files = BTreeMap::new();
    collect_files(dir, dir, &mut files)?;
    if files.is_empty() {
        return Err(Error::Invalid {
            message: "fixtures directory contains no files".into(),
        });
    }
    Ok(files)
}

/// Digest every file under the fixtures folder. Links and other reparse
/// points (symlinks, junctions) are refused rather than skipped: their targets
/// sit outside the folder, so no digest could bind what a reader would see.
fn collect_files(root: &Path, current: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    let entries = fs::read_dir(current).map_err(|e| Error::io(current.display().to_string(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::io(current.display().to_string(), e))?;
        let path = entry.path();
        let rel = file_key(path.strip_prefix(root).unwrap_or(&path))?;
        let meta =
            fs::symlink_metadata(&path).map_err(|e| Error::io(path.display().to_string(), e))?;
        if is_link_or_reparse_point(&meta) {
            return Err(link_refused(&rel));
        }
        if meta.is_dir() {
            collect_files(root, &path, files)?;
        } else if meta.is_file() {
            let digest = file_digest(&path)?;
            insert_fixture_digest(files, rel, digest)?;
        } else {
            return Err(Error::Invalid {
                message: format!(
                    "fixtures folder contains {rel:?}, which is neither a regular file nor a folder"
                ),
            });
        }
    }
    Ok(())
}

/// The digest key of a file below the fixtures folder: its folder and file
/// names joined with `/`. Each name must be UTF-8 without `\`, so the key is
/// the same on every OS and no two files can share one (on Linux and macOS
/// `sub\x.json` is a single file name, which would otherwise read as `sub/x.json`).
pub fn file_key(rel: &Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(name) => parts.push(key_component(name)?),
            other => {
                return Err(Error::Invalid {
                    message: format!(
                        "fixture path {:?} has a {other:?} component; only plain names are bound",
                        rel.display()
                    ),
                })
            }
        }
    }
    Ok(parts.join("/"))
}

/// One folder or file name of a fixture path, refused when it is not UTF-8 or
/// contains `\`.
pub fn key_component(name: &std::ffi::OsStr) -> Result<&str> {
    let text = name.to_str().ok_or_else(|| Error::Invalid {
        message: format!("fixture name {name:?} is not UTF-8; it cannot be bound by digest"),
    })?;
    if text.contains('\\') {
        return Err(Error::Invalid {
            message: format!(
                "fixture name {text:?} contains a backslash; it would share a digest key with a nested path, so it is refused"
            ),
        });
    }
    Ok(text)
}

/// Record one file's digest; two files with the same key are refused.
pub fn insert_fixture_digest(
    files: &mut BTreeMap<String, String>,
    key: String,
    digest: String,
) -> Result<()> {
    if files.contains_key(&key) {
        return Err(Error::Invalid {
            message: format!("two fixture files share the digest key {key:?}"),
        });
    }
    files.insert(key, digest);
    Ok(())
}

fn link_refused(rel: &str) -> Error {
    Error::Invalid {
        message: format!(
            "fixtures folder contains a link or reparse point at {rel:?}; links are refused because their targets are outside the fixtures folder and cannot be bound by digest"
        ),
    }
}

/// True for symlinks, junctions and any other Windows reparse point.
fn is_link_or_reparse_point(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    write_output(path, text.as_bytes())
}

/// What already sits at an output path decides how it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputTarget {
    /// Nothing yet, or a regular file: a temporary file is renamed into place.
    Replace,
    /// A character device or FIFO (Unix), or the NUL device (Windows): the
    /// bytes are written to it directly, and it is never renamed over.
    WriteThrough,
}

/// Classify the output path. A link, a folder, a block device, a socket or
/// another special file is refused (exit 3), so an output never replaces a
/// device node, a FIFO or a socket.
fn output_target(path: &Path) -> Result<OutputTarget> {
    let shown = path.display();
    let refuse = |what: &str| {
        Error::Invalid {
        message: format!(
            "refusing to write {shown}: it is {what}; outputs go to regular files, a character device such as /dev/null or NUL, or a FIFO"
        ),
    }
    };
    // Windows resolves these names to devices in any folder, present or not.
    match windows_device_name(path) {
        Some(name) if name == "NUL" => return Ok(OutputTarget::WriteThrough),
        Some(_) => return Err(refuse("a Windows device other than NUL")),
        None => {}
    }
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            if is_link_or_reparse_point(&meta) || meta.is_dir() {
                return Err(Error::Invalid {
                    message: format!(
                        "refusing to write {shown}: it is a link or a folder, and outputs are never written through links"
                    ),
                });
            }
            if meta.is_file() {
                return Ok(OutputTarget::Replace);
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::FileTypeExt;
                let kind = meta.file_type();
                if kind.is_char_device() || kind.is_fifo() {
                    return Ok(OutputTarget::WriteThrough);
                }
                if kind.is_block_device() {
                    return Err(refuse("a block device"));
                }
                if kind.is_socket() {
                    return Err(refuse("a socket"));
                }
            }
            Err(refuse(
                "neither a regular file nor a character device or FIFO",
            ))
        }
        // Missing, or unreadable metadata: the write reports the real error.
        Err(_) => Ok(OutputTarget::Replace),
    }
}

/// The reserved DOS device a path names on Windows (`NUL`, `CON`, `COM1`, ...,
/// also as `\\.\NUL` or `sub\NUL`), upper-cased; `None` elsewhere and on
/// other systems. `NUL.txt` is an ordinary name on current Windows.
fn windows_device_name(path: &Path) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let text = path.as_os_str().to_string_lossy();
    let last = text.rsplit(['\\', '/']).next()?;
    let name = last.trim_end_matches([' ', '.', ':']).to_ascii_uppercase();
    let numbered = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|n| n.len() == 1 && n.as_bytes()[0].is_ascii_digit())
    };
    let reserved = matches!(
        name.as_str(),
        "NUL" | "CON" | "PRN" | "AUX" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT");
    reserved.then_some(name)
}

/// Write to an existing character device or FIFO (or NUL) in place. The open
/// neither creates nor truncates; on Unix the open handle's type is checked
/// again, so a regular file swapped in after the path check is never written.
fn write_through(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let shown = path.display().to_string();
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| Error::io(shown.clone(), e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let kind = file
            .metadata()
            .map_err(|e| Error::io(shown.clone(), e))?
            .file_type();
        if !(kind.is_char_device() || kind.is_fifo()) {
            return Err(Error::Invalid {
                message: format!(
                    "refusing to write {shown}: it changed from a device or FIFO while it was opened"
                ),
            });
        }
    }
    file.write_all(bytes)
        .and_then(|()| file.flush())
        .map_err(|e| Error::io(shown, e))
}

/// Write one output file. A link (or other reparse point), a folder, a block
/// device or a socket at the path is refused. A character device or FIFO
/// (`/dev/null`, Windows `NUL`) is written to directly. Otherwise the bytes go
/// to a fresh temporary file beside it that is then renamed into place, so a
/// link created meanwhile is replaced, never written through.
pub fn write_output(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let shown = path.display().to_string();
    if output_target(path)? == OutputTarget::WriteThrough {
        return write_through(path, bytes);
    }
    let folder = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    fs::create_dir_all(folder).map_err(|e| Error::io(folder.display().to_string(), e))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| Error::Invalid {
            message: format!("output path {shown} names no file"),
        })?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (tmp, mut file) = loop {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = folder.join(format!(".{name}.{}-{n}.cfp-tmp", std::process::id()));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(file) => break (tmp, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::io(tmp.display().to_string(), e)),
        }
    };
    let written = file.write_all(bytes).and_then(|()| file.flush());
    drop(file);
    if let Err(e) = written.and_then(|()| fs::rename(&tmp, path)) {
        let _ = fs::remove_file(&tmp);
        return Err(Error::io(shown, e));
    }
    Ok(())
}

/// Where an output will land once its missing folders are made: the existing
/// path itself, or the canonical form of its deepest existing ancestor joined
/// with the rest of the path (`.` dropped, `..` applied, as creating the
/// missing folders and then the file would resolve them). `None` only when no
/// ancestor can be resolved.
fn resolved_output(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if let Ok(found) = fs::canonicalize(path) {
        return Some(found);
    }
    let absolute = std::path::absolute(path).ok()?;
    let parts: Vec<Component> = absolute.components().collect();
    for split in (1..parts.len()).rev() {
        let prefix: PathBuf = parts[..split].iter().collect();
        let Ok(mut at) = fs::canonicalize(&prefix) else {
            continue;
        };
        for part in &parts[split..] {
            match part {
                Component::Normal(name) => at.push(name),
                Component::ParentDir => {
                    at.pop();
                }
                _ => {}
            }
        }
        return Some(at);
    }
    None
}

/// The canonical form of every input, and the inputs that are folders
/// (`--fixtures`, `--packs`, `--pack`) with the spelling the user gave.
struct Inputs<'a> {
    paths: Vec<PathBuf>,
    folders: Vec<(PathBuf, &'a Path)>,
}

impl<'a> Inputs<'a> {
    fn new(inputs: &[&'a Path]) -> Self {
        let mut paths = Vec::new();
        let mut folders = Vec::new();
        for input in inputs {
            if let Ok(found) = fs::canonicalize(input) {
                if found.is_dir() {
                    folders.push((found.clone(), *input));
                }
                paths.push(found);
            }
        }
        Inputs { paths, folders }
    }

    /// Refuse a resolved output that is one of the inputs or lies inside an
    /// input folder.
    fn check(&self, output: &Path, at: &Path) -> Result<()> {
        if self.paths.iter().any(|p| p == at) {
            return Err(Error::Invalid {
                message: format!(
                    "output {} is one of this command's inputs; choose another path",
                    output.display()
                ),
            });
        }
        if let Some((_, given)) = self.folders.iter().find(|(f, _)| at.starts_with(f)) {
            return Err(Error::Invalid {
                message: format!(
                    "output {} lies inside the input folder {}; a command never writes into a folder it reads (a new file there would change what was approved); choose a path outside it",
                    output.display(),
                    given.display()
                ),
            });
        }
        Ok(())
    }
}

/// Refuse outputs that would replace one of the command's own inputs, lie
/// inside an input folder, or share one path, before anything is read or
/// written.
pub fn check_outputs(outputs: &[&Path], inputs: &[&Path]) -> Result<()> {
    let inputs = Inputs::new(inputs);
    let mut seen: Vec<PathBuf> = Vec::new();
    for output in outputs {
        // A device or FIFO is never an input (inputs are regular files) and
        // may take several outputs (`--html /dev/null --json /dev/null`); any
        // other special file is refused here, before anything runs.
        if output_target(output)? == OutputTarget::WriteThrough {
            continue;
        }
        let Some(at) = resolved_output(output) else {
            continue;
        };
        inputs.check(output, &at)?;
        if seen.contains(&at) {
            return Err(Error::Invalid {
                message: format!("two outputs share the path {}", output.display()),
            });
        }
        seen.push(at);
    }
    Ok(())
}

/// Refuse an output folder (`generate --out`) that is one of the inputs or
/// lies inside an input folder; the files written into it would land there.
pub fn check_output_folder(folder: &Path, inputs: &[&Path]) -> Result<()> {
    let inputs = Inputs::new(inputs);
    match resolved_output(folder) {
        Some(at) => inputs.check(folder, &at),
        None => Ok(()),
    }
}

pub fn migration_canonical_digest(migration: &Migration) -> Result<String> {
    let value = serde_json::to_value(migration).map_err(|e| Error::Invalid {
        message: e.to_string(),
    })?;
    canonical_digest(&value)
}

pub fn as_path_hint(path: &Path) -> PathHint {
    PathHint {
        path: crate::parse::portable_path(path),
    }
}

/// The digest-map key for a catalog path: its normal name components joined
/// with `/`. Anything else (parent or current-folder parts, empty parts, roots,
/// drive or stream prefixes) leaves or escapes the fixtures folder and is
/// refused.
pub fn fixture_key(relative: &str) -> Result<String> {
    let outside = |why: &str| Error::Invalid {
        message: format!("fixture path {relative:?} leaves the fixtures folder: {why}"),
    };
    let mut parts = Vec::new();
    for part in relative.split(['/', '\\']) {
        match part {
            "" => return Err(outside("empty, root or UNC component")),
            "." | ".." => return Err(outside("current or parent folder component")),
            _ if part.contains(':') => {
                return Err(outside("drive, device or stream component"));
            }
            _ if part.trim_end_matches([' ', '.']).is_empty() => {
                return Err(outside("component made only of dots or spaces"));
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        return Err(outside("no file name"));
    }
    Ok(parts.join("/"))
}

/// Resolve a catalog path to a file that sits inside the fixtures folder.
/// The path must be relative with normal components only, no component on the
/// way may be a link or reparse point, and the canonical file must be under the
/// canonical fixtures folder.
pub fn fixture_file(fixtures_dir: &Path, relative: &str) -> Result<PathBuf> {
    let key = fixture_key(relative)?;
    let mut path = fixtures_dir.to_path_buf();
    let mut walked = Vec::new();
    for part in key.split('/') {
        path.push(part);
        walked.push(part);
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if is_link_or_reparse_point(&meta) {
                return Err(link_refused(&walked.join("/")));
            }
        }
    }
    let root = fs::canonicalize(fixtures_dir)
        .map_err(|e| Error::io(fixtures_dir.display().to_string(), e))?;
    let resolved = fs::canonicalize(&path).map_err(|e| Error::io(key.clone(), e))?;
    if !resolved.starts_with(&root) {
        return Err(Error::Invalid {
            message: format!("fixture path {relative:?} leaves the fixtures folder"),
        });
    }
    Ok(path)
}

/// Every catalog item must name a file inside the fixtures folder that the
/// digest map binds. Returns the catalog so callers can use it.
pub fn check_catalog_paths(
    fixtures_dir: &Path,
    bound: &BTreeMap<String, String>,
) -> Result<crate::verify::FixtureCatalog> {
    // The catalog is read once, and only the bytes the digest map binds are used.
    let path = fixtures_dir.join("catalog.json");
    let bytes = crate::digest::read_bounded_file(&path)?;
    expect_bound(bound, "catalog.json", &bytes)?;
    let catalog = crate::verify::parse_catalog(&path, &bytes)?;
    for item in &catalog.items {
        let key = fixture_key(&item.path)?;
        fixture_file(fixtures_dir, &item.path)?;
        if !bound.contains_key(&key) {
            return Err(Error::Invalid {
                message: format!(
                    "fixture path {:?} of catalog item {:?} is not a bound file in the fixtures folder",
                    item.path, item.id
                ),
            });
        }
    }
    Ok(catalog)
}

/// The bytes just read must be the ones the digest map binds: a file that
/// changed between two reads is refused, never bound unseen.
fn expect_bound(bound: &BTreeMap<String, String>, key: &str, bytes: &[u8]) -> Result<()> {
    let actual = crate::digest::sha256_prefixed(bytes);
    match bound.get(key) {
        Some(expected) if *expected == actual => Ok(()),
        expected => Err(Error::StaleDigest {
            what: format!("fixture {key} (it changed while it was being read)"),
            expected: expected.cloned().unwrap_or_else(|| "<not bound>".into()),
            actual,
        }),
    }
}

/// Refuse to bind a catalogued fixture that verify could not read: each
/// must be UTF-8 JSON (strict, no duplicate keys). The error names the item
/// and its catalog path.
pub fn check_fixtures_are_json(
    fixtures_dir: &Path,
    catalog: &crate::verify::FixtureCatalog,
    bound: &BTreeMap<String, String>,
) -> Result<()> {
    for item in &catalog.items {
        let path = fixture_file(fixtures_dir, &item.path)?;
        let named = |message: String| Error::Parse {
            path: format!("fixture {:?} ({})", item.id, item.path),
            message,
        };
        let bytes = crate::digest::read_bounded_file(&path)?;
        expect_bound(bound, &fixture_key(&item.path)?, &bytes)?;
        let text = crate::digest::utf8_text(&path, &bytes).map_err(|_| {
            named("not UTF-8 text; fixtures must be UTF-8 JSON (a UTF-8 BOM is allowed)".into())
        })?;
        crate::strict_json::from_str(&text).map_err(|e| named(format!("not valid JSON: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Linux and macOS `\` is an ordinary file-name character, so a file
    /// named `sub\x.json` must not share the digest key of `sub/x.json`.
    #[test]
    fn fixture_digest_keys_are_unambiguous() {
        use std::ffi::OsStr;
        assert!(key_component(OsStr::new("sub\\x.json")).is_err());
        assert_eq!(key_component(OsStr::new("x.json")).unwrap(), "x.json");
        assert_eq!(
            file_key(&Path::new("sub").join("x.json")).unwrap(),
            "sub/x.json"
        );
        assert!(file_key(&Path::new("..").join("x.json")).is_err());
        let mut files = BTreeMap::new();
        insert_fixture_digest(&mut files, "a.json".into(), "sha256:1".into()).unwrap();
        assert!(insert_fixture_digest(&mut files, "a.json".into(), "sha256:2".into()).is_err());
    }

    #[test]
    fn proposal_kind_is_unapproved() {
        let value = serde_json::json!({
            "schema_version":"1",
            "kind":"proposal",
            "id":"x"
        });
        assert!(matches!(
            parse_migration_value(&value),
            Err(Error::Unapproved)
        ));
    }

    #[test]
    fn missing_lifecycle_is_not_treated_as_approved() {
        let value = serde_json::json!({
            "schema_version": "1",
            "kind": "migration",
            "id": "x",
            "source_contract": {"path": "a.yaml", "digest": "sha256:aa"},
            "destination_contract": {"path": "b.yaml", "digest": "sha256:bb"},
            "rules": [],
            "fixtures": {"catalog.json": "sha256:cc"},
            "approval": {
                "reviewer": "r",
                "approved_at": "2026-09-22T00:00:00Z",
                "bound_digests": {
                    "proposal": "sha256:p",
                    "rules": "sha256:r",
                    "source_contract": "sha256:s",
                    "destination_contract": "sha256:d",
                    "fixtures": {}
                },
                "identity_boundary": "local"
            }
        });
        let err = parse_migration_value(&value).unwrap_err();
        assert!(matches!(err, Error::Invalid { .. }));
        assert!(err.to_string().contains("lifecycle"));
    }
}
