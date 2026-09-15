//! Regression test for the DRIFT.md §C8 fix.
//!
//! `compile_stmt`'s `TryCatch` arm used to never compile `finally_body` at
//! all, in any mode -- not even to a `SKIP` -- and the source's own comment
//! called this out of scope. It is now compiled onto the join point BOTH
//! the normal (`try_body` -> `JMP`) and the caught (`catch_body` fall-
//! through) paths converge on, so it runs on either path, per `finally`'s
//! usual meaning.
//!
//! `try`/`catch` themselves have no runtime exception-dispatch mechanism in
//! this VM (nothing ever jumps to the catch label -- see
//! `src/vm/engine.rs`, which has no notion of "catch" at all), so the only
//! path actually reachable today is the normal one: `try_body` runs, then
//! (bug, now fixed) `finally_body` must ALSO run before the function
//! continues. That's exactly what this test pins down.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

const PROGRAM: &str = r#"
program TryFinally implements ISolve {
    public function solve(input: str): number {
        return 0;
    }

    public function try_then_finally(): number {
        create counter: number;
        assign counter = 0;

        try {
            add counter, 1;
        } catch (e) {
            add counter, 100;
        } finally {
            add counter, 10;
        }

        sum counter;
        return counter;
    }

    public function try_with_no_finally_is_unaffected(): number {
        create counter: number;
        assign counter = 0;

        try {
            add counter, 1;
        } catch (e) {
            add counter, 100;
        }

        sum counter;
        return counter;
    }
}
"#;

fn compile_and_load() -> (VM, String) {
    let mut progs = compiler::compile(PROGRAM).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    (vm, name)
}

fn run(func: &str) -> ExecResult {
    let (mut vm, name) = compile_and_load();
    vm.call(&name, func, vec![])
}

#[test]
fn finally_body_runs_on_the_normal_no_exception_path() {
    // try_body adds 1 (counter=1); the normal path JMPs past catch_body
    // (add 100, never reached -- there is no exception mechanism to trigger
    // it); finally_body must still run and add 10 -> 11. Before the fix,
    // `finally_body` was never compiled at all, so this would be 1.
    match run("try_then_finally") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 11,
                "finally (add counter, 10) must run after the try body on the normal path");
        }
        other => panic!("expected Return/Ok(Int(11)), got {:?}", other),
    }
}

#[test]
fn a_finally_body_actually_compiles_to_real_bytecode() {
    // Direct bytecode check, independent of execution: the ADD opcode from
    // inside `finally { add counter, 10; }` must be present. Before the
    // fix, `finally_body` compiled to nothing -- not even a SKIP.
    let mut progs = compiler::compile(PROGRAM).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let func = prog.functions.iter().find(|f| f.name == "try_then_finally").unwrap();
    // try_body has exactly one ADD (add counter, 1); finally_body has
    // exactly one more (add counter, 10); catch_body's ADD is also present
    // in the bytecode stream (compiled, even though unreachable at
    // runtime -- see module doc) for a total of three.
    //
    // Count INSTRUCTIONS, not raw bytes. An earlier version of this test
    // scanned `func.bytecode` for bytes equal to `op::ADD` (0x03) and
    // asserted 3 -- but a flat byte scan cannot tell an opcode from an
    // operand byte, and this function's operand stream happens to contain
    // one incidental 0x03 (operand payloads here are 2-byte `name_hash`
    // values and 8-byte little-endian immediates; any of those bytes can
    // collide with any opcode). That made the test read 4 and fail for a
    // reason that has nothing to do with the C8 fix it guards. `debug_lines`
    // carries exactly one entry per emitted instruction, tagged with that
    // instruction's opcode, so counting there is exact and immune to
    // operand collisions.
    let add_count = func
        .debug_lines
        .iter()
        .filter(|d| d.opcode == cubelang::compiler::op::ADD)
        .count();
    assert_eq!(add_count, 3,
        "expected 3 ADDs (try + catch + finally, all compiled) in the bytecode, got {}", add_count);
}

#[test]
fn try_catch_with_no_finally_is_unaffected_by_the_fix() {
    // Regression guard: a TryCatch with NO finally block must behave
    // exactly as before -- the normal path still just runs try_body.
    match run("try_with_no_finally_is_unaffected") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 1, "with no finally, the normal path is just the try body");
        }
        other => panic!("expected Return/Ok(Int(1)), got {:?}", other),
    }
}
