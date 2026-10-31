//! The branch on the wire (2026-09-28): the counterfactual fork.
//!
//! The host enumerates branches; each is one RunRequest carrying a `Branch`
//! (keys to exclude, facts to assume) applied to that request's CLONE of the
//! cached knowledge store. Every request is a fresh VM, so "X did not happen"
//! lives in one branch and nowhere else -- the next request on the same
//! resident process sees the store untouched. Every result carries a
//! `BranchReport`: the ops and jumps the branch cost and every QUERY it made,
//! so the host compares branches by what they did, not only by what they
//! returned.
use cubelang::reasoning::{Branch, RunRequest, RunResult, run_result};
use cubelang::vm::{ExecResult, VM, Value};
use prost::Message;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn call_proto_many(reqs: Vec<RunRequest>) -> Vec<RunResult> {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let n = reqs.len();
    let mut stdin = child.stdin.take().unwrap();
    for req in reqs {
        let mut buf = Vec::new(); req.encode(&mut buf).unwrap();
        stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
        stdin.write_all(&buf).unwrap();
        stdin.flush().unwrap();
    }
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert!(child.wait().unwrap().success());
    let mut results = Vec::new();
    let mut at = 0usize;
    while at < out.len() {
        let len = u32::from_be_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        results.push(RunResult::decode(&out[at + 4..at + 4 + len]).unwrap());
        at += 4 + len;
    }
    assert_eq!(results.len(), n);
    results
}

/// Counts the facts the store holds for a mention: 0 = it abstained.
const COUNT_PROGRAM: &str = r#"
program Count implements ISolve {
    @external
    public function solve(mention: str): number {
        query mention;
        create ctx : number;
        pop ctx;
        let n = ctx.len();
        return n;
    }
}
"#;

fn symbol_of(r: &RunResult) -> String {
    match &r.result {
        Some(run_result::Result::Symbol(s)) => s.clone(),
        other => panic!("expected Symbol, got {:?}", other),
    }
}

