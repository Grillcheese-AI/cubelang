//! VM step 0 (2026-09-29): what the branch runner needs before it trusts a
//! program. Agreed by the round-3 panel, seven or eight of eight.
//!
//! 1. An absent role is Null, structurally (`tests/recover_scope.rs` has the
//!    n=500 probe; here it is checked through the wire).
//! 2. Trace-only mnemonics were already strict errors; the permission
//!    attributes the VM does not honour (`@hook`/`@before`/`@after`/`@cron`)
//!    join them, and the three it does honour are enforced: `@internal` and
//!    `@system` refused as the host's entry, `@once` refused on a second run.
//! 3. Budgets on the request: ops, queries, wall clock, beside the jumps.
//! 4. The report carries the runner-up similarity, the margin and a closed
//!    reason per recover.
use cubelang::compiler;
use cubelang::reasoning::{Budget, RecoverReason, RunRequest, RunResult, run_result};
use cubelang::vm::{ExecResult, VM, Value};
use prost::Message;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn call_proto(req: RunRequest) -> RunResult {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut buf = Vec::new(); req.encode(&mut buf).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(&buf).unwrap();
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    assert!(child.wait().unwrap().success());
    let len = u32::from_be_bytes(out[..4].try_into().unwrap()) as usize;
    RunResult::decode(&out[4..4 + len]).unwrap()
}

fn error_of(r: &RunResult) -> String {
    match &r.result {
        Some(run_result::Result::Error(e)) => e.clone(),
        other => panic!("expected Error, got {:?}", other),
    }
}

// ── 1. absent role through the wire ─────────────────────────────────────────

// `recover` takes its role by NAME identity (a literal role symbol), so the
// two probes are two functions, not one parameterised on the role.
const FRAME_PROGRAM: &str = r#"
use vsa;
program Frame implements ISolve {
    @external
    public function solve(x: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        bind frame, VERB, "chased";
        return recover(frame, SUBJECT);
    }
    @external
    public function absent(x: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        bind frame, VERB, "chased";
        return recover(frame, LOCATION);
    }
}
"#;

#[test]
fn a_present_role_reports_winner_runner_up_and_margin() {
    let r = call_proto(RunRequest { program: FRAME_PROGRAM.into(), args: vec!["x".into()],
                                    fn_name: "solve".into(), ..Default::default() });
    assert!(r.ok, "{:?}", r);
    assert!(matches!(&r.result, Some(run_result::Result::Symbol(s)) if s == "cat"), "{:?}", r.result);
    let sim = r.similarity.expect("similarity");
    let runner = r.runner_up.expect("runner-up");
    assert!(runner < sim);
    assert_eq!(r.recover_reason, Some(RecoverReason::Recovered as i32));
    let rep = r.report.expect("report");
    assert_eq!(rep.recovers.len(), 1);
    let hop = &rep.recovers[0];
    assert_eq!(hop.winner, "cat");
    assert_eq!(hop.role, "SUBJECT");
    // A `create`d register is known to the VM by its hashed key only (the
    // name table holds parameters and bind symbols); the report carries the
    // key, and the declared name where the table knows it.
    assert!(hop.reg == "frame" || hop.reg.starts_with("r_"), "{}", hop.reg);
    assert_eq!(hop.pool, 3);
    assert!((hop.margin.unwrap() - (sim - runner)).abs() < 1e-12);
    assert_eq!(hop.reason, RecoverReason::Recovered as i32);
}

#[test]
fn an_absent_role_is_null_on_the_wire_with_its_reason() {
    let r = call_proto(RunRequest { program: FRAME_PROGRAM.into(), args: vec!["x".into()],
                                    fn_name: "absent".into(), ..Default::default() });
    assert!(r.ok, "{:?}", r);
    assert!(r.result.is_none(), "Null leaves the oneof unset, got {:?}", r.result);
    assert_eq!(r.similarity, None, "no similarity is computed for an absent role");
    assert_eq!(r.runner_up, None);
    assert_eq!(r.recover_reason, Some(RecoverReason::AbsentRole as i32));
    let rep = r.report.expect("report");
    assert_eq!(rep.recovers.len(), 1);
    assert_eq!(rep.recovers[0].reason, RecoverReason::AbsentRole as i32);
    assert_eq!(rep.recovers[0].winner, "");
    assert_eq!(rep.recovers[0].pool, 3, "the frame's own fillers, reported even when unused");
}

