//! The layered store (2026-10-24): knowledge in frozen, shared segments; the conversation history
//! searchable by word; both reachable from programs and over run-proto.

use cubelang::compiler;
use cubelang::reasoning::{Branch, RunRequest, RunResult, run_result};
use cubelang::vm::history::{History, SearchMode};
use cubelang::vm::knowledge::KnowledgeStore;
use cubelang::vm::{ExecResult, VM, Value};
use prost::Message;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

const FACTS: &str = concat!(
    "{\"key\":\"printing press\",\"text\":\"Gutenberg, c. 1440\",\"source\":\"dbpedia:Printing_press\"}\n",
    "{\"key\":\"mercury\",\"text\":\"the planet\",\"source\":\"dbpedia:Mercury_(planet)\"}\n",
    "{\"key\":\"mercury\",\"text\":\"the element\",\"source\":\"dbpedia:Mercury_(element)\"}\n",
    "{\"alias\":\"movable type press\",\"of\":\"printing press\"}\n",
);

const QUERY: &str = r#"
program Ground implements ISolve {
    @external
    public function solve(mention: str): number {
        query mention;
        create hits : number;
        pop hits;
        let n = hits.len();
        return n;
    }
}
"#;

const RECALL: &str = r#"
use history;
program Recall implements ISolve {
    public function solve(input: str): str { return input; }
    @external
    public function find(words: str): number {
        let hits = search(words);
        return hits;
    }
    @external
    public function fuzzy(words: str): number {
        let hits = search_fuzzy(words);
        return hits;
    }
    @external
    public function prefix(words: str): number {
        let hits = search_prefix(words, 2);
        return hits;
    }
    @external
    public function one(id: i64): number {
        let t = turn(id);
        return t;
    }
    @external
    public function last(n: i64): number {
        let r = recent(n);
        return r;
    }
    @external
    public function count(words: str): number {
        let hits = search(words);
        let n = hits.len();
        return n;
    }
}
"#;

