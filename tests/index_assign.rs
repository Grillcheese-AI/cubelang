//! Regression tests for the DRIFT.md §C4 fix (`container[key] = value`) and
//! for the compound-assignment bug found alongside it.
//!
//! Two separate silent-wrong-answer bugs lived in `compile_stmt`'s one
//! `Stmt::Assign` arm:
//!
//! 1. **§C4** -- the arm was an `if let SelfAccess / else if let Ident` chain
//!    with no else. An index target (`arr[0] = x`, `regs[k] = v`,
//!    `self.patterns[k] = h`) fell off the end and emitted ZERO bytecode.
//!    DRIFT.md records it as "the most common single violation across the two
//!    complex examples" and the mechanism behind `examples/gsm8k.cube`
//!    returning a meaningless 0.
//!
//! 2. **Compound assignment** -- `AssignStmt::op` was parsed into the AST and
//!    then never read, so `total += 5` emitted `ASSIGN total, 5`. A program
//!    doing `assign total = 10; total += 5;` returned **5**, not 15, silently.
//!
//! Reading a map was impossible too (`op::INDEX` handled only `Value::Array`),
//! so the read and write gaps hid each other: you could not write a map entry,
//! and if you somehow had one you could not read it back.

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

// ── Compound assignment ─────────────────────────────────────────────────

const COMPOUND: &str = r#"
program C implements ISolve {
    public function solve(input: str): number {
        return 0;
    }

    public function plus_eq(): number {
        create total : number;
        assign total = 10;
        total += 5;
        sum total;
        return total;
    }

    public function minus_eq(): number {
        create total : number;
        assign total = 10;
        total -= 4;
        sum total;
        return total;
    }

    public function star_eq(): number {
        create total : number;
        assign total = 10;
        total *= 3;
        sum total;
        return total;
    }

    public function slash_eq(): number {
        create total : number;
        assign total = 12;
        total /= 4;
        sum total;
        return total;
    }

    public function plain_eq_still_overwrites(): number {
        create total : number;
        assign total = 10;
        total = 5;
        sum total;
        return total;
    }
}
"#;

#[test]
fn plus_equals_accumulates_instead_of_overwriting() {
    // THE regression: this returned 5 before the fix, because `+=` compiled
    // to a plain ASSIGN and the operator was discarded.
    assert_eq!(int_of(run_src(COMPOUND, "plus_eq")), 15);
}

#[test]
fn the_other_compound_operators_work_too() {
    assert_eq!(int_of(run_src(COMPOUND, "minus_eq")), 6);
    assert_eq!(int_of(run_src(COMPOUND, "star_eq")), 30);
    assert_eq!(int_of(run_src(COMPOUND, "slash_eq")), 3);
}

#[test]
fn plain_assignment_still_overwrites() {
    // The control: `=` must NOT have become an accumulate.
    assert_eq!(int_of(run_src(COMPOUND, "plain_eq_still_overwrites")), 5);
}

// ── Index assignment: arrays ────────────────────────────────────────────

const ARRAYS: &str = r#"
program A implements ISolve {
    public function solve(input: str): number {
        return 0;
    }

    public function overwrite_in_range(): number {
        let xs = [10, 20, 30];
        xs[1] = 99;
        let got = xs[1];
        sum got;
        return got;
    }

    public function untouched_elements_survive(): number {
        let xs = [10, 20, 30];
        xs[1] = 99;
        let got = xs[2];
        sum got;
        return got;
    }

    public function assigning_at_len_appends(): number {
        let xs = [10, 20];
        xs[2] = 30;
        let n = xs.len();
        sum n;
        return n;
    }

    public function declared_array_starts_empty_and_grows(): number {
        create xs : array<number>;
        xs[0] = 7;
        let got = xs[0];
        sum got;
        return got;
    }
}
"#;

#[test]
fn an_array_element_can_be_overwritten() {
    // Before the fix `xs[1] = 99` emitted nothing at all, so this read back
    // the original 20.
    assert_eq!(int_of(run_src(ARRAYS, "overwrite_in_range")), 99);
}

#[test]
fn writing_one_element_leaves_the_others_alone() {
    assert_eq!(int_of(run_src(ARRAYS, "untouched_elements_survive")), 30);
}

#[test]
fn assigning_one_past_the_end_appends() {
    assert_eq!(int_of(run_src(ARRAYS, "assigning_at_len_appends")), 3);
}

#[test]
fn a_declared_array_starts_empty_and_accepts_index_zero() {
    assert_eq!(int_of(run_src(ARRAYS, "declared_array_starts_empty_and_grows")), 7);
}

