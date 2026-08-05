//! Task 4: standard interfaces (VM-internal) + validated `implements`.
//!
//! A VM-internal, name-addressed registry (`vm::interfaces::InterfaceRegistry`)
//! — separate from Task 3's `vm::registry::ModuleRegistry`, since an interface
//! is a CONTRACT, not a callable impl — seeds two standard interfaces:
//! `ISolve` (one abstract fn, `solve`) and `ISolver` (three: `parse`, `solve`,
//! `verify`). `implements X` now resolves DIRECTLY against this registry (no
//! `use` prerequisite — interfaces are implemented, not called), falling back
//! to an in-file inline `interface` decl for a name the registry doesn't
//! recognize. Either way, once resolved, every required (abstract,
//! non-optional) member must be provided by a same-name/same-arity function
//! in the program body, or it's a compile error.
//!
//! `implements` is NOT required yet — an empty (or absent) `implements` list
//! still compiles; Task 7 flips that switch once every example conforms.
//!
//! These are the brief's required cases (a)-(e); the rest pin down supporting
//! behaviour (arity mismatch, inline-decl fallback, registry-wins-over-inline
//! precedence, and a couple of real example files as a durable regression
//! guard) discovered while building the mechanism.

use cubelang::compiler;

// ── (a) `implements ISolver` missing a required abstract fn (`verify`) ────

const ISOLVER_MISSING_VERIFY: &str = r#"
program MissingVerify implements ISolver {
    public function parse(raw: str): str {
        return raw;
    }
    public function solve(input: str): str {
        return input;
    }
}
"#;

#[test]
fn a_implements_isolver_missing_verify_is_a_compile_error() {
    let err = compiler::compile(ISOLVER_MISSING_VERIFY)
        .expect_err("ISolver requires verify(input, output) too -- must not compile");
    let msg = err.to_string();
    assert!(msg.contains("verify"), "{}", msg);
    assert!(msg.contains("ISolver"), "{}", msg);
}

// ── (b) `implements INotReal` -- neither registry nor in-file decl ────────

const IMPLEMENTS_UNKNOWN: &str = r#"
program ImplementsUnknown implements INotReal {
    public function run(): void {
    }
}
"#;

#[test]
fn b_implements_an_unknown_interface_is_a_compile_error() {
    let err = compiler::compile(IMPLEMENTS_UNKNOWN)
        .expect_err("INotReal is in neither the registry nor an in-file decl");
    let msg = err.to_string();
    assert!(msg.contains("INotReal"), "{}", msg);
}

// ── (c) valid `implements ISolver` providing parse/solve/verify -- ok ─────

const ISOLVER_COMPLETE: &str = r#"
program CompleteSolver implements ISolver {
    public function parse(raw: str): str {
        return raw;
    }
    public function solve(input: str): str {
        return input;
    }
    public function verify(input: str, output: str): bool {
        return true;
    }
}
"#;

#[test]
fn c_a_complete_isolver_implementer_compiles_ok() {
    compiler::compile(ISOLVER_COMPLETE)
        .expect("parse/solve/verify all present -- must compile");
}

// ── (d) valid `implements ISolve` providing solve -- ok ───────────────────

const ISOLVE_COMPLETE: &str = r#"
program CompleteSolve implements ISolve {
    public function solve(input: str): str {
        return input;
    }
}
"#;

#[test]
fn d_a_complete_isolve_implementer_compiles_ok() {
    compiler::compile(ISOLVE_COMPLETE)
        .expect("solve present -- ISolve only requires that one fn");
}

// ── (e) a program with NO `implements` still compiles ─────────────────────

const NO_IMPLEMENTS: &str = r#"
program NoImplements {
    public function run(): void {
    }
}
"#;

#[test]
fn e_a_program_with_no_implements_still_compiles() {
    // required-enforcement (rejecting an empty/absent `implements`) is
    // deferred to Task 7 -- Task 4 must not newly reject this.
    compiler::compile(NO_IMPLEMENTS)
        .expect("no implements at all must still compile -- required isn't enforced yet");
}

// ── Supporting: arity, not just name, is checked ───────────────────────────

const ISOLVE_WRONG_ARITY: &str = r#"
program WrongAritySolve implements ISolve {
    public function solve(): str {
        return "no input param";
    }
}
"#;

#[test]
fn solve_with_the_wrong_arity_does_not_satisfy_isolve() {
    // ISolve's `solve` requires arity 1; this program's `solve` takes zero
    // params -- name matches, arity doesn't, so it must still be an error.
    let err = compiler::compile(ISOLVE_WRONG_ARITY)
        .expect_err("solve() with 0 params must not satisfy solve(input) arity 1");
    let msg = err.to_string();
    assert!(msg.contains("solve"), "{}", msg);
}

// ── Supporting: in-file inline `interface` decl still works (no registry
// entry for its name) -- the brief's explicit backward-compat requirement.

const INLINE_INTERFACE_SATISFIED: &str = r#"
interface IWidget {
    abstract public function spin(speed: number): void;
}

