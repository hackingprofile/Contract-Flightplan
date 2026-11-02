use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::error::{Error, Result};

pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_DEPTH: usize = 32;
pub const MAX_REF_CHAIN: usize = 16;
pub const MAX_PROPERTIES: usize = 256;
pub const MAX_ENUM: usize = 64;
pub const MAX_ARRAY: usize = 256;
pub const MAX_RULES: usize = 32;
/// Schema nodes one contract's bodies may resolve to once every `$ref` is
/// expanded in place; a small contract whose references fan out exceeds it.
pub const MAX_SCHEMA_NODES: usize = 100_000;
/// Estimated bytes those resolved schemas may hold.
pub const MAX_SCHEMA_BYTES: usize = 64 * 1024 * 1024;
/// Changes plus distinct notes one diff may list.
pub const MAX_DIFF_ENTRIES: usize = 100_000;
/// Compiled size (program, and lazy DFA cache) one `pattern` may take; a
/// larger one is reported as unsupported.
pub const MAX_PATTERN_BYTES: usize = 512 * 1024;
/// Distinct `pattern` strings one contract may declare; each compiles once.
pub const MAX_PATTERNS: usize = 256;

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn sha256_prefixed(bytes: &[u8]) -> String {
    format!("sha256:{}", sha256_hex(bytes))
}

/// Read an input file of at most `MAX_BYTES`. Only regular files are read: a
/// device (`/dev/zero`), FIFO or socket, also reached through a symlink, could
/// stream without end or block. The file is opened first (on Unix without
/// blocking, so opening a FIFO returns at once) and the type that decides is
/// the OPEN handle's, so nothing swapped in between a check and the open is
/// ever read or waited on. The read itself stops one byte past the bound,
/// whatever the file's reported length.
pub fn read_bounded_file(path: &Path) -> Result<Vec<u8>> {
    let shown = path.display().to_string();
    let file = open_for_reading(path).map_err(|e| {
        // The open failed (a socket on Unix, a folder on Windows): say what the
        // path is when that is known. Nothing was read either way.
        match fs::metadata(path) {
            Ok(meta) if !meta.is_file() => not_regular(&shown),
            _ => Error::io(shown.clone(), e),
        }
    })?;
    let meta = file.metadata().map_err(|e| Error::io(shown.clone(), e))?;
    check_regular(&meta, &shown)?;
    read_capped(file, &shown)
}

/// `O_NONBLOCK` where its value is known: opening a FIFO with it returns at
/// once instead of waiting for a writer. It has no effect on reading a regular
/// file, the only kind read after the handle check, so it is left set (clearing
/// it would need an unsafe `fcntl` call for no change in behaviour).
#[cfg(unix)]
const O_NONBLOCK: Option<i32> = if cfg!(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
)) {
    Some(0x0004)
} else if cfg!(any(target_os = "linux", target_os = "android")) {
    if cfg!(any(target_arch = "mips", target_arch = "mips64")) {
        Some(0x0080)
    } else if cfg!(any(target_arch = "sparc", target_arch = "sparc64")) {
        Some(0x4000)
    } else {
        Some(0o4000)
    }
} else {
    None
};

#[cfg(unix)]
fn open_for_reading(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = fs::OpenOptions::new();
    options.read(true);
    match O_NONBLOCK {
        Some(flag) => {
            options.custom_flags(flag);
        }
        // An unknown Unix: keep the path check before the open; the handle is
        // still checked after it.
        None => {
            let meta = fs::metadata(path)?;
            if !meta.is_file() {
                return Err(std::io::Error::other("not a regular file"));
            }
        }
    }
    options.open(path)
}

/// On Windows opening never waits on a writer; the open handle's type decides.
#[cfg(not(unix))]
fn open_for_reading(path: &Path) -> std::io::Result<fs::File> {
    fs::File::open(path)
}

fn not_regular(shown: &str) -> Error {
    Error::Invalid {
        message: format!(
            "{shown} is not a regular file (a folder, device, FIFO or socket); only regular files are read"
        ),
    }
}