#[test]
fn writing_far_past_the_end_is_an_error_not_a_silent_clamp() {
    // Replacing a silent drop with a silent clamp would preserve the bug
    // class. Out-of-range must fail loudly.
    let src = r#"
program A implements ISolve {
    public function solve(input: str): number {
        let xs = [10, 20];
        xs[9] = 30;
        return 0;
    }
}
"#;
    match run_src(src, "solve") {
        ExecResult::Error(msg) => {
            assert!(msg.contains("past the end"),
                "expected an out-of-range SET_INDEX error, got: {}", msg);
        }
        other => panic!("expected an error for an out-of-range index write, got {:?}", other),
    }
}

// ── Index assignment: maps ──────────────────────────────────────────────

const MAPS: &str = r#"
program M implements ISolve {
    public function solve(input: str): str {
        create handlers : map<str, str>;
        handlers["greet"] = "say_hello";
        handlers["bye"] = "say_goodbye";
        let got = handlers["greet"];
        return got;
    }

    public function a_missing_key_reads_null(): str {
        create handlers : map<str, str>;
        handlers["greet"] = "say_hello";
        let got = handlers["nope"];
        return got;
    }

    public function a_key_can_be_overwritten(): str {
        create handlers : map<str, str>;
        handlers["greet"] = "say_hello";
        handlers["greet"] = "say_hi";
        let got = handlers["greet"];
        return got;
    }

    # The key is itself a register, not a literal -- this is the
    # `regs[reg] = ...` shape from gsm8k.cube.
    public function a_register_can_be_the_key(): str {
        create handlers : map<str, str>;
        create k : str;
        assign k = "dynamic";
        handlers[k] = "resolved";
        let got = handlers[k];
        return got;
    }
}
"#;

#[test]
fn a_map_entry_can_be_written_and_read_back() {
    // Both halves were impossible before: no SET_INDEX to write it, and
    // op::INDEX handled only Value::Array so a map read returned Null.
    assert_eq!(str_of(run_src(MAPS, "solve")), "say_hello");
}

#[test]
fn a_missing_map_key_reads_null_rather_than_erroring() {
    match run_src(MAPS, "a_missing_key_reads_null") {
        ExecResult::Return(Value::Null) | ExecResult::Ok(Value::Null) => {}
        other => panic!("expected Null for an absent key, got {:?}", other),
    }
}

#[test]
fn a_map_key_can_be_overwritten() {
    assert_eq!(str_of(run_src(MAPS, "a_key_can_be_overwritten")), "say_hi");
}

#[test]
fn a_register_can_be_used_as_the_map_key() {
    // This is the gsm8k.cube `regs[reg] = ...` shape.
    assert_eq!(str_of(run_src(MAPS, "a_register_can_be_the_key")), "resolved");
}

// ── self.field[key] ─────────────────────────────────────────────────────

#[test]
fn a_storage_field_can_be_index_assigned() {
    // `self.patterns[key] = handler` -- one of the four occurrences DRIFT.md
    // counted across the complex examples.
    let src = r#"
program S implements ISolve {
    storage {
        patterns : map<str, str>;
    }

    public function constructor() {
        self.patterns["seed"] = "initial";
    }

    public function solve(input: str): str {
        self.patterns["added"] = "later";
        let got = self.patterns["added"];
        return got;
    }
}
"#;
    assert_eq!(str_of(run_src(src, "solve")), "later");
}

// ── Targets that still cannot be encoded ────────────────────────────────

#[test]
fn a_nested_index_target_is_a_hard_error_not_a_silent_drop() {
    // `grid[0][1] = v` needs a temporary to hold `grid[0]`, and this
    // compiler has no lowering pass (see DRIFT.md C11). It must fail to
    // compile rather than emit nothing.
    let src = r#"
program N implements ISolve {
    public function solve(input: str): number {
        let grid = [1, 2];
        grid[0][1] = 5;
        return 0;
    }
}
"#;
    let err = compiler::compile(src)
        .expect_err("a nested index target must not compile silently");
    let msg = err.to_string();
    assert!(
        msg.contains("cannot be encoded"),
        "expected an encoding error naming the target, got: {}",
        msg
    );
}

#[test]
fn compound_assignment_to_an_index_target_is_refused() {
    // `m[k] += 1` would need a read-modify-write through a temporary.
    // Refused loudly rather than silently doing a plain assign.
    let src = r#"
program N implements ISolve {
    public function solve(input: str): number {
        let xs = [1, 2];
        xs[0] += 5;
        return 0;
    }
}
"#;
    let err = compiler::compile(src)
        .expect_err("compound assignment to an index target must not compile");
    let msg = err.to_string();
    assert!(
        msg.contains("compound assignment"),
        "expected the compound-assignment refusal, got: {}",
        msg
    );
}

#[test]
fn index_assignment_passes_strict() {
    // It executes now, so `--strict` must accept it -- and the old
    // "unsupported assignment target" strict violation must be gone.
    compiler::compile_strict(MAPS).expect("index assignment executes, so strict must accept it");
    compiler::compile_strict(ARRAYS).expect("index assignment executes, so strict must accept it");
    compiler::compile_strict(COMPOUND).expect("compound assignment executes under strict");
}
