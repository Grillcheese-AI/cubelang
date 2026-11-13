//! Pinned-output regression test for every `.cube` file under `examples/`.
//!
//! Modelled on `tests/opcode_sync.rs`'s spirit ("a silent divergence ships
//! wrong behavior without failing locally") and `tests/cli_strict.rs`'s
//! style (drives the real compiled `cubelang` binary as a subprocess, via
//! `env!("CARGO_BIN_EXE_cubelang")`, since `cmd_run` is private to
//! `src/main.rs`). Every example in `examples/` — including the two that
//! FIXED 2026-09-14 no longer compile at all (`gsm8k.cube`,
//! `conversation_agent/conversation_agent.cube` — see `docs/DRIFT.md`,
//! "Bugs found" #1/#2, and the C11-operand-encoding + strict-by-default
//! fixes) — has its exact, currently-observed outcome pinned here. A silent
//! regression (a formerly-clean example starting to error, a formerly-
//! broken one silently starting to produce a DIFFERENT wrong answer, or one
//! of the two now-rejected examples silently starting to compile again)
//! fails this test rather than going unnoticed. `examples/README.md`
//! documents the same commands/outputs for a human reader; keep the two in
//! sync when either changes.

use std::path::{Path, PathBuf};
use std::process::Command;

fn cubelang() -> Command {
    Command::new(env!("CARGO_BIN_EXE_cubelang"))
}

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples")
}

/// Run `cubelang run <relative_path> --json <extra_args...>` from the repo
/// root (so relative `--knowledge` paths in `extra_args` resolve the same
/// way a person typing the command in `examples/README.md` would expect)
/// and parse stdout as JSON. Panics with full stdout/stderr on a spawn
/// failure or unparsable stdout — every example here is expected to emit
/// exactly one JSON line on stdout, success or failure.
fn run_json(relative_path: &str, extra_args: &[&str]) -> serde_json::Value {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = examples_dir().join(relative_path);
    assert!(path.is_file(), "example not found: {}", path.display());

    let mut args: Vec<&str> = vec!["run", path.to_str().unwrap(), "--json"];
    args.extend_from_slice(extra_args);

    let out = cubelang()
        .args(&args)
        .current_dir(&repo_root)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn cubelang run {}: {}", relative_path, e));

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "example {} did not emit valid JSON on stdout\n  error: {}\n  stdout: {:?}\n  stderr: {:?}",
            relative_path, e, stdout, stderr
        )
    })
}

/// Run `cubelang run <relative_path> <extra_args...>` (no `--json`) and
/// return the raw `Output` -- for the two examples that no longer compile at
/// all (see below): a compile error goes to stderr with nothing on stdout,
/// so `run_json`'s "parse stdout as JSON" contract doesn't apply here.
fn run_raw(relative_path: &str, extra_args: &[&str]) -> std::process::Output {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path = examples_dir().join(relative_path);
    assert!(path.is_file(), "example not found: {}", path.display());

    let mut args: Vec<&str> = vec!["run", path.to_str().unwrap()];
    args.extend_from_slice(extra_args);

    cubelang()
        .args(&args)
        .current_dir(&repo_root)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn cubelang run {}: {}", relative_path, e))
}

fn assert_ok_result(relative_path: &str, extra_args: &[&str], expected_result: i64) {
    let json = run_json(relative_path, extra_args);
    assert_eq!(json["ok"], true, "{}: expected ok:true, got {}", relative_path, json);
    assert_eq!(
        json["result"], expected_result,
        "{}: expected result {}, got {}",
        relative_path, expected_result, json
    );
}

// ── The 11 examples that execute correctly today ────────────────────────────

#[test]
fn ask_min_suspends_with_the_moon_landing_candidates() {
    let json = run_json("ask_min.cube", &[]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["suspended"], true);
    assert_eq!(json["question"], "which moon landing did you mean");
    assert_eq!(json["candidates"], serde_json::json!([1959, 1969]));
}

#[test]
fn compare_min_selects_the_larger_candidate() {
    assert_ok_result("compare_min.cube", &[], 58);
}