fn talk() -> History {
    let mut h = History::new();
    h.append("user", "Who invented the printing press?", 1);
    h.append("cubby", "Johannes Gutenberg, around 1440, in Mainz.", 2);
    h.append("user", "And what did Gutenberg print first?", 3);
    h.append("cubby", "The Gutenberg Bible.", 4);
    h
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cubelang-store-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

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

fn turn_ids(hits: &[Value]) -> Vec<i64> {
    hits.iter().map(|h| field(h, "turn").as_i64()).collect()
}

fn recall_vm(h: History) -> VM {
    let mut vm = VM::new();
    vm.history = h;
    vm.load(compiler::compile_strict(RECALL).unwrap_or_else(|e| panic!("{e}")).remove(0));
    vm
}

#[test]
fn a_program_searches_the_history() {
    let mut h = talk();
    h.freeze().unwrap();
    h.append("user", "Thanks, that settles it.", 5);
    let mut vm = recall_vm(h);
    let hits = array(vm.call_entry("Recall", "find", vec![Value::Str("Gutenberg".into())]));
    assert_eq!(turn_ids(&hits), vec![4, 3, 2]);
    assert!(matches!(field(&hits[0], "text"), Value::Str(s) if s == "The Gutenberg Bible."));
    assert!(matches!(field(&hits[0], "speaker"), Value::Str(s) if s == "cubby"));
    assert_eq!(turn_ids(&array(vm.call_entry("Recall", "fuzzy", vec![Value::Str("gutenburg bibble".into())]))), vec![4]);
    assert_eq!(turn_ids(&array(vm.call_entry("Recall", "prefix", vec![Value::Str("guten".into())]))), vec![4, 3],
               "a cap of 2");
    assert_eq!(turn_ids(&array(vm.call_entry("Recall", "find", vec![Value::Str("settles".into())]))), vec![5],
               "the turn not yet frozen");
    match vm.call_entry("Recall", "one", vec![Value::Int(2)]) {
        ExecResult::Ok(t) | ExecResult::Return(t) => assert!(matches!(field(&t, "text"), Value::Str(s) if s.contains("Mainz"))),
        other => panic!("{:?}", other),
    }
    assert_eq!(turn_ids(&array(vm.call_entry("Recall", "last", vec![Value::Int(2)]))), vec![5, 4]);
    assert!(vm.queried.iter().any(|(k, n)| k == "history:Gutenberg" && *n == 3), "{:?}", vm.queried);
}

#[test]
fn a_history_search_spends_the_query_budget() {
    let mut vm = recall_vm(talk());
    vm.query_budget = 1;
    assert_eq!(array(vm.call_entry("Recall", "find", vec![Value::Str("gutenberg".into())])).len(), 3);
    match vm.call_entry("Recall", "find", vec![Value::Str("gutenberg".into())]) {
        ExecResult::Error(e) => assert!(e.contains("query budget exhausted"), "{e}"),
        other => panic!("{:?}", other),
    }
}

#[test]
fn a_forgotten_turn_is_not_found() {
    let mut h = talk();
    h.freeze().unwrap();
    assert!(h.forget(3));
    let mut vm = recall_vm(h);
    assert_eq!(turn_ids(&array(vm.call_entry("Recall", "find", vec![Value::Str("gutenberg".into())]))), vec![4, 2]);
    match vm.call_entry("Recall", "one", vec![Value::Int(3)]) {
        ExecResult::Ok(Value::Null) | ExecResult::Return(Value::Null) => {}
        other => panic!("{:?}", other),
    }
}

#[test]
fn a_forked_store_shares_its_segment() {
    let mut base = KnowledgeStore::new();
    for i in 0..20_000 {
        base.insert(&format!("entity {i}"), format!("fact about entity {i}"), "synthetic");
    }
    base.compact().unwrap();
    let bytes = base.segment_bytes();
    let mut forks: Vec<KnowledgeStore> = (0..100).map(|_| base.clone()).collect();
    for (i, f) in forks.iter_mut().enumerate() {
        assert_eq!(f.remove(&format!("entity {i}")), 1);
        assert_eq!(f.top_len(), 1, "a fork holds only its own change");
    }
    assert_eq!(base.segment_bytes(), bytes);
    assert_eq!(base.lookup("entity 7").len(), 1);
    assert!(forks[7].lookup("entity 7").is_empty());
    assert_eq!(forks[8].lookup("entity 7").len(), 1);
}

fn call_proto(req: RunRequest) -> RunResult {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut buf = Vec::new();
    req.encode(&mut buf).unwrap();
    stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(&buf).unwrap();
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    child.wait().unwrap();
    let len = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
    RunResult::decode(&out[4..4 + len]).unwrap()
}

fn symbol(r: &RunResult) -> String {
    match &r.result {
        Some(run_result::Result::Symbol(s)) => s.clone(),
        other => panic!("expected a symbol, got {:?} (ok={})", other, r.ok),
    }
}

fn cli(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cubelang")).args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn run_proto_reads_a_segment_file_and_a_history_directory() {
    let dir = temp("proto");
    let facts = dir.join("facts.jsonl");
    std::fs::write(&facts, FACTS).unwrap();
    let seg = dir.join("facts.cubeseg");
    let (ok, out, err) = cli(&["knowledge", "build", facts.to_str().unwrap(), "-o", seg.to_str().unwrap()]);
    assert!(ok, "{out}{err}");
    assert!(out.contains("3 facts and 1 aliases -> 3 keys"), "{out}");

    let ground = |mention: &str, branch: Option<Branch>| call_proto(RunRequest {
        program: QUERY.into(), fn_name: "solve".into(), args: vec![mention.into()],
        knowledge_path: seg.to_str().unwrap().into(), branch, ..Default::default()
    });
    assert_eq!(symbol(&ground("Mercury", None)), "2");
    assert_eq!(symbol(&ground("movable-type press", None)), "1");
    let excluded = ground("movable type press", Some(Branch { id: "b".into(), exclude: vec!["printing press".into()], ..Default::default() }));
    assert_eq!(symbol(&excluded), "0", "the branch's exclusion took the alias with it");
    assert_eq!(excluded.report.unwrap().excluded, 2);

    let hist = dir.join("history");
    for (speaker, text) in [("user", "Who invented the printing press?"), ("cubby", "Johannes Gutenberg, around 1440.")] {
        let (ok, out, err) = cli(&["history", "append", hist.to_str().unwrap(), "--speaker", speaker, text]);
        assert!(ok, "{out}{err}");
    }
    let res = call_proto(RunRequest {
        program: RECALL.into(), fn_name: "count".into(), args: vec!["gutenburg".into()],
        history_path: hist.to_str().unwrap().into(), ..Default::default()
    });
    assert_eq!(symbol(&res), "0", "exact search: the typo finds nothing");
    let res = call_proto(RunRequest {
        program: RECALL.into(), fn_name: "count".into(), args: vec!["gutenberg".into()],
        history_path: hist.to_str().unwrap().into(), ..Default::default()
    });
    assert_eq!(symbol(&res), "1");
    let missing = call_proto(RunRequest {
        program: RECALL.into(), fn_name: "count".into(), args: vec!["x".into()],
        history_path: dir.join("nope").to_str().unwrap().into(), ..Default::default()
    });
    assert!(!missing.ok, "a history that is not there is an error, not an empty one");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_history_cli_appends_searches_forgets_and_collects() {
    let dir = temp("cli");
    let h = dir.join("h");
    let hs = h.to_str().unwrap();
    for (i, text) in ["Who invented the printing press?", "Johannes Gutenberg, around 1440, in Mainz.",
                      "And what did Gutenberg print first?", "The Gutenberg Bible."].iter().enumerate() {
        let (ok, out, err) = cli(&["history", "append", hs, "--speaker", if i % 2 == 0 { "user" } else { "cubby" }, text]);
        assert!(ok, "{out}{err}");
        assert_eq!(out.trim(), (i + 1).to_string());
    }
    let (ok, out, _) = cli(&["history", "search", hs, "gutenburg", "--fuzzy"]);
    assert!(ok);
    assert_eq!(out.lines().count(), 3, "{out}");
    let (ok, _, _) = cli(&["history", "forget", hs, "2"]);
    assert!(ok);
    let (_, out, _) = cli(&["history", "search", hs, "mainz"]);
    assert_eq!(out.lines().count(), 0, "forgotten at once: {out}");
    let (ok, _, _) = cli(&["history", "merge", hs]);
    assert!(ok);
    let (ok, out, _) = cli(&["history", "gc", hs]);
    assert!(ok);
    assert!(out.contains("removed"), "{out}");
    for e in std::fs::read_dir(&h).unwrap() {
        let bytes = std::fs::read(e.unwrap().path()).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("Mainz"), "the forgotten turn is out of every file");
    }
    let (_, out, _) = cli(&["history", "info", hs]);
    assert!(out.contains("3 turns, 1 segment(s)"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn search_modes_on_a_frozen_history() {
    let mut h = talk();
    h.freeze().unwrap();
    let ids = |q: &str, m: SearchMode| h.search(q, m, 10).iter().map(|x| x.turn.id).collect::<Vec<_>>();
    assert_eq!(ids("printing press", SearchMode::Exact), vec![1]);
    assert_eq!(ids("PRINTING-press!", SearchMode::Exact), vec![1], "normalized like the knowledge keys");
    assert_eq!(ids("bib", SearchMode::Prefix), vec![4]);
    assert_eq!(ids("johanes", SearchMode::Fuzzy), vec![2]);
    assert!(ids("", SearchMode::Exact).is_empty());
}