fn facts_file(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("cubelang_branch_{}_{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("history.jsonl");
    std::fs::write(&path, concat!(
        r#"{"key":"siege of vienna","text":"1683, the Ottoman siege lifted","source":"store"}"#, "\n",
        r#"{"alias":"vienna 1683","of":"siege of vienna"}"#, "\n",
        r#"{"key":"treaty of karlowitz","text":"1699, after the siege","source":"store"}"#, "\n",
    )).unwrap();
    path.to_string_lossy().to_string()
}

fn req(kp: &str, mention: &str, branch: Option<Branch>) -> RunRequest {
    RunRequest { program: COUNT_PROGRAM.into(), args: vec![mention.into()], fn_name: "solve".into(),
                 knowledge_path: kp.into(), branch, ..Default::default() }
}

#[test]
fn a_branch_excludes_a_key_and_its_aliases_in_its_own_vm_only() {
    let kp = facts_file("exclude");
    let without = Branch { id: "without_siege".into(), exclude: vec!["Siege of Vienna".into()], assume: vec![], max_jumps: 0 };
    let res = call_proto_many(vec![
        req(&kp, "siege of vienna", None),                     // actual          -> 1
        req(&kp, "siege of vienna", Some(without.clone())),    // the branch      -> 0 (abstains)
        req(&kp, "vienna 1683", Some(without.clone())),        // its alias too   -> 0
        req(&kp, "treaty of karlowitz", Some(without.clone())),// the rest stays  -> 1
        req(&kp, "siege of vienna", None),                     // the store after -> 1: untouched
    ]);
    assert!(res.iter().all(|r| r.ok), "{:?}", res);
    let got: Vec<String> = res.iter().map(symbol_of).collect();
    assert_eq!(got, vec!["1", "0", "0", "1", "1"]);
    let rep = res[1].report.as_ref().expect("every result carries a report");
    assert_eq!(rep.branch_id, "without_siege");
    assert_eq!(rep.excluded, 2, "the key and its one alias");
    assert_eq!(rep.excluded_keys, vec!["siege of vienna".to_string(), "siege of vienna".to_string()],
               "echoed back as applied: once for the key, once for the alias it took with it");
    assert_eq!(rep.max_jumps, 1_000_000);
    assert_eq!(rep.assumed, 0);
    assert_eq!(rep.queried.len(), 1);
    assert_eq!((rep.queried[0].key.as_str(), rep.queried[0].hits), ("siege of vienna", 0));
    assert!(rep.ops > 0);
    assert_eq!(res[0].report.as_ref().unwrap().queried[0].hits, 1);
    assert_eq!(res[0].report.as_ref().unwrap().branch_id, "");
}

#[test]
fn a_branch_assumes_a_fact_and_a_bad_assumption_is_that_branchs_error() {
    let kp = facts_file("assume");
    let with = Branch { id: "with_peace".into(), exclude: vec![],
                        assume: vec![r#"{"key":"peace of 1684","text":"assumed","source":"branch"}"#.into()], max_jumps: 0 };
    let bad = Branch { id: "bad".into(), exclude: vec![], assume: vec![r#"{"key":"x","text":"no source"}"#.into()], max_jumps: 0 };
    let res = call_proto_many(vec![
        req(&kp, "peace of 1684", Some(with.clone())),   // assumed in the branch -> 1
        req(&kp, "peace of 1684", None),                 // not in the store       -> 0
        req(&kp, "peace of 1684", Some(bad)),            // an unattributed fact is refused
        req(&kp, "siege of vienna", None),               // and the process keeps serving
    ]);
    assert_eq!(symbol_of(&res[0]), "1");
    assert_eq!(res[0].report.as_ref().unwrap().assumed, 1);
    assert_eq!(symbol_of(&res[1]), "0");
    assert!(!res[2].ok);
    match &res[2].result {
        Some(run_result::Result::Error(e)) => assert!(e.contains("branch bad: assume") && e.contains("source"), "{}", e),
        other => panic!("expected an error, got {:?}", other),
    }
    assert_eq!(res[2].report.as_ref().unwrap().branch_id, "bad");
    assert_eq!(symbol_of(&res[3]), "1");
}

#[test]
fn the_report_counts_ops_and_jumps_of_a_loop() {
    let program = r#"
program Loop implements ISolve {
    @external
    public function solve(input: str): number {
        create total : number;
        assign total = 0;
        create count : number;
        assign count = 5;
        while (count > 0) {
            add total, 7;
            sub count, 1;
        }
        return total;
    }
}
"#;
    let r = &call_proto_many(vec![RunRequest { program: program.into(), args: vec!["_".into()],
                                               fn_name: "solve".into(), ..Default::default() }])[0];
    assert_eq!(symbol_of(r), "35");
    let rep = r.report.as_ref().unwrap();
    assert!(rep.jumps >= 5, "five back-edges at least, got {}", rep.jumps);
    assert!(rep.ops > rep.jumps);
    assert!(rep.queried.is_empty());
}

#[test]
fn a_branch_sets_the_jump_budget_and_exhaustion_is_that_branchs_error() {
    let program = r#"
program Loop implements ISolve {
    @external
    public function solve(input: str): number {
        create count : number;
        assign count = 50;
        while (count > 0) {
            sub count, 1;
        }
        return count;
    }
}
"#;
    let mk = |max_jumps: u64| RunRequest { program: program.into(), args: vec!["_".into()], fn_name: "solve".into(),
        branch: Some(Branch { id: format!("j{}", max_jumps), exclude: vec![], assume: vec![], max_jumps }), ..Default::default() };
    let res = call_proto_many(vec![mk(0), mk(10), mk(1000)]);
    assert_eq!(symbol_of(&res[0]), "0");
    assert_eq!(res[0].report.as_ref().unwrap().max_jumps, 1_000_000, "0 = the default");
    assert!(!res[1].ok, "ten jumps cannot run fifty iterations");
    match &res[1].result {
        Some(run_result::Result::Error(e)) => assert!(e.contains("jump budget exhausted"), "{}", e),
        other => panic!("expected the budget error, got {:?}", other),
    }
    let rep = res[1].report.as_ref().unwrap();
    assert_eq!((rep.max_jumps, rep.jumps), (10, 10), "the report keeps what the exhausted branch cost");
    assert_eq!(symbol_of(&res[2]), "0");
    assert_eq!(res[2].report.as_ref().unwrap().max_jumps, 1000);
}

#[test]
fn snapshot_and_restore_put_the_program_visible_state_back_and_keep_the_counters() {
    let mut vm = VM::new();
    vm.knowledge.insert("a", "fact a", "t");
    vm.registers.insert("r".into(), Value::Int(1));
    vm.storage.insert("k".into(), Value::Str("v".into()));
    let snap = vm.snapshot();
    // a branch: mutate everything, and run something so the counters move
    vm.knowledge.remove("a");
    vm.knowledge.insert("b", "fact b", "t");
    vm.registers.insert("r".into(), Value::Int(2));
    vm.storage.insert("k".into(), Value::Str("changed".into()));
    let programs = cubelang::compiler::compile_strict(COUNT_PROGRAM).unwrap();
    for p in programs { vm.load(p); }
    let out = vm.call("Count", "solve", vec![Value::Str("b".into())]);
    assert!(matches!(out, ExecResult::Ok(Value::Int(1)) | ExecResult::Return(Value::Int(1))), "{:?}", out);
    let ops = vm.ops_executed;
    assert!(ops > 0 && vm.queried.len() == 1);
    vm.restore(snap);
    assert_eq!(vm.knowledge.lookup("a").len(), 1);
    assert_eq!(vm.knowledge.lookup("b").len(), 0);
    assert!(matches!(vm.registers.get("r"), Some(Value::Int(1))));
    assert!(matches!(vm.storage.get("k"), Some(Value::Str(s)) if s == "v"));
    assert_eq!(vm.ops_executed, ops, "a discarded branch still ran: the counters keep what it cost");
    assert_eq!(vm.queried.len(), 1);
}
