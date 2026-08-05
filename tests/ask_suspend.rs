//! ASK / Suspend / resume — the third execution outcome.
//!
//! A program that has grounded several real candidates and cannot choose
//! between them must neither pick one (a confabulation) nor fail (nothing went
//! wrong). It suspends, hands the question outward, and resumes when told.
//!
//! THE CALLER IS THE MODEL, NOT A HUMAN. The trunk renders the question and
//! candidates into language, the user answers in language, and the trunk maps
//! that answer back to one of the candidates. Because the trunk only ever
//! *selects* (never *supplies*), the selection is checkable — and `resume`
//! checks it.

use cubelang::compiler;
use cubelang::vm::{ExecResult, Suspension, VM, Value};

const ASK_PROGRAM: &str = r#"
program AskMin {
    storage { asked: mutable u64 = 0; }

    @system @once
    public function constructor() { assign asked = 0; }

    @external
    public function solve(): number {
        create luna : number;
        assign luna = 1959;
        create apollo : number;
        assign apollo = 1969;
        ask "which moon landing did you mean", luna, apollo;
        create chosen : number;
        pop chosen;
        sum chosen;
        return chosen;
    }
}
"#;

fn load() -> (VM, String) {
    let mut progs = compiler::compile(ASK_PROGRAM).expect("compile");
    let prog = progs.pop().expect("one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    (vm, name)
}

fn suspend(vm: &mut VM, name: &str) -> Suspension {
    match vm.call(name, "solve", vec![]) {
        ExecResult::Suspend(s) => s,
        other => panic!("expected Suspend, got {:?}", other),
    }
}

#[test]
fn ask_suspends_with_both_candidates_and_picks_neither() {
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);

    assert_eq!(s.candidates.len(), 2, "both grounded candidates surface");
    assert_eq!(s.function, "solve");
    assert_eq!(s.program, name);
    match (&s.candidates[0], &s.candidates[1]) {
        (Value::Int(a), Value::Int(b)) => assert_eq!((*a, *b), (1959i64, 1969i64)),
        other => panic!("unexpected candidates: {:?}", other),
    }
}

#[test]
fn resume_binds_the_chosen_candidate() {
    // Answering with the Apollo candidate must return it, not the other.
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);
    let apollo = s.candidates[1].clone();

    match vm.resume(s, apollo) {
        ExecResult::Ok(Value::Int(v)) | ExecResult::Return(Value::Int(v)) => assert_eq!(v, 1969),
        other => panic!("expected 1969, got {:?}", other),
    }
}

#[test]
fn resume_with_the_other_candidate_returns_the_other() {
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);
    let luna = s.candidates[0].clone();

    match vm.resume(s, luna) {
        ExecResult::Ok(Value::Int(v)) | ExecResult::Return(Value::Int(v)) => assert_eq!(v, 1959),
        other => panic!("expected 1959, got {:?}", other),
    }
}

#[test]
fn resume_rejects_an_invented_selection() {
    // THE GUARANTEE. The trunk is the confabulating component; here it may only
    // *choose*, never *supply*. A value that was never offered is rejected by
    // the VM rather than silently believed.
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);

    match vm.resume(s, Value::Int(1968)) {
        ExecResult::Error(e) => {
            assert!(e.contains("not among"), "unexpected error text: {}", e);
            assert!(e.contains("chosen, not invented"), "unexpected error text: {}", e);
        }
        other => panic!("a fabricated selection must be rejected, got {:?}", other),
    }
}

#[test]
fn resume_rejects_null() {
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);
    match vm.resume(s, Value::Null) {
        ExecResult::Error(_) => {}
        other => panic!("Null is not a candidate; must reject, got {:?}", other),
    }
}

#[test]
fn suspension_is_self_describing() {
    // `call()` restores current_program on the way out, so the VM has forgotten
    // it by the time the host sees the Suspend. The suspension must carry it.
    let (mut vm, name) = load();
    let s = suspend(&mut vm, &name);
    assert!(!s.program.is_empty(), "suspension must name its program");
    assert!(!s.function.is_empty(), "suspension must name its function");
}