#[test]
fn decision_min_approves_over_threshold() {
    assert_ok_result("decision_min.cube", &[], 80);
}

#[test]
fn fibonacci_computes_fib_10() {
    assert_ok_result("fibonacci.cube", &[], 55);
}

#[test]
fn ground_min_abstains_with_no_knowledge_loaded() {
    // No --knowledge: QUERY finds nothing -> the gap branch (outcome 0).
    assert_ok_result("ground_min.cube", &[], 0);
}

#[test]
fn ground_min_asks_when_knowledge_has_an_ambiguous_key() {
    // With examples/facts_min.jsonl loaded, "mercury" has TWO facts on file
    // -> real QUERY retrieval (docs/DRIFT.md §A3) drives the ASK branch.
    let json = run_json(
        "ground_min.cube",
        &["--knowledge", "examples/facts_min.jsonl", "--fn", "solve", "--arg", "mercury"],
    );
    assert_eq!(json["ok"], true);
    assert_eq!(json["suspended"], true);
    assert_eq!(json["question"], "which one did you mean");
    let candidates = json["candidates"].as_array().expect("candidates array");
    assert_eq!(candidates.len(), 2, "mercury has exactly 2 facts in facts_min.jsonl");
}

#[test]
fn loop_min_accumulates_six_sevens() {
    assert_ok_result("loop_min.cube", &[], 42);
}

#[test]
fn qc_decision_fails_when_no_registers_are_injected() {
    // Plain `run`, no --arg: in_conf/in_class are unset. Until fix list P6-1
    // (2026-11-13) they read as 0, so 0 < min_confidence(70) was always true
    // and the answer was always REVIEW (2), a disposition nothing measured.
    // A missing value now never compares: the run fails with TypeMismatch.
    let json = run_json("qc_decision.cube", &[]);
    assert_eq!(json["ok"], false, "{}", json);
    assert!(json["error"].as_str().unwrap_or("").starts_with("TypeMismatch"), "{}", json);
}

#[test]
fn recall_min_returns_the_corrected_belief() {
    assert_ok_result("recall_min.cube", &[], 65);
}

#[test]
fn conversation_min_scores_a_generic_reply() {
    assert_ok_result(
        "conversation_agent/conversation_min.cube",
        &[],
        10,
    );
}

// ── The 2 examples that no longer compile at all (see their own header
// comments and docs/DRIFT.md "Bugs found" 1/2) ──────────────────────────────
//
// FIXED 2026-09-14: both used to be pinned as silently-wrong-but-compiling
// (gsm8k.cube returned a meaningless {"result":0}; conversation_agent.cube
// at least failed loudly, but only at RUNTIME, with a plain `run`). Two
// fixes landed together and change what "the correct pinned outcome" means
// for both files:
//
//   - C11 (a non-encodable expression operand -- BinOp, MapLit, StructLit,
//     MethodCall, Index, ...) is now a hard COMPILE error in EVERY mode,
//     not just `--strict`. Both files are full of exactly this shape
//     (arithmetic/struct-literal/map-literal/method-call RHS's their
//     authors never guarded against, unlike the safe-subset examples).
//   - `run` is strict by default now (no flag needed) -- so on top of the
//     always-on C11 errors, both files' `match` and unsupported-assign-
//     target violations (docs/DRIFT.md §C3/§C4) are ALSO now reported.
//
// Neither file's logic was touched (per the task: let them fail honestly,
// don't rewrite them) -- they simply no longer compile, which is now the
// correct, honest outcome for a program that could previously only ever
// produce a silently-wrong or outright-broken result. `run --json` prints
// the compile error to stderr and emits NOTHING on stdout (no JSON at all),
// since the failure happens before the VM ever starts -- hence `run_raw`
// instead of `run_json` here.

