//! Containers compile and run (2026-10-24, DRIFT C10), with the storage fixes they need.
//!
//! A `container` (or `world`, `modal`, `robot`) compiled to nothing: no bytecode, no error. It now
//! compiles to a program of its own: `config` and `world` are storage with constant starting
//! values, `programs { }` binds the programs it runs (called as `self.programs.b.f(x)` or
//! `b.f(x)`), `permissions { }` is installed on load, `safety { }` caps every run entered through
//! it (and refuses what it cannot enforce), and the VM boots it: bound constructors, its own
//! constructor, then `on_start`.
//!
//! Fixed on the way: a storage field's declared default was ignored (DRIFT B11); a helper
//! function's `self.x` read a fresh, empty register scope instead of the program's storage; and a
//! 64-deep call chain overflowed a 2 MiB thread.

use cubelang::compiler::{self, CompiledProgram};
use cubelang::vm::{ExecResult, VM, Value};

fn compile(src: &str) -> Vec<CompiledProgram> {
    compiler::compile_strict(src).unwrap_or_else(|e| panic!("{e}"))
}

fn compile_err(src: &str) -> String {
    match compiler::compile_strict(src) {
        Ok(_) => panic!("expected a compile error"),
        Err(e) => e,
    }
}

fn booted(src: &str) -> (VM, String) {
    let progs = compile(src);
    let entry = progs.iter().find(|p| p.container.is_some()).unwrap_or(&progs[0]).name.clone();
    let mut vm = VM::new();
    for p in progs {
        vm.load(p);
    }
    match vm.boot(&entry) {
        ExecResult::Error(e) => panic!("boot: {e}"),
        _ => {}
    }
    (vm, entry)
}

fn int(r: ExecResult) -> i64 {
    match r {
        ExecResult::Ok(Value::Int(n)) | ExecResult::Return(Value::Int(n)) => n,
        other => panic!("expected an integer, got {:?}", other),
    }
}

fn text(r: ExecResult) -> String {
    match r {
        ExecResult::Ok(Value::Str(s)) | ExecResult::Return(Value::Str(s)) => s,
        other => panic!("expected text, got {:?}", other),
    }
}

fn error(r: ExecResult) -> String {
    match r {
        ExecResult::Error(e) => e,
        other => panic!("expected an error, got {:?}", other),
    }
}

const LAB: &str = r#"
program Solver implements ISolve {
    storage { base: mutable i64 = 0; }
    public function constructor() {
        assign base = 100;
    }
    public function solve(input: str): str { return input; }
    public function add_base(x: i64): i64 {
        add x, self.base;
        return x;
    }
    @restricted
    public function secret(): str { return "granted"; }
    @internal
    public function inner(): str { return "inner"; }
}

program Router implements ISolve {
    public function solve(input: str): str {
        let got = Solver.secret();
        return got;
    }
}

program Stranger implements ISolve {
    public function solve(input: str): str {
        let got = Solver.secret();
        return got;
    }
}

container MathLab {
    config {
        name: "MathLab";
        version: "0.1.0";
        dimension: 8192;
        gpu: true;
    }
    global storage {
        total: mutable i64 = 0;
        seen: mutable map<str, i64>;
    }
    programs {
        solver: deploy Solver();
        router: deploy Router();
    }
    permissions {
        grant solver.secret to router;
    }
    world {
        time: mutable u64 = 0;
        rules: immutable array<str>;
    }

    @system @once
    public function on_start() {
        self.world.rules = ["no cheating"];
        self.total = 1;
    }

    @external
    public function name(): str { return self.config.name; }
    @external
    public function dimension(): i64 { return self.config.dimension; }
    @external
    public function gpu(): i64 { return self.config.gpu; }
    @external
    public function total(): i64 { return self.total; }
    @external
    public function tick(): u64 {
        let t = self.world.time;
        add t, 1;
        self.world.time = t;
        return self.world.time;
    }
    @external
    public function rules(): array<str> { return self.world.rules; }
    @external
    public function via_self(x: i64): i64 {
        let r = self.programs.solver.add_base(x);
        return r;
    }
    @external
    public function via_binding(x: i64): i64 {
        let r = solver.add_base(x);
        return r;
    }
    @external
    public function routed(): str {
        let r = router.solve("x");
        return r;
    }
    @external
    public function direct_secret(): str {
        let r = solver.secret();
        return r;
    }
    @external
    public function direct_inner(): str {
        let r = solver.inner();
        return r;
    }
}
"#;