program SpinningWidget implements IWidget {
    public function spin(speed: number): void {
    }
}
"#;

#[test]
fn inline_interface_decl_is_used_as_a_fallback_when_the_registry_does_not_know_the_name() {
    compiler::compile(INLINE_INTERFACE_SATISFIED)
        .expect("IWidget isn't a registry interface -- must fall back to the in-file decl");
}

const INLINE_INTERFACE_MISSING_MEMBER: &str = r#"
interface IWidget {
    abstract public function spin(speed: number): void;
    abstract public function stop(): void;
}

program HalfWidget implements IWidget {
    public function spin(speed: number): void {
    }
}
"#;

#[test]
fn inline_interface_decl_missing_member_is_still_a_compile_error() {
    let err = compiler::compile(INLINE_INTERFACE_MISSING_MEMBER)
        .expect_err("HalfWidget never defines stop() -- IWidget requires it");
    assert!(err.to_string().contains("stop"), "{}", err);
}

const INLINE_INTERFACE_OPTIONAL_MEMBER_NOT_REQUIRED: &str = r#"
interface IWidget {
    abstract public function spin(speed: number): void;
    optional public function calibrate(): void;
}

program NoCalibrate implements IWidget {
    public function spin(speed: number): void {
    }
}
"#;

#[test]
fn an_optional_inline_member_is_not_required() {
    // Mirrors gsm8k.cube's real shape: `optional ... learn(...)` alongside
    // the required parse/solve/verify triad.
    compiler::compile(INLINE_INTERFACE_OPTIONAL_MEMBER_NOT_REQUIRED)
        .expect("calibrate() is optional -- SpinNoCalibrate must still compile");
}

// ── Supporting: the registry wins over a same-named, weaker inline decl ───
// (tamper-proofing -- the whole point of moving interfaces VM-internal).

const INLINE_ISOLVER_SHADOW_ATTEMPT: &str = r#"
interface ISolver {
    abstract public function solve(input: str): str;
}

program ShadowsISolver implements ISolver {
    public function solve(input: str): str {
        return input;
    }
}
"#;

#[test]
fn a_local_interface_block_cannot_redefine_a_registry_interface_to_something_weaker() {
    // This program's OWN `interface ISolver { ... }` only requires `solve`
    // -- if inline decls could win over the registry, this would compile.
    // They can't: `ISolver` is a registry name, so it ALWAYS means the
    // registry's parse/solve/verify triad, regardless of what a same-named
    // in-file block claims.
    let err = compiler::compile(INLINE_ISOLVER_SHADOW_ATTEMPT).expect_err(
        "ISolver is a VM-internal name -- a local redefinition must not weaken it");
    let msg = err.to_string();
    assert!(msg.contains("parse") || msg.contains("verify"), "{}", msg);
}

// ── Supporting: interface errors surface under --strict too ───────────────

#[test]
fn interface_errors_also_surface_under_strict_mode() {
    let err = compiler::compile_strict(ISOLVER_MISSING_VERIFY)
        .expect_err("missing verify() is an error strict or not");
    assert!(err.contains("verify"), "{}", err);
}

// ── Supporting: real example files as a durable regression guard ─────────
// These are the actual programs shipped in examples/ -- compiled here, not
// just eyeballed via a manual CLI sweep, so a future change can't silently
// regress them again.

#[test]
fn qc_decision_cube_example_still_compiles() {
    // Inline `interface ISolver { ... }` with the full parse/solve/verify
    // triad, identical in shape to the registry's ISolver -- must compile
    // regardless of which resolution path (registry-first) wins.
    let source = include_str!("../examples/qc_decision.cube");
    compiler::compile(source).expect("qc_decision.cube must still compile");
}

#[test]
fn fibonacci_cube_example_still_compiles() {
    // Fibonacci declared `implements ISolver` with no inline decl and no
    // parse/verify -- true even before this task, just never checked. Fixed
    // by dropping the inaccurate claim (see examples/fibonacci.cube) rather
    // than backfilling stub methods nothing else needed; this test is the
    // regression guard for that fix.
    let source = include_str!("../examples/fibonacci.cube");
    compiler::compile(source).expect("fibonacci.cube must still compile");
}

#[test]
fn gsm8k_cube_example_still_compiles() {
    // Exercises the `optional ... learn(...)` member alongside the required
    // parse/solve/verify triad against a real file, not just the synthetic
    // INLINE_INTERFACE_OPTIONAL_MEMBER_NOT_REQUIRED fixture above.
    let source = include_str!("../examples/gsm8k.cube");
    compiler::compile(source).expect("gsm8k.cube must still compile");
}

#[test]
fn conversation_agent_cube_example_still_compiles() {
    // implements its OWN inline `IConversationAgent` -- not a registry name
    // at all, so this is a pure inline-decl-fallback regression guard.
    let source = include_str!("../examples/conversation_agent/conversation_agent.cube");
    compiler::compile(source).expect("conversation_agent.cube must still compile");
}
