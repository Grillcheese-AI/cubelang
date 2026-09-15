//! Regression tests for the DRIFT.md §C3 / §A7 fix: `match (expr) { arms }`
//! selects ONE arm at runtime.
//!
//! Before this fix `compile_match` emitted no comparison and no jumps at
//! all -- it compiled every arm's body unconditionally, in source order, and
//! emitted an `endmatch` label nothing ever targeted. DRIFT.md called this
//! "arguably the single most dangerous entry on this list: it looks exactly
//! like working branching logic and produces a plausible-looking (wrong)
//! result rather than an error."
//!
//! Note the ordering dependency: these tests could not have been made to
//! pass by changing the compiler alone. `op::COMPARE` forced both operands
//! through `resolve_i64`, and `Value::as_i64` returns 0 for every
//! `Value::Str`, so EVERY string comparison was true -- the first string arm
//! would always have won. `src/vm/engine.rs` had to learn string comparison
//! first; `tests/str_semantics.rs` covers that half directly.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

fn run_src(src: &str, func: &str) -> ExecResult {
    let mut progs = compiler::compile(src).expect("compiles");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    vm.call(&name, func, vec![])
}

fn int_of(r: ExecResult) -> i64 {
    match r {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => n,
        other => panic!("expected an Int result, got {:?}", other),
    }
}

fn str_of(r: ExecResult) -> String {
    match r {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => s,
        other => panic!("expected a Str result, got {:?}", other),
    }
}

// ── Numeric subject ─────────────────────────────────────────────────────

const NUMERIC: &str = r#"
program M implements ISolve {
    public function solve(input: str): number {
        return 0;
    }

    # Each arm adds a different, non-overlapping amount, so the returned
    # number identifies exactly which arms ran. Only arm 2 should.
    public function pick_two(): number {
        create acc : number;
        assign acc = 0;
        create subject : number;
        assign subject = 2;

        match (subject) {
            1 => add acc, 100;
            2 => add acc, 20;
            3 => add acc, 3;
        }

        sum acc;
        return acc;
    }

    # No arm matches and there is no wildcard: the whole construct is a
    # no-op and acc keeps its initial value.
    public function no_arm_matches(): number {
        create acc : number;
        assign acc = 7;
        create subject : number;
        assign subject = 99;

        match (subject) {
            1 => add acc, 100;
            2 => add acc, 20;
        }

        sum acc;
        return acc;
    }

    # The wildcard runs only when every tested arm has failed.
    public function falls_through_to_wildcard(): number {
        create acc : number;
        assign acc = 0;
        create subject : number;
        assign subject = 42;

        match (subject) {
            1 => add acc, 100;
            2 => add acc, 20;
            _ => add acc, 5;
        }

        sum acc;
        return acc;
    }

    # A wildcard that is NOT reached must not run.
    public function wildcard_not_reached_when_an_arm_matches(): number {
        create acc : number;
        assign acc = 0;
        create subject : number;
        assign subject = 1;

        match (subject) {
            1 => add acc, 100;
            _ => add acc, 5;
        }

        sum acc;
        return acc;
    }
}
"#;

#[test]
fn exactly_one_numeric_arm_runs() {
    // THE regression: before the fix every arm ran, so this returned
    // 100 + 20 + 3 = 123 instead of 20.
    assert_eq!(
        int_of(run_src(NUMERIC, "pick_two")),
        20,
        "only the arm whose pattern equals the subject may run"
    );
}

#[test]
fn no_matching_arm_and_no_wildcard_runs_nothing() {
    assert_eq!(
        int_of(run_src(NUMERIC, "no_arm_matches")),
        7,
        "with no matching arm and no wildcard the match must be a no-op"
    );
}

#[test]
fn wildcard_runs_when_nothing_else_matches() {
    assert_eq!(
        int_of(run_src(NUMERIC, "falls_through_to_wildcard")),
        5,
        "the `_` arm is the default and must run when every tested arm failed"
    );
}

#[test]
fn wildcard_does_not_run_when_an_earlier_arm_matched() {
    assert_eq!(
        int_of(run_src(NUMERIC, "wildcard_not_reached_when_an_arm_matches")),
        100,
        "a matched arm must jump past the wildcard, not fall into it"
    );
}

// ── String subject ──────────────────────────────────────────────────────

const STRINGS: &str = r#"
program S implements ISolve {
    public function solve(input: str): str {
        return "unused";
    }

    public function route_compare(): str {
        create intent : str;
        assign intent = "comparison";

        match (intent) {
            "implementation" => return "show_program";
            "comparison"     => return "compare";
            "explanation"    => return "explain";
            _                => return "respond";
        }
        return "fell_through";
    }

    public function route_first(): str {
        create intent : str;
        assign intent = "implementation";

        match (intent) {
            "implementation" => return "show_program";
            "comparison"     => return "compare";
            _                => return "respond";
        }
        return "fell_through";
    }

    public function route_default(): str {
        create intent : str;
        assign intent = "something_else_entirely";

        match (intent) {
            "implementation" => return "show_program";
            "comparison"     => return "compare";
            _                => return "respond";
        }
        return "fell_through";
    }
}
"#;

#[test]
fn a_string_match_selects_the_matching_arm_not_the_first_one() {
    // The sharpest test in this file. With the old integer-only COMPARE,
    // every `Value::Str` resolved to 0, so "comparison" == "implementation"
    // was TRUE and this returned "show_program" -- the first arm, always,
    // for every possible subject.
    assert_eq!(str_of(run_src(STRINGS, "route_compare")), "compare");
}

#[test]
fn a_string_match_still_selects_a_genuine_first_arm_hit() {
    // The control for the test above: selecting arm 1 must be because the
    // pattern really matched, not because every comparison is true.
    assert_eq!(str_of(run_src(STRINGS, "route_first")), "show_program");
}

#[test]
fn a_string_match_falls_through_to_the_wildcard() {
    assert_eq!(str_of(run_src(STRINGS, "route_default")), "respond");
}

// ── Strict mode ─────────────────────────────────────────────────────────

#[test]
fn match_is_no_longer_a_strict_violation() {
    // `match` was on the `--strict` blocklist for exactly one reason: "no
    // arm selection at runtime". There is arm selection now, so a program
    // using `match` must pass strict compilation.
    compiler::compile_strict(NUMERIC)
        .expect("match now executes, so it must no longer be a --strict violation");
}

#[test]
fn strict_still_descends_into_arm_bodies() {
    // Removing `match` from the blocklist must not create a hole: a
    // non-executing construct nested INSIDE an arm body still has to be
    // caught. `throw` is on the blocklist and never compiles to anything.
    let src = r#"
program Bad implements ISolve {
    public function solve(input: str): number {
        create subject : number;
        assign subject = 1;
        match (subject) {
            1 => throw "boom";
        }
        return 0;
    }
}
"#;
    let err = compiler::compile_strict(src)
        .expect_err("a non-executing statement inside a match arm must still be a strict violation");
    let msg = err.to_string();
    assert!(
        msg.contains("throw"),
        "strict must still report the nested violation, got: {}",
        msg
    );
}
