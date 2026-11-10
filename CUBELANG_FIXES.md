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

## P3 — Nick's swarm and MoWM drafts (Cowork, 2026-11-07; Nick's yes)

Four drafts, in `examples/drafts/`: `orchestrator.cube`, `bridge_mesh.cube`, `mowm_manager.cube` and
`mowm_spawner.cube`. Two copies have comments trimmed; the code is Nick's as written. They were checked with
`cubelang check` on the release binary built 2026-09-29. The source has commits through 2026-10-31, so first re-run
each draft against a fresh build and tick off whatever already passes. Build it in a separate target directory: the
main `target/release` binary is not overwritten.

The drafts are written to the full spec, while the compiler runs the safe subset above (flat operands, no `for`,
`match`, `call` or early `return` yet). Most of what follows is the gap between the two. Which gaps to close first is
CubbyLLM's call: orders 0062 (sandboxed helpers) and 0063 (split questions) say what they need.

- [ ] **P3-1 (P0 class): a `container` compiles to nothing, silently.** `bridge_mesh.cube` passes `check`, but
  `compile` warns "no programs found". The `container`, its `config` and `io` blocks, `vm.start_from_file`,
  `vm.wire` and `vm.bridge` emit no bytecode. Strict must reject any item it drops; containers compiling is P2-level
  work. Regression test: the draft.
- [ ] **P3-2: `@hook` with a wildcard target.** `@hook(*.error)` gives "expected identifier, got Star". Support
  `*.event`, or give a clear error. `@hook` itself is a strict compile error per the README.
- [ ] **P3-3: `??`** is used in the README's examples but not parsed. Implement it, or remove it from the docs and
  record it in `docs/DRIFT.md`.
- [ ] **P3-4: `grant` / `revoke ... from ...`** are documented (README, "Grant / Revoke Access at Runtime") but not
  parsed inside a function body. Implement them, or state where they are allowed.
- [ ] **P3-5: reserved words.** A parameter named `event` fails with "unexpected token in expression: Event". Publish
  the reserved-word list, and make the error name the keyword.
- [ ] **P3-6: `@pure` as an attribute** fails ("expected 'function' or 'constructor', got At"). `pure` is a modifier.
  Accept `@pure` as an alias, or say so in the error.
