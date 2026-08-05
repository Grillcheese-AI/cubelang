//! Bugfix C: `op::RECALL`'s engine handler (`engine.rs:638-651`) reads only
//! `operands.get(0)` as the lookup key in BOTH the 1-arg and 2-arg forms.
//!
//! `recall reg, "key"` (2-arg form) is real, parseable grammar (`ast.rs:553`,
//! `parser.rs:1364-1378`) that compiles to operand0=`Named(reg)`
//! (destination), operand1=key -- the SAME layout `STORE` uses
//! (`compiler.rs:1321-1341`, confirmed by reading both side by side). `STORE`
//! correctly reads `operands.get(1)` for its key. So a 2-arg RECALL was using
//! the DESTINATION register's own name-hash as the memory lookup key instead
//! of operand1's real key -- and because `HammingIndex::query` (`index.rs`)
//! has no similarity threshold, it doesn't miss or error, it silently
//! returns some *unrelated* trace's value with high confidence. `SPEC.md:2656`
//! documents the intended format as `RECALL [x] key`, i.e. `x` (optional) is
//! the destination, `key` is always the lookup key -- matching `STORE x key`.
//!
//! The fix mirrors STORE's own operand-selection idiom exactly: for the
//! 2-arg form, operand0 is the destination register and operand1 is the key.
//! The 1-arg form (`recall "key"` / `recall key;`, operand0 is the key, no
//! explicit destination) is untouched.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

const PROGRAM: &str = r#"
program RecallTwoArg implements ISolve {
    public function solve(input: str): number {
        return 0;
    }

    public function two_arg_roundtrip(): number {
        create a : number;
        assign a = 111;
        store a, "alpha";

        create b : number;
        assign b = 222;
        store b, "beta";

        create dest : number;
        recall dest, "beta";
        return dest;
    }

    public function two_arg_roundtrip_a_different_key(): number {
        create a : number;
        assign a = 111;
        store a, "alpha";

        create b : number;
        assign b = 222;
        store b, "beta";

        create dest : number;
        recall dest, "alpha";
        return dest;
    }

    public function two_arg_recall_of_a_never_stored_key_on_empty_memory(): number {
        create dest : number;
        recall dest, "nope";
        return dest;
    }

    public function one_arg_recall_store_correct_recall(): number {
        # Mirrors examples/recall_min.cube's store -> correct -> recall
        # pattern exactly, to guard that the 1-arg form (untouched by this
        # fix) still behaves the same: the corrected belief comes back.
        create belief : number;
        assign belief = 40;
        store belief, "estimate";
        remember belief;

        create correction : number;
        assign correction = 65;
        store correction, "estimate";

        recall estimate;

        create recalled : number;
        assign recalled = 0;
        add recalled, estimate;
        return recalled;
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
fn two_arg_recall_returns_the_value_stored_under_its_own_key() {
    // store b, "beta" (222); recall dest, "beta" -- dest must hold 222, the
    // value stored under "beta", not a.name/dest.name's own hash-neighbour.
    match run("two_arg_roundtrip") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 222, "recall dest, \"beta\"; must recall the value stored under \"beta\"");
        }
        other => panic!("expected Return/Ok(Int(222)), got {:?}", other),
    }
}

#[test]
fn two_arg_recall_of_a_different_key_returns_a_different_value() {
    // Same two stored keys, but this time recall "alpha" -- proves real
    // key-selective recovery, not a hardcoded/constant answer (mirrors
    // asm_vsa.rs's b2 control and assign_global_literal.rs's "different
    // literal" controls).
    match run("two_arg_roundtrip_a_different_key") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 111, "recall dest, \"alpha\"; must recall the value stored under \"alpha\"");
        }
        other => panic!("expected Return/Ok(Int(111)), got {:?}", other),
    }
}

#[test]
fn two_arg_recall_of_a_never_stored_key_on_empty_memory_is_null() {
    // Control: nothing was ever stored, so HippocampalMemory::recall's
    // `index.is_empty()` guard returns None regardless of which key is
    // queried -- exercises the 2-arg form's None branch cleanly. (Recalling
    // an unstored key against a NON-empty memory would instead surface
    // HammingIndex::query's separate, out-of-scope missing-similarity-
    // threshold behavior -- deliberately not what this control is testing.)
    match run("two_arg_recall_of_a_never_stored_key_on_empty_memory") {
        ExecResult::Return(Value::Null) | ExecResult::Ok(Value::Null) => {}
        other => panic!("expected Return/Ok(Null) for a never-stored key on empty memory, got {:?}", other),
    }
}

#[test]
fn one_arg_recall_is_unaffected_guard() {
    // The 1-arg form must be byte-for-byte unchanged by this fix: it still
    // reads operand0 as both the lookup key and the implicit destination.
    match run("one_arg_recall_store_correct_recall") {
        ExecResult::Return(Value::Int(n)) | ExecResult::Ok(Value::Int(n)) => {
            assert_eq!(n, 65, "1-arg recall must still surface the corrected belief, unchanged");
        }
        other => panic!("expected Return/Ok(Int(65)), got {:?}", other),
    }
}
