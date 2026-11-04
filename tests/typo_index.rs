//! One typo index over a store's shards (CubbyLLM order 0046, 2026-11-04): `vm::typo_index` and `cubelang
//! knowledge typo-index`. Pinned: a store opened with the index answers every typo lookup as the same shards
//! without it do; an index built over other shards is not used, and the store says so; a layer added after the
//! index (a fork's freeze) is still searched; the CLI builds it and `batch` uses it.

use std::path::{Path, PathBuf};
use std::process::Command;

use cubelang::vm::knowledge::{KnowledgeStore, fuzzy_lookup};
use cubelang::vm::typo_index;

struct Names(u64);
impl Names {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn ch(&mut self) -> char {
        const A: [char; 16] = ['a', 'e', 'i', 'o', 'u', 'r', 's', 't', 'n', 'l', 'm', 'b', 'é', 'ü', 'k', ' '];
        A[(self.next() % 16) as usize]
    }
    fn name(&mut self) -> String {
        let n = 2 + (self.next() % 12) as usize;
        let s: String = (0..n).map(|_| self.ch()).collect();
        let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if s.is_empty() { "a".into() } else { s }
    }
    fn typo(&mut self, s: &str, n: usize) -> String {
        let mut c: Vec<char> = s.chars().collect();
        for _ in 0..n {
            let at = (self.next() as usize) % (c.len() + 1);
            let x = self.ch();
            match self.next() % 3 {
                0 => c.insert(at, x),
                _ if at < c.len() && self.next() % 2 == 0 => { c.remove(at); }
                _ if at < c.len() => c[at] = x,
                _ => c.push(x),
            }
        }
        c.into_iter().collect()
    }
}

/// Four shards, keys split by a hash as the names store is, written to `dir`; every name, for the queries.
fn shards(dir: &Path, names: &mut Names) -> (Vec<PathBuf>, Vec<String>) {
    std::fs::create_dir_all(dir).unwrap();
    let mut stores: Vec<KnowledgeStore> = (0..4).map(|_| KnowledgeStore::new()).collect();
    let mut all = Vec::new();
    for i in 0..3000 {
        let n = names.name();
        let h = n.bytes().fold(7u64, |h, b| h.wrapping_mul(31).wrapping_add(b as u64));
        stores[(h % 4) as usize].insert(&n, format!("Q{i}"), "test:names");
        all.push(n);
    }
    let mut paths = Vec::new();
    for (i, s) in stores.iter().enumerate() {
        let p = dir.join(format!("shard_{i:02}.cubeseg"));
        s.to_segment().unwrap().write(&p).unwrap();
        paths.push(p);
    }
    (paths, all)
}

fn tmp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cubelang-typo-{}-{}", tag, std::process::id()))
}

#[test]
fn the_index_answers_every_typo_lookup_as_the_shards_do() {
    let dir = tmp("same");
    let mut names = Names(11);
    let (paths, all) = shards(&dir, &mut names);
    let plain = KnowledgeStore::open_layers(&paths).unwrap();
    let segs: Vec<_> = paths.iter().map(|p| cubelang::vm::knowledge::KnowledgeSegment::open(p).unwrap()).collect();
    let n = typo_index::build(&segs, &paths, &dir).unwrap();
    assert!(n > 2000 && n <= 3000, "{n} keys");
    let (indexed, note) = typo_index::open_dir(&dir).unwrap();
    assert!(indexed.has_typo_index() && note.is_none() && !plain.has_typo_index());
    for i in 0..400 {
        let q = names.typo(&all[i * 7 % all.len()], 1 + i % 2);
        for d in 0..=2 {
            assert_eq!(fuzzy_lookup(&indexed, &q, d, usize::MAX), fuzzy_lookup(&plain, &q, d, usize::MAX), "{q:?} at {d}");
            assert_eq!(fuzzy_lookup(&indexed, &q, d, 5), fuzzy_lookup(&plain, &q, d, 5), "{q:?} at {d}, limit 5");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_index_over_other_shards_is_not_used_and_a_later_layer_is_still_searched() {
    let dir = tmp("stale");
    let mut names = Names(5);
    let (paths, _) = shards(&dir, &mut names);
    let segs: Vec<_> = paths.iter().map(|p| cubelang::vm::knowledge::KnowledgeSegment::open(p).unwrap()).collect();
    typo_index::build(&segs, &paths, &dir).unwrap();
    // a fork that freezes a new layer over the indexed shards still finds its keys by typo
    let (mut fork, _) = typo_index::open_dir(&dir).unwrap();
    fork.insert("gutenberg", "printer", "test:fork");
    fork.freeze().unwrap();
    assert!(fork.has_typo_index());
    assert_eq!(fuzzy_lookup(&fork, "gutenbirg", 1, 10).iter().map(|h| h.key.as_str()).collect::<Vec<_>>(), vec!["gutenberg"]);
    // a shard rewritten after the index was built: the index is not used, and the store says why
    let mut extra = KnowledgeStore::new();
    extra.insert("a name added later", "Q9", "test:names");
    extra.to_segment().unwrap().write(&dir.join("shard_04.cubeseg")).unwrap();
    let (store, note) = typo_index::open_dir(&dir).unwrap();
    assert!(!store.has_typo_index());
    assert!(note.unwrap().contains("other shards"));
    assert_eq!(fuzzy_lookup(&store, "a name added latr", 1, 10).len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_cli_builds_the_index_and_batch_reads_through_it() {
    let dir = tmp("cli");
    let mut names = Names(3);
    let (_, all) = shards(&dir, &mut names);
    let bin = env!("CARGO_BIN_EXE_cubelang");
    let out = Command::new(bin).args(["knowledge", "typo-index", dir.to_str().unwrap()]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(r["shards"], 4);
    let q = dir.join("q.jsonl");
    std::fs::write(&q, format!("{}\n", serde_json::json!({"id": "a", "key": names.typo(&all[0], 1)}))).unwrap();
    let out = Command::new(bin).args(["knowledge", "batch", dir.to_str().unwrap(), q.to_str().unwrap(), "--edits", "1"])
        .output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stderr).contains("typo index"), "{}", String::from_utf8_lossy(&out.stderr));
    let r: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(r["near"].as_array().unwrap().iter().any(|h| h["key"] == all[0].as_str()));
    let _ = std::fs::remove_dir_all(&dir);
}
