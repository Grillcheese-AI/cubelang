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

// FIXED 2026-09-14 -- un-ignored. The gap was not specific to parameters:
// `op::COMPARE` pushed both operands through `resolve_i64`, and
// `Value::as_i64` returns 0 for every `Value::Str`, so EVERY string
// comparison compared 0 to 0 and was unconditionally true. COMPARE now
// compares two strings as strings (see `src/vm/engine.rs`).
#[test]
fn string_equality_against_a_parameter_discriminates() {
    assert_eq!(question_of(run(PARAM_EQ, "solve", vec![Value::Str("abc".into())])), "eq_true");
    assert_eq!(question_of(run(PARAM_EQ, "solve", vec![Value::Str("zzz".into())])), "eq_false");
}

#[test]
fn string_equality_against_a_register_literal_is_correct() {
    // Written as "the control: this one DOES work today, so the gap above is
    // specific". It did pass -- but for the WRONG reason: with the old
    // integer-only COMPARE, `b == "abc"` was true because both sides
    // resolved to 0, not because the strings matched. It would have passed
    // just as happily with `b = "zzz"`. The negative case below is what
    // makes this an actual control; it would have failed before the
    // 2026-09-14 COMPARE fix.
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

    // The negative case the original control was missing.
    let src_mismatch = src.replace(r#"assign b = "abc";"#, r#"assign b = "zzz";"#);
    assert_ne!(
        src_mismatch, src,
        "the replace above must actually have changed the fixture"
    );
    assert_eq!(
        question_of(run(&src_mismatch, "solve", vec![Value::Str("_".into())])),
        "eq_false",
        "two different strings must NOT compare equal"
    );
}
