//! WO-1.2 -- `recover` scoped to the frame's own bound symbols.
//!
//! `recover(reg, role)` does UNBIND + cosine cleanup. The question this file
//! pins is *what it cleans up against*: every symbol the VM has ever seen as a
//! name, or only the fillers actually bound into THAT frame.
//!
//! Why it matters: the cleanup always returns its nearest candidate, so a role
//! that was never bound in a bound frame does not come back Null -- it comes
//! back as whatever symbol happened to be closest to quasi-orthogonal noise.
//! With a global candidate pool that can be any string the VM has ever seen,
//! including another program's register name or an unrelated literal. Scoping
//! it to the frame's own bindings makes the wrong answer at least a symbol
//! that belongs to this structure, and makes the similarity mean what the
//! thresholds assume it means.
//!
//! These numbers are load-bearing for the sibling CubbyLLM repo, which
//! calibrates tau_vm = 1.0 / 0.4736328125 / 0.22021484375 against them, so the
//! test prints them rather than only asserting -- run with
//! `cargo test --test recover_scope -- --nocapture` to read them.

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM, Value};

/// A frame with three roles bound, plus a role that is never bound anywhere,
/// plus a second frame that shares the VM (so the global pool is genuinely
/// larger than any one frame's bindings).
const SRC: &str = r#"
use vsa;

program RecoverScope implements ISolve {

    public function solve(input: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        bind frame, VERB, "chased";
        return recover(frame, SUBJECT);
    }

    # A role that is NOT bound in this frame. The frame itself IS bound, so
    # there is a hypervector to unbind against -- this is the B11a case, not
    # the never-bound-frame case.
    public function recover_absent_role(): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        bind frame, VERB, "chased";
        return recover(frame, LOCATION);
    }

    # A second, independent frame in the same VM. Its fillers enter the
    # global name table, so a globally-scoped cleanup can return one of them
    # for a role asked of the FIRST frame.
    public function other_frame(): str {
        create other: number;
        bind other, SUBJECT, "helicopter";
        bind other, OBJECT, "umbrella";
        return recover(other, SUBJECT);
    }

    public function recover_wrong_role(): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        return recover(frame, OBJECT);
    }

    # The frame holds no hypervector at all: nothing was ever bound into it.
    public function recover_unbound_frame(): str {
        create frame: number;
        return recover(frame, SUBJECT);
    }

    # Puts a literal in the program's symbol table that is bound into NO
    # frame, so the global candidate pool is strictly larger than any frame's
    # bindings. Without something like this the two scopes could agree by
    # accident and the test would prove nothing.
    public function unrelated(): str {
        return "unrelated_literal";
    }
}
"#;

fn run(func: &str) -> ExecResult {
    let mut progs = compiler::compile(SRC).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    vm.call(&name, func, vec![])
}

fn str_of(r: ExecResult) -> Option<String> {
    match r {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => Some(s),
        ExecResult::Return(Value::Null) | ExecResult::Ok(Value::Null) => None,
        other => panic!("expected Str or Null, got {:?}", other),
    }
}

/// Measure directly against the engine so the similarity is visible, not just
/// the winning symbol.
fn measure(role: &str, fillers: &[(&str, &str)]) -> Option<(String, f64)> {
    // Load the real program so the VM's name table is populated the way it is
    // in production -- `load` registers every string literal in the program's
    // symbol table, which is exactly the global candidate pool under test.
    // ('helicopter', 'umbrella' and 'unrelated_literal' live in SRC precisely
    // so the global pool is strictly larger than any one frame's bindings.)
    let mut progs = compiler::compile(SRC).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let mut vm = VM::new();
    vm.load(prog);

    for (r, f) in fillers {
        vm.vsa_bind_into("frame", r, f);
    }
    vm.vsa_unbind_cleanup_known("frame", role)
}

#[test]
fn print_the_calibration_numbers() {
    let bound = [("SUBJECT", "cat"), ("OBJECT", "mouse"), ("VERB", "chased")];

    let genuine = measure("SUBJECT", &bound);
    let absent = measure("LOCATION", &bound);

    println!("\n--- WO-1.2 recover scoping: the numbers tau_vm is calibrated against ---");
    println!("genuine recovery  (SUBJECT, bound):   {:?}", genuine);
    println!("absent role       (LOCATION, unbound in a BOUND frame): {:?}", absent);
    println!("tau_vm reference: 1 hop 1.0 | 2 hop 0.4736328125 | 3 hop 0.22021484375");
    println!("-----------------------------------------------------------------------\n");

    let (sym, sim) = genuine.expect("a bound role must recover something");
    assert_eq!(sym, "cat", "the genuine recovery must return the planted filler");
    // The bar is tau_vm at 3 hops (0.22021484375), not a round number. A
    // 3-role bundle's recovered cosine sits around 0.27 -- which is exactly
    // why tau_vm is 0.22 at that depth. An earlier draft of this test asserted
    // `> 0.3` and failed on healthy output, the same mistake WO-0.4's
    // margin alarm made: a threshold derived from the bundle size is not a
    // threshold you can also pick by eye.
    const TAU_VM_3HOP: f64 = 0.22021484375;
    assert!(sim > TAU_VM_3HOP,
        "a genuine 3-role recovery must clear tau_vm(3)={TAU_VM_3HOP}, got {sim:.4}");

    // The absent role still returns SOMETHING -- `recover` is never Null on a
    // bound frame. What this work order changes is WHICH something.
    let (asym, asim) = absent.expect("a bound frame always yields a nearest candidate");
    assert!(asim < 0.3, "an unbound role must stay near noise, got {asim:.4}");
    println!("absent-role winner: {asym:?} at {asim:.4}");
}

#[test]
fn an_absent_role_resolves_within_the_frame_not_across_the_vm() {
    // THE work-order assertion. `LOCATION` is bound in no frame. Whatever the
    // cleanup returns must be one of THIS frame's own fillers -- not
    // 'helicopter', 'umbrella' or 'unrelated_literal', which exist only in the
    // global name table.
    let bound = [("SUBJECT", "cat"), ("OBJECT", "mouse"), ("VERB", "chased")];
    let (sym, sim) = measure("LOCATION", &bound)
        .expect("a bound frame always yields a nearest candidate");
    assert!(
        ["cat", "mouse", "chased"].contains(&sym.as_str()),
        "an absent role must resolve to one of the frame's OWN fillers, got {sym:?} at {sim:.4}"
    );
}

#[test]
fn genuine_recoveries_are_unaffected_by_the_scoping() {
    assert_eq!(str_of(run("solve")).as_deref(), Some("cat"));
    assert_eq!(str_of(run("recover_wrong_role")).as_deref(), Some("mouse"));
    assert_eq!(str_of(run("other_frame")).as_deref(), Some("helicopter"));
}

#[test]
fn a_never_bound_frame_still_recovers_nothing() {
    // The control `tests/asm_vsa.rs` b3 also pins, restated here because this
    // change is the one most likely to break it: a frame holding no
    // hypervector has no bindings to scope to, and must stay Null rather than
    // becoming "the nearest of an empty pool".
    assert_eq!(str_of(run("recover_unbound_frame")), None);
}
