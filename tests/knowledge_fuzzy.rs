//! The knowledge store's typo lookup (CubbyLLM order 0046, 2026-10-31): `knowledge::fuzzy_lookup`, the
//! `knowledge` module's `fuzzy`, and `cubelang knowledge fuzzy`. Pinned: every answer is the one a scan of
//! every live key gives (a reference Levenshtein written here, not the store's), across frozen segments, the
//! top layer, a masked key and an alias; 0 edits is the exact lookup; a hit carries its distance.

use cubelang::compiler;
use cubelang::vm::knowledge::{FuzzyHit, KnowledgeStore, fuzzy_lookup, normalize};
use cubelang::vm::{ExecResult, VM, Value};
use std::process::Command;

/// The reference: Levenshtein on characters, by the full table.
fn lev(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut t = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in 0..=a.len() { t[i][0] = i; }
    for j in 0..=b.len() { t[0][j] = j; }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            t[i][j] = (t[i - 1][j] + 1).min(t[i][j - 1] + 1).min(t[i - 1][j - 1] + usize::from(a[i - 1] != b[j - 1]));
        }
    }
    t[a.len()][b.len()]
}

fn scan(store: &KnowledgeStore, mention: &str, edits: u32) -> Vec<FuzzyHit> {
    let q = normalize(mention);
    let mut out: Vec<FuzzyHit> = store.keys().into_iter()
        .map(|k| (lev(&q, &k) as u32, k))
        .filter(|(e, _)| *e <= edits.min(2))
        .map(|(e, k)| FuzzyHit { facts: store.lookup(&k), key: k, edits: e })
        .collect();
    out.sort_by(|a, b| a.edits.cmp(&b.edits).then_with(|| a.key.cmp(&b.key)));
    out
}

/// A deterministic stream of names: short and long, accented, several words.
struct Names(u64);
impl Names {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn word(&mut self) -> String {
        const A: [&str; 16] = ["a", "e", "i", "o", "u", "r", "s", "t", "n", "l", "m", "b", "é", "ü", "k", "z"];
        let n = 2 + (self.next() % 8) as usize;
        (0..n).map(|_| A[(self.next() % 16) as usize]).collect()
    }
    fn name(&mut self) -> String {
        let w = 1 + (self.next() % 3) as usize;
        (0..w).map(|_| self.word()).collect::<Vec<_>>().join(" ")
    }
    /// `s` with `n` random edits (insert, delete, substitute).
    fn typo(&mut self, s: &str, n: usize) -> String {
        let mut c: Vec<char> = s.chars().collect();
        for _ in 0..n {
            let at = (self.next() as usize) % (c.len() + 1);
            match self.next() % 3 {
                0 => c.insert(at, 'x'),
                1 if at < c.len() => { c.remove(at); }
                _ if at < c.len() => c[at] = 'q',
                _ => c.push('y'),
            }
        }
        c.into_iter().collect()
    }
}

/// Two frozen segments, a top layer, a key masked over a segment, an alias.
fn layered(names: &mut Names) -> (KnowledgeStore, Vec<String>) {
    let mut ks = KnowledgeStore::new();
    let mut all = Vec::new();
    for layer in 0..3 {
        for i in 0..400 {
            let n = names.name();
            ks.insert(&n, format!("fact {layer}.{i}"), format!("src:{layer}.{i}"));
            all.push(n);
        }
        if layer < 2 {
            ks.freeze().unwrap();
        }
    }
    ks.insert("gutenberg", "printer", "dbpedia:Johannes_Gutenberg");
    ks.insert("gutenburg", "a misspelt village", "test:village");
    assert!(ks.alias("gutenberg press", "gutenberg"));
    assert!(ks.remove(&all[3]) >= 1, "a segment key masked by the top layer");
    all.extend(["gutenberg".to_string(), "gutenburg".to_string(), "gutenberg press".to_string()]);
    (ks, all)
}

#[test]
fn every_typo_lookup_answers_as_a_scan_of_every_key_would() {
    let mut names = Names(7);
    let (ks, all) = layered(&mut names);
    let mut checked = 0;
    for i in 0..300 {
        let base = &all[(names.next() as usize) % all.len()];
        let mention = match i % 4 {
            0 => base.clone(),
            1 => names.typo(base, 1),
            2 => names.typo(base, 2),
            _ => names.name(),
        };
        for d in 0..=2 {
            assert_eq!(fuzzy_lookup(&ks, &mention, d, usize::MAX), scan(&ks, &mention, d), "{mention:?} at {d}");
            checked += 1;
        }
    }
    assert_eq!(checked, 900);
    // the masked key is gone at every distance; the alias is a key of its own
    assert!(fuzzy_lookup(&ks, &all[3], 0, 10).is_empty());
    let g = fuzzy_lookup(&ks, "Gutenberg!", 1, 10);
    assert_eq!(g.iter().map(|h| (h.key.as_str(), h.edits)).collect::<Vec<_>>(), vec![("gutenberg", 0), ("gutenburg", 1)]);
    assert_eq!(g[0].facts, ks.lookup("gutenberg"), "0 edits is the exact lookup");
    assert_eq!(fuzzy_lookup(&ks, "gutenberg pres", 1, 10)[0].key, "gutenberg press");
}

