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
//!
//! VM step 0 (2026-09-29): the absent role is now STRUCTURAL. A frame records
//! the roles bound into it, and `recover` of a role that is not among them
//! returns Null with reason `absent_role` before any similarity is computed.
//! The similarity threshold guards only crosstalk among roles that are
//! present. The genuine numbers below do not move.

use cubelang::compiler;
use cubelang::vm::{ExecResult, RecoverReason, VM, Value};

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
/// the winning symbol -- plus what the VM says about it: the reason and the
/// runner-up.
fn measure_with_reason(role: &str, fillers: &[(&str, &str)])
    -> (Option<(String, f64)>, Option<RecoverReason>, Option<f64>)
{
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
    let got = vm.vsa_unbind_cleanup_known("frame", role);
    (got, vm.last_recover_reason, vm.last_recover_runner_up)
}

#[test]
fn print_the_calibration_numbers() {
    let bound = [("SUBJECT", "cat"), ("OBJECT", "mouse"), ("VERB", "chased")];

    let (genuine, greason, grunner) = measure_with_reason("SUBJECT", &bound);
    let (absent, areason, _) = measure_with_reason("LOCATION", &bound);

    println!("\n--- WO-1.2 recover scoping: the numbers tau_vm is calibrated against ---");
    println!("genuine recovery  (SUBJECT, bound):   {:?} runner-up {:?} ({:?})", genuine, grunner, greason);
    println!("absent role       (LOCATION, unbound in a BOUND frame): {:?} ({:?})", absent, areason);
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

    // The genuine recovery also reports its runner-up: with three fillers in
    // the pool there is one, and the margin over it is what separates a
    // clean recovery from a near-tie.
    let runner = grunner.expect("a 3-filler pool has a runner-up");
    assert!(runner < sim, "the runner-up must sit below the winner ({runner:.4} vs {sim:.4})");
    assert_eq!(greason, Some(RecoverReason::Recovered));

    // VM step 0: the absent role is Null, structurally, with its reason. No
    // similarity is computed for it, so there is no number to threshold.
    assert!(absent.is_none(), "an absent role must be Null, got {absent:?}");
    assert_eq!(areason, Some(RecoverReason::AbsentRole));
}

#[test]
fn an_absent_role_is_null_structurally() {
    // THE step-0 assertion. `LOCATION` is bound in no frame. The frame knows
    // its own roles, so the answer is Null before any cleanup runs -- not
    // 'cat', 'mouse' or 'chased' at noise level, and not 'helicopter',
    // 'umbrella' or 'unrelated_literal' from the global name table.
    let bound = [("SUBJECT", "cat"), ("OBJECT", "mouse"), ("VERB", "chased")];
    let (got, reason, runner) = measure_with_reason("LOCATION", &bound);
    assert!(got.is_none(), "an absent role must be Null, got {got:?}");
    assert_eq!(reason, Some(RecoverReason::AbsentRole));
    assert!(runner.is_none(), "no similarity is computed for an absent role");

    // Through the program too: `recover(frame, LOCATION)` returns Null.
    assert_eq!(str_of(run("recover_absent_role")), None);
}

#[test]
fn absent_role_probe_is_null_in_every_one_of_500_frames() {
    // The gate the panel set (round 3): absent-role probe Null in 100% of
    // n >= 500, present-role recovery unchanged. Frames of 2..=4 roles drawn
    // from a pool of 8, fillers from a pool of 24, one absent role each.
    let roles = ["SUBJECT", "OBJECT", "VERB", "LOCATION", "TIME", "AGENT", "PATIENT", "INSTRUMENT"];
    let fillers: Vec<String> = (0..24).map(|i| format!("filler{i}")).collect();
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    let mut next = || { seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17; seed };
    let mut null_absent = 0usize;
    let mut right_present = 0usize;
    let n = 500;
    for _ in 0..n {
        let k = 2 + (next() % 3) as usize;
        let mut order: Vec<usize> = (0..roles.len()).collect();
        for i in (1..order.len()).rev() { let j = (next() % (i as u64 + 1)) as usize; order.swap(i, j); }
        let present = &order[..k];
        let absent = order[k];
        let mut vm = VM::new();
        let mut planted: Vec<(usize, usize)> = Vec::new();
        for &r in present {
            let f = (next() % fillers.len() as u64) as usize;
            vm.vsa_bind_into("frame", roles[r], &fillers[f]);
            planted.push((r, f));
        }
        if vm.vsa_unbind_cleanup_known("frame", roles[absent]).is_none()
            && vm.last_recover_reason == Some(RecoverReason::AbsentRole) {
            null_absent += 1;
        }
        let (r, f) = planted[0];
        if let Some((sym, _)) = vm.vsa_unbind_cleanup_known("frame", roles[r]) {
            if sym == fillers[f] { right_present += 1; }
        }
    }
    println!("absent-role probe: {null_absent}/{n} Null; present-role recovery: {right_present}/{n} right");
    assert_eq!(null_absent, n, "every absent role must be Null");
    assert!(right_present * 100 >= n * 95,
        "present-role recovery must not regress ({right_present}/{n})");
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