// ── 2. attributes ───────────────────────────────────────────────────────────

#[test]
fn unenforced_attributes_are_strict_errors_and_loose_warnings() {
    // `@cron` left this list on 2026-10-24: the scheduler enforces it (tests/contracts.rs).
    for (attr, name) in [("@hook(Other.Event)", "@hook"), ("@before(solve)", "@before"),
                         ("@after(solve)", "@after")] {
        let src = format!(r#"
program P implements ISolve {{
    {attr}
    public function solve(x: str): str {{ return x; }}
}}
"#);
        let err = compiler::compile_strict(&src).err()
            .unwrap_or_else(|| panic!("{name} must be a strict error"));
        assert!(err.contains(name) && err.contains("not enforced"), "{err}");
        assert!(compiler::compile(&src).is_ok(), "{name} still compiles under --loose");
    }
}

#[test]
fn the_enforced_attributes_pass_strict() {
    let src = r#"
program P implements ISolve {
    @system @once
    constructor() { }
    @external
    public function solve(x: str): str { return x; }
    @internal
    function helper(x: str): str { return x; }
}
"#;
    assert!(compiler::compile_strict(src).is_ok());
}

const ACCESS_PROGRAM: &str = r#"
program Access implements ISolve {
    @system @once
    constructor() { }
    @external
    public function solve(x: str): str { return x; }
    @internal
    function helper(x: str): str { return x; }
    @system
    function lifecycle(x: str): str { return x; }
    function unmarked(x: str): str { return x; }
    @once
    function single(x: str): str { return x; }
}
"#;

fn req(fn_name: &str) -> RunRequest {
    RunRequest { program: ACCESS_PROGRAM.into(), args: vec!["v".into()], fn_name: fn_name.into(),
                 ..Default::default() }
}

#[test]
fn internal_and_system_are_refused_as_the_hosts_entry() {
    let r = call_proto(req("solve"));
    assert!(r.ok, "{:?}", r);
    let r = call_proto(req("unmarked"));
    assert!(r.ok, "an unmarked function is callable, as it always was: {:?}", r);
    let r = call_proto(req("helper"));
    assert!(!r.ok);
    assert!(error_of(&r).contains("@internal"), "{:?}", r);
    let r = call_proto(req("lifecycle"));
    assert!(!r.ok);
    assert!(error_of(&r).contains("@system"), "{:?}", r);
}

#[test]
fn once_refuses_a_second_run_in_the_same_vm() {
    let mut progs = compiler::compile_strict(ACCESS_PROGRAM).unwrap();
    let prog = progs.pop().unwrap();
    let mut vm = VM::new();
    vm.load(prog);
    let first = vm.call_entry("Access", "single", vec![Value::Str("a".into())]);
    assert!(matches!(first, ExecResult::Return(Value::Str(ref s)) if s == "a"), "{first:?}");
    let second = vm.call_entry("Access", "single", vec![Value::Str("b".into())]);
    match second {
        ExecResult::Error(e) => assert!(e.contains("@once"), "{e}"),
        other => panic!("a second run of a @once function must be refused, got {other:?}"),
    }
    // The constructor is @once too: the runtime's own second call is refused.
    let _ = vm.init("Access");
    // (call_with_constructor already ran it once per VM in run-proto; here
    // the first init IS the first run, so a second one is the refusal.)
    match vm.init("Access") {
        ExecResult::Error(e) => assert!(e.contains("@once"), "{e}"),
        other => panic!("{other:?}"),
    }
}

// ── 3. budgets ──────────────────────────────────────────────────────────────

const LOOP_PROGRAM: &str = r#"
program Loop implements ISolve {
    @external
    public function solve(x: str): number {
        create count : number;
        assign count = 1000;
        create total : number;
        assign total = 0;
        while (count > 0) {
            add total, 1;
            sub count, 1;
        }
        return total;
    }
}
"#;

const QUERY_PROGRAM: &str = r#"
program Q implements ISolve {
    @external
    public function solve(x: str): number {
        query x;
        query x;
        query x;
        return 3;
    }
}
"#;

#[test]
fn an_op_budget_stops_the_run_and_the_report_says_what_it_had() {
    let unbounded = call_proto(RunRequest { program: LOOP_PROGRAM.into(), args: vec!["v".into()],
                                            fn_name: "solve".into(), ..Default::default() });
    assert!(unbounded.ok, "{:?}", unbounded);
    let ops = unbounded.report.as_ref().unwrap().ops;
    assert!(ops > 1000);

    let r = call_proto(RunRequest { program: LOOP_PROGRAM.into(), args: vec!["v".into()], fn_name: "solve".into(),
                                    budget: Some(Budget { max_ops: 500, ..Default::default() }), ..Default::default() });
    assert!(!r.ok);
    assert!(error_of(&r).contains("op budget"), "{:?}", r);
    let rep = r.report.unwrap();
    assert_eq!(rep.budget.as_ref().unwrap().max_ops, 500);
    assert_eq!(rep.ops, 501, "the refused opcode is the one that crossed the line");
}

#[test]
fn a_jump_budget_on_the_request_supersedes_the_branch_field() {
    let r = call_proto(RunRequest { program: LOOP_PROGRAM.into(), args: vec!["v".into()], fn_name: "solve".into(),
                                    budget: Some(Budget { max_jumps: 10, ..Default::default() }), ..Default::default() });
    assert!(!r.ok);
    assert!(error_of(&r).contains("jump budget"), "{:?}", r);
    assert_eq!(r.report.unwrap().max_jumps, 10);
}

#[test]
fn a_query_budget_refuses_the_query_past_it() {
    let r = call_proto(RunRequest { program: QUERY_PROGRAM.into(), args: vec!["anything".into()], fn_name: "solve".into(),
                                    budget: Some(Budget { max_queries: 2, ..Default::default() }), ..Default::default() });
    assert!(!r.ok);
    assert!(error_of(&r).contains("query budget"), "{:?}", r);
    let rep = r.report.unwrap();
    assert_eq!(rep.queried.len(), 2, "two queries ran, the third was refused before the lookup");
    assert_eq!(rep.budget.unwrap().max_queries, 2);
}

#[test]
fn a_wall_budget_is_reported_and_a_generous_one_does_not_fire() {
    let r = call_proto(RunRequest { program: LOOP_PROGRAM.into(), args: vec!["v".into()], fn_name: "solve".into(),
                                    budget: Some(Budget { max_wall_ms: 10_000, ..Default::default() }), ..Default::default() });
    assert!(r.ok, "{:?}", r);
    let rep = r.report.unwrap();
    assert_eq!(rep.budget.unwrap().max_wall_ms, 10_000);
    assert!(rep.wall_ms < 10_000);
}

#[test]
fn a_wall_budget_fires_on_a_long_run() {
    let mut progs = compiler::compile_strict(LOOP_PROGRAM).unwrap();
    let prog = progs.pop().unwrap();
    let mut vm = VM::new();
    vm.load(prog);
    // A deadline already in the past: the first check (every 64 opcodes)
    // refuses.
    vm.deadline = Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    match vm.call_entry("Loop", "solve", vec![Value::Str("v".into())]) {
        ExecResult::Error(e) => assert!(e.contains("wall budget"), "{e}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn no_budget_means_unlimited_and_the_report_says_so() {
    let r = call_proto(RunRequest { program: LOOP_PROGRAM.into(), args: vec!["v".into()],
                                    fn_name: "solve".into(), ..Default::default() });
    assert!(r.ok);
    let b = r.report.unwrap().budget.unwrap();
    assert_eq!((b.max_ops, b.max_queries, b.max_wall_ms), (0, 0, 0));
    assert_eq!(b.max_jumps, 1_000_000);
}
