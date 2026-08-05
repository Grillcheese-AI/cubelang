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
    // time. See the task report's CONCERNS for the reasoning.
    match run(PLAIN_SAME_NAME_AS_MODULE, "run") {
        ExecResult::Return(v) | ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
        other => panic!("expected the registry's greet() (42) to win, got {:?}", other),
    }
}