#[test]
fn gsm8k_no_longer_compiles_a_non_encodable_operand_is_now_a_hard_error() {
    let out = run_raw("gsm8k.cube", &["--json"]);
    assert!(!out.status.success(), "gsm8k.cube must no longer compile");
    assert!(out.stdout.is_empty(),
        "a compile-time failure must emit nothing on stdout: {:?}",
        String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("BinOp"),
        "expected a BinOp operand-encoding error (learn()'s `expected.result - actual.result`): {}",
        stderr);
    // The `match` assertion that used to live here was removed 2026-09-14:
    // `match` is no longer a strict violation, because `compile_match` now
    // emits a real COMPARE/COND arm-selection chain (DRIFT.md C3/A7, pinned
    // by tests/match_arms.rs). gsm8k.cube still fails to compile -- on the
    // C11 operand errors above, and on its unsupported `regs[reg] = ...`
    // assign target (C4) -- just no longer on `match`.
}

#[test]
fn conversation_agent_no_longer_compiles_a_non_encodable_operand_is_now_a_hard_error() {
    // Previously failed only at RUNTIME ("CALL: unresolved function name"),
    // and only under `run --json`'s ok:false JSON shape. It now fails at
    // COMPILE time instead -- earlier and more honestly -- for the same
    // C11 reason as gsm8k.cube above (this file is full of `BinOp`/
    // `StructLit`/`MethodCall` RHS's, e.g. `build_state`'s string
    // concatenation and `Reply { ... }` struct literals).
    let out = run_raw("conversation_agent/conversation_agent.cube", &["--json"]);
    assert!(!out.status.success(), "conversation_agent.cube must no longer compile");
    assert!(out.stdout.is_empty(),
        "a compile-time failure must emit nothing on stdout: {:?}",
        String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("BinOp"),
        "expected a BinOp operand-encoding error: {}", stderr);
}

// ── The 4 new examples added by this audit ──────────────────────────────────

#[test]
fn vsa_bind_recover_recovers_the_planted_subject() {
    let json = run_json("vsa_bind_recover.cube", &[]);
    assert_eq!(json["ok"], true);
    assert_eq!(json["result"], "cat");
    let sim = json["similarity"].as_f64().expect("similarity present for a real recovery");
    assert!(sim > 0.3, "expected a confident recovery similarity, got {}", sim);
}

#[test]
fn call_recursion_min_computes_factorial_5() {
    assert_ok_result("call_recursion_min.cube", &[], 120);
}

#[test]
fn for_loop_min_sums_the_array_via_real_for() {
    assert_ok_result("for_loop_min.cube", &[], 60);
}

#[test]
fn agent_min_runs_the_think_act_observe_chain() {
    assert_ok_result("agent_min.cube", &[], 1);
}

#[test]
fn match_router_selects_the_matching_string_arm() {
    // Added 2026-09-14 with the DRIFT.md C3/A7 fix (real `match` arm
    // selection). This is the CLI-level pin for it; `tests/match_arms.rs`
    // covers the behaviour directly.
    //
    // Two separate bugs had to be fixed for "compare" to be the answer:
    // arm selection itself (every arm used to run, in source order), and
    // `op::COMPARE`'s integer-only operand resolution (`Value::as_i64` is 0
    // for every string, so every string comparison was TRUE and the FIRST
    // arm -- "show_program" -- would always have won). A regression in
    // either one shows up here as a different string.
    let json = run_json("match_router.cube", &[]);
    assert_eq!(json["ok"], true, "match_router.cube must run clean: {}", json);
    assert_eq!(
        json["result"], "compare",
        "the arm whose pattern equals the subject must be the one that runs: {}",
        json
    );
}

// ── The four drafts under examples/drafts/ (pinned 2026-11-11) ──────────────
//
// `examples/drafts/` holds design drafts: programs written to the full
// specification and kept as targets (`CUBELANG_FIXES.md`, P3). None compiles
// today. Each test below pins where its draft stops NOW, on the same terms
// as the two rejected examples above: exit 1, nothing on stdout, the stop
// named on stderr. When a feature lands and a draft gets further, its test
// fails: move the pin to the new stop (or to a real result), and update
// `CUBELANG_FIXES.md` P3 and `examples/README.md` in the same change. The
// line:column in an assertion is the draft's own, so editing a draft moves
// it too.

