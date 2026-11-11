//! One typo index over a store built as shards (CubbyLLM order 0046, 2026-11-04).
//!
//! A names store of ~10^8 keys is built as shards split by a hash of the key, so every shard holds keys under
//! every prefix. A typo lookup walks a Levenshtein row down an index, and the prefixes it walks near the root are
//! the same in each shard: over 32 shards the same walk is done 32 times. The typo index is every shard's keys in
//! one `fst::Set` (keys only, no facts), built once beside the shards (`cubelang knowledge typo-index DIR`), so
//! the walk is done once; facts are still read from the shards. It answers for the shards it was built over, by
//! file name and byte size (`_typo.json`); against any other set of shards it is not used, and the store says so.
//!
//! Not QUERY: the exact lookup never reads it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fst::{IntoStreamer, Streamer};

use crate::vm::knowledge::{KnowledgeSegment, KnowledgeStore};
use crate::vm::segment::Bytes;

/// The index's file and its manifest, beside the shards.
pub const FILE: &str = "_typo.fst";
pub const MANIFEST: &str = "_typo.json";

/// Every key of the shards it was built over, once.
pub struct TypoIndex {
    pub(crate) set: fst::Set<Bytes>,
    /// How many of the store's layers (the first ones) it answers for.
    pub(crate) covers: usize,
}

impl std::fmt::Debug for TypoIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypoIndex({} keys over {} layers)", self.set.len(), self.covers)
    }
}

/// The shard files of a directory, by name: every `.cubeseg` in it.
pub fn shards(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut segs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: cannot read: {}", dir.display(), e))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "cubeseg"))
        .collect();
    if segs.is_empty() {
        return Err(format!("{}: a directory with no .cubeseg in it", dir.display()));
    }
    segs.sort();
    Ok(segs)
}

/// (file name, bytes) of each shard, in order: what an index answers for.
fn identity(paths: &[PathBuf]) -> Result<serde_json::Value, String> {
    let mut out = Vec::new();
    for p in paths {
        let len = std::fs::metadata(p).map_err(|e| format!("{}: {}", p.display(), e))?.len();
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(serde_json::json!([name, len]));
    }
    Ok(serde_json::Value::Array(out))
}

/// Build the index over `segs` (the shards at `paths`, opened) into `dir`: the keys of every shard, merged in
/// order, streamed to the file. Returns the number of keys.
pub fn build(segs: &[KnowledgeSegment], paths: &[PathBuf], dir: &Path) -> Result<u64, String> {
    let tmp = dir.join(format!("{FILE}.tmp"));
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    let mut b = fst::SetBuilder::new(std::io::BufWriter::new(file)).map_err(|e| e.to_string())?;
    let mut op = fst::map::OpBuilder::new();
    for s in segs {
        op.push(s.index().stream());
    }
    let mut union = op.union().into_stream();
    let mut n = 0u64;
    while let Some((k, _)) = union.next() {
        b.insert(k).map_err(|e| e.to_string())?;
        n += 1;
    }
    let mut w = b.into_inner().map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    drop(w);
    std::fs::rename(&tmp, dir.join(FILE)).map_err(|e| e.to_string())?;
    let man = serde_json::json!({"shards": identity(paths)?, "keys": n});
    crate::vm::segment::write_atomic(&dir.join(MANIFEST), man.to_string().as_bytes()).map_err(|e| e.to_string())?;
    Ok(n)
}

/// The index for the shards at `paths`, if `dir` holds one built over exactly them. `Ok(None)`: none there;
/// `Err`: one there, built over other shards (or unreadable), so not used.
pub fn open(dir: &Path, paths: &[PathBuf]) -> Result<Option<TypoIndex>, String> {
    let (f, m) = (dir.join(FILE), dir.join(MANIFEST));
    if !f.exists() {
        return Ok(None);
    }
    let man: serde_json::Value = std::fs::read_to_string(&m).ok().and_then(|s| serde_json::from_str(&s).ok())
        .ok_or_else(|| format!("{}: no readable manifest; the typo index is not used", m.display()))?;
    if man["shards"] != identity(paths)? {
        return Err(format!("{}: built over other shards; the typo index is not used", f.display()));
    }
    let data = std::fs::read(&f).map_err(|e| format!("{}: {}", f.display(), e))?;
    let set = fst::Set::new(Bytes::new(Arc::from(data.into_boxed_slice()))).map_err(|e| format!("{}: {}", f.display(), e))?;
    Ok(Some(TypoIndex { set, covers: paths.len() }))
}

/// A directory of shards as one store, with its typo index when it holds a current one. The note says why an
/// index there was not used.
pub fn open_dir(dir: &Path) -> Result<(KnowledgeStore, Option<String>), String> {
    let paths = shards(dir)?;
    let mut store = KnowledgeStore::open_layers(&paths)?;
    let note = match open(dir, &paths) {
        Ok(Some(t)) => {
            store.set_typo_index(t);
            None
        }
        Ok(None) => None,
        Err(e) => Some(e),
    };
    Ok((store, note))
}
