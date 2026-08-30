//! String semantics the VM does NOT yet honour — pinned as `#[ignore]`d
//! contracts (2026-08-30) so the gap is tracked rather than rediscovered.
//!
//! Found while building the stand-in's chat program: under `--strict` both
//! constructs compile and `validate` reports "all executing", yet
//!   * `s.contains("x")` evaluates false for a string that contains "x";
//!   * `input == "abc"` where `input` is a function PARAMETER is true for
//!     any argument (a literal-to-register comparison, `b == "abc"` with
//!     `assign b = "abc"`, is correct).
//! A silent stub that passes verify-before-execute is the failure class the
//! strict verifier exists to catch, so until these execute, chat programs
//! keep string filtering host-side. Run with `cargo test -- --ignored` to see
//! the current behaviour; flip the attributes when the VM is fixed.
use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

fn run(src: &str, func: &str, args: Vec<Value>) -> ExecResult {
    let mut progs = compiler::compile_strict(src).expect("compiles strict");
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    let _ = vm.call(&name, "constructor", vec![]);
    vm.call(&name, func, args)
}

fn question_of(r: ExecResult) -> String {
    match r {
        ExecResult::Suspend(s) => match s.question { Value::Str(q) => q, other => format!("{:?}", other) },
        other => panic!("expected Suspend, got {:?}", other),
    }
}

const CONTAINS: &str = r#"
program P implements ISolve {
    public function solve(input: str): str {
        create b : str;
        assign b = "I am being honest with you.";
        create yes : str;
        assign yes = "yes";
        let has = b.contains("honest");
        if (has) { ask "contains_true", yes; } else { ask "contains_false", yes; }
        create chosen : str;
        pop chosen;
        return chosen;
    }
}
"#;

const PARAM_EQ: &str = r#"
program P implements ISolve {
    public function solve(input: str): str {
        create yes : str;
        assign yes = "yes";
        if (input == "abc") { ask "eq_true", yes; } else { ask "eq_false", yes; }
        create chosen : str;
        pop chosen;
        return chosen;
    }
}
"#;

#[test]
#[ignore = "VM gap 2026-08-30: str.contains evaluates false under strict (silent stub)"]
fn str_contains_evaluates_on_a_register_string() {
    assert_eq!(question_of(run(CONTAINS, "solve", vec![Value::Str("_".into())])), "contains_true");
}

#[test]
#[ignore = "VM gap 2026-08-30: comparing a function parameter to a literal is true for any input"]
fn string_equality_against_a_parameter_discriminates() {
    assert_eq!(question_of(run(PARAM_EQ, "solve", vec![Value::Str("abc".into())])), "eq_true");
    assert_eq!(question_of(run(PARAM_EQ, "solve", vec![Value::Str("zzz".into())])), "eq_false");
}

#[test]
fn string_equality_against_a_register_literal_is_correct() {
    // the control: this one DOES work today, so the gap above is specific
    let src = r#"
program P implements ISolve {
    public function solve(input: str): str {
        create b : str;
        assign b = "abc";
        create yes : str;
        assign yes = "yes";
        if (b == "abc") { ask "eq_true", yes; } else { ask "eq_false", yes; }
        create chosen : str;
        pop chosen;
        return chosen;
    }
}
"#;
    assert_eq!(question_of(run(src, "solve", vec![Value::Str("_".into())])), "eq_true");
}
