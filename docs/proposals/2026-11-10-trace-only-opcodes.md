# Proposed semantics for the opcodes that have none yet (2026-11-10)

For cubelang `CUBELANG_FIXES.md` P5-6. Thirteen opcodes compile but compute nothing in the Rust VM: the twelve P5-6
names, plus **UNIFY**, which logs "UNIFY a b" and does nothing (it bypasses `trace_structural`, so it was missed in
P5's count). None has a working version in cubemind's Python VM either. The starting points are the one-line
comments in `opcode-vsa-rs/src/ir.rs` and the surface syntax in `docs/SPEC.md`.

**Approved by Nick on 2026-11-10, all thirteen as written** ("I approve"). Each opcode leaves strict mode's
blocklist only when it lands with its tests (P5-8).

**What it brings, in plain words:** every instruction Cubby's programs can use will either do exactly what its
paragraph says, or refuse to compile. None will look like it worked while doing nothing.
- **Over a conventional model:** there is no instruction set to check. Whatever the network does is the behaviour.

## Common rules
- **Vectors:** the Rust VM's vectors are MAP-bipolar ({-1, +1}, D = 4096 in the VM), not MoWM's block codes. Every
  operation below is defined on those.
- **Deterministic:** the same program, inputs and seed give the same result. Ties are broken by a fixed tie vector
  derived from the VM's seed, never at random.
- **Failures are codes** (P4-4): `TypeMismatch`, `EmptyInput`, `NotDivisible`, `AbsentRole`, `EmptyPool`,
  `BudgetExceeded`, `Unsupported`. An opcode never falls back to a default value.
- **Reported:** each run lists what each of these opcodes did (P4): inputs and outputs by hash, plus similarities
  and margins where a cleanup chose a symbol.
- **Not proof on their own:** a value produced by a vector opcode (MERGE, MAP_ROLES, ANALOGY, TEMPORAL_BIND) is a
  representation. It reaches a certified answer only through a later exact check.

## The opcodes

| Opcode | Surface syntax | Proposed meaning | Can feed a certified answer directly |
|---|---|---|---|
| MERGE | `merge a, b` | bundle two vectors into `a` | no |
| SPLIT | `split x, n` | cut a vector or an array into n parts | yes (arrays) |
| FILTER | `filter x, rel, v` | keep the array elements that pass one comparison | yes |
| REDUCE | `reduce x, op` | fold an array or registers with one operation | yes (numbers) |
| MAP_ROLES | `map_roles x, map` | rename the roles of a role-filler frame | no |
| TEMPORAL_BIND | `temporal_bind x, t` | bind a vector to an exact time | no |
| ANALOGY | `analogy a, b, c` | a : b :: c : ? | no |
| INFER | `infer x` | forward-chain the rules to a fixpoint | yes, with its derivations |
| UNIFY, INST, GEN | `unify a, b` … | Hindley–Milner type inference | yes (type checks) |
| BROADCAST | `emit Event {…}` | publish an event to the host | no (a side effect) |
| SYNC | `await joined arr` | wait for other runs' results | yes, if those runs were |

### MERGE a b
`a := bundle(a, b)`: add the two vectors element by element and take the sign. Where they differ (the sum is 0),
the fixed tie vector decides. Both must be vectors (`TypeMismatch` otherwise). Bundling more than two is REDUCE's
job, so the result does not depend on the order of pairwise merges. This is not a world merge: MoWM's
BindComposeMerge stays in MoWM.

### SPLIT x n
- On a vector of dimension D: n contiguous chunks of D / n, as an array (`NotDivisible` if n doesn't divide D).
- On an array: n contiguous slices, whose sizes differ by at most one, in order.
- `n = 0` is `EmptyInput`.

This is not a world split: that is the `mowm.split` host call (P5-2).

### FILTER x rel v
It keeps the elements of the array `x` for which `element rel v` holds, in order. `rel` is one of `==`, `!=`, `<`,
`<=`, `>`, `>=`, and `v` is a literal or a register. One comparison only: anything richer is written as a loop. On a
vector it fails with `TypeMismatch`, because filtering bipolar components has no meaning.

### REDUCE x op
A left fold, in the given order, over an array or a list of registers. `op` is `add`, `mul`, `min` or `max` for
numbers, and `bundle` or `bind` for vectors. Integer overflow fails with `BudgetExceeded`, never wrapping. An empty
input fails with `EmptyInput`.

### MAP_ROLES x map
`x` is a frame built from role ⊗ filler bindings, and `map` lists old role → new role pairs. For each pair, the
filler is recovered with the existing `recover`, so its reason, similarity, runner-up and margin are reported. The
frame is then rebuilt by bundling new role ⊗ filler. Roles not in the map are kept. If a recover's reason is not
RECOVERED, MAP_ROLES fails with `AbsentRole`. No threshold is applied inside the VM; the host judges the margins.

### TEMPORAL_BIND x t
`x := x ⊗ T(t)`. T is the codebook vector of the time's canonical string (an ISO date, or an integer step). The
binding is its own inverse: binding again with the same t gives `x` back. Times match exactly, and near times are
**not** similar. A fuzzy time encoding would be a separate opcode with its own calibration. The VM never reads the
wall clock; t comes from the program or the host.

### ANALOGY a b c
`d := c ⊗ (a ⊗ b)`, because in MAP-bipolar the relation r with b = a ⊗ r is a ⊗ b. Then d is cleaned up against
the codebook. The result is a symbol, with its similarity, runner-up and margin, like `recover`. An empty codebook
fails with `EmptyPool`.

### INFER x
Forward-chains the host's rule library over the facts in `x`, or the run's knowledge store, until nothing new is
derived or the step budget runs out (`BudgetExceeded`). Rules apply in a fixed order. Each derived fact is reported
as one derivation line (rule, conclusion, premises; P4-3), so a separate checker can re-check it (CubbyLLM 0055).
The rules come from a path the host passes, like `knowledge_path`. The VM never chooses its own rules.

### UNIFY a b, INST x, GEN x
Hindley–Milner type inference, over a new type-term value:
- **UNIFY** is Robinson unification with the occurs check. It returns the most general substitution, or fails with
  `TypeMismatch`.
- **INST** replaces a polytype's bound variables with fresh type variables, named from a per-run counter.
- **GEN** quantifies the type variables of τ that are not free in the environment.

**The first user:** type-checking program skeletons against the ontology's domains and ranges (CubbyLLM 0057's
addendum of 2026-11-09). If that check is done on the host instead, these three can wait.

### BROADCAST x (`emit`)
It publishes an event (its name, and the value of `x`) to the host through the host-call channel (P5-1). The host
routes it to its subscribers; the event is recorded in the run report. It changes no register. The `ir.rs` comment
("broadcast vector to all working registers") is dropped: copying one value into every register would silently
overwrite state.

### SYNC [handles] (`await joined`)
A host call that waits for other runs, named by their handles, and returns their results in handle order, each
with its report hash. Parallel parts are separate VM sessions driven by the host (CubbyLLM 0063). A program whose
parts are not certified gets no certification from SYNC. Until 0063 runs parallel sessions, strict mode rejects it.

## Tests (for each opcode, once approved)
- A fixture per row of the table: its success case, and each of its failure codes.
- Determinism: run twice, same report hash.
- The vector opcodes: the algebraic identities hold. Binding twice with the same vector is the identity; ANALOGY
  on a constructed a : b :: c : d returns d; MAP_ROLES with the identity map returns the frame's fillers unchanged.