- [ ] **P3-7: destructuring.** `for (let [k, v] of map)` and `let [a, b] = f()` both fail ("expected identifier, got
  LBracket"). Support them, or document that they are not supported.
- [ ] **P3-8: undeclared interfaces.** `implements IOrchestrator, IDeployable` fails: "neither a VM interface nor an
  in-file `interface` declaration". Ship these as VM interfaces, or document the in-file declaration they need.
- [ ] **P3-9: operands.** A map literal (`{}`) and a method call's result can't be operands ("cannot be encoded as
  an operand ... compute it into a register first"). This is the safe subset's flat-operand rule. Lift it, or make
  the message point to the rewrite.
- [ ] **P3-10: not reached yet**, to check once the drafts get further:
  - `throw` (strict rejects it, per the CLI help);
  - the ternary `?:`;
  - named arguments (`embed(input, model: ...)`);
  - `emit` with a struct literal;
  - `set.insert` and `map.delete`;
  - `vm.timestamp()` and `vm.deploy_from_file()`.

This file's global invariant holds for every item: `cargo test` green, a regression test per fix, opcode byte values
unchanged.

---

## P4 — The run report CheezML needs, kept confidential (Cowork, 2026-11-10; Nick's yes)

CubbyLLM's host turns every VM run into a CheezML record (vocabulary: https://w3id.org/cheezml/; CubbyLLM order
0064). A record may only state what ran, so the run report must carry it. Today `RunResult` and `BranchReport`
(`proto/reasoning.proto`) give the outcome, ops, jumps, budgets, wall time, each QUERY's key and hit count, and each
recover with its runner-up and margin. That is not enough for a certificate. Nick also asked that the answers stay
confidential: "it should also be encrypted so not everyone can see the answers".

**In plain words:** the VM says exactly which facts it read and which checks passed, so Cubby's receipt is built
from what really happened, and that receipt stays private.
- **Over a conventional model:** a model cannot say which facts it used; it can only explain afterwards.

New fields are added to the proto without renumbering or changing existing ones.

- [ ] **P4-1: the facts read.** Each QUERY and RECALL reports the facts it matched, not only a count: each one's id
  (the knowledge line's key and its position), its `source`, and its revision or content hash. A store hit with no
  source is reported as such, never filled in.
- [ ] **P4-2: the checks that ran.** Every COMPARE and every guard reports, in order: the relation, the operands (by
  register name, with values only as hashes) and the result. The host maps them to the checks it declared. A check
  that did not run leaves no entry.
- [ ] **P4-3: the rules applied.** When a run applies a rule (a rule-library program or a folded rule), it reports
  one line per step: the rule's id or hash, the conclusion, the premises. This is the shape of CubbyLLM 0055's
  derivations file (asserted, derivations, digest), so a separate checker can re-check it.
- [ ] **P4-4: errors as codes.** `RunResult.error` becomes a closed enum (budget exceeded, naming which; compile
  error; strict-mode reject; runtime fault by kind; an empty program). Free text is kept only in a debug field that
  the host never writes into a record.
- [ ] **P4-5: what ran.** The report carries the sha-256 of the source and of the compiled bytecode, whether strict
  mode was on, and the VM's version and commit.
- [ ] **P4-6: nothing compiled is an error.** A program that compiles to no bytecode fails the run (see P3-1). It is
  never an ok result.
- [ ] **P4-7: a hash of the report.** The host gets a sha-256 over a canonical encoding of the report (wall time
  left out, so the same run gives the same hash). Test: the same request twice gives the same hash.
- [ ] **P4-8: confidentiality.**
  - **No values in logs.** The VM never writes a question, an answer, a fact's text or a recovered symbol to its
    logs or stderr. Debug output that does is behind an explicit development flag that a release build rejects.
  - **No plain text over a network.** `run-proto` stays on a local pipe or socket. If it ever listens on a network
    address, it requires TLS with client certificates and refuses a plain connection.
  - **Values by hash in the report.** The report carries ids and hashes. A value appears only where the host needs
    it for the answer: the result, and an ASK's question and candidates.
  - **Knowledge isolation.** The resident process caches parsed stores by (path, mtime). The host passes an allowed
    root per tenant; a `knowledge_path` outside it is refused, and the cache is never shared between roots. A
    branch's `assume` facts are never persisted.
  - **Tests:** a run over planted secret strings leaves none of them in logs, stderr or the report's debug-free
    fields; a plain connection to a network listener is refused; a knowledge path outside its root is refused.
- [ ] **P4-9: what stays on the host side** (CubbyLLM 0064, not this repo): encrypting the records at rest, signing
  them, and who may read them. The VM's part is to leak nothing on the way there.

**Added 2026-11-10 for CheezML v0.2** (Nick's yes: "Carry it into the VM and store"). A v0.2 record names the store
state a run read and the date the question was about (`cheez:storeSnapshot` and `cheez:asOf`), and a run that read a
fact without both fails validation. A record only states what ran, so the report must carry them. They are the
store's two clocks seen from the run: which state of the store was read (recorded time), and which moment in the
world the question is about (valid time).

**In plain words:** every run says which copy of the knowledge base it read and which date it answered for, so the
same question can be run again later against the same copy and give the same answer.
- **Over a conventional model:** a model cannot say which state of its knowledge an answer came from, and it cannot
  be asked again as of last month.

- [ ] **P4-10: the store state read.** For each store a run reads (a knowledge file today, the `wdstore` crate from
  CubbyLLM 0055's step 3), the report carries:
  - its snapshot hash: for a knowledge file, the sha-256 of its bytes as loaded; for `wdstore`, the snapshot hash
    the store gives (CubbyLLM 0055's addendum of 2026-11-10);
  - the release it was built from, when the store's manifest names one: its name, version and hash;
  - for a branch, the hash of its `assume` lines and of its matched exclusions, so base and branch together name
    exactly what was read;
  - one hash over all of the above, in a fixed order: the run's snapshot. A CheezML record names one snapshot per
    run, and this is the one it cites.

  The resident cache keeps the hash with each parsed store and recomputes it when the file changes.
  - **Pinned runs:** the request may carry `expect_snapshot`. If the store's hash differs, the run fails with the
    code `store_changed` before any read. It never reads another state instead.
  - **Tests:** the same file gives the same hash; one changed line gives another; a pinned run against a changed
    store is refused; a recorded run replayed against its snapshot gives the same report hash (P4-7).
- [ ] **P4-11: the as-of date.** The request carries `as_of`, a date and time with its zone: the moment in the
  world the question is about. The report echoes it.
  - **No other clock.** A program never reads the system clock. A date used in a program comes from `as_of` or from
    a recorded host call (P5-1), so a replay gives the same result on any day.
  - **Facts with dates.** When a store's facts carry the period they held (`wdstore` keeps Wikidata's start, end
    and point-in-time qualifiers with their precision), a QUERY matches only the facts that held at `as_of`. The
    report counts the facts left out for that reason. A fact whose dates are too coarse to decide is reported with
    its own code, never guessed.
  - **Strict mode** rejects a run that reads a store with no `as_of`. Outside strict mode the report says it was
    not given, and the host cannot write a certificate from that run.
  - **Tests:** a fact that ended before `as_of` is not matched and is counted; a fact dated to the year, asked as
    of a day inside that year, gives the "too coarse" code; the same request run on two different days gives the
    same report hash.
- [ ] **P4-12: a program can fetch an outside source, through a recorded call** (revised 2026-11-10; Nick: "it
  needs to do some outbound connection to fetch news or other sources"). News feeds, a live Wikidata lookup and other
  sources are reached from inside a run, with host functions of a `source` module on the host-call channel (P5-1).
  - **Who opens the connection.** The host does, and hands the VM the result. In CubbyLLM this is `cubby_serve`'s
    fetchers, by Nick's rule of 2026-10-22 (what fetches lives in the server and hands the core a fetch function).
    For the standalone `cubelang` command, a built-in fetcher can be attached with an explicit flag. The VM's core
    has no network client of its own, so a program can only reach what the run was granted. Nick confirmed this
    design on 2026-11-10 ("I'll pick your safe way"): the socket stays out of the VM.
  - **Off by default.** A program must `use source`, and the request names the sources the run may reach. A call
    to a source outside that list is refused with a code before any connection is made.
  - **What the report records** for each call, which is what a `cheez:SourceRead` needs: the source's id and
    address, the query's hash, the response's hash, the time, and the licence the host holds for that source.
  - **Replay.** A recorded run replays from its recorded responses and never touches the network (P5-1).
  - **A response is data.** It is never compiled or run as a program. Facts taken from it enter the store only
    through the host's gate, with their source (CubbyLLM 0007 and 0024); the store then has a new state. The run
    in progress keeps reading the state it started with, and the next run reads the new one, so a run still names
    one snapshot (P4-10).
  - **Only the query leaves.** The report lists every query sent and where, by hash, so an audit shows what left
    the machine. A tenant whose list of sources is empty sends nothing.
  - **Limits.** `max_source_calls` joins `Budget`, with a time and a size limit per call. A failed fetch comes back
    as a code and fails the call. It never becomes a default value.
  - **Tests:** a `source` call without `use source` does not compile; a source outside the run's list is refused
    with no connection attempted; a recorded run replays with the network off to the same report hash; a response
    holding CubeLang text is not executed; a dependency check fails the build if a network client is added to the
    VM's core.

The global invariant holds: `cargo test` green, a regression test per fix, opcode byte values unchanged.

---

## P5 — Opcodes that compute nothing today, and world calls to MoWM (Cowork, 2026-11-10; Nick's yes)

Nick: "we also need to code the missing function like to create a world and the other ones". His decision
(2026-11-10): **MoWM owns the worlds** and the VM calls it through a host-call channel, so there is one world tree
and one encoder and the calibrated thresholds stay valid. Pure vector operations are ported into the VM itself.

**Today:**
- 25 opcodes are routed to `trace_structural` in `src/vm/engine.rs`: they log a step and compute nothing. They are
  INST, GEN, SEQ, DIFF, DETECT_PATTERN, PREDICT, MATCH, DEBATE, DISCOVER, DECODE, SCORE, SPECIALIZE, REWARD, INFER,
  MERGE, SPLIT, FILTER, MAP_ROLES, REDUCE, TEMPORAL_BIND, ANALOGY, BROADCAST, EXPLORE, FORGE and SYNC.
- The module registry (`src/vm/registry.rs`) holds `demo` and `vsa.recover` only.
- Nick's MoWM drafts (`examples/drafts/`) call world functions that exist nowhere: the axiom centroid, the best
  member, the depth z, the tau threshold, spawning a child, a transition step, cross-examining worlds,
  `vm.timestamp`.
- cubemind's Python VM (`cubemind/reasoning/vm.py`, named the reference in MoWM's v0.6 ISA spec) has real versions
  of about half of them.

**In plain words:** a CubeLang program can create, split, route to and question MoWM's worlds, and every one of
those steps is recorded, so a world decision is as checkable as an answer.
- **Over a conventional model:** a mixture of experts routes inside the network. Nobody can replay why one expert
  was chosen, or prove that a new one was created by the rule.

- [ ] **P5-1: the host-call channel.** A registry function can be marked `host`. Calling it sends
  `HostCall {call_id, module, function, args}` on the `run-proto` connection; the host answers
  `HostResult {call_id, value | error code, world_state_hash}`; the VM carries on.
  - Local pipe only (P4-8). A `max_host_calls` budget joins `Budget`.
  - Every call and its result go into the run report, arguments and results by hash (P4).
  - **Replay:** given the recorded results, a run reproduces exactly with no host attached. Test: record, replay,
    same report hash.
  - A host error comes back as a code and fails the call. It never becomes a default value.
  - A program reaches only the host functions of modules it `use`s (the registry's deny-by-default rule).
- [ ] **P5-2: the `mowm` module, as host functions.** The handlers live in the mowm repo
  (`docs/specs/2026-11-10-vm-world-calls.md` there). First the three that CubbyLLM's routing and split records need:
  - `create(name)` gives a new world's id;
  - `split(parent)` gives a child at z + 1, and fails with a code at z_max;
  - `route(text)` gives the outcome (route or spawn), the best world and its score, the runner-up, the margin,
    and the ids of the calibrations behind the thresholds used.

  Then:
  - `best_member(world, text)` and `centroid_score(world, text)`;
  - `depth(world)`, `parent(world)`, `list()`;
  - `learn(world, fact)`;
  - `retire(world)`: retired, never deleted.

  A program compares a score only with a threshold that MoWM returned with its calibration id, never with a literal
  of its own. The compiler rejects a score compared with a literal in strict mode.
- [ ] **P5-3: SPECIALIZE runs.** It becomes a host call to MoWM's transition rule: find the world for a transition,
  or spawn one when the best similarity is below tau, else consolidate. It returns the world id and whether it
  spawned. These are cubemind's VSAVM semantics (`_specialize`, its WorldManager).
- [ ] **P5-4: port the pure vector opcodes,** with cubemind's VSAVM as the reference: SEQ, DIFF, MATCH, DECODE,
  DETECT_PATTERN and PREDICT, and EXPLORE with REWARD (a UCB bandit, deterministic).
  - **Two vector families.** The Rust VM's vectors are MAP-bipolar (D = 4096 in the VM), while cubemind and MoWM
    use block codes (k = 80, l = 128). Similarity values therefore differ between the two VMs.
  - **Parity test per opcode:** on fixtures built so the answer is unambiguous, both VMs give the same discrete
    result (index, label, id, decision). The similarities are reported side by side, not required to be equal.
  - **Choosing one vector family** for all three repos is Nick's decision, kept separate from this job. `engine.rs`
    notes that reconciling the dimensions is out of scope.
- [ ] **P5-5: learned opcodes stay outside the VM.** SCORE (cubemind uses a learned value model, CVL), FORGE (LoRA
  adapters), DISCOVER and DEBATE become host calls to cubemind if a program needs them.
  - Their outputs are proposals. The report marks every value derived from them.
  - Such a value is never certified unless a later deterministic check verifies it.
- [ ] **P5-6: no reference semantics yet.** INST, GEN, INFER, MERGE, SPLIT, FILTER, MAP_ROLES, REDUCE,
  TEMPORAL_BIND, ANALOGY, BROADCAST and SYNC are not implemented in cubemind's VSAVM either. Neither is **UNIFY**:
  it logs "UNIFY a b" and computes nothing, so it joins this list. The proposed paragraphs are in
  `docs/proposals/2026-11-10-trace-only-opcodes.md`. **Nick approved all thirteen as written (2026-11-10).**
  - **Suggested order,** by who needs them:
    - first INFER (its derivation lines feed CubbyLLM 0055's certificates), then FILTER, REDUCE and SPLIT;
    - then UNIFY, INST and GEN, with CubbyLLM 0057's type checks;
    - then MERGE, MAP_ROLES, TEMPORAL_BIND, ANALOGY and BROADCAST;
    - SYNC last, once CubbyLLM 0063 runs parallel sessions.
  - Strict mode keeps rejecting each one until it lands with its tests (P5-8).
- [ ] **P5-7: the drafts' `vm.*` helpers.**
  - `vm.timestamp` is the request's time, passed in by the host, fixed for the run and recorded, so runs stay
    replayable.
  - `vm.get_program` gives the program's hash (P4-5).
  - `log` writes to the debug log, with no values (P4-8).
  - `vm.deploy_from_file`, `vm.start_from_file`, `vm.wire` and `vm.bridge` stay with P3-1 (containers).
- [ ] **P5-8: off the blocklist one by one.** Each opcode or function that lands leaves the strict-mode blocklist,
  with its regression test, and `validate` (P0-3) reports it as executing.
- [ ] **P5-9: every vector carries its kind** (Nick's yes, 2026-11-10). The Rust VM's vectors are MAP-bipolar, and
  MoWM's and cubemind's are block codes. The two must never meet by accident.
  - Every vector value carries its family (`map-bipolar` or `block-code`), its size (D, or k × l) and the
    codebook's seed.
  - Binding, bundling, similarity or cleanup across two families or two sizes fails with a new code,
    `VectorFamilyMismatch` (added to P4-4's list). It never returns a similarity.
  - Vectors that arrive from the host (P5-1) say their family; MoWM's are block codes.
  - The run report lists the families each run used (P4).
  - **Tests:** a mixed-family bind fails with the code; a same-family bind works; the report names the family.
  - The opcode bytes don't change. The family goes in the value's tag, so stored memories keep their format.
  - Which family the VM should use in the end is measured first, in CubbyLLM 0046's R9.

**Order:** P5-1 with P5-9, then P5-2's first three calls, then P5-3, P5-4 and P5-6 (approved).

The global invariant holds: `cargo test` green, a regression test per fix, opcode byte values unchanged.

## Notes
- The QC pilot ships on P0 + P1 only. P2 is the path to richer programs later.
- `examples/qc_decision.cube` is the reference safe-subset program; keep it
  compiling and passing `validate` (P0-3) as a guardrail for the executing half.
- Verified working today: 96/96 tests green; qc_decision.cube compiles to
  309 bytes of safe-subset bytecode and runs deterministically.