#[test]
fn a_container_compiles_to_a_program_with_its_meta() {
    let progs = compile(LAB);
    let lab = progs.iter().find(|p| p.name == "MathLab").expect("the container compiles");
    let meta = lab.container.as_ref().expect("with its meta");
    assert_eq!(meta.kind, "container");
    assert_eq!(meta.programs, vec![("solver".to_string(), "Solver".to_string()),
                                   ("router".to_string(), "Router".to_string())]);
    assert_eq!(meta.permissions, vec![("Solver".to_string(), "secret".to_string(), "Router".to_string(), true)]);
    for field in ["config", "world", "total", "seen"] {
        assert!(lab.storage_fields.iter().any(|f| f == field), "{field} is storage");
    }
}

#[test]
fn config_storage_and_world_hold_their_constants() {
    let (mut vm, lab) = booted(LAB);
    assert_eq!(text(vm.call_entry(&lab, "name", vec![])), "MathLab");
    assert_eq!(int(vm.call_entry(&lab, "dimension", vec![])), 8192);
    assert_eq!(int(vm.call_entry(&lab, "gpu", vec![])), 1, "true is 1, as the literal compiles in code");
    // on_start ran at boot, after the constant start values
    assert_eq!(int(vm.call_entry(&lab, "total", vec![])), 1);
    assert_eq!(int(vm.call_entry(&lab, "tick", vec![])), 1);
    assert_eq!(int(vm.call_entry(&lab, "tick", vec![])), 2);
    match vm.call_entry(&lab, "rules", vec![]) {
        ExecResult::Ok(Value::Array(r)) | ExecResult::Return(Value::Array(r)) => {
            assert!(matches!(r.as_slice(), [Value::Str(s)] if s == "no cheating"));
        }
        other => panic!("rules: {:?}", other),
    }
    // a map field with no default starts empty
    assert!(matches!(vm.get(&VM::register_key("seen")), Some(Value::Map(m)) if m.is_empty()));
}

#[test]
fn a_container_calls_its_programs_which_were_booted() {
    let (mut vm, lab) = booted(LAB);
    // Solver's constructor ran at boot: its base is 100
    assert_eq!(int(vm.call_entry(&lab, "via_self", vec![Value::Int(5)])), 105);
    assert_eq!(int(vm.call_entry(&lab, "via_binding", vec![Value::Int(7)])), 107);
}

#[test]
fn permissions_admit_the_granted_program_and_no_other() {
    let (mut vm, lab) = booted(LAB);
    // MathLab -> Router.solve -> Solver.secret: Router is granted it
    assert_eq!(text(vm.call_entry(&lab, "routed", vec![])), "granted");
    // the container itself is not on the list
    let e = error(vm.call_entry(&lab, "direct_secret", vec![]));
    assert!(e.contains("MathLab may not call Solver.secret") && e.contains("@restricted"), "{e}");
    // a program nobody granted
    let e = error(vm.call_entry("Stranger", "solve", vec![Value::Str("x".into())]));
    assert!(e.contains("Stranger may not call Solver.secret"), "{e}");
    // @internal stays internal to Solver
    let e = error(vm.call_entry(&lab, "direct_inner", vec![]));
    assert!(e.contains("@internal"), "{e}");
}

