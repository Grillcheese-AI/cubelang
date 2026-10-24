//! ASK inside a called function (2026-10-24), and the arguments of an intra-program call.
//!
//! Before: an ASK in a callee was an error ("ASK inside a CALL is not supported"), so a skill could
//! not call another skill that asks; and an intra-program call bound its arguments only as
//! `arg0..argN`, so `helper(x)` read an unbound `x` and silently returned Int(0).
//!
//! Now a suspension carries the callers waiting on the asker (each one's resume point and
//! registers); `resume` finishes the asker, hands each caller its callee's result, and continues it
//! after its CALL. A frame that asks again re-suspends with the same chain attached.

use cubelang::compiler;
use cubelang::reasoning::{RunRequest, RunResult, run_result};
use cubelang::vm::{ExecResult, Suspension, VM, Value};
use prost::Message;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

const NESTED: &str = r#"
program Nested implements ISolve {
    public function pick(a: number, b: number): number {
        ask "which one", a, b;
        create chosen : number;
        pop chosen;
        return chosen;
    }
    public function pick_twice(a: number, b: number, c: number): number {
        ask "first", a, b;
        create first : number;
        pop first;
        ask "second", b, c;
        create second : number;
        pop second;
        add first, second;
        return first;
    }
    public function middle(a: number, b: number): number {
        create offset : number;
        assign offset = 5;
        let r = pick(a, b);
        add r, offset;
        return r;
    }
    public function echo(x: str): str { return x; }
    public function second_of(x: number, y: number): number { return y; }

    @external
    public function solve(input: str): number {
        create a : number;
        assign a = 100;
        let got = pick(1959, 1969);
        add got, a;
        return got;
    }
    @external
    public function twice(input: str): number {
        create base : number;
        assign base = 1000;
        let got = pick_twice(1, 2, 3);
        add got, base;
        return got;
    }
    @external
    public function deep(input: str): number {
        let v = middle(30, 40);
        return v;
    }
    @external
    public function then_ask(input: str): number {
        let x = pick(10, 20);
        ask "and then", 1, 2;
        create y : number;
        pop y;
        add x, y;
        return x;
    }
    @external
    public function args(input: str): str { return echo(input); }
    @external
    public function two_args(input: str): number { return second_of(7, 8); }
}
"#;

fn load() -> VM {
    let mut progs = compiler::compile_strict(NESTED).unwrap_or_else(|e| panic!("{e}"));
    let mut vm = VM::new();
    vm.load(progs.remove(0));
    vm
}

fn suspended(r: ExecResult) -> Suspension {
    match r {
        ExecResult::Suspend(s) => s,
        other => panic!("expected a suspension, got {other:?}"),
    }
}

fn int(r: ExecResult) -> i64 {
    match r {
        ExecResult::Return(Value::Int(v)) | ExecResult::Ok(Value::Int(v)) => v,
        other => panic!("expected an int, got {other:?}"),
    }
}

#[test]
fn a_callee_can_ask_and_its_caller_continues_with_its_own_registers() {
    let mut vm = load();
    let s = suspended(vm.call("Nested", "solve", vec![Value::Str("q".into())]));
    assert_eq!(s.function, "pick", "the suspension names the function that asked");
    assert_eq!(s.callers.len(), 1);
    assert_eq!(s.callers[0].function, "solve");
    assert!(matches!((&s.candidates[0], &s.candidates[1]), (Value::Int(1959), Value::Int(1969))),
            "the callee's arguments arrived: {:?}", s.candidates);
    // the caller's `a` (100) is not the callee's `a` (1959)
    assert_eq!(int(vm.resume(s.clone(), Value::Int(1969))), 2069);
    let mut vm = load();
    let s = suspended(vm.call("Nested", "solve", vec![Value::Str("q".into())]));
    assert_eq!(int(vm.resume(s, Value::Int(1959))), 2059);
}

#[test]
fn an_invented_answer_is_refused_inside_a_call_too() {
    let mut vm = load();
    let s = suspended(vm.call("Nested", "solve", vec![Value::Str("q".into())]));
    match vm.resume(s, Value::Int(1968)) {
        ExecResult::Error(e) => assert!(e.contains("not among"), "{e}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_callee_that_asks_twice_resumes_twice() {
    let mut vm = load();
    let s1 = suspended(vm.call("Nested", "twice", vec![Value::Str("q".into())]));
    assert!(matches!(&s1.question, Value::Str(q) if q == "first"), "{:?}", s1.question);
    let s2 = suspended(vm.resume(s1, Value::Int(2)));
    assert!(matches!(&s2.question, Value::Str(q) if q == "second"), "{:?}", s2.question);
    assert_eq!(s2.function, "pick_twice");
    assert_eq!(s2.callers.len(), 1, "the waiting caller is still attached");
    assert_eq!(int(vm.resume(s2, Value::Int(3))), 1005);
}

#[test]
fn two_levels_deep() {
    let mut vm = load();
    let s = suspended(vm.call("Nested", "deep", vec![Value::Str("q".into())]));
    let chain: Vec<&str> = s.callers.iter().map(|c| c.function.as_str()).collect();
    assert_eq!(chain, vec!["middle", "deep"], "innermost caller first");
    assert_eq!(int(vm.resume(s, Value::Int(40))), 45, "middle adds its own offset after pick returns");
}

#[test]
fn the_caller_may_ask_after_its_callee_did() {
    let mut vm = load();
    let s1 = suspended(vm.call("Nested", "then_ask", vec![Value::Str("q".into())]));
    assert_eq!(s1.function, "pick");
    let s2 = suspended(vm.resume(s1, Value::Int(20)));
    assert_eq!(s2.function, "then_ask");
    assert!(s2.callers.is_empty(), "the entry function's own ASK is a top-level one");
    assert_eq!(int(vm.resume(s2, Value::Int(2))), 22);
}

#[test]
fn arguments_bind_to_the_callees_declared_names() {
    let mut vm = load();
    match vm.call("Nested", "args", vec![Value::Str("hello".into())]) {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => assert_eq!(s, "hello"),
        other => panic!("`return echo(input)` must return the argument, not Int(0): {other:?}"),
    }
    assert_eq!(int(vm.call("Nested", "two_args", vec![Value::Str("q".into())])), 8);
}

fn proto(req: RunRequest) -> RunResult {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut buf = Vec::new();
    req.encode(&mut buf).unwrap();
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

#[test]
fn over_run_proto_the_answers_replay_through_the_chain() {
    let ask = proto(RunRequest { program: NESTED.into(), fn_name: "twice".into(), args: vec!["q".into()],
                                 ..Default::default() });
    match &ask.result {
        Some(run_result::Result::Suspended(s)) => assert_eq!(s.function, "pick_twice"),
        other => panic!("{other:?}"),
    }
    let done = proto(RunRequest { program: NESTED.into(), fn_name: "twice".into(), args: vec!["q".into()],
                                  answers: vec!["1".into(), "3".into()], ..Default::default() });
    match &done.result {
        Some(run_result::Result::Symbol(s)) => assert_eq!(s, "1004"),
        other => panic!("{other:?}"),
    }
}
