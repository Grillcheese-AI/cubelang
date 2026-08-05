//! Task 3: the `use` capability/module system.
//!
//! A VM-internal, name-addressed registry (`vm::registry::ModuleRegistry`)
//! exposes native modules. A top-level `use <name>;` brings one into scope
//! for the whole compilation unit — deny-by-default: a module not `use`'d is
//! invisible to call resolution even though the registry always knows it.
//! A program may reclaim a `use`'d, overridable name with its own
//! `function name() override { ... }`, validated at compile time.
//!
//! Resolution precedence for a bare `name()` call (`vm::engine`'s
//! `op::CALL`): a validated program override > the registry's native
//! implementation > the program's own plain function (unchanged, pre-Task-3
//! behaviour) > error.
//!
//! These four are the brief's required cases (a)-(d); the rest pin down
//! supporting behaviour discovered while building the mechanism (see the
//! task report for the reasoning behind each).

use cubelang::compiler;
use cubelang::vm::{ExecResult, VM};

fn compile_and_load(source: &str) -> (VM, String) {
    let mut progs = compiler::compile(source).expect("compile");
    let prog = progs.pop().expect("exactly one program");
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    (vm, name)
}

fn run(source: &str, func: &str) -> ExecResult {
    let (mut vm, name) = compile_and_load(source);
    vm.call(&name, func, vec![])
}

// ── (a) `use demo; ... return greet();` -> the VM impl's value ────────────

const USE_DEMO: &str = r#"
use demo;

program UsesDemo {
    public function run(): number {
        return greet();
    }
}
"#;

#[test]
fn a_use_demo_resolves_bare_call_to_the_registry_impl() {
    match run(USE_DEMO, "run") {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
        other => panic!("expected the registry's greet() value (42), got {:?}", other),
    }
}

// ── (b) a program `override` wins over the registry impl ──────────────────

const USE_DEMO_OVERRIDE: &str = r#"
use demo;

program UsesDemoOverride {
    public function greet() override {
        return 999;
    }
    public function run(): number {
        return greet();
    }
}
"#;

#[test]
fn b_a_valid_program_override_wins_over_the_registry_impl() {
    match run(USE_DEMO_OVERRIDE, "run") {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 999),
        other => panic!("expected the override's value (999), got {:?}", other),
    }
}

#[test]
fn compiled_program_carries_used_modules_and_the_override_flag() {
    // A more direct look at what compilation actually produced, alongside
    // the end-to-end run above.
    let progs = compiler::compile(USE_DEMO_OVERRIDE).expect("compile");
    assert_eq!(progs[0].used_modules, vec!["demo".to_string()]);
    let greet = progs[0].functions.iter().find(|f| f.name == "greet").expect("greet");
    assert!(greet.is_override, "greet() override must set CompiledFunction::is_override");
    let run_fn = progs[0].functions.iter().find(|f| f.name == "run").expect("run");
    assert!(!run_fn.is_override, "run() has no override marker");
}

// ── (c) `override` on a non-overridable or absent name is a compile error ─

const OVERRIDE_SEALED: &str = r#"
use demo;

program OverridesSealed {
    public function sealed() override {
        return 1;
    }
}
"#;

#[test]
fn c1_override_on_a_non_overridable_name_is_a_compile_error() {
    let err = compiler::compile(OVERRIDE_SEALED)
        .expect_err("`sealed` is in demo but overridable: false — must not compile");
    assert!(err.to_string().contains("sealed"), "{}", err);
}

const OVERRIDE_ABSENT: &str = r#"
use demo;

program OverridesAbsentName {
    public function nonexistent_capability() override {
        return 1;
    }
}
"#;

#[test]
fn c2_override_on_a_name_absent_from_every_used_module_is_a_compile_error() {
    let err = compiler::compile(OVERRIDE_ABSENT)
        .expect_err("no `use`'d module exposes this name — must not compile");
    assert!(err.to_string().contains("nonexistent_capability"), "{}", err);
}

const OVERRIDE_NO_USE_AT_ALL: &str = r#"
program OverridesWithoutAnyUse {
    public function greet() override {
        return 1;
    }
}
"#;

#[test]
fn c3_override_with_no_use_declaration_at_all_is_a_compile_error() {
    // `greet` IS overridable in `demo` -- but nothing `use`s `demo`, so
    // deny-by-default means the name isn't visible to be overridden either.
    let err = compiler::compile(OVERRIDE_NO_USE_AT_ALL)
        .expect_err("no `use` at all — override has nothing legitimate to claim");
    assert!(err.to_string().contains("greet"), "{}", err);
}

#[test]
fn override_errors_also_surface_under_strict_mode() {
    // `check_override_validity` runs unconditionally (not just under
    // `--strict`); `compile_strict` folds it into the same combined string.
    let err = compiler::compile_strict(OVERRIDE_SEALED)
        .expect_err("sealed is not overridable, strict or not");
    assert!(err.contains("sealed"), "{}", err);
}

// ── (d) calling `greet()` without `use demo;` is an error ─────────────────

const NO_USE: &str = r#"
program NoUse {
    public function run(): number {
        return greet();
    }
}
"#;

#[test]
fn d_calling_a_module_fn_without_use_is_a_runtime_error() {
    match run(NO_USE, "run") {
        ExecResult::Error(e) => assert!(e.contains("greet"), "{}", e),
        other => panic!("deny-by-default: expected an Error, got {:?}", other),
    }
}

// ── Supporting behaviour discovered while building the mechanism ──────────

const PLAIN_CALL_NO_USE: &str = r#"
program PlainCall {
    public function helper(): number {
        return 5;
    }
    public function run(): number {
        return helper();
    }
}
"#;

