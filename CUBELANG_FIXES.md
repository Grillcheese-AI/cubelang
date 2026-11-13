# CubeLang / Cubby — Fix List for Claude Code

What is still to do in the compiler and the VM. What runs today is in `docs/REFERENCE.md`; where the README's
specification and the code differ is in `docs/DRIFT.md`. An item leaves this list when a program shows it running.
Closed items are at the end, each with the program that showed it.

Brought in line with the code on 2026-11-11 (Cowork; Nick: "cubelang is turing complete and those function are
done, the repo md files are not current"). The safe-subset fence that opened this file, and the P0 and P2 items it
guarded, are closed: `for`, `match`, a call with recursion, an early `return` and register-to-register `assign`
all execute. What still computes nothing is the trace-only half (the extended reasoning opcodes, P5) and the forms
P3 lists.

GLOBAL INVARIANT — apply to every task:
- Run `cargo test` after each change. Every test stays green.
- Add a regression test for every fix.
- Do NOT change opcode byte values — they are synced across opcode-vsa-rs,
  cubelang, and the cubemind Python VM. Changing them breaks cross-repo sync.

---

## P1 — Make the QC slice real (integration + measurement)

*(Not checked on 2026-11-11: these three tasks live in the cubemind repository.)*

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

## P3 — Nick's swarm and MoWM drafts (Cowork, 2026-11-07; Nick's yes)

Four drafts, in `examples/drafts/`: `orchestrator.cube`, `bridge_mesh.cube`, `mowm_manager.cube` and
`mowm_spawner.cube`. Two copies have comments trimmed; the code is Nick's as written. They are written to the full
specification, and the items below are what the compiler does not take yet. Which to close first is CubbyLLM's
call: orders 0062 (sandboxed helpers) and 0063 (split questions) say what they need.

Run again on 2026-11-11, on the build of this line's code (d25dbc5). Where each draft stops today:
- `orchestrator.cube`: 39:11, "expected identifier, got Star" (P3-2).
- `bridge_mesh.cube`: the container is compiled; the draft stops on a method call used as an operand (22:9, P3-9)
  and on strict naming its `io` block (P3-1).
- `mowm_manager.cube`: 27:5, "expected 'function' or 'constructor', got At" (P3-6).
- `mowm_spawner.cube`: 66:41, a field read whose base is not a variable, and `throw` (P3-10).

Each stop is pinned in `tests/examples_pinned.rs` (2026-11-11): when a draft gets further, its test fails, and
this list, the pin and `examples/README.md` change together.

- [ ] **P3-1: a container's `io` block.** A `container` is compiled now; it was dropped silently, and
  `bridge_mesh.cube` used to pass `check` with no program found. Strict names what it cannot run: the draft stops on
  its `io` block. Not checked again: `vm.start_from_file`, `vm.wire` and `vm.bridge` (see P5-7). Regression test:
  the draft.
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
- [ ] **P3-9: operands.** A method call's result can't be an operand ("cannot be encoded as an operand ... compute
  it into a register first"); the message points to the rewrite. A map literal passes now. Lift the rule for a
  method call, or keep it as the stated rule.
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

**Added 2026-11-10 for CheezML v0.2** (Nick's yes: "Carry it into the VM and store"; the three items approved as
written the same day: "all good"). A v0.2 record names the store
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

## P6 — Comparisons that cannot pass by accident, and the forms a generated program needs (Cowork, 2026-11-13; Nick's yes)

Found by CubbyLLM's order 0067, which will have a model write CubeLang programs that check replies against rules.
Its probe ran 71 small programs on a release build of this line (3f37515): 37 gave the value wanted, 29 failed to
compile or run, 5 gave a wrong value with no error. They are in CubbyLLM's `validation/logs/vm_ops_probe_2026-11-13.json`,
each with its source, the value wanted and the value got.

**Today:**
- A missing value (null: an index past a list's end, an index into a string, a misspelled name) or the number 0
  **compares equal to any string literal** (`==` true, `!=` false), and against a number it acts as 0 (`< 7` true).
  In the probe this gave four wrong values with no error: a prefix miss (`"hx"` in `"hello"`) and a suffix miss both
  pass, two different strings' characters compare equal, and a count of capitals in `"HeLLo"` comes out 130 (wanted 3).
- `"hello"[1]` returns null, silently.
- No `&&`, `||`, `!` or `else if` in a condition; no call, index, method call or arithmetic inside an expression (only
  as a `let`'s right side); no `break`; `div` gives 3.5 for 7 / 2 and there is no `%`; no string helpers.

**In plain words:** a check a model writes either runs on real values or stops and says it could not check. A typo or
an index off by one can no longer make a wrong reply pass.
- **Over a conventional model:** a model judging its own reply gives a sampled verdict that nobody can replay. Here the
  verdict is a program run, and since P6-1 it cannot come from a value that was never there.

- [x] **P6-1: a missing value never compares** (blocks CubbyLLM 0067's second reader; first).
  - Comparing null with anything, or a string with a number, fails the run with a new code, `TypeMismatch` (added to
    P4-4's list). It never returns true or false.
  - An index past a list's end fails with `IndexOutOfRange`, never null.
  - A name that was never declared is a compile error in strict mode.
  - **Tests:** the four probe programs above fail with their codes; `0 == "x"` and `null != "x"` fail; `"x" == "x"`
    is true and `"x" == "y"` false, as today; number comparisons unchanged; `examples/qc_decision.cube` still passes
    `validate`.
- [x] **P6-2: indexing a string gives its character.** `s[i]` returns the i-th character (Unicode scalar values, the
  way the host counts them); past the end fails with `IndexOutOfRange`. Tests: `"hello"[1] == "e"`; the probe's
  composed prefix and suffix programs give 1 on a hit and 0 on a miss.
- [ ] **P6-3: `&&`, `||`, `!` and `else if` in conditions,** short-circuit. Tests: both sides of each, and a three-way
  `else if`.
- [ ] **P6-4: expressions as operands:** calls, method calls, indexes and arithmetic inside a condition, a `return`
  or an argument (`if (xs.len() > 3)`, `return a + b * 2`). Tests: the probe's failing spellings now give the values
  wanted.
- [ ] **P6-5: whole-number division and `%`.** `div` keeps its meaning. If a new form needs a new opcode, it takes a
  free byte and the cross-repo sync note; no existing byte changes. Tests: 7 // 2 is 3, 7 % 2 is 1, a negative case.
- [ ] **P6-6: `break`** in `for` and `while`. Test: a loop that stops at its fourth turn.
- [ ] **P6-7: string helpers:** `starts_with`, `ends_with`, `upper`, `lower`, `contains`, and `+` for concatenation.
  Tests: a hit and a miss for each, in English and French (accents kept).

- [x] **P6-8: long lists fail loudly** (found by CubbyLLM 0067's step 2, 2026-11-13; with P6-1). As Code A reported
  it: a list literal of 254 or more strings makes the run return nothing, and one of exactly 255 numbers errors. The
  first test reproduces both and pins the exact edge. A list longer than the VM can hold fails with a code that names
  the limit (`TooLong`), never an empty result. Raising the limit is a separate choice, made only if the bytecode
  format allows it without changing an opcode byte. Until then CubbyLLM's views refuse more than 250 rows.

- [x] **P6-9: two different strings never share a key** (found by CubbyLLM 0067's step 2, 2026-11-13; Nick: "should
  be fixed in Code B cubelang version"). The compiler keys every identifier and string literal by BLAKE3's first two
  bytes (`compiler.rs` `name_hash`, `engine.rs` `name_table`), so two spellings with one key read as one:
  `"thing" == "government"` is true (both `15d0`; also `"eptwcv"` / `"rmcens"`), and two variables with one key would
  share a register. 121 of 500 generated programs carried such a pair.
  - Two different strings compare unequal and two names never share a register, whatever their hash. Interning by
    the whole string (a program's own table, the operand an index into it) keeps a two-byte operand without a hash;
    a wider key is the other way. Either way no opcode byte changes, and the cubemind and opcode-vsa-rs VMs get the
    sync note if the operand's meaning changes.
  - If any key stays a hash, a collision inside one program is a compile error naming both spellings.
  - **Tests:** the smallest program above returns 0; the two pairs found compare unequal; two colliding variable names
    keep their own values.

- [x] **P6-10: P6 without the slowdown** (found by CubbyLLM 0072's rerun, 2026-11-13). Every verdict stayed the same
  on `676df1d` (47,000+ runs), but the same programs run 1.5 to 3.5 times slower than before (medians, same machine,
  same day: a 50-item merge check 21.6 ms on main's September build against 76.8 ms; a sort check 18.0 against 63.1;
  a one-pass replace check 0.7 against 1.7; CubbyLLM `validation/logs/verb_programs_2026-11-13_p6.json`). CubbyLLM
  0072's line, p95 at most 50 ms at 50 items, held at 34 ms on `3f37515` and fails at 82.8 ms now.
  - Profile first and name the cause (suspects: hashing an 8-byte key on every register access, the missing-value
    bookkeeping, the collision check if it runs at run time instead of compile time).
  - **Line:** on CubbyLLM's timing programs, `3f37515` rebuilt in its own target directory and the fixed build timed in
    the same session: the fixed build no slower than `3f37515` by more than 10% at the median, with every P6 test green
    and no verdict changed. Then CubbyLLM 0072's time line is read again on it.

**Order:** P6-1 with P6-8 and P6-9, then P6-2, then P6-10 (before CubbyLLM 0069's latency part), then the rest in any order. Each probe program that fails today becomes a
regression test with the value it should give. The global invariant holds.

## Notes
- `examples/qc_decision.cube` stays compiling and passing `validate`: it is the guardrail for the executing half.
- The tree holds 499 test functions (2026-11-11). The "96 tests" this file used to name was June's count.

## Closed, each checked by running (2026-11-11, on this line's code)
- **P0-1, strict mode:** the default for `run`, `compile` and `check`; `--loose` opts out.
- **P0-2, register-to-register `assign`:** `assign a = 5; assign b = a; sum b` returns 5.
- **P0-3, `validate`:** `validate examples/qc_decision.cube` prints "all executing, no stubs".
- **P2-1, a real `for`:** `for (let x of [1, 2, 3])` sums to 6; `examples/for_loop_min.cube` returns 60.
- **P2-2, `match` runs one arm:** a three-arm match on 2 runs the second arm only (fixed 2026-09-14).
- **P2-3, a call:** `examples/call_recursion_min.cube` returns 120, with recursion. A callee reads its declared
  parameter names (2026-10-24; before that, only `arg0`, `arg1`).
- **P2-4, an early `return`:** a `return` inside an `if` ends the function with that value.

The items as first written, with the fence, are in this file's history (a6f2362).

## Closed 2026-11-13, each checked by running (Code B, `676df1d`; the cowork build rebuilt from it)
- **P6-1:** a compare that meets a missing value fails: `let v = xs[5]; if (v == "a")` on a list of 2 gives `IndexOutOfRange`; `0 == "x"` gives `TypeMismatch`; an undeclared name is `UndeclaredName` (`tests/p6_compare.rs`).
- **P6-8:** a list literal of 254 items or more is `TooLong` at compile time; 253 run (`tests/p6_compare.rs`).
- **P6-2:** `"hello"[1] == "e"` is true; `len()` counts characters ("été" is 3).
- **P6-9:** `"thing" == "government"` returns 0 on 8-byte keys; two colliding names keep their own values (`tests/p6_keys.rs`).
- **Found and fixed with them:** a bare `if (t)` ran as `if (t == 6)`; STORE / RECALL keyed the memory by display tokens (`recall_min`'s 65 was nearest-key luck).
- **P6-10 (2026-11-13, Code B, `b3b3360`):** a register's name built in one allocation, COMPARE and INDEX look a name up once. On CubbyLLM's timing programs, alternated in one session with `3f37515` and `676df1d`: 0.74x to 0.97x of `3f37515`'s median on every program of 1 ms or more (merge at 50 items 20.13 ms against 24.44 and 90.16); 525 of 525 verdicts agree (CubbyLLM `validation/logs/vm_p610_timing_2026-11-13.json`). The cowork build is rebuilt from it.
