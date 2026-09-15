# CubeLang Documentation Drift Audit

Method: every finding below was reached by reading `src/` (never by trusting
README.md/SPEC.md/ASK_SUSPEND.md's prose) and, wherever possible, confirmed
by executing `$CUBELANG_EXE` or citing a passing/failing test. Each entry is
tagged **[read]**, **[ran]**, or **[test]** for how it was verified. See
`docs/REFERENCE.md` for the full inventory these findings are drawn from.

---

## A. Documented but wrong or absent

### A1. Permission attributes are advertised as enforced; none of them are enforced **(highest-value finding)**
README.md:51-60 and docs/SPEC.md:52-60 carry an identical table:

> `@external` | Callable by other programs and outside requests (API, user)
> `@system` | Only callable by the VM runtime (lifecycle hooks)
> `@once` | Can only be called a single time (constructor-like)
> `@restricted(programs)` | Only callable by the listed programs
> `@ratelimit(n, period)` | Max n calls per period
> `@cron(interval)` | Triggered on a schedule by the VM

**[read]** Every one of these parses into `FunctionSig.permissions:
Vec<Permission>` (`src/ast.rs:184-193`, `src/parser.rs:745-786`) and that
field is never read again anywhere in `src/compiler.rs` or
`src/vm/engine.rs` — no access control, no call-count guard, no scheduler,
no rate limiter exists in the VM. Exhaustive grep for `Permission::External`,
`Permission::System`, `Permission::Once`, `Permission::Cron`,
`Permission::Restricted`, `Permission::Ratelimit` outside `parser.rs` returns
nothing. Every example in this repo writes `@system @once` on its
constructor as pure convention — calling `constructor` twice (nothing stops
you) runs it twice, in full, with no guard.

### A2. Most `Modifier`s are advertised implicitly by naming convention; only two half-matter
`Public`/`Private`/`Sequential`/`Parallel`/`Joined`/`Singleton`/`Mutable`/
`Immutable`/`Async` (`src/ast.rs:188-206`) all parse and are never consulted
again in `compiler.rs`/`engine.rs`. `pure` (used on every example's
`verify`) has zero compiled effect — no purity checking exists. `private`
enforces no call restriction: a "private" helper compiles to a normal
callable function, reachable by `CALL` like any other. **[read]**

### A3. `docs/ASK_SUSPEND.md`'s own status table is stale about QUERY
That doc's own "Where this sits" table currently reads:

> link entity | `QUERY` | **trace-only** — no retrieval

**[read]** `src/vm/engine.rs:575-616` — QUERY now does an exact-key lookup
against `VM::knowledge` and returns provenance-carrying chunks; the code
comment there literally narrates the same history the doc's table is
frozen at ("Was: ... Now: an EXACT lookup"). **[ran]**:
`examples/ground_min.cube` run with `--knowledge examples/facts_min.jsonl`
performs real retrieval today. The doc needs its own table row updated —
the mechanism moved out from under a doc that otherwise (the ASK/Suspend
material itself) is accurate and current.

### A4. `cubelang check`'s own usage string contradicts its own implementation
`src/main.rs:59` (`print_usage`): `"check, k   Parse and check .cube without
compiling"`. **[read]** `src/main.rs:325-349` (`cmd_check`) calls
`compiler::compile_ast_strict` — `check` fully compiles, in strict mode.
**[ran]**: `check examples/gsm8k.cube` fails with the exact 3 strict-mode
diagnostics `compile --strict` gives (see §C); a true parse-only check would
have no reason to reject those constructs. This is a one-line doc bug
sitting inside the binary's own `--help` output, not just the external docs.

### A5. README's GSM8K sample has drifted from the shipped example
README.md:1853: `program GSM8K implements ISolver, IDeployable {`.
**[read]** The real `examples/gsm8k.cube:26` reads
`program GSM8K implements ISolverLearn {` — no `IDeployable` at all. This is
a real post-migration artifact: `src/compiler.rs:2169-2172`'s own comment
says the Task 7 migration moved `gsm8k.cube` from an inline `ISolver`-shaped
interface to the registry's `ISolverLearn`; README's copy was never updated
to match. `IDeployable` is also not one of the 4 registry interfaces
(REFERENCE.md §2) — it only compiles at all if the same file also carries an
inline `interface IDeployable { ... }` block, which README's combined
example does provide elsewhere but the GSM8K snippet at line 1853 does not
show inline, making the snippet non-self-contained as printed.

### A6. Declaring your own `interface X { ... }` is taught as *the* way to declare interfaces; for the 4 built-in names it is dead code
README.md:1890-2010 presents an "interface catalogue" — `interface
IDeployable`, `interface ISolver`, `interface IAgent`, etc. — as ordinary
`.cube` source a program author writes. **[read]** For `ISolve`, `ISolver`,
`ISolverLearn`, `IAgent` specifically, `Compiler::check_implements`
(`src/compiler.rs:824-943`) resolves against the **VM-internal Rust registry
first, unconditionally** (`src/vm/interfaces.rs`) — an in-file block with
one of those 4 names is parsed, stored in `inline_interfaces`, and then never
consulted for those names at all. README's own copies of `ISolver`/`IAgent`
happen to match the registry's shape exactly, so nothing breaks in practice
today — but the doc never states that these 4 names are hardcoded and
tamper-proof, or that a local redefinition of them would be silently
ignored rather than either honored or rejected. `ISolverLearn` — the
interface actually implemented by 2 of the repo's 11 examples — does not
appear in README's catalogue at all (see B9).

### A7. `match` is presented as branching control flow; every arm always runs
docs/SPEC.md uses `match (...)  { ... }` as if it selects one arm, in at
least 4 places (lines 1524, 1542, 1738, 2283) — e.g. a fusion-strategy
dispatch, a decision router. **[read]** `src/compiler.rs`'s
`strict_check_stmt` names the actual behavior directly: `"match (no arm
selection at runtime)"` (`src/compiler.rs:978`), and `src/validate.rs:39-40`
confirms: *"every arm body executes unconditionally."* **[ran]**:
`examples/gsm8k.cube`'s `match (op[0]) { "+" => add reg, op[1]; "-" => sub
reg, op[1]; ... }` (intended as an if/elif chain over an operator) instead
runs `add`, `sub`, `mul`, `div` on `reg` in sequence, every time, regardless
of `op[0]`'s actual value. **[test]** `src/compiler.rs:2345-2373` and
`src/validate.rs`'s own test suite pin this down directly. No doc anywhere
carries this caveat next to a `match` example.

> **Status 2026-09-14 — FIXED.** `match` now selects exactly one arm. See
> C3 below for the lowering, the string-comparison prerequisite, and what
> is still not supported. The docs cited above (SPEC.md's four `match`
> examples) are now *accurate* rather than aspirational; no caveat needs to
> be added to them. `src/validate.rs`'s "every arm body executes
> unconditionally" text is now itself the stale claim and should be
> updated.

### A8. The one mechanism CubbyLLM's actual emitter depends on is undocumented
Neither README.md nor docs/SPEC.md mentions `use vsa;` or `recover(` at all
(0 matches, grepped in full). **[read]** `cubbyllm/reasoning/programs.py`
(the sibling repo, consumer side) emits exactly this pattern today:
`use vsa; ... bind frame, ROLE, "val"; ... return recover(frame, ROLE);` —
it is the live, in-production instruction surface, and it is absent from
both language documents. See B2/B9.

---

## B. Real but undocumented

### B1. The VM-internal interface registry itself
`src/vm/interfaces.rs` — the whole tamper-proof, registry-first resolution
mechanism (REFERENCE.md §2) has no counterpart in README/SPEC prose at all;
the docs describe interfaces purely as ordinary user-declared `.cube`
constructs. This is the single most important fact for the "what should the
emitter be trained to write" question: **the model should never be trained
to *declare* `ISolve`/`ISolver`/`ISolverLearn`/`IAgent` itself — only to
`implements` one of the 4 fixed names.**

### B2. `use <module>;` / `ModuleRegistry` / `vsa.recover` / sealed vs. overridable
`src/vm/registry.rs` in full — deny-by-default module system, the
postfix-`override`/sealed-collision compile-time checks, and the one
production-relevant registry function, `recover(reg, role)`. None of this
(module registry, `use` statement semantics, `recover`, sealed capabilities)
appears in README.md or docs/SPEC.md.

### B3. The `asm { MNEMONIC operand, ...; }` escape hatch
`src/ast.rs` `AsmBlock`/`AsmInstr`/`AsmOperand`, Task 6 — the only surface
path to opcodes with no dedicated statement (`UNBIND` chief among them).
Operand tagging rule (ALL-CAPS bareword → role symbol, quoted string →
global/literal, other identifier → register, numeric literal → immediate)
is a real, specific convention with no documentation outside the source
comment.

### B4. RETURN / CALL / array-opcode design (and its known limits)
`src/compiler.rs:53-61` comments plus `CUBELANG_OPCODES_PLAN.md` — RETURN
(0x39), CALL (0x13, v1 = register snapshot/restore, no real frames, capped
`call_depth`, documented as unsuitable for CALL inside a tight loop),
MAKE_ARRAY/LEN/INDEX (0x3A-0x3C) backing real `for`. None of this is in
README/SPEC; a reader of those docs would not know CALL exists at all, let
alone that it clones the entire register map per invocation.

### B5. The exact `--strict` blocklist
No doc anywhere enumerates, in prose, everything `--strict`/`compile_strict`
rejects. It exists only as code (`is_trace_only_ext_op`, `strict_check_stmt`)
and as this document's §C below. `cubelang_client.py` (the actual consumer)
gestures at it ("trace-only ext ops, `match`, ...") but does not enumerate
it either — which is exactly the gap this document's §C closes.

> **Status 2026-09-14.** Two entries LEFT the blocklist because they now
> execute: **`match`** (real arm selection, §C3 — `cubelang check` no longer
> reports it either) and **`assign` to an index target** (SET_INDEX, §C4).
> `cubelang_client.py`'s "trace-only ext ops, `match`, ..." gesture in the
> sibling CubbyLLM repo was stale as a result and has been corrected there.
>
> Three entries joined the always-on error set — these fail in EVERY mode,
> not just `--strict`, because a mode flag is not an acceptable gate for a
> statement that silently vanishes: §C11 operand-encoding errors, `asm`
> unknown mnemonics, and (new with §C4) un-encodable assignment targets.

### B6. `run-proto` is unconditionally strict; `run`/`compile` are opt-in
Asymmetry confirmed at `src/main.rs`'s `run_request_result` (always calls
`compiler::compile_strict`, no flag) vs. `cmd_run`/`cmd_compile` (loose
unless `--strict` is passed). Nowhere stated in README/SPEC. A program that
passes loose `cubelang run` can still be rejected outright over the
`run-proto` transport with no code change on the program's part — only the
transport changed.

### B7. `check`'s real behavior (see A4) is itself a real, useful, undocumented feature
Once you know `check` is a full strict compile, it is a legitimate
lightweight preflight — just not the "syntax only" tool its own `--help`
text describes it as.

### B8. The `TypeExpr` vs. `Value` split (REFERENCE.md §4) and the complete absence of a type-checking pass
Neither doc states that a declared type is unchecked decoration, or that
`number`/`quantity`/`symbol` (used throughout the shipped examples) are not
built-in types at all — they are indistinguishable, at the lexer level, from
a typo.

### B9. `ISolverLearn` is unnamed in either doc
Used by 2 of 11 examples (`gsm8k.cube`, `conversation_agent.cube`); absent
from README's interface catalogue and from SPEC.md (grepped, 0 matches for
`ISolverLearn` in either file).

### B10. Constructor auto-run semantics
`call_with_constructor`'s "best-effort, errors silently discarded, no
once-guard, re-runs every invocation" behavior (REFERENCE.md §5) is not
described anywhere in prose.

### B11a. `recover()` on an unbound role in an already-bound frame is noise, not Null

> **Status 2026-09-14 — the candidate pool is FIXED (WO-1.2); the not-Null part is NOT.**
>
> The cleanup now runs against **the frame's own bound fillers**
> (`VM.vsa_frames`, register-keyed, taken/restored with `registers` in
> `exec_intra_call` so a callee cannot leak fillers into a caller's frame).
> `op::UNBIND` was assembling its own `name_table.values()` pool inline and
> now routes through the same `vsa_unbind_cleanup_known`, so the policy has
> one definition rather than two that could drift apart.
>
> Measured (a frame holding SUBJECT/OBJECT/VERB, a second frame's fillers also
> in the name table — `tests/recover_scope.rs`):
>
> | | before | after |
> |---|---|---|
> | genuine `SUBJECT` | `"cat"` @ 0.27002 | `"cat"` @ 0.27002 |
> | absent `LOCATION` | `"umbrella"` @ 0.02148 | `"cat"` @ 0.01221 |
>
> `"umbrella"` belonged to a **different frame** — the confabulation
> mechanism, observed rather than argued. A genuine recovery is unchanged to
> the bit (same unbind, same true filler, and the true filler is always in its
> own frame's pool), so the sibling repo's τ_vm calibration
> (1.0 / 0.4736328125 / 0.22021484375) needed no adjustment; measured through
> that harness over 150 accepted bindings, the accepted distribution was
> identical and accepted-vs-control separation improved **10.3× → 14.3×**.
>
> **A register holding a hypervector with no recorded bindings still falls
> back to the global pool** — hand-assembled bytecode reaches UNBIND without
> going through `vsa_bind_into`, and scoping it to an empty pool would
> silently turn a working recovery into `None`.
>
> **Still true, and still the open question:** an absent role does not return
> `Null`. It now returns a low-similarity symbol from the *right structure*
> instead of from anywhere, which is strictly better but is not Null. A caller
> must still read the `similarity` side-channel. Whether the VM should ever
> return Null here is a separate decision.

`recover(reg, role)` performs UNBIND + cosine cleanup against **every symbol
the VM currently knows**, not just symbols used in `reg`'s own frame
[src, `src/vm/registry.rs:165-186` doc]. **[ran]**: a frame with `SUBJECT`
bound but not `OBJECT`, then `recover(frame, OBJECT)`, does **not** return
`Null` — it returns whatever symbol is globally nearest (in
`examples/vsa_bind_recover.cube`'s case, an unrelated storage-field name
picked up as noise) at a **low similarity** (~0.03, vs. ~0.5 for a genuine
recovery). Only a frame that was never bound *at all* reliably recovers
`Null` (`tests/asm_vsa.rs`'s `b3_an_unbound_frame_is_the_control...`). A
caller must inspect the `similarity` side-channel (`run --json`'s top-level
field, `RunResult.similarity` over the wire) to tell a real recovery from
noise — nothing forces this check, and neither doc mentions it.

### B11. Storage fields lose everything except their name at compile time
`CompiledProgram::storage_fields: Vec<String>` — type, default value, and
mutability are all dropped by the time bytecode exists (REFERENCE.md §5).
A program's `storage { threshold: mutable u64 = 0; }` does **not** actually
initialize `threshold` to 0 via the storage declaration; every example
re-assigns it by hand inside `constructor()` because the declaration alone
does nothing at runtime. Undocumented.

---

## C. Non-executing constructs — the full list (per the task: "must be named")

Verified by reading `compiler::is_trace_only_ext_op` (`src/compiler.rs:
1869-1880`), `compiler::Compiler::strict_check_stmt` (`src/compiler.rs:
960-1063`), and `validate::walk_stmt` (`src/validate.rs:150-180`), then
confirmed **[ran]** against `examples/gsm8k.cube` and
`examples/conversation_agent/conversation_agent.cube`, the two examples
complex enough to exercise most of this list.

### C1. Extended opcodes that compile but only emit a trace line (no value-level effect)
`infer`, `map_roles`, `filter`, `score`, `detect_pattern`, `decode`,
`reduce`, `merge`, `split`, `debate`, `predict`, `discover`, `diff`, `seq`,
`specialize`, `reward`, `match` *(the ExtOp opcode form)*, `temporal_bind`,
`analogy`, `gen`, `inst`, `broadcast`, `explore`, `forge`, `sync` — 24
mnemonics, all lexed as real keywords (`Op*` in `src/token.rs`), all parse
into real `Stmt::Opcode(OpcodeStmt::Extended{..})` nodes, all compile
cleanly with **no diagnostic in loose mode**, all execute as
`self.trace_structural(...)` in the VM — a trace-log line and nothing else.
`--strict`/`validate` are the only ways to learn this without reading
source. (`ask` — 0x1D — left this list on 2026-08-30 and now genuinely
executes; do not confuse the two.)

### C2. `unify a, b`
Base opcode (0x35, not part of `ExtOp`), same "trace line, no computed
result" behavior. Flagged separately from C1 in both `strict_check_stmt`
and `validate.rs` because it is not an `ExtOp` variant.

### C3. `match (expr) { arms }` (the statement, not the opcode)
Parses and compiles; **every arm's body executes unconditionally, in
source order** — there is no runtime pattern comparison at all. See A7 for
the confirmed real-world consequence in `gsm8k.cube`. This is arguably the
single most dangerous entry on this list: it looks exactly like working
branching logic and produces a plausible-looking (wrong) result rather than
an error.

#### Status 2026-09-14 — FIXED, and it needed a VM fix first
`compile_match` now lowers a match to the same COMPARE/COND primitive
`compile_if` already used — an if/else-if chain, which is all a match over
scalar patterns is:

```text
    COMPARE subject, p1, kind=0(eq)
    COND  -> _arm_2          ; COND jumps when the flag is FALSE
    <b1>
    JMP   -> _endmatch
  LABEL _arm_2
    COMPARE subject, p2, kind=0
    COND  -> _arm_3
    <b2>
    JMP   -> _endmatch
  LABEL _arm_3
    <bN>                     ; `_` wildcard: no test, always taken
  LABEL _endmatch
```

`_` is the default arm; arms written after it are unreachable and are not
emitted. `match` has been removed from the `--strict` blocklist (see B5),
and strict still descends into arm bodies via the ordinary recursion.
Pinned by `tests/match_arms.rs` (9 tests) and
`compiler::tests::strict_accepts_match_now_that_arm_selection_is_real`.

**The prerequisite, which is the more important finding.** This could not
be fixed in the compiler alone. `op::COMPARE` resolved both operands with
`resolve_i64`, and `Value::as_i64` returns 0 for every `Value::Str` — so
**every string comparison in CubeLang was unconditionally TRUE**, silently.
`if (intent == "question")` took the then-branch for any value of `intent`
whatsoever. Building arm selection on top of that would have traded the
run-every-arm bug for a first-arm-always-wins bug: strictly worse, because
it would look correct. `src/vm/engine.rs`'s COMPARE now compares two
strings as strings (eq/ne and lexicographic ordering); mixed Str/Int
comparisons keep the numeric path unchanged. This also un-ignored
`tests/str_semantics.rs`'s
`string_equality_against_a_parameter_discriminates`, and revealed that the
"control" test beside it had been passing for the wrong reason — it, too,
compared 0 to 0. A negative case was added.

**Still not supported, on purpose:** the subject and every pattern must be
operand-encodable (C11). `match (op[0])` is a compile error, not silent
garbage — write `index o, op, 0; match (o) { ... }`. Patterns are compared
by equality only; there is no destructuring, no range or guard pattern, and
no binding pattern. `_` is the only wildcard spelling.

**Still open, adjacent:** COMPARE kind 6 (truthiness) still tests
`as_i64() != 0`, so a non-empty string is still falsy. Left alone
deliberately — different blast radius, and the test that would cover it
(`str_contains_evaluates_on_a_register_string`) is blocked on `str.contains`
being a stub, not on this.

### C4. `assign` to anything other than `self.field` or a bare identifier
`arr[0] = x`, `regs[reg] = eval(reg)`, `self.patterns[key] = handler`,
`self.memory_index["k"] = v` — **compile to zero bytecode**, silently, in
loose mode. **[ran]**: this is the exact mechanism that makes
`examples/gsm8k.cube` return `0` (see "Bugs found" below) and is the most
common single violation across the two complex examples (4 separate
occurrences across `gsm8k.cube` + `conversation_agent.cube`).

#### Status 2026-09-14 — FIXED (and a second bug was hiding in the same arm)
`compile_stmt`'s Assign arm was an `if let SelfAccess / else if let Ident`
chain **with no else**. Anything that matched neither fell off the end and
emitted nothing at all. It is now `compile_assign`, and:

- **`container[key] = value` works**, via a new opcode `SET_INDEX` (0x3D),
  operands `[container, key, value]`. It handles `Value::Array` (integer
  key) and `Value::Map` (string key), for both a local register and a
  storage field (`self.patterns[k] = h`). Added to `opcode-vsa-rs` in
  lockstep, as `tests/opcode_sync.rs` requires.
- **Reading a map works too.** `op::INDEX` handled only `Value::Array`, so
  `m["k"]` returned Null even for a present key. The read and write gaps hid
  each other: you could not write a map entry, and could not have read one
  back if you had. `compile_let`'s Index arm also only accepted a bare
  identifier as the container, so `let v = self.field[k];` failed while the
  matching write succeeded — now symmetric.
- **`create m : map<K,V>;` starts as an empty container** rather than Null,
  so no separate initialisation step is needed. Every other declared type
  still starts Null, so nothing numeric changes.
- Out-of-range, negative, and wrong-container-type index writes are **VM
  errors**, not silent clamps. Replacing a silent drop with a silent clamp
  would have preserved the bug class. Assigning at exactly `len` appends.

**The second bug, found while fixing this one: compound assignment was
compiled as plain assignment.** `AssignStmt::op` was parsed into the AST and
then never read by the compiler, so `total += 5` emitted `ASSIGN total, 5`.
Verified against the shipped binary: `assign total = 10; total += 5;`
returned **5**, not 15, with no error in any mode. `+=`/`-=`/`*=`/`/=` now
emit ADD/SUB/MUL/DIV, which already do a read-modify-write.

**Still un-encodable, now a hard error in every mode rather than a silent
drop** (same rationale as C11): a nested index (`grid[0][1] = v`), a nested
field (`a.b.c = v`), a call result as a target, and compound assignment to
an index target (`m[k] += 1` — it needs a temporary this compiler cannot
allocate; read the element into a register, apply the operator, assign
back). `match` and `assign` have both left the `--strict` blocklist as a
result — see B5.

Pinned by `tests/index_assign.rs` (16 tests) and
`compiler::tests::an_index_assign_target_compiles_and_a_nested_one_is_a_hard_error`.

### C5. A bare expression-statement that is not a `Call`/`MethodCall`
An unrecognized leading identifier used as a statement (`frobnicate d;` in
the original P0-1 bug report that motivated `--strict`) compiles to a single
`SKIP` and produces **no compile error, ever, in loose mode** — this is the
literal example the strict-mode safety check's own doc comment in
`src/compiler.rs` uses to justify its own existence.

### C6. `assert condition;` where `condition` is not itself a `Call`/`MethodCall`
`compile_expr_discard` only emits a real check for a call-shaped condition.
`assert x > 0;`, `assert output.text == "";`, `assert some_flag;` — i.e.
almost every assertion a person would naturally write — silently compile to
a single `SKIP`. **No example in this repo uses `assert` at all**, so this
gap has not yet bitten a shipped program, but it is real and untested by
anything other than the compiler's own unit tests.

### C7. `throw expr;`
Never compiled, in any mode, strict or loose — falls through
`compile_stmt`'s catch-all to a no-op. `--strict` at least flags it
(`src/compiler.rs:1063-1066`); loose mode does not.

### C8. `try { } catch { } finally { }`'s `finally` block
`compile_stmt`'s `TryCatch` arm never compiles `finally_body` at all, in any
mode — and this gap is **not caught even by `--strict`**, per the source's
own comment (`src/compiler.rs:1005-1009`: *"a separate, pre-existing gap...
no per-statement bytecode check can catch it here"*). This is the one
construct on this list that survives strict-mode verification entirely.

### C9. The raw `LOOP` opcode keyword (`loop`, distinct from `for`)
Only reachable via `asm { LOOP ...; }`; always `trace_structural`
("structural marker"), never executes iteration itself. Not flagged by
`--strict` (only reachable through `asm`, and unrecognized-mnemonic checking
is separate from execution-semantics checking) — worth naming explicitly
since it is easy to assume "loop" and "for" are the same feature.

### C10. Entire top-level forms that never reach the compiler at all
Distinct from "compiles to a no-op": `container`/`world`/`modal`/`robot`
declarations (and everything inside them — `config`, `io`, `programs`,
`agents`, `permissions`, `world`, `modalities`, `sensors`, `actuators`,
`safety`, `fusion` blocks), top-level `event`, `extend`, and top-level
`struct`/`enum`/`type` are **never visited by `Compiler::compile`**
(`src/compiler.rs:672`: `if let TopLevel::Program(p) = item`) — not flagged,
not erred, not warned, in any mode including `--strict`. **[ran]**: a
container-only file with zero `program` blocks passes `cubelang check`
(reports `"ok (... 0 programs ...)"` because there is nothing to iterate
over) and then fails `cubelang run` with `"error: no programs in <file>"`.
A large fraction of README.md's own examples (every `container`/`world`/
`robot` example, `IAgent` container walkthrough, `ArmBot`) are therefore
demonstrating syntax that **cannot be run at all** by the shipped VM — not
"runs with reduced semantics," but zero bytecode, period.

### C11. A non-trivial expression used as a call argument (or general RHS) silently becomes garbage — and `--strict` does NOT catch it
`compiler::Compiler::compile_expr_operand` (`src/compiler.rs:1648-1670`) has
a real encoding only for `IntLit`/`FloatLit`/`StringLit`/`BoolLit`/`Null`/
`Ident`/`SelfAccess`/simple `Field`. Its catch-all (`_ => { emit OP_NONE
placeholder }`) silently swallows everything else — critically,
`Expr::BinOp` (ordinary arithmetic like `n - 1`), `UnaryOp`, `StructLit`,
`MapLit`, `Lambda`, `Await`, `Deploy`, `Proxy`, and a nested `Call`/
`MethodCall` used as an argument. This path is reached by call arguments
(`emit_call`'s `for a in args { self.compile_expr_operand(a); }`), array
literal elements, index expressions, and any `let`/`assign` RHS that isn't
one of Plan-2/3's specially-handled shapes (`Call`, `ArrayLit`,
`len()`, `Index`).

**[ran]**, minimal probe (not shipped as an example — a pure bug
demonstration, kept out of `examples/` to avoid adding another confusing
"looks right, isn't" fixture):
```
function double_it(n: number): number { create r:number; assign r=n; add r,n; return r; }
function solve(input: number): number {
    create x:number; assign x=5; create y:number; assign y=3;
    let z = double_it(x - y);   # intended: double_it(2) == 4
    return z;
}
```
returns `0`, not `4` — `x - y` resolves to nothing usable, not to 2.
**Confirmed NOT caught by `--strict`**: `cubelang check` on this exact
source reports `"ok"` — `strict_check_stmt` inspects statement *shapes*
(is this a `Call`/`MethodCall`, is this target `self.field`, ...) but never
descends into an individual expression operand to ask whether
`compile_expr_operand` can actually encode it. This is the one entry on
this list that a model emitting `--strict`-clean CubeLang could still hit
silently: **never pass anything but a literal, a bare identifier, or
`self.field` as a call argument or a `let`/`assign` right-hand side** — do
all arithmetic via `add`/`sub`/`mul`/`div` statements on a register first.

#### Status 2026-09-14 — FIXED, and the fix is load-bearing on the language surface
The `_ => { emit OP_NONE placeholder }` arm is gone. Every un-encodable
expression form now pushes onto `operand_errors` and fails the compile **in
every mode**, not just `--strict` — a mode flag was not an acceptable gate
for the one silent-garbage path `--strict` structurally could not catch.
The diagnostic names the form and the `Program::function` it occurred in.

The structural fact behind C11, which the entry above stops short of
stating: **there is no expression-lowering pass in this compiler, and no
temporary-register allocator.** `compile_expr_operand` encodes *atoms* and
nothing else; `compile_let`'s Plan-2/3 arms (`Call`, `ArrayLit`, `len()`,
`Index`) are hand-written special cases, not a general lowering path. So
this is not a hole to be patched one form at a time — it is the shape of
the language: **CubeLang's real surface is a three-address register
machine.** Computation is a statement (`sub d, a, b`), never a nested
expression (`let d = a - b;`).

That is the correct surface to keep, not a limitation to engineer around:
1. It is the EVM analogy taken seriously — EVM bytecode has no nested
   expressions either, and a contract is a flat instruction sequence.
2. A flat, fixed-arity instruction grammar is far easier to constrain with
   GBNF and far easier for a small emitter to generalize over than a
   recursive expression grammar. Nested expressions would hand the emitter
   an unbounded recursive surface to learn for zero expressive gain.

**Consequence for anyone writing `.cube` by hand or generating it:** the
high-level expression syntax the parser accepts (`BinOp`, `UnaryOp`,
`MapLit`, `StructLit`, `Lambda` in operand position) parses but has never
compiled. It is now an error rather than silent garbage. Write the
three-address form.

---

## Bugs found (reported, not fixed — per task rules)

1. **`examples/gsm8k.cube` silently returns a meaningless answer.**
   `cubelang run examples/gsm8k.cube --json` (loose mode, no error) returns
   `{"ok":true,"result":0}`. The actual arithmetic
   (`regs[reg] = eval(reg)`, a map-index assign — C4 above) and the operator
   dispatch (`match (op[0]) { "+" => add ...; }` — C3 above) both silently
   no-op, so `0` is the VM's untouched default, not GSM8K's computed answer.
   Nothing on the loose path signals this. **[ran]**

2. **`examples/conversation_agent/conversation_agent.cube` does not run at
   all**, even in loose mode: `cubelang run ... --json` →
   `{"ok":false,"error":"CALL: unresolved function name"}`. **[ran]**. Cross-
   checked: the only harness that ever exercises this directory
   (`examples/run_conversation_min.rs`) runs the much simpler sibling file
   `conversation_min.cube`, never `conversation_agent.cube` — so this
   file's runtime failure has apparently never been caught by anything in
   the repo's own build or test suite.

> **Status 2026-09-14 for (1) and (2).** Both now fail at COMPILE time with
> the C11 operand diagnostic instead of producing a meaningless answer or a
> late runtime error — the honest outcome, and the one the two tests in
> `tests/interfaces.rs` now pin (`gsm8k_cube_example_fails_on_the_c11_operand_bug`,
> `conversation_agent_cube_example_fails_on_the_c11_operand_bug`; the
> matching CLI-level pins live in `tests/examples_pinned.rs`). Those tests
> previously asserted "must still compile" — an assertion that was only ever
> true because the compiler was silently discarding the statements it could
> not encode.
>
> **What each file still needs**, verified by running the current binary
> against both (2026-09-14, after the C3 fix):
>
> - `match` is no longer reported for either file. C3 is done.
> - Every remaining error on both files is a C11 operand-encoding error:
>   `MapLit` and `StructLit` in constructors and returns, `BinOp` as a `let`
>   RHS, `MethodCall` and `Index` as operands. Those are not compiler gaps
>   to fill — see C11's status note on why the three-address surface is the
>   language, not a limitation. They are rewrites.
> - `gsm8k.cube` additionally needs C4 (map-index assign, `regs[reg] = ...`
>   — still a silent no-op), which IS a real gap and has to be implemented
>   before that file can work at all.
>
> So the ordering is: **implement C4 map-index assign, rewrite both files in
> three-address form, then flip their pins to "compiles and produces the
> right answer."** `conversation_agent.cube` needs only the rewrite.
> `examples/match_router.cube` is the small, working demonstration of the
> C3 fix in the meantime.

3. **`examples/parse_gsm8k.rs` only parses**, never compiles or runs,
   `gsm8k.cube` (`cubelang::parser::parse` only). Given (1), the file's
   "advanced" struct/map/loop machinery has never actually been exercised
   end-to-end by anything in this repo.

4. **`examples/qc_decision.cube` requires host-injected registers
   (`in_class`, `in_conf`) that plain `cubelang run` never provides.** Its
   own header comment documents this contract, but running it the obvious
   way (`cubelang run examples/qc_decision.cube --json`, no `--arg`) silently
   compares an unset register (resolves to 0) against the 70 threshold and
   always returns `2` (REVIEW) — reproduced. Not a VM bug, but a footgun:
   nothing in the CLI surfaces "this program expects registers you didn't
   set." Addressed in the new `examples/README.md` with an explicit note
   rather than a code fix.

5. **`compile_strict`'s own doc comment is stale.** `src/compiler.rs:1961`
   (doc above `compile_strict`) still lists "intra-program `call`" among the
   constructs strict mode rejects; `src/validate.rs:16-17` and
   `src/compiler.rs:998` both correctly note CALL now executes for real
   (Plan-2) and is no longer flagged. A source-internal contradiction
   between two comments in the same file, left as-is per the task's "do not
   change compiler/VM" rule (a doc comment inside compiled source is still
   source).

---

## What was not verified

- The 3-repo opcode byte sync (`opcode-vsa-rs`, the Python VM) that
  `tests/opcode_sync.rs` and `CUBELANG_OPCODES_PLAN.md` reference — neither
  sibling repo was in scope for this audit.
- Full line-by-line reconciliation of every README.md/SPEC.md code sample
  against the grammar (~2700 lines of README grammar/EBNF alone) — this
  audit targeted the highest-value, most consequential gaps (interfaces,
  attributes, strict-mode surface, storage/constructor semantics) per the
  task's stated priorities, not exhaustive prose diffing.
- `src/vm/hypervec.rs`/`index.rs`'s numerical properties (quasi-
  orthogonality claims, actual collision rates) were read but not
  independently measured.