#[test]
fn immutable_storage_is_set_only_by_the_constructor_and_on_start() {
    let src = LAB.replace(
        "public function name(): str { return self.config.name; }",
        "public function name(): str { self.world.rules = [\"none\"]; return self.config.name; }");
    let e = compile_err(&src);
    assert!(e.contains("`world.rules` is immutable"), "{e}");

    let src = LAB.replace(
        "public function name(): str { return self.config.name; }",
        "public function name(): str { self.config = {}; return \"x\"; }");
    assert!(compile_err(&src).contains("`config` is immutable"));

    // a mutable world field is writable anywhere (the `tick` function above), and a program's own
    // immutable field is the constructor's alone
    let e = compile_err(r#"
program P implements ISolve {
    storage { limit: immutable i64 = 3; }
    public function constructor() { assign limit = 4; }
    public function solve(input: str): str {
        assign limit = 5;
        return input;
    }
}
"#);
    assert!(e.contains("`limit` is immutable: only the constructor may set it"), "{e}");
}

#[test]
fn bindings_and_calls_are_checked_at_compile_time() {
    let e = compile_err("container C { programs { s: deploy Missing(); } }");
    assert!(e.contains("which is no program of this file"), "{e}");

    let e = compile_err(&LAB.replace("let r = solver.add_base(x);", "let r = solver.nope(x);"));
    assert!(e.contains("Solver has no function nope"), "{e}");

    let e = compile_err(&LAB.replace("let r = solver.add_base(x);", "let r = solver.add_base(x, x);"));
    assert!(e.contains("Solver.add_base takes 1 argument(s), but is given 2"), "{e}");

    let e = compile_err(&LAB.replace("let r = self.programs.solver.add_base(x);", "let r = self.programs.other.add_base(x);"));
    assert!(e.contains("MathLab binds no program `other`"), "{e}");

    let e = compile_err(&LAB.replace("grant solver.secret to router;", "grant solver.nothing to router;"));
    assert!(e.contains("Solver has no function nothing"), "{e}");

    // a proxy binding is refused (the parser does not take `proxy` as an expression yet)
    let e = compile_err(&LAB.replace("solver: deploy Solver();", "solver: proxy Solver(target: router);"));
    assert!(e.contains("Proxy") || e.contains("proxies are not implemented"), "{e}");
}

#[test]
fn safety_refuses_what_it_cannot_enforce_and_enforces_the_rest() {
    let e = compile_err("robot Arm { safety { max_velocity: 180; } }");
    assert!(e.contains("safety `max_velocity` is not a limit this VM can enforce"), "{e}");

    let e = compile_err("container C { safety { max_ops: 0; } }");
    assert!(e.contains("must be a positive integer"), "{e}");

    // a denied opcode in a bound program is a compile error...
    let e = compile_err(r#"
program Asker implements ISolve {
    public function solve(input: str): str {
        ask "which", 1, 2;
        return input;
    }
}
container C {
    programs { a: deploy Asker(); }
    safety { deny: ["ask"]; }
    @external
    public function go(): str { let r = a.solve("x"); return r; }
}
"#);
    assert!(e.contains("C denies ASK, but Asker.solve uses it"), "{e}");

    // ...and limits hold at run time, only inside the container's runs
    let (mut vm, c) = booted(r#"
program Deep implements ISolve {
    public function solve(input: str): str { return input; }
    public function down(n: i64): i64 {
        if (n <= 0) { return 0; }
        let m = n;
        sub m, 1;
        let r = down(m);
        add r, 1;
        return r;
    }
}
container C {
    programs { d: deploy Deep(); }
    safety { max_call_depth: 4; }
    @external
    public function shallow(): i64 { let r = d.down(2); return r; }
    @external
    public function deep(): i64 { let r = d.down(10); return r; }
}
"#);
    assert_eq!(int(vm.call_entry(&c, "shallow", vec![])), 2);
    let e = error(vm.call_entry(&c, "deep", vec![]));
    assert!(e.contains("call depth exceeded (4)"), "{e}");
    // outside the container the VM's own limit (64) applies again
    assert_eq!(int(vm.call_entry("Deep", "down", vec![Value::Int(10)])), 10);
}

#[test]
fn a_denied_opcode_is_refused_at_run_time_in_code_the_compiler_never_saw() {
    let container = r#"
program Worker implements ISolve {
    public function solve(input: str): str { return input; }
}
container C {
    programs { w: deploy Worker(); }
    safety { deny: ["ask"]; }
    @external
    public function go(): str { let r = w.solve("x"); return r; }
}
"#;
    let (mut vm, c) = booted(container);
    assert_eq!(text(vm.call_entry(&c, "go", vec![])), "x");
    // the same program name, loaded later with code that asks
    let swapped = compile(r#"
program Worker implements ISolve {
    public function solve(input: str): str {
        ask "which", 1, 2;
        return input;
    }
}
"#);
    for p in swapped {
        vm.load(p);
    }
    let e = error(vm.call_entry(&c, "go", vec![]));
    assert!(e.contains("denied by a container's safety block") && e.contains("Worker.solve"), "{e}");
    // outside the container's run the opcode is not refused
    assert!(matches!(vm.call_entry("Worker", "solve", vec![Value::Str("x".into())]), ExecResult::Suspend(_)));
}

#[test]
fn a_program_a_container_calls_may_ask_and_the_run_resumes() {
    let (mut vm, c) = booted(r#"
program Picker implements ISolve {
    public function solve(input: str): str { return input; }
    public function pick(a: i64, b: i64): i64 {
        ask "which one", a, b;
        create chosen : i64;
        pop chosen;
        return chosen;
    }
}
container C {
    programs { p: deploy Picker(); }
    @external
    public function go(): i64 {
        let r = p.pick(1, 2);
        add r, 10;
        return r;
    }
}
"#);
    let susp = match vm.call_entry(&c, "go", vec![]) {
        ExecResult::Suspend(s) => s,
        other => panic!("expected the callee's question, got {:?}", other),
    };
    assert_eq!(susp.program, "Picker");
    assert_eq!(susp.callers.len(), 1);
    assert_eq!(susp.callers[0].program, "C");
    assert_eq!(int(vm.resume(susp, Value::Int(2))), 12);
}

#[test]
fn a_grant_can_name_a_caller_that_is_not_a_program() {
    let (mut vm, _) = booted(r#"
program Vault implements ISolve {
    public function solve(input: str): str { return input; }
    @restricted
    public function open(): str { return "opened"; }
}
container C {
    programs { v: deploy Vault(); }
    permissions {
        grant v.open to alice;
        grant v.open to bob;
        revoke v.open from bob;
    }
}
"#);
    vm.caller = Some("alice".into());
    assert_eq!(text(vm.call_entry("Vault", "open", vec![])), "opened");
    vm.caller = Some("bob".into());
    assert!(error(vm.call_entry("Vault", "open", vec![])).contains("bob may not call it"));
    vm.caller = None;
    assert!(error(vm.call_entry("Vault", "open", vec![])).contains("an anonymous caller"));
}

#[test]
fn what_the_vm_cannot_run_is_a_strict_error() {
    let src = r#"
container C {
    io { inputs { text: str; } outputs { answer: str; } }
    agents { tutor: agent.create({ name: "T" }); }
    @external
    public function go(): str { return "ok"; }
}
"#;
    let e = compile_err(src);
    assert!(e.contains("an `io` block") && e.contains("an `agents` block"), "{e}");
    assert!(compiler::compile(src).is_ok(), "accepted without --strict, as other non-executing constructs are");
}

#[test]
fn a_programs_storage_defaults_are_set_on_load() {
    let src = r#"
program P implements ISolve {
    storage {
        n: mutable i64 = 41;
        greeting: mutable str = "hello";
        point: mutable map<str, i64> = { x: 1, y: -2 };
    }
    public function solve(input: str): str { return self.greeting; }
    public function next(): i64 {
        add n, 1;
        return n;
    }
}
"#;
    let mut vm = VM::new();
    vm.load(compile(src).remove(0));
    assert_eq!(text(vm.call_entry("P", "solve", vec![Value::Str("x".into())])), "hello");
    assert_eq!(int(vm.call_entry("P", "next", vec![])), 42);
    match vm.get(&VM::register_key("point")) {
        Some(Value::Map(m)) => {
            assert!(matches!(m.get("x"), Some(Value::Int(1))) && matches!(m.get("y"), Some(Value::Int(-2))));
        }
        other => panic!("point: {:?}", other),
    }

    let e = compile_err(r#"
program P implements ISolve {
    storage { n: mutable i64 = helper(); }
    public function solve(input: str): str { return input; }
}
"#);
    assert!(e.contains("its default is not a constant"), "{e}");

    let e = compile_err(r#"
program P implements ISolve {
    storage { n: mutable i64 = "five"; }
    public function solve(input: str): str { return input; }
}
"#);
    assert!(e.contains("storage field `n` is declared i64 but is given str"), "{e}");
}

#[test]
fn a_helper_sees_the_programs_storage() {
    let src = r#"
program Counter implements ISolve {
    storage { count: mutable i64 = 0; }
    public function bump() {
        self.count += 1;
    }
    public function solve(input: str): str { return input; }
    public function run(): i64 {
        bump();
        bump();
        bump();
        return self.count;
    }
}
"#;
    let mut vm = VM::new();
    vm.load(compile(src).remove(0));
    assert_eq!(int(vm.call_entry("Counter", "run", vec![])), 3);
    // a callee's locals still do not leak into its caller
    let src = r#"
program Scope implements ISolve {
    public function helper(): i64 {
        create mine : i64;
        assign mine = 9;
        return mine;
    }
    public function solve(input: str): str { return input; }
    public function run(): i64 {
        create mine : i64;
        assign mine = 1;
        let x = helper();
        return mine;
    }
}
"#;
    let mut vm = VM::new();
    vm.load(compile(src).remove(0));
    assert_eq!(int(vm.call_entry("Scope", "run", vec![])), 1);
}

#[test]
fn each_program_keeps_its_own_registers() {
    let (mut vm, c) = booted(r#"
program A implements ISolve {
    storage { secret: mutable str = "a-secret"; count: mutable i64 = 0; }
    public function solve(input: str): str { return self.secret; }
    public function bump(): i64 {
        self.count += 1;
        create leftover : str;
        assign leftover = "a-left";
        return self.count;
    }
}
program B implements ISolve {
    storage { secret: mutable str = "b-secret"; count: mutable i64 = 100; }
    public function solve(input: str): str { return self.secret; }
    public function bump(): i64 {
        self.count += 1;
        return self.count;
    }
    @inputs(leftover)
    public function peek(): str { return leftover; }
}
container C {
    programs { a: deploy A(); b: deploy B(); }
    @external
    public function both(): i64 {
        let x = a.bump();
        let y = b.bump();
        add x, y;
        return x;
    }
}
"#);
    // two fields of the same name, one per program
    assert_eq!(int(vm.call_entry(&c, "both", vec![])), 1 + 101);
    assert_eq!(int(vm.call_entry(&c, "both", vec![])), 2 + 102);
    assert_eq!(text(vm.call_entry("A", "solve", vec![Value::Str("x".into())])), "a-secret");
    assert_eq!(text(vm.call_entry("B", "solve", vec![Value::Str("x".into())])), "b-secret");
    assert!(matches!(vm.storage_value("A", "count"), Some(Value::Int(2))));
    assert!(matches!(vm.storage_value("B", "count"), Some(Value::Int(102))));
    // what A's run left in a register is not B's to read
    assert!(matches!(vm.call_entry("A", "bump", vec![]), ExecResult::Ok(_) | ExecResult::Return(_)));
    match vm.call_entry("B", "peek", vec![]) {
        ExecResult::Ok(v) | ExecResult::Return(v) => assert!(!matches!(&v, Value::Str(s) if s == "a-left"), "{:?}", v),
        ExecResult::Error(_) => {}
        other => panic!("{:?}", other),
    }
}

#[test]
fn a_call_chain_as_deep_as_the_limit_runs_on_any_thread() {
    let src = r#"
program R implements ISolve {
    public function solve(input: str): str { return input; }
    public function down(n: i64): i64 {
        if (n <= 0) { return 0; }
        let m = n;
        sub m, 1;
        let r = down(m);
        add r, 1;
        return r;
    }
}
"#;
    let progs = compile(src);
    // a 1 MiB thread, like a Windows main thread
    let r = std::thread::Builder::new().stack_size(1024 * 1024).spawn(move || {
        let mut vm = VM::new();
        for p in progs {
            vm.load(p);
        }
        int(vm.call_entry("R", "down", vec![Value::Int(60)]))
    }).unwrap().join().unwrap();
    assert_eq!(r, 60);
}

#[test]
fn a_container_survives_a_cubebin_round_trip() {
    for p in compile(LAB) {
        let back = CompiledProgram::from_cubebin(&p.to_cubebin()).unwrap();
        assert_eq!(back.container, p.container, "{}", p.name);
        assert_eq!(back.storage_defaults, p.storage_defaults, "{}", p.name);
    }
}

#[test]
fn the_cli_runs_a_files_container() {
    let dir = std::env::temp_dir().join(format!("cubelang-containers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("lab.cube");
    std::fs::write(&file, LAB).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_cubelang"))
        .args(["run", file.to_str().unwrap(), "--fn", "via_binding", "--arg", "1"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("101"), "stdout: {stdout}\nstderr: {stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}
