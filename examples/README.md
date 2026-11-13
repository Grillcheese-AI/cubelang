# Examples

Every program below was run under the built `cubelang.exe` (referred to
as `$CUBELANG_EXE`) as part of the `docs/DRIFT.md` audit; the "Expect"
column is the actual observed output, not an aspiration. See each file's own
header comment for the full story, and `docs/REFERENCE.md`/`docs/DRIFT.md`
for the mechanisms these programs exercise.

Run everything at once (PowerShell) from the repo root:

```powershell
Get-ChildItem examples\*.cube, examples\conversation_agent\*.cube | ForEach-Object {
    Write-Host "=== $($_.FullName) ==="
    & .\cubelang.exe run $_.FullName --json
}
```

or (bash, on a build where `cubelang` has no `.exe` suffix):

```bash
for f in examples/*.cube examples/conversation_agent/*.cube; do
    echo "=== $f ==="
    $CUBELANG_EXE run "$f" --json
done
```

The pinned-output integration test is `tests/examples_pinned.rs`
(`cargo test --test examples_pinned`) — it runs every file below and
asserts the exact result shown here, so a regression here fails CI instead
of rotting silently.

| Example | Demonstrates | Command | Expect |
|---|---|---|---|
| `ask_min.cube` | `ISolve`, ASK/Suspend | `run examples/ask_min.cube --json` | `{"suspended":true,"question":"which moon landing did you mean","candidates":[1959,1969],...}` |
| `compare_min.cube` | `ISolver`, if/else on COMPARE | `run examples/compare_min.cube --json` | `{"result":58,...}` |
| `decision_min.cube` | `ISolver`, storage+constructor, store/remember | `run examples/decision_min.cube --json` | `{"result":80,...}` |
| `fibonacci.cube` | `ISolve`, real `while` (Turing-complete iteration) | `run examples/fibonacci.cube --json` | `{"result":55,...}` |
| `ground_min.cube` | `ISolve`, real QUERY (not trace-only — see DRIFT §A3), LEN, ASK | `run examples/ground_min.cube --json` | `{"result":0,...}` (no `--knowledge`: QUERY finds nothing → gap) |
| `gsm8k.cube` | **Aspirational / no longer compiles at all** — struct/map/typed surface; a `BinOp`/`StructLit`/`MapLit`/`MethodCall`/`Index` used as an operand is now a hard compile error in every mode (`docs/DRIFT.md` §C11), on top of map-index assigns (see header, `docs/DRIFT.md` "Bugs found" #1) | `run examples/gsm8k.cube --json` | compile error (exit 1, nothing on stdout); stderr names `BinOp`/`MapLit`/... — see `tests/examples_pinned.rs` |
| `loop_min.cube` | `ISolver`, real `while`, store/remember | `run examples/loop_min.cube --json` | `{"result":42,...}` |
| `qc_decision.cube` | `ISolver`, the host-injects-registers pattern (see header WARNING) | `run examples/qc_decision.cube --json` | `{"ok":false,"error":"TypeMismatch: ..."}` (no registers injected on this plain invocation: a missing value never compares, P6-1) |
| `recall_min.cube` | `ISolver`, STORE overwrite + RECALL (cosine cleanup) | `run examples/recall_min.cube --json` | `{"result":65,...}` |
| `conversation_agent/conversation_agent.cube` | **Broken — no longer compiles at all** (previously failed only at runtime; now fails earlier and more honestly at compile time for the same C11 reason as `gsm8k.cube` — see header, `docs/DRIFT.md` "Bugs found" #2) | `run examples/conversation_agent/conversation_agent.cube --json` | compile error (exit 1, nothing on stdout); stderr names `BinOp`/... — see `tests/examples_pinned.rs` |
| `conversation_agent/conversation_min.cube` | `ISolver`, the working minimal conversational shape | `run examples/conversation_agent/conversation_min.cube --json` | `{"result":10,...}` |
| `vsa_bind_recover.cube` | `use vsa;`, real `bind`, `recover(reg, role)` — the actual CubbyLLM emitter surface (see `docs/DRIFT.md` §A8/§B2) | `run examples/vsa_bind_recover.cube --json` | `{"result":"cat","similarity":~0.52,...}` |
| `call_recursion_min.cube` | Real intra-program `CALL`/`RETURN`, recursion | `run examples/call_recursion_min.cube --json` | `{"result":120,...}` (5!) |
| `for_loop_min.cube` | Real `for (let x of arr)` (distinct from the dead `LOOP` opcode, `docs/DRIFT.md` §C9) | `run examples/for_loop_min.cube --json` | `{"result":60,...}` |
| `agent_min.cube` | `IAgent` (`think`/`act`/`observe`) — the one engine interface no prior example implemented | `run examples/agent_min.cube --json` | `{"result":1,...}` |
| `match_router.cube` | Real `match` arm selection over a **string** subject (`docs/DRIFT.md` §C3/§A7) | `run examples/match_router.cube --json` | `{"result":"compare",...}` |

## Notes

- FIXED 2026-09-14: `compile`/`run`/`check` are now **strict by default** —
  the `run ... --json` commands in the table above carry no `--strict` flag
  and get strict verification anyway; pass `--loose` to opt out (accepts
  trace-only opcodes etc. as silent no-ops again, same as the old
  default). Note `match` is no longer in that set: as of 2026-09-14 it
  selects exactly one arm and passes strict — see `match_router.cube` above
  and `docs/DRIFT.md` §C3. Every WORKING example in this table was already strict-clean
  before this flip, so none of their commands or expected output changed —
  only `gsm8k.cube` and `conversation_agent/conversation_agent.cube` (below)
  are affected, and they no longer compile even with `--loose` (see next
  note).
- `gsm8k.cube` and `conversation_agent/conversation_agent.cube` are kept
  intentionally, not deleted, and not rewritten to fix their logic: they are
  the only fixtures in the repo that exercise the parser's struct/map/typed
  surface (`gsm8k.cube` is parsed — never compiled — by
  `cargo run --example parse_gsm8k`). Deleting them would silently lose that
  coverage; instead both carry a prominent header explaining exactly what
  does and does not work, per `docs/DRIFT.md`. As of 2026-09-14 neither
  compiles in ANY mode, including `--loose`: both use a `BinOp`/`StructLit`/
  `MapLit`/`MethodCall`/`Index` expression as a call argument or `let`/
  `assign` RHS somewhere, which `compile_expr_operand` cannot encode
  (`docs/DRIFT.md` §C11) — a hard compile error unconditionally now, not
  just under `--strict`. This is the correct, honest outcome for two files
  that could previously only ever silently return a meaningless answer or
  fail confusingly at runtime.
- `qc_decision.cube` is only meaningful when a host injects `in_class`/
  `in_conf` registers before calling `solve` (see its header's HOST SHIM
  CONTRACT). Running it plainly, as the table above does, is a valid but
  different scenario (no registers set → TypeMismatch since P6-1; it used to read 0 and always answer REVIEW) — both are real,
  both are documented, neither is a bug.
- `.cubebin` files (`gsm8k.cubebin`, `qc_decision.cubebin`) are pre-compiled
  snapshots, not re-verified by this audit beyond `cubelang inspect`/
  `disasm` sanity (they decode; see `docs/REFERENCE.md` for what `inspect`/
  `disasm` show).

## Drafts (`examples/drafts/`)

Four design drafts, written to the full specification and kept as targets.
None compiles today, so they are not in the table above and the two "run
everything" loops do not reach them. `tests/examples_pinned.rs` pins where
each one stops now: when a feature lands and a draft gets further, its test
fails, and the row below changes in the same change. The open item behind
each stop is in `CUBELANG_FIXES.md` (P3).

| Draft | Where it stops today (exit 1, nothing on stdout) |
|---|---|
| `drafts/bridge_mesh.cube` | strict compile: `implements` two interfaces that are declared nowhere (2:1, P3-8); a method call used as an operand (21:9 and 22:9, P3-9); its `io` block (line 8, P3-1) |
| `drafts/mowm_manager.cube` | parse error at 27:5, "expected 'function' or 'constructor', got At": `@pure` written as an attribute (P3-6) |
| `drafts/mowm_spawner.cube` | strict compile: the same two interfaces (11:1, P3-8); method calls and compound expressions used as operands (P3-9); a field read whose base is not a variable (66:41) and `throw` (line 41), both P3-10 |
| `drafts/orchestrator.cube` | parse error at 39:11, "expected identifier, got Star": the wildcard in `@hook(*.error)` (P3-2) |