#[test]
fn edits_are_capped_at_two_and_the_limit_keeps_the_nearest() {
    let mut names = Names(11);
    let (ks, _) = layered(&mut names);
    assert_eq!(fuzzy_lookup(&ks, "gutenbxxg", 5, usize::MAX), scan(&ks, "gutenbxxg", 2));
    let all = fuzzy_lookup(&ks, "gutenberg", 2, usize::MAX);
    assert_eq!(fuzzy_lookup(&ks, "gutenberg", 2, 1), all[..1].to_vec());
    assert!(fuzzy_lookup(&ks, "", 2, 10).is_empty() && fuzzy_lookup(&ks, "gutenberg", 2, 0).is_empty());
}

#[test]
fn a_long_mention_answers_as_the_scan() {
    let mut names = Names(13);
    let mut ks = KnowledgeStore::new();
    let long: String = (0..120).map(|_| names.word()).collect::<Vec<_>>().join(" ");
    ks.insert(&long, "a long title", "test:long");
    ks.insert("short", "x", "test:short");
    ks.freeze().unwrap();
    let typo = names.typo(&long, 2);
    assert_eq!(fuzzy_lookup(&ks, &typo, 2, 10), scan(&ks, &typo, 2));
    assert_eq!(fuzzy_lookup(&ks, &typo, 2, 10).len(), 1);
}

const FIND: &str = r#"
use knowledge;
program Find implements ISolve {
    public function solve(input: str): str { return input; }
    @external
    public function near(name: str): number {
        let hits = fuzzy(name, 1);
        return hits;
    }
    @external
    public function exact(name: str): number {
        let hits = fuzzy(name, 0, 5);
        return hits;
    }
}
"#;

fn array(r: ExecResult) -> Vec<Value> {
    match r {
        ExecResult::Ok(Value::Array(v)) | ExecResult::Return(Value::Array(v)) => v,
        other => panic!("expected an array, got {:?}", other),
    }
}

fn field<'a>(v: &'a Value, k: &str) -> &'a Value {
    match v {
        Value::Map(m) => m.get(k).unwrap_or_else(|| panic!("no field {k} in {:?}", v)),
        other => panic!("expected a map, got {:?}", other),
    }
}

#[test]
fn a_program_looks_a_name_up_within_an_edit_and_each_hit_says_how_far() {
    let mut vm = VM::new();
    vm.knowledge.insert("gutenberg", "printer", "dbpedia:Johannes_Gutenberg");
    vm.knowledge.insert("gutenburg", "a misspelt village", "test:village");
    vm.knowledge.freeze().unwrap();
    vm.load(compiler::compile_strict(FIND).unwrap_or_else(|e| panic!("{e}")).remove(0));
    let hits = array(vm.call_entry("Find", "near", vec![Value::Str("Gutenbirg".into())]));
    let got: Vec<(String, i64, String)> = hits.iter().map(|h| match (field(h, "key"), field(h, "edits"), field(h, "source")) {
        (Value::Str(k), Value::Int(e), Value::Str(s)) => (k.clone(), *e, s.clone()),
        other => panic!("{:?}", other),
    }).collect();
    assert_eq!(got, vec![("gutenberg".into(), 1, "dbpedia:Johannes_Gutenberg".into()), ("gutenburg".into(), 1, "test:village".into())]);
    assert!(array(vm.call_entry("Find", "exact", vec![Value::Str("Gutenbirg".into())])).is_empty(), "0 edits: exact only");
    assert!(vm.queried.iter().any(|(q, n)| q == "knowledge~1:Gutenbirg" && *n == 2), "{:?}", vm.queried);
}

#[test]
fn the_cli_prints_one_line_per_fact_nearest_first() {
    let d = std::env::temp_dir().join(format!("cubelang-fuzzy-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let facts = d.join("facts.jsonl");
    std::fs::write(&facts, concat!(
        "{\"key\":\"gutenberg\",\"text\":\"printer\",\"source\":\"dbpedia:Johannes_Gutenberg\"}\n",
        "{\"key\":\"gutenburg\",\"text\":\"a misspelt village\",\"source\":\"test:village\"}\n")).unwrap();
    let seg = d.join("facts.cubeseg");
    let bin = env!("CARGO_BIN_EXE_cubelang");
    assert!(Command::new(bin).args(["knowledge", "build", facts.to_str().unwrap(), "-o", seg.to_str().unwrap()]).status().unwrap().success());
    let out = Command::new(bin).args(["knowledge", "fuzzy", seg.to_str().unwrap(), "Gutenberg", "--edits", "1"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let lines: Vec<serde_json::Value> = String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.iter().map(|l| (l["key"].as_str().unwrap(), l["edits"].as_u64().unwrap())).collect::<Vec<_>>(),
               vec![("gutenberg", 0), ("gutenburg", 1)]);
    let _ = std::fs::remove_dir_all(&d);
}