#[test]
fn plain_intra_program_call_is_unaffected_by_the_registry() {
    // No `use`, no `override` anywhere: today's pre-Task-3 behaviour must be
    // completely untouched. Also exercises the `return foo();` compiler fix
    // (see task report) on the plain-program-function branch, not just the
    // registry branch.
    match run(PLAIN_CALL_NO_USE, "run") {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 5),
        other => panic!("plain intra-program call regressed: {:?}", other),
    }
}

const PLAIN_SAME_NAME_AS_MODULE: &str = r#"
use demo;

program ShadowsWithoutOverride {
    public function greet(): number {
        return 123;
    }
    public function run(): number {
        return greet();
    }
}
"#;

#[test]
fn a_plain_same_named_function_does_not_silently_shadow_a_used_module_without_override() {
    // Precedence edge case worth pinning down: `greet` exists BOTH in the
    // program and in a `use`'d module, but the program's `greet` is NOT
    // marked `override`. Per the stated precedence (override > registry >
    // plain program fn > error), the registry wins -- a program cannot
    // accidentally shadow a used capability just by reusing its name; it
    // must opt in with `override`, which is itself validated at compile
    // time. See the task report's CONCERNS for the reasoning. Review round 1
    // ruled this correct and asked only for the SEALED variant (below) to
    // become a compile error, since an overridable collision has a
    // discoverable fix (`override`) and a sealed one does not.
    match run(PLAIN_SAME_NAME_AS_MODULE, "run") {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
        other => panic!("expected the registry's greet() (42) to win, got {:?}", other),
    }
}

// ── Fix round 1, item 2: a plain fn colliding with a SEALED capability ────
// (as opposed to the overridable case just above, which stays as-is).

// NOTE: no caller of `sealed()` here on purpose. `sealed` is itself a
// reserved keyword (the `sealed { ... }` encrypted-storage block) that's a
// soft keyword only in DECLARATION position (`expect_ident`, e.g. a
// function name) -- not yet in `parse_primary_expr`'s expression-atom list,
// so `return sealed();` fails to PARSE ("unexpected token in expression:
// Sealed"), independent of anything Task 3 touches. `check_sealed_collision`
// fires on the colliding DECLARATION alone, so this test doesn't need a
// caller to exercise it -- and using one would conflate a pre-existing,
// unrelated parser gap with the compile error under test.
const SEALED_COLLISION: &str = r#"
use demo;

program CollidesWithSealed {
    public function sealed(): number {
        return 1;
    }
}
"#;

#[test]
fn a_plain_function_colliding_with_a_sealed_capability_is_a_compile_error() {
    // `sealed` is demo's non-overridable export. The program's own `sealed`
    // (no `override` marker -- there's no valid one to give it, since
    // `override` on a sealed name is itself already a compile error) would
    // otherwise be silently unreachable: registry-wins picks demo's sealed()
    // every time. That's dead code with no fix but renaming, so it's now a
    // compile error instead of a silent trap.
    let err = compiler::compile(SEALED_COLLISION)
        .expect_err("a plain fn named `sealed` collides with demo's sealed capability");
    let msg = err.to_string();
    assert!(msg.contains("sealed"), "{}", msg);
    assert!(msg.contains("demo"), "{}", msg);
}

// ── Fix round 1, item 1: .cubebin must not silently drop capability info ──
//
// The binary format doesn't carry `used_modules`/`is_override` (see those
// fields' docs). Compiling a `use`/`override` program straight to .cubebin
// used to succeed silently and then behave DIFFERENTLY once reloaded --
// `use demo; return greet();` returns 42 run from source, but a save+load
// round trip would drop the `use` and error at runtime instead, with no
// warning anywhere. `CompiledProgram::save` now refuses instead.

#[test]
fn use_demo_program_refuses_to_serialize_to_cubebin() {
    let progs = compiler::compile(USE_DEMO).expect("compile");
    let path = std::env::temp_dir().join("use_demo_capability_guard_test.cubebin");
    let _ = std::fs::remove_file(&path);

    let err = progs[0].save(&path).expect_err(
        "a program with a non-empty used_modules must refuse to serialize");
    assert!(err.to_string().contains("use"), "{}", err);
    assert!(!path.exists(), "save must refuse BEFORE writing anything, not after");
}

#[test]
fn override_program_also_refuses_to_serialize_to_cubebin() {
    // Distinct from the case above: this program's guard-relevant signal is
    // an `is_override` function, checked independently of `used_modules`
    // (USE_DEMO_OVERRIDE happens to carry both, but the guard is an OR).
    let progs = compiler::compile(USE_DEMO_OVERRIDE).expect("compile");
    let path = std::env::temp_dir().join("use_demo_override_capability_guard_test.cubebin");
    let _ = std::fs::remove_file(&path);

    let err = progs[0].save(&path).expect_err("an override-marked function must also refuse");
    assert!(!path.exists(), "save must refuse BEFORE writing anything, not after");
    drop(err);
}

#[test]
fn a_plain_program_still_round_trips_through_cubebin_unchanged() {
    // No `use`, no `override` anywhere: the new guard must be a complete
    // no-op for this program. Round-trips through a REAL save+load (not
    // just to_cubebin/from_cubebin in memory) and still runs identically.
    let progs = compiler::compile(PLAIN_CALL_NO_USE).expect("compile");
    let path = std::env::temp_dir().join("plain_call_capability_guard_round_trip_test.cubebin");
    progs[0].save(&path).expect("a plain program must still serialize fine");

    let mut vm = VM::new();
    let name = vm.load_file(&path).expect("must still load fine");
    match vm.call(&name, "run", vec![]) {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 5),
        other => panic!("round-tripped plain program should behave identically: {:?}", other),
    }
    let _ = std::fs::remove_file(&path);
}