fn draft_stops(relative_path: &str, expected: &[&str]) {
    let out = run_raw(relative_path, &["--json"]);
    assert!(!out.status.success(), "{} is a draft: it must not run clean yet", relative_path);
    assert!(out.stdout.is_empty(),
        "a compile-time failure must emit nothing on stdout: {:?}",
        String::from_utf8_lossy(&out.stdout));
    let stderr = String::from_utf8_lossy(&out.stderr);
    for want in expected {
        assert!(stderr.contains(want),
            "{}: its stop must name {:?}, got: {}", relative_path, want, stderr);
    }
}

#[test]
fn draft_bridge_mesh_stops_on_method_call_operands_and_its_io_block() {
    // The container parses and is compiled; strict refuses what it cannot run
    // (P3-8 undeclared interfaces, P3-9 a method call as an operand, P3-1 the
    // `io` block).
    draft_stops("drafts/bridge_mesh.cube", &[
        "strict compile",
        "`IOrchestrator` is neither a VM interface nor an in-file `interface` declaration",
        "21:9: expression form MethodCall cannot be encoded as an operand",
        "22:9: expression form MethodCall cannot be encoded as an operand",
        "an `io` block",
    ]);
}

#[test]
fn draft_mowm_manager_stops_at_pure_written_as_an_attribute() {
    // Does not parse: `@pure` on the line after `@external` (P3-6; `pure` is
    // a modifier).
    draft_stops("drafts/mowm_manager.cube", &[
        "27:5: expected 'function' or 'constructor', got At",
    ]);
}

#[test]
fn draft_mowm_spawner_stops_on_operands_a_field_read_base_and_throw() {
    // Parses; strict refuses it (P3-8, P3-9, and the two P3-10 items it now
    // reaches: a field read whose base is not a variable, and `throw`).
    draft_stops("drafts/mowm_spawner.cube", &[
        "strict compile",
        "`IOrchestrator` is neither a VM interface nor an in-file `interface` declaration",
        "expression form MethodCall cannot be encoded as an operand",
        "expression form BinOp cannot be encoded as an operand",
        "66:41: a field read needs a variable at its base",
        "strict: throw",
    ]);
}

#[test]
fn draft_orchestrator_stops_at_the_wildcard_hook_target() {
    // Does not parse: `@hook(*.error)` (P3-2).
    draft_stops("drafts/orchestrator.cube", &[
        "39:11: expected identifier, got Star",
    ]);
}

// ── Coverage guard ───────────────────────────────────────────────────────────

/// Every `.cube` file physically present under `examples/` must be named by
/// at least one test above -- so a NEW example added later without a pinned
/// test fails this guard instead of silently rotting unexercised.
#[test]
fn every_example_file_is_covered_by_a_pinned_test() {
    let covered: &[&str] = &[
        "ask_min.cube",
        "compare_min.cube",
        "decision_min.cube",
        "fibonacci.cube",
        "ground_min.cube",
        "gsm8k.cube",
        "loop_min.cube",
        "qc_decision.cube",
        "recall_min.cube",
        "conversation_agent/conversation_agent.cube",
        "conversation_agent/conversation_min.cube",
        "vsa_bind_recover.cube",
        "call_recursion_min.cube",
        "for_loop_min.cube",
        "agent_min.cube",
        "match_router.cube",
        "drafts/bridge_mesh.cube",
        "drafts/mowm_manager.cube",
        "drafts/mowm_spawner.cube",
        "drafts/orchestrator.cube",
    ];

    let mut found: Vec<PathBuf> = Vec::new();
    collect_cube_files(&examples_dir(), &mut found);

    let root = examples_dir();
    let mut uncovered = Vec::new();
    for f in &found {
        let rel = f.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if !covered.contains(&rel.as_str()) {
            uncovered.push(rel);
        }
    }
    assert!(
        uncovered.is_empty(),
        "example file(s) with no pinned test in tests/examples_pinned.rs: {:?}",
        uncovered
    );
}

fn collect_cube_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_cube_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "cube") {
            out.push(path);
        }
    }
}
