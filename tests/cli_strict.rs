//! Task 8 (Feature B, verify-before-execute), CLI side.
//!
//! `compile --strict` already had real strict verification
//! (`compiler::compile_strict`); the from-source `run` and `check` entry
//! points did not -- `run` compiled loosely even when a program couldn't
//! serialize to `.cubebin` (a `use`-ing program) and had no way to ask for
//! strict verification at all, and `check` never called the compiler in the
//! first place (parse-only). These tests drive the real compiled `cubelang`
//! binary as a subprocess -- same style as `tests/proto_stdio.rs`'s
//! `call_proto` -- since `cmd_run`/`cmd_check` are private to `src/main.rs`
//! and not reachable any other way.

use std::io::Write;
use std::process::Command;

/// Compiles cleanly under loose `compiler::compile` (implements present,
/// `solve` provided, matching ISolve) -- `infer` is a normal statement
/// there. Only `compile_strict`'s `strict_check_stmt` rejects it, since
/// `infer` is a trace-only ext op (`is_trace_only_ext_op`) that never
/// executes value-level semantics under the current VM. Same minimal shape
/// `tests/proto_stdio.rs`'s wire-level strict-violation test and
/// `src/compiler.rs`'s own `strict_rejects_trace_only_infer` unit test use.
const STRICT_VIOLATION: &str = r#"
program Test implements ISolve {
    public function solve(input: str): void {
        infer x;
    }
}
"#;

/// Mirrors `cubbyllm/bridges/programs/reasoning_bridge.cube` -- the same
/// shape `tests/proto_stdio.rs::reasoning_bridge_program` runs over the
/// wire: a clean bind -> recover, no trace-only ops, no `for`/`match`.
/// Passes `compile_strict` cleanly.
const REASONING_BRIDGE: &str = r#"
use vsa;
program ReasoningBridge implements ISolve {
    public function solve(mention: str): str {
        create frame: number;
        bind frame, SUBJECT, "cat";
        bind frame, OBJECT, "mouse";
        return recover(frame, SUBJECT);
    }
}
"#;

/// Write `source` to a fresh temp `.cube` file and return its path -- `run`
/// and `check` both take a file path, not stdin. `name` plus the process id
/// keep concurrent test-binary runs from colliding on the same path.
fn write_cube_file(name: &str, source: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir()
        .join(format!("cubelang_task8_{}_{}.cube", name, std::process::id()));
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(source.as_bytes()).unwrap();
    path
}

fn cubelang() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cubelang"))
}

// ── `run --strict` ──────────────────────────────────────────────────────────

#[test]
fn run_strict_rejects_a_strict_violating_program() {
    let path = write_cube_file("run_strict_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["run", path.to_str().unwrap(), "--strict", "--json"])
        .output()
        .expect("spawn cubelang run --strict");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success(), "run --strict must exit non-zero on a strict violation");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("infer"),
        "stderr should name the offending construct (infer): {}", stderr);
}

#[test]
fn run_strict_still_allows_the_reasoning_bridge_program() {
    let path = write_cube_file("run_strict_reasoning_bridge", REASONING_BRIDGE);
    let out = cubelang()
        .args(["run", path.to_str().unwrap(), "--strict",
               "--fn", "solve", "--arg", "_", "--json"])
        .output()
        .expect("spawn cubelang run --strict");
    let _ = std::fs::remove_file(&path);

    assert!(out.status.success(),
        "run --strict must not break the real reasoning-bridge path: stderr={}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("expected valid JSON on stdout, got {:?}: {}", stdout, e));
    assert_eq!(json["ok"], true);
    assert_eq!(json["result"], "cat");
}

/// FIXED 2026-09-14 (strict-by-default): `run`/`compile`/`check` now default
/// to strict verification -- "the language's accepted surface should equal
/// its executing surface" (Nick, docs/DRIFT.md) -- so plain `run` (no flags
/// at all) now REJECTS a strict-only violation exactly like `run --strict`
/// always did. This replaces the old regression guard that pinned the
/// opposite behavior (a plain `run` silently accepting the violation); see
/// `run_with_loose_still_allows_the_violation` below for the new opt-out.
#[test]
fn run_without_any_flag_now_rejects_the_violation_by_default() {
    let path = write_cube_file("run_default_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["run", path.to_str().unwrap(), "--json"])
        .output()
        .expect("spawn cubelang run");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success(),
        "plain run (strict by default now) must reject a strict-only violation");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("infer"),
        "stderr should name the offending construct (infer): {}", stderr);
}

/// The new opt-out: `--loose` gets back the old permissive behavior for a
/// strict-only violation (trace-only opcodes, `match`, ...) -- but NOT for a
/// non-encodable expression operand (docs/DRIFT.md §C11), which is always a
/// hard error; that distinction is covered separately in
/// `tests/operand_errors.rs`.
#[test]
fn run_with_loose_still_allows_the_violation() {
    let path = write_cube_file("run_loose_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["run", path.to_str().unwrap(), "--loose", "--json"])
        .output()
        .expect("spawn cubelang run");
    let _ = std::fs::remove_file(&path);

    assert!(out.status.success(),
        "run --loose must still compile a strict-only violation: stderr={}",
        String::from_utf8_lossy(&out.stderr));
}

/// `--strict` must still work as an explicit (now redundant) flag, since
/// existing scripts/docs pass it.
#[test]
fn run_with_explicit_strict_still_rejects_the_violation() {
    let path = write_cube_file("run_explicit_strict_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["run", path.to_str().unwrap(), "--strict", "--json"])
        .output()
        .expect("spawn cubelang run --strict");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success(), "run --strict must still exit non-zero on a strict violation");
}

// ── `check` ─────────────────────────────────────────────────────────────────

#[test]
fn check_reports_non_ok_for_a_strict_violating_program() {
    let path = write_cube_file("check_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["check", path.to_str().unwrap()])
        .output()
        .expect("spawn cubelang check");
    let _ = std::fs::remove_file(&path);

    assert!(!out.status.success(), "check must exit non-zero for a strict-violating program");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.to_lowercase().contains("infer"),
        "check's error should name the offending construct (infer): {}", stderr);
}

#[test]
fn check_reports_ok_for_a_clean_program() {
    let path = write_cube_file("check_clean", REASONING_BRIDGE);
    let out = cubelang()
        .args(["check", path.to_str().unwrap()])
        .output()
        .expect("spawn cubelang check");
    let _ = std::fs::remove_file(&path);

    assert!(out.status.success(),
        "check must exit 0 for a program that passes strict verification: stderr={}",
        String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("ok"), "expected the existing 'ok' output shape: {}", stdout);
}

/// `check --loose` (new, strict-by-default fix): opts `check` out of strict
/// verification the same way `run --loose`/`compile --loose` do, so a
/// strict-only violation no longer fails it.
#[test]
fn check_with_loose_reports_ok_for_a_strict_violating_program() {
    let path = write_cube_file("check_loose_violation", STRICT_VIOLATION);
    let out = cubelang()
        .args(["check", "--loose", path.to_str().unwrap()])
        .output()
        .expect("spawn cubelang check --loose");
    let _ = std::fs::remove_file(&path);

    assert!(out.status.success(),
        "check --loose must accept a strict-only violation: stderr={}",
        String::from_utf8_lossy(&out.stderr));
}
