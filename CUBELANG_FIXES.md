# CubeLang / Cubby — Fix List for Claude Code

Context: the CubeLang VM splits into an **executing half** (arithmetic, control
flow, memory) and a **trace-only half** (the extended reasoning opcodes, which
currently log a step but compute nothing). The QC monetization slice ships ONLY
the executing half. These tasks (a) make it impossible to silently ship a stub,
(b) wire the brain-layer integration, and (c) close the validation gaps.

GLOBAL INVARIANT — apply to every task:
- Run `cargo test` after each change. All 96 tests must stay green (0 failed).
- Add a regression test for every fix.
- Do NOT change opcode byte values — they are synced across opcode-vsa-rs,
  cubelang, and the cubemind Python VM. Changing them breaks cross-repo sync.

SAFE-SUBSET FENCE (the executing half — reference for P0 work):
  create, assign(literal), add, sub, mul, div, sum, push, pop, query,
  store, recall, remember, forget, compare, if/else, while. Flat operands only.
  Everything else (for, match, return-as-branch, call, and all extended
  reasoning opcodes) is NOT safe to ship until its P2 task lands.

---

## P0 — Safety: never silently no-op (do these first)

- [ ] **P0-1: `--strict` compile mode that rejects non-executing constructs.**
  - Files: `src/compiler.rs`, `src/main.rs` (wire the flag), `src/vm/engine.rs`
    (the opcodes routed to `trace_structural` are the authoritative non-executing
    list).
  - Problem: extended opcodes (predict/match/score/analogy/infer/discover/...),
    plus `for`, `match`, and `call`, compile and run without error but do not
    affect the result. In a QC rule this is a silent-wrong-answer bug.
  - Fix: add `cubelang compile --strict`. In strict mode, emit a hard compile
    error naming the offending construct and source line if the program uses any
    trace-only opcode, `for`, `match`, or `call`.
  - Acceptance: `compile examples/qc_decision.cube --strict` succeeds; a program
    containing `match`/`for`/`predict` fails `--strict` with a clear named error.

- [ ] **P0-2: Fix `assign` register-to-register.**
  - Files: `src/vm/engine.rs` (the `op::ASSIGN` arm).
  - Problem: `assign x = y` (y a register) stores the register-NAME STRING, not
    y's value, because ASSIGN uses `to_value()` instead of `resolve_value()`.
    Only literal assignment currently works. This is a footgun for any rule that
    copies one register into another.
  - Fix: in the ASSIGN arm, resolve a Named/Global operand to the source
    register's value (mirror how `op::ADD` calls `self.resolve_value(...)`).
    Keep literal assignment behaviour identical.
  - Acceptance: new test — `assign a = 5; assign b = a; sum b` returns 5
    (currently returns a string/0). All 96 existing tests stay green.

- [ ] **P0-3: `validate` subcommand — report executing vs trace-only opcodes.**
  - Files: `src/main.rs`, plus `src/compiler.rs` or new `src/validate.rs`.
  - Problem: there is no way to confirm at a glance that a program is fully
    executable before shipping it.
  - Fix: add `cubelang validate <file>` that prints, per function, which opcodes
    execute vs trace-only, and flags single-pass `for`, unconditional `match`
    arms, and SKIP-emitting `return`.
  - Acceptance: `validate examples/qc_decision.cube` reports "all executing, no
    stubs"; running it on a program with reasoning opcodes lists the trace-only
    ops by name and line.

---

## P1 — Make the QC slice real (integration + measurement)

- [ ] **P1-1: Brain -> CubeLang host shim.**
  - Files: new `cubemind/scripts/qc_runner.py` (or `cubelang/examples/run_qc.rs`).
  - Task: load `qc_decision.cubebin`, run the constructor, inject
    `in_class:int` and `in_conf:int(0..100)` as registers, call `solve`, return
    `{disposition, pass_count, reject_count, review_count, total_count}`.
    Convert the brain layer's float confidence to integer percent in the shim
    (round(conf*100)) — the VM compares via i64, so a raw float resolves to 0.
  - Acceptance (deterministic across runs):
      (class=0, conf=95) -> PASS (0)
      (class=2, conf=95) -> REJECT (1), last_defect_class stored = 2
      (class=1, conf=40) -> REVIEW (2)

- [ ] **P1-2: MVTec AD eval harness for the brain layer (the monetization gate).**
  - Files: new `cubemind/benchmarks/mvtec_brain_eval.py`.
  - Task: run the `create_cubemind` + live_brain forward loop over the public
    MVTec AD industrial-anomaly dataset. Measure, per category: precision,
    recall, number of examples needed to teach a class, and ms/image on the
    target hardware.
  - Acceptance: emits a per-category table {precision, recall, n_teach,
    ms_per_image}. Assert NO target numbers — the harness measures honestly so
    we learn the real numbers before quoting any to a buyer.

- [ ] **P1-3: VSA-head vs Linear-head A/B (the missing efficiency measurement).**
  - Files: `cubemind/model/cubby/` — extend `train_torch.py` with a
    `--head {vsa,linear}` switch if not already present; new `ab_compare.py`.
  - Task: train two runs identical in everything except head type, same corpus,
    same budget, same seed; report val PPL and a coherence proxy on ONE common
    held-out split.
  - Acceptance: a single two-row table (vsa / linear) with PPL + sample
    coherence side by side, on the same split. This is the claim the paper
    currently asserts but does not measure.

---

## P2 — VM completeness (deferred; NOT needed for the QC pilot)

Do these only after P0/P1. Each removes one item from the strict-mode blocklist
in P0-1 once it genuinely executes — update P0-1's blocklist as each lands.

- [ ] **P2-1: Real `for` iteration.** `src/vm/engine.rs` (`op::LOOP`) +
  `src/compiler.rs` (`compile_for`). Currently emits a single-pass LOOP marker.
  Acceptance: `for x of [1,2,3] { add total, x }` yields total = 6.

- [ ] **P2-2: `match` arm selection.** `src/compiler.rs` (`compile_match`) +
  engine. Currently compiles every arm body unconditionally. Acceptance: only
  the matching arm's body executes.

- [ ] **P2-3: Intra-program `CALL`.** `src/vm/engine.rs` (`op::CALL`). Currently
  trace-only. Acceptance: a function calling another runs the callee and returns.

- [ ] **P2-4: `return` early-exit.** `src/compiler.rs` + engine. Currently emits
  SKIP; the VM returns the accumulator regardless of where `return` appears.
  Acceptance: a `return` mid-function stops execution and returns that value.

---

## Notes
- The QC pilot ships on P0 + P1 only. P2 is the path to richer programs later.
- `examples/qc_decision.cube` is the reference safe-subset program; keep it
  compiling and passing `validate` (P0-3) as a guardrail for the executing half.
- Verified working today: 96/96 tests green; qc_decision.cube compiles to
  309 bytes of safe-subset bytecode and runs deterministically.