fn check_regular(meta: &fs::Metadata, shown: &str) -> Result<()> {
    if !meta.is_file() {
        return Err(not_regular(shown));
    }
    if meta.len() > MAX_BYTES as u64 {
        return Err(too_large(shown, meta.len()));
    }
    Ok(())
}

/// Read at most `MAX_BYTES` from `reader`; one byte more is a `file_size` bound.
pub fn read_capped(reader: impl Read, shown: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::io(shown.to_string(), e))?;
    if bytes.len() > MAX_BYTES {
        return Err(too_large(shown, bytes.len() as u64));
    }
    Ok(bytes)
}

fn too_large(shown: &str, len: u64) -> Error {
    Error::Bound {
        kind: "file_size".into(),
        location: shown.into(),
        message: format!("file is {len} bytes or more; max is {MAX_BYTES}"),
    }
}

pub fn file_digest(path: &Path) -> Result<String> {
    Ok(sha256_prefixed(&read_bounded_file(path)?))
}

pub fn utf8_text(path: &Path, bytes: &[u8]) -> Result<String> {
    let bytes = if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        &bytes[3..]
    } else {
        bytes
    };
    String::from_utf8(bytes.to_vec()).map_err(|e| Error::Parse {
        path: path.display().to_string(),
        message: format!("not UTF-8: {e}"),
    })
}

pub fn value_depth(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.iter().map(value_depth).max().unwrap_or(0) + 1,
        Value::Object(map) => map.values().map(value_depth).max().unwrap_or(0) + 1,
        _ => 1,
    }
}

pub fn check_depth(value: &Value, location: &str) -> Result<()> {
    let depth = value_depth(value);
    if depth > MAX_DEPTH {
        return Err(Error::Bound {
            kind: "depth".into(),
            location: location.into(),
            message: format!("nesting depth {depth} exceeds {MAX_DEPTH}"),
        });
    }
    Ok(())
}

pub fn canonical_json(value: &Value) -> Result<String> {
    serde_json::to_string(&canonicalize(value)).map_err(|e| Error::Invalid {
        message: format!("canonical JSON failed: {e}"),
    })
}

pub fn canonical_digest(value: &Value) -> Result<String> {
    Ok(sha256_prefixed(canonical_json(value)?.as_bytes()))
}

pub fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for key in keys {
                out.insert(key.clone(), canonicalize(&map[key]));
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

pub fn now_rfc3339_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339_utc(secs)
}

/// True for an RFC 3339 UTC timestamp with whole seconds, `YYYY-MM-DDTHH:MM:SSZ`
/// (the form the migration schema records), with a real calendar date and time.
pub fn is_rfc3339_utc_seconds(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() != 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
        || b[19] != b'Z'
    {
        return false;
    }
    let num = |range: std::ops::Range<usize>| -> Option<u32> {
        let part = &text[range];
        part.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        num(0..4),
        num(5..7),
        num(8..10),
        num(11..13),
        num(14..16),
        num(17..19),
    ) else {
        return false;
    };
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&day) && hour < 24 && minute < 60 && second < 60
}

pub fn format_rfc3339_utc(secs: u64) -> String {
    let (year, month, day) = civil_from_days((secs / 86_400) as i64);
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let min = (rem % 3_600) / 60;
    let sec = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_sorts_object_keys() {
        let value = json!({"b": 1, "a": {"y": 2, "x": 3}});
        assert_eq!(
            canonical_json(&value).unwrap(),
            r#"{"a":{"x":3,"y":2},"b":1}"#
        );
    }

    #[test]
    fn rfc3339_known_epoch_day() {
        assert_eq!(format_rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339_utc(1_725_000_000), "2024-08-30T06:40:00Z");
    }

    /// An endless stream (what `/dev/zero` is) stops one byte past the bound.
    #[test]
    fn bounded_read_stops_on_an_endless_stream() {
        let err = read_capped(std::io::repeat(0), "endless").unwrap_err();
        assert!(matches!(err, Error::Bound { ref kind, .. } if kind == "file_size"));
        assert_eq!(read_capped(&b"{}"[..], "small").unwrap(), b"{}".to_vec());
    }

    #[test]
    fn digest_is_stable() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
