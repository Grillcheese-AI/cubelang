//! The store's state, named by one hash (CubbyLLM 0055's snapshot addendum, for CheezML v0.2's
//! `cheez:storeSnapshot`): the base manifest's hash, the change log's length and its head hash, hashed together.
//! With no overlay change the log is empty and the snapshot is the base's.
//!
//! The change log (`changes.log`) is append-only, one JSON object a line, each starting with the hash of the line
//! before it (`{"prev": "<blake3 of the previous line>", ...}`; the first line's prev is ""), so the last line's
//! hash stands for the whole log and a line changed anywhere breaks the chain. A pin to an earlier state is a
//! position in the log: `at(dir, n)` is the snapshot after its first n lines. The overlay writes the changes (each
//! line: what changed, the source revision, the loader's code hash, the recorded time); the base file never changes.

use std::io::Write;

pub const LOG: &str = "changes.log";

pub fn line_hash(line: &str) -> String { blake3::hash(line.as_bytes()).to_hex().to_string() }

fn prev_of(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("{\"prev\": \"")?;
    rest.split_once('"').map(|x| x.0)
}

/// The log's lines checked as a chain: (lines, head hash) of its first `upto` lines (all when None).
pub fn chain(dir: &str, upto: Option<usize>) -> Result<(usize, String), String> {
    let text = match std::fs::read_to_string(format!("{dir}/{LOG}")) { Ok(t) => t, Err(_) => String::new() };
    let (mut n, mut head) = (0usize, String::new());
    for (k, line) in text.lines().enumerate() {
        if upto.is_some_and(|u| k >= u) { break; }
        match prev_of(line) {
            Some(p) if p == head => {}
            _ => return Err(format!("{LOG}: line {} does not follow line {k} (the chain is broken)", k + 1)),
        }
        head = line_hash(line); n += 1;
    }
    if let Some(u) = upto { if u > n { return Err(format!("{LOG}: no state after {u} changes (the log has {n})")); } }
    Ok((n, head))
}

/// Appends one change (`body`: a JSON object's members, without braces) after the log's head.
pub fn append(dir: &str, body: &str) -> Result<String, String> {
    let (_, head) = chain(dir, None)?;
    let line = format!("{{\"prev\": \"{head}\", {body}}}");
    if line.contains('\n') { return Err("a change on more than one line".into()); }
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(format!("{dir}/{LOG}")).map_err(|e| e.to_string())?;
    f.write_all(format!("{line}\n").as_bytes()).and_then(|_| f.sync_all()).map_err(|e| e.to_string())?;
    Ok(line_hash(&line))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Snapshot { pub manifest: String, pub changes: usize, pub head: String, pub hash: String }

/// The base's content hash: every file's name and blake3 as the manifest lists them, sorted. A rebuild that writes
/// the same bytes names the same state, whatever its timing (the manifest itself also holds the build's duration).
pub fn base(dir: &str) -> Result<String, String> {
    let m = std::fs::read_to_string(format!("{dir}/manifest.json")).map_err(|e| format!("manifest.json: {e}"))?;
    let mut files: Vec<(String, String)> = Vec::new();
    for part in m.split("{\"file\": \"").skip(1) {
        let name = part.split('"').next().unwrap_or("").to_string();
        let h = part.split("\"blake3\": \"").nth(1).and_then(|x| x.split('"').next()).ok_or(format!("manifest.json: {name} has no hash"))?;
        files.push((name, h.to_string()));
    }
    if files.is_empty() { return Err("manifest.json lists no files".into()); }
    files.sort();
    let text: String = files.iter().map(|(n, h)| format!("{n} {h}\n")).collect();
    Ok(blake3::hash(text.as_bytes()).to_hex().to_string())
}

/// The snapshot after the log's first `upto` changes (all when None).
pub fn at(dir: &str, upto: Option<usize>) -> Result<Snapshot, String> {
    let manifest = base(dir)?;
    let (changes, head) = chain(dir, upto)?;
    let hash = blake3::hash(format!("wdstore snapshot 1\nmanifest {manifest}\nchanges {changes}\nhead {head}\n").as_bytes()).to_hex().to_string();
    Ok(Snapshot { manifest, changes, head, hash })
}

/// The state named `want`, or an error with its code: a read against a named snapshot never reads another.
pub fn expect(dir: &str, want: &str) -> Result<Snapshot, String> {
    let (n, _) = chain(dir, None)?;
    for k in (0..=n).rev() {
        let s = at(dir, Some(k))?;
        if s.hash == want { return Ok(s); }
    }
    Err(format!("snapshot_not_held: {want} is not a state of this store"))
}
