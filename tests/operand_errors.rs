//! Regression tests for the DRIFT.md §C11 fix, as P6-4 changed it (2026-11-13).
//!
//! `Compiler::compile_expr_operand` (src/compiler.rs) only has a real encoding for a literal, a bare identifier,
//! `self.field`, and a simple `expr.field`. Every other expression shape used as an operand -- a `BinOp` chief among
//! them -- used to silently fall through to an opaque `OP_NONE` placeholder (the exact bug `docs/DRIFT.md` reproduces
//! with `double_it(x - y)` returning 0 instead of 4). C11 made it a hard compile error in every mode.
//!
//! P6-4 (fix list, 2026-11-13): such an expression is now computed into a register first (`src/lower.rs`), so each
//! program below compiles and gives the value it means -- the DRIFT.md reproduction returns 4. C11's line holds: no
//! operand is ever silent garbage. A form that still has no computation (`await` as a value) stays a hard compile
//! error naming the form, in every mode.
//!
//! One test per path named in C11's brief: `emit_call`'s argument loop, array-literal elements, index expressions,
//! and `let`/`assign` RHS.

use cubelang::compiler;
use cubelang::vm::{ExecResult, Value, VM};

fn solve(source: &str, strict: bool) -> i64 {
    let mut progs = if strict {
        compiler::compile_strict(source).unwrap_or_else(|e| panic!("compiles strict: {}", e))
    } else {
        compiler::compile(source).unwrap_or_else(|e| panic!("compiles: {}", e))
    };
    let prog = progs.pop().unwrap();
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    match vm.call(&name, "solve", vec![Value::Str("x".into())]) {
        ExecResult::Ok(v) | ExecResult::Return(v) => v.as_i64(),
        other => panic!("expected a value, got {:?}", other),
    }
}

// ── (a) call argument: the DRIFT.md reproduction itself ─────────────────────

const CALL_ARGUMENT_BINOP: &str = r#"
program DoubleIt implements ISolve {
    function double_it(n: number): number {
        create r: number;
        assign r = n;
        add r, n;
        return r;
    }

    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let z = double_it(x - y);
        return z;
    }
}
"#;

#[test]
fn a_binop_call_argument_gives_its_value_in_loose_mode() {
    // DRIFT.md's reproduction: 0 before C11, a compile error after it, 4 since P6-4
    assert_eq!(solve(CALL_ARGUMENT_BINOP, false), 4);
}

#[test]
fn a_binop_call_argument_gives_its_value_under_strict() {
    assert_eq!(solve(CALL_ARGUMENT_BINOP, true), 4);
}

// ── (b) array-literal element ────────────────────────────────────────────────

const ARRAY_LITERAL_ELEMENT_BINOP: &str = r#"
program ArrLit implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let xs = [1, x - y, 3];
        return xs[1];
    }
}
"#;

#[test]
fn b_binop_array_literal_element_gives_its_value() {
    assert_eq!(solve(ARRAY_LITERAL_ELEMENT_BINOP, true), 2);
}

// ── (c) index expression ─────────────────────────────────────────────────────

const INDEX_EXPRESSION_BINOP: &str = r#"
program IndexExpr implements ISolve {
    public function solve(input: str): number {
        let xs = [10, 20, 30];
        create x: number;
        assign x = 2;
        create y: number;
        assign y = 1;
        let z = xs[x - y];
        return z;
    }
}
"#;

#[test]
fn c_binop_index_expression_gives_its_value() {
    assert_eq!(solve(INDEX_EXPRESSION_BINOP, true), 20);
}

// ── (d) `let` RHS ─────────────────────────────────────────────────────────────

const LET_RHS_BINOP: &str = r#"
program LetRhs implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        let diff = x - y;
        return diff;
    }
}
"#;

#[test]
fn d_binop_let_rhs_gives_its_value() {
    assert_eq!(solve(LET_RHS_BINOP, true), 2);
}

// ── (e) `assign` RHS ──────────────────────────────────────────────────────────

const ASSIGN_RHS_BINOP: &str = r#"
program AssignRhs implements ISolve {
    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        create z: number;
        assign z = 0;
        z = x - y;
        return z;
    }
}
"#;

#[test]
fn e_binop_assign_rhs_gives_its_value() {
    assert_eq!(solve(ASSIGN_RHS_BINOP, true), 2);
}

// ── A form with no computation is still a hard error, in every mode ─────────────

const AWAIT_OPERAND: &str = r#"
program AwaitOperand implements ISolve {
    public function solve(input: str): number {
        let v = await input;
        return 0;
    }
}
"#;

#[test]
fn an_await_as_a_value_is_still_a_hard_compile_error() {
    let msg = compiler::compile(AWAIT_OPERAND).expect_err("an await has no operand form").to_string();
    assert!(msg.contains("Await") && msg.contains("cannot be encoded as an operand"), "{}", msg);
    assert!(compiler::compile_strict(AWAIT_OPERAND).is_err());
}

// ── The workaround C11's error message recommended still works ──────────────

const WORKAROUND_COMPUTE_INTO_A_REGISTER_FIRST: &str = r#"
program Workaround implements ISolve {
    function double_it(n: number): number {
        create r: number;
        assign r = n;
        add r, n;
        return r;
    }

    public function solve(input: str): number {
        create x: number;
        assign x = 5;
        create y: number;
        assign y = 3;
        create diff: number;
        assign diff = x;
        sub diff, y;
        let z = double_it(diff);
        return z;
    }
}
"#;

#[test]
fn the_recommended_workaround_still_compiles_clean() {
    assert_eq!(solve(WORKAROUND_COMPUTE_INTO_A_REGISTER_FIRST, true), 4);
}
