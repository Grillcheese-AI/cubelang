# CubeLang Reference

Extracted mechanically from source (not from README.md/SPEC.md, which are the
subject of `docs/DRIFT.md`). Every claim below cites a `file:line`. Binary
built as `cubelang.exe` at the repo root; invoked below as `$CUBELANG_EXE`.

Verification key: **[src]** = read directly from source, **[ran]** = observed
by executing `$CUBELANG_EXE`, **[test]** = covered by an existing test in
`tests/`.

## 1. Compiler pipeline, at a glance

`lexer::Lexer` (`src/lexer.rs`) → `parser::parse` (`src/parser.rs`, ~2384
lines) → `ast::SourceFile` (`src/ast.rs`) → `loader::load` (`src/loader.rs`,
resolves `import "path.cube";` into one merged AST, Task 5) →
`compiler::compile`/`compile_strict` (`src/compiler.rs`, ~2586 lines) →
`Vec<CompiledProgram>` (bytecode + symbol table) → `vm::engine::VM`
(`src/vm/engine.rs`, ~2530 lines) executes one function at a time via
`VM::call(program, function, args)`.

**Load-bearing fact for everything below** [src, `src/compiler.rs:672`]:
`Compiler::compile` only ever visits `TopLevel::Program(p)` items:

```rust
for item in &source.items {
    if let TopLevel::Program(p) = item {
```

Every other top-level form — `container`/`world`/`modal`/`robot` and every
block inside them (`config`, `io`, `storage` at container scope, `programs`,
`agents`, `permissions`, `world`, `modalities`, `sensors`, `actuators`,
`safety`, `fusion`), top-level `event`, `extend`, and top-level `struct`/
`enum`/`type` — is **parsed into the AST and then never visited again**. It
produces zero bytecode, unconditionally, in both loose and `--strict` mode.
See `docs/DRIFT.md` §C for the confirming run.

> **Superseded 2026-10-24.** Top-level `struct`/`enum`/`type` are read (§4), and a
> `container`/`world`/`modal`/`robot` compiles to a program of its own name:
> - its functions are the program's; the VM boots it (`VM::boot`): each bound program's
>   `constructor`, its own `constructor`, then `on_start`;
> - `config { k: v; }` is an immutable storage field `config`, a map of constants
>   (`self.config.k`); `storage { }` / `global storage { }` are its storage; `world { }` is a
>   storage field `world`, a map of the world's fields (`self.world.time`), each `immutable` one
>   written only by the constructor and `on_start`;
> - `programs { b: deploy P(); }` binds the programs it runs (`P`, `P.new()` also bind), called as
>   `self.programs.b.f(x)` or `b.f(x)`. A call into another program, `Other.f(x)` from any
>   program, is a CALL of `Other.f`: the VM admits it with the calling program as the caller
>   (`@internal`, `@restricted`, grants, `@ratelimit`) and runs it as `Other`;
> - `permissions { grant b.f to c; revoke b.f from c; }` is installed on load (`c` is a binding's
>   program, or any caller name, such as a registered key holder);
> - `safety { }` takes `max_jumps`, `max_ops`, `max_queries`, `max_wall_ms`, `max_call_depth` and
>   `deny: ["ask", ...]` (opcode names): each run entered through the container runs under them
>   (they tighten the VM's own and come off on return); a denied opcode is a compile error in
>   the code the container can reach and a run-time refusal in any code it reaches. Any other
>   safety key (a robot's `max_velocity`) is an error in every mode: a safety setting that is not
>   enforced is worse than none.
>
> What the VM cannot run (`io`, `agents`, `modalities`, `sensors`, `actuators`, `fusion`,
> `extends` another container, top-level `event` and `extend`) is a strict error. A file with a
> container runs its container (`cubelang run`, `run-proto`). Pinned by `tests/containers.rs`.

## 2. Engine-provided interfaces (`ISolve`/`ISolver`/`ISolverLearn`/`IAgent`)

Source of truth: `src/vm/interfaces.rs` (185 lines), `InterfaceRegistry`.
This is a **VM-internal, Rust-source contract table** — tamper-proof by
construction: nothing in `.cube` source can add to it, and an in-file
`interface X { ... }` block can never override a name it already knows.

| Interface | Required members (name: arity) | Seeded at |
|---|---|---|
| `ISolve` | `solve: 1` | `src/vm/interfaces.rs:79-83` |
| `ISolver` | `parse: 1`, `solve: 1`, `verify: 2` | `src/vm/interfaces.rs:84-90` |
| `ISolverLearn` | `parse: 1`, `solve: 1`, `verify: 2`, `learn: 3` | `src/vm/interfaces.rs:91-98` |
| `IAgent` | `think: 2`, `act: 2`, `observe: 2` | `src/vm/interfaces.rs:99-114` |

`ISolverLearn` = `ISolver`'s triad plus a **required** (not optional) `learn`
— added 2026-08-30 for `gsm8k.cube` and `conversation_agent.cube`, which each
used to carry their own inline interface with `learn` merely `optional`
[src, `src/vm/interfaces.rs:23-37`]. `IAgent` deliberately requires only the
three abstract members; the SPEC's `plan`/`reflect` are optional there and so
are absent from the registry entirely [src, `src/vm/interfaces.rs:99-107`,
confirmed by `tests/interfaces.rs`'s
`iagent_requires_think_act_observe_arity_2_and_nothing_optional`].

**Resolution order** — this is the one design decision that matters for an
emitter [src, `src/compiler.rs:824-943`, `Compiler::check_implements`]:

1. Registry first, unconditionally. A name the registry knows (`ISolve`,
   `ISolver`, `ISolverLearn`, `IAgent`) **always** resolves to the registry's
   contract, even if the same file also declares its own
   `interface ISolver { ... }` block with a different (weaker) shape — that
   block is silently ignored for interface-checking purposes.
2. Only for a name the registry does **not** recognize does an in-file
   `interface X { ... }` decl (gathered by `Compiler::compile` into
   `inline_interfaces` before any program compiles, `src/compiler.rs:663`)
   get consulted, and only its `abstract`, non-`optional` members become the
   required set [src, `src/compiler.rs:901-912`].
3. A name that resolves to neither is a compile error, checked
   unconditionally (not just under `--strict`) [src, `src/compiler.rs:914-923`].
4. `implements` with an **empty** list is *also* an unconditional compile
   error as of Task 7 — every `program` must declare at least one interface
   [src, `src/compiler.rs:883-892`].
5. Conformance check: for every required `(name, arity)`, the program body
   must contain a `ProgramItem::Function` or `ProgramItem::Constructor` with
   that exact name and parameter count [src, `src/compiler.rs:929-943`].

Interfaces are **implemented, not called** — they never go through the
`use`/`ModuleRegistry` call-resolution machinery at all [src,
`src/vm/interfaces.rs:62-67`]. There is no VM-level dispatch through an
interface handle; a program that `implements ISolve` is invoked the same way
any program is: `VM::call(program_name, "solve", args)` [src,
`src/vm/engine.rs:345`].

Model-emitter takeaway: the only 4 interface names an emitted program can
*meaningfully* `implements` today are `ISolve`, `ISolver`, `ISolverLearn`,
`IAgent`. Any other name (`IDeployable`, `IClassifier`, ... — all present in
README.md's catalogue, none in the registry) compiles only if the file *also*
supplies a matching in-file `interface` block; there is no built-in support
for it.

## 3. Opcodes

Authoritative table: `pub mod op { ... }`, `src/compiler.rs:22-89`. 41 named
byte constants; `SKIP = 0xFF` is the compiler's own "no real effect" filler.
`tests/opcode_sync.rs` is the cross-repo sync test mentioned in
`CUBELANG_OPCODES_PLAN.md` (checks these bytes agree with the sibling
`opcode-vsa-rs`/Python VM tables — out of scope for this audit to re-verify
against those other repos).

### 3.1 Core opcodes (bytes 0x00–0x15, 0x21, 0x34–0x3C)

| Mnemonic | Byte | Surface keyword | Executes? |
|---|---|---|---|
| CREATE | 0x00 | `create x : T` | yes |
| DESTROY | 0x01 | (no surface form found) | — |
| ASSIGN | 0x02 | `assign x = v` | yes |
| ADD | 0x03 | `add x, v` | yes |
| SUB | 0x04 | `sub x, v` | yes |
| MUL | 0x05 | `mul x, v` | yes |
| DIV | 0x06 | `div x, v` | yes (div-by-zero → `ExecResult::Error`) |
| TRANSFER | 0x07 | `transfer a, b, v` | yes [src, `src/vm/engine.rs:903`] |
| COPY | 0x08 | (register-register copy; internal) | yes |
| PUSH | 0x09 | `push x` | yes |
| POP | 0x0A | `pop x` | yes |
| COMPARE | 0x0B | `compare a, b` / condition operand | yes |
| QUERY | 0x0C | `query x` | **yes**, exact-key knowledge lookup (see §3.4) |
| STORE | 0x0D | `store x, "key"` | yes |
| RECALL | 0x0E | `recall "key"` / `recall x, "key"` | yes, cosine cleanup recall |
| BIND_ROLE | 0x0F | `bind_role x, ROLE` | yes, real VSA bind |
| UNBIND | 0x10 | **no surface statement** — only via `asm { UNBIND ...; }` | yes |
| COND | 0x11 | (compiler-generated, `if`/`while`/`for` scaffolding) | yes |
| LOOP | 0x12 | (compiler-generated for-loop marker; not the `for` statement itself) | **no** — `trace_structural` only [src, `src/vm/engine.rs:476` region: `"LOOP (structural marker)"`] |
| CALL | 0x13 | function call expression / `return f(...)` | yes (v1: register snapshot/restore, see §3.3) |
| JMP | 0x14 | (compiler-generated) | yes |
| LABEL | 0x15 | (compiler-generated) | yes (runtime no-op, resolved in pre-scan) |
| REMEMBER | 0x21 | `remember x` | yes (hippocampal store) |
| FORGET | 0x22 | `forget x` | yes (drops from memory + storage) |
| SUM | 0x34 | `sum x` | yes (copies register → accumulator) |
| UNIFY | 0x35 | `unify a, b` | **no** — trace-only [src, `src/compiler.rs:1868-1880`, `src/validate.rs:32`] |
| INST | 0x36 | `inst ...` | no — trace-only |
| GEN | 0x37 | `gen ...` | no — trace-only |
| NEWVAR | 0x38 | — | (not surfaced as a statement in `OpcodeStmt`) |
| RETURN | 0x39 | `return`, `return expr;` | yes (Plan-1, added after `CUBELANG_OPCODES_PLAN.md`) |
| MAKE_ARRAY | 0x3A | array literal `[a, b, c]` | yes (Plan-3) |
| LEN | 0x3B | `.len()` on an array-valued expr | yes |
| INDEX | 0x3C | `arr[i]` | yes |

`bind` (the statement form `OpcodeStmt::Bind`) and `bind_role` are both real
and both compile; there is no dedicated surface form for `unbind` — the only
way to reach `UNBIND` (0x10) from `.cube` source is the `asm { UNBIND ...; }`
escape hatch [src, `src/ast.rs:` `AsmBlock` doc, ~line 400]. In practice the
shipped recovery path is the `vsa` registry module's `recover(reg, role)`
(§5), which performs the real UNBIND+cleanup internally so a generated
program never needs `asm` at all [src, `src/vm/registry.rs:150-165`].

### 3.2 Extended ("reasoning") opcodes — most are trace-only

`ExtOp` (`src/ast.rs`, ~28 variants) covers bytes 0x16, 0x18–0x1E, 0x23–0x33.
`compiler::is_trace_only_ext_op` (`src/compiler.rs:1869-1880`) is the
**authoritative list** of which ones do NOT execute value-level semantics —
see `docs/DRIFT.md` §C for the full enumeration and how it was verified.
Only `ask` (0x1D) and `forget` (0x22, technically a core opcode byte, see
table above) execute among the "extended" surface keywords; everything else
in `is_trace_only_ext_op`'s list is `trace_structural` only
[src, `src/vm/engine.rs:957-1024`].

### 3.3 CALL — v1, not real call frames

[src, `CUBELANG_OPCODES_PLAN.md` §2, `src/vm/engine.rs:782-` region]
Registers, stack, storage, memory, and the accumulator are **VM-global** —
there are no call frames. `CALL` v1 is: snapshot `self.registers`, run the
callee with fresh `arg0..argN` bindings, restore the snapshot on return, and
deliver the result via `stack.push` (the same convention `POP` already
handles). This is documented as intentionally not scaling to hot inner loops
(full register-map clone per call) — v2 real call frames are still on the
roadmap, not built. `call_depth` is capped (see `ASK inside a CALL` below).

> **2026-10-24:** a callee's arguments now bind to its declared parameter names
> (before, only `arg0..argN`, so `helper(x)` read an unbound `x` and returned
> `Int(0)`), and an `ASK` inside a callee suspends with the waiting callers'
> frames (docs/ASK_SUSPEND.md). Registers are still copied per call; that
> performance note stands. Storage fields are no longer set aside with the
> caller's locals: a callee's `self.x` is the program's `x` (it read a fresh,
> empty scope). A chain hops to a fresh native stack every 8 levels, so the
> 64-level limit holds on any thread (a 64-deep chain had outgrown a 2 MiB
> test thread). `Other.f(x)` calls another program (§1).

### 3.4 QUERY is real, not trace-only — a documentation-vs-source gap in the repo's own docs

[src, `src/vm/engine.rs:575-616`, code comment: *"Was: read a register,
write a trace line, retrieve nothing. ... Now: an EXACT lookup against the
VM's knowledge store."*] `QUERY <mention>` looks the register's string value
(or its literal symbol name) up in `VM::knowledge` (`src/vm/knowledge.rs`, an
exact-key hash map with provenance, loaded via `--knowledge facts.jsonl` or
`RunRequest.knowledge_path`), and pushes an array of `{text, score: 1.0,
source}` maps onto the stack. A miss pushes an **empty array**, never an
error and never a nearest-neighbour guess. `docs/ASK_SUSPEND.md`'s own table
(current on disk) still lists QUERY as "trace-only — no retrieval"; that line
is now stale relative to the source it lives beside. See `docs/DRIFT.md` §A.

### 3.5 ASK / Suspend — the third `ExecResult`

[src, `src/vm/engine.rs:88-124` (`ExecResult`, `Suspension`), `981-1020`
(`op::ASK`), `251` (`VM::resume`)] Fully covered by `docs/ASK_SUSPEND.md`
(accurate, current) and `tests/ask_suspend.rs` [test]. Summary:
`ExecResult` is `Ok | Return | Suspend(Suspension) | Error`. `ASK "q", c1,
c2, ...;` suspends with the question + candidates; the caller (the model,
never a human, per the doc) answers via `VM::resume(susp, answer)`, which
**rejects** any answer not `==` one of the original candidates
(`same_selection`, Int/Str/Bool/Float only — `Value` has no `PartialEq`
because it holds `Hvec`, and "equal hypervectors" is a similarity question).
`ASK` inside a `CALL` works since 2026-10-24: the suspension carries the
waiting callers' frames (docs/ASK_SUSPEND.md).

## 4. Types

> **2026-10-24: types are checked now** (`src/types.rs` has the rules;
> `tests/types.rs` pins them). At compile time, a literal where the compiler
> sees it: a `return`, a typed `let`, a literal argument to one of the
> program's own functions, a struct literal's fields. At run time, every
> value crossing a function boundary: the host's arguments (text read as the
> declared scalar first, so run-proto's "5" is an `i64`), every intra-program
> call's arguments, every return. `null` is a value of every type; `number`,
> `quantity`, `symbol`, `void`, `any` and the AI-native names check nothing,
> on purpose; fixed-width ints carry their range; structs are structural.
> Under strict (the default), a type name nothing declares (no struct, enum,
> file-level or program-level `type` alias) is a compile error. The audit
> text below describes the state before.

Two entirely separate systems, and this split is the single largest
undocumented gap in the type story:

**(a) `TypeExpr` — what the parser accepts as a type annotation**
[src, `src/ast.rs:325-338`]: `Named(String)` (any bare identifier — `u32`,
`str`, `Input`, `ISolver`, and equally `number`, `quantity`, `symbol`,
anything else a program's author writes), plus `Array<T>`, `Map<K,V>`,
`Set<T>`, `Promise<T>`, `Channel<T>`, `Tuple<T1,T2,...>`, `Union[T1|T2]`,
`Nullable(T?)`, `Fn((T1,T2)->R)`, `Void`. The lexer additionally recognizes
~30 built-in type *keywords* as their own `TokenKind` (`u8..u64`, `i8..i64`,
`f32`, `f64`, `bool`, `str`, `byte`, `void`, `vec`, `emb`, `role`, `opcode`,
`ctx`, `rag`, `module`, `mdx`, `agent`, `cloned_agent`, `file`, `url`,
`dataset`, `external_program`, `request`, `response`) [src,
`src/token.rs:174-200`, `src/lexer.rs:401-436`] — but **`number`, `quantity`,
`symbol` are not among them**; they lex as plain `Ident`, confirmed
[src, `src/lexer.rs:732` test: `TokenKind::Ident("number".into())`]. Every
`.cube` example in this repo (`fibonacci.cube`, `qc_decision.cube`, ...)
types its registers `: number`, which is therefore not a built-in type at
all — it is an arbitrary, unchecked name.

**(b) `Value` — what the VM actually holds at runtime**
[src, `src/vm/engine.rs:20-31`]: exactly 8 variants — `Int(i64)`,
`Float(f64)`, `Str(String)`, `Bool(bool)`, `Null`, `Array(Vec<Value>)`,
`Map(HashMap<String, Value>)`, `Hvec(Hypervec)`. There is **no dedicated
struct/record runtime value** — a `struct` declaration (`gsm8k.cube`'s
`ArithStep`/`Problem`/`Solution`, `conversation_agent.cube`'s `Turn`/`Reply`)
parses (`ast::StructDecl`) but, per §1, top-level `struct` is never visited
by `Compiler::compile`, and `Expr::StructLit` has no dedicated bytecode path
either.

**No type-checking pass exists anywhere in `src/`.** `TypeExpr` is carried
through the AST (`Param::ty`, `StorageField::ty`, `FunctionSig::return_type`,
...) purely for documentation/interface-arity purposes (arity, not type, is
what `check_implements` actually checks, §2) and is never consulted by
`compiler.rs` to reject a type mismatch. A declared type is decoration; the
only thing that is ever checked is parameter *count*.

## 5. Storage, constructors, and the module/capability system

**`storage { ... }` block** [src, `src/ast.rs:230-244` `StorageBlock`/
`StorageField` (`name`, `mutable: bool`, `ty`, optional `default`)]: fields
are collected into `CompiledProgram::storage_fields: Vec<String>` (names
only — no default-value initialization, no type, no mutability flag survives
compilation) [src, `src/compiler.rs:693-697`]. The `mutable` flag on
`StorageField` (every example writes `mutable u64 = 0` etc.) is **parsed and
then never read again anywhere in `compiler.rs` or `vm/engine.rs`** — no
immutability enforcement exists; see `docs/DRIFT.md` §A.

**Constructors** [src, `src/main.rs:910-916` `call_with_constructor`,
`src/vm/engine.rs:385-387` `VM::init`]: a function literally named
`constructor` (parsed via `ProgramItem::Constructor(FunctionDecl)`, the
`constructor` keyword just names the function) is called automatically,
**best-effort** — its result and any error are silently discarded
(`let _ = vm.call(program, "constructor", Vec::new());`) — immediately
before the target function, on both `cubelang run` and `run-proto`. There is
no once-only guard at the VM level; `@once` (parsed, see §6) does not enforce
this — calling `constructor` twice re-runs it twice.

> **2026-10-24.** A storage field's default is its starting value: it must be a constant (a
> literal, or an array, map or struct of literals; anything else is a compile error that says to
> set it in the constructor), is type-checked against the field, and is set when the VM loads the
> program, before any constructor. A map or array field with no default starts empty. `.cubebin`
> carries them. A field declared `immutable` is written only by the constructor (a local of the
> same name is the same register, so it counts). `@once` is enforced (§6). The host runs a
> program through `VM::boot` (its constructor; a container's sequence, §1), and a constructor
> that fails now stops the run instead of being dropped. Each program's registers are its own:
> the VM parks a program's storage (and what its last run left in registers) while another
> program runs, so two programs with a field of the same name never share it, and one cannot read
> what another left behind (`VM::storage_value(program, field)` reads a parked program's field).

**`use <module>;` / `ModuleRegistry`** [src, `src/vm/registry.rs`, full
file]: a second, sibling registry to `InterfaceRegistry` — deny-by-default
call resolution. `ModuleRegistry::resolve(used, name)` only finds a function
if the current file's `used_modules` (populated from top-level `Use(String)`
AST nodes) actually names the module; `ModuleRegistry::get` is the
un-gated lookup used only for compile-time `override` validation. Two
modules ship today: `demo` (`greet` overridable, `sealed` not — a pure test
fixture, `src/vm/registry.rs:150-163`) and **`vsa`**
(`recover(reg, role)` — real UNBIND + cosine cleanup against every known
symbol, returns `Value::Str` or `Value::Null`, `src/vm/registry.rs:165-186`).
`recover` is an ordinary registry **function call**, not a keyword or
opcode statement — this is the mechanism `CubbyLLM`'s
`cubbyllm/reasoning/programs.py` actually emits today (`return recover(frame,
{role});`).

**`override`** — two unrelated mechanisms share the English word, kept in
separate AST fields on purpose [src, `src/ast.rs:172-183`]:
- `FunctionSig.is_override: bool` — a **postfix** marker,
  `function name() override { ... }`, Task 3's capability system. Valid only
  when `name` is exposed `overridable: true` by a `use`'d module
  (`check_override_validity`, `src/compiler.rs:764-782`); a plain (no
  `override`) function whose name collides with a **sealed**
  (`overridable: false`) module export is *also* a compile error
  (`check_sealed_collision`, `src/compiler.rs:794-815`) — there is no way to
  reach a program function shadowed by a sealed capability at all.
- `Modifier::Override` — a **prefix** modifier, `override function name()`,
  reserved by SPEC.md for a different, not-yet-built inheritance feature
  (`extend` blocks). Parses; has no compiled behavior anywhere found in this
  audit.

Call-resolution precedence at runtime, per `op::CALL`'s own doc comment
[src, `src/vm/registry.rs:26-28`]: **validated program override > registry
implementation > plain program-local function > error.**

## 6. Attributes / permissions / modifiers — parsed, essentially all decorative

> **Superseded 2026-10-24.** `@external`, `@internal`, `@system`, `@once`, `@restricted`,
> `@ratelimit`, `@cron` and the `private` modifier are now enforced, and `.cubebin` v4 carries
> them; see `docs/CONTRACTS.md`. `@hook`/`@before`/`@after` remain unenforced (strict errors).
> The audit text below describes the state before that.

[src, `src/ast.rs:184-206` `Permission` enum, `188-206` `Modifier` enum;
`src/parser.rs:700-822` `parse_permissions`/parses modifiers into
`FunctionSig.permissions`/`FunctionSig.modifiers`]

`@external`, `@internal`, `@system`, `@hook(...)`, `@before(...)`,
`@after(...)`, `@cron(...)`, `@once`, `@restricted([...])`,
`@ratelimit(n, period)` — every one of these parses into
`FunctionSig.permissions: Vec<Permission>` and **that field is never read
again anywhere in `src/compiler.rs` or `src/vm/engine.rs`** (confirmed by
exhaustive grep: no `Permission::External`, `Permission::System`,
`Permission::Once`, `Permission::Hook`, `Permission::Cron`,
`Permission::Restricted`, or `Permission::Ratelimit` match arm exists outside
`parser.rs` itself). There is no access control, no once-only call guard, no
cron scheduler, no rate limiter, no hook dispatcher anywhere in the VM. Every
`.cube` example in this repo writes `@system @once` on its `constructor` and
`@external` on its public entry points as pure convention; none of it is
enforced.

`Modifier::{Public, Private, Abstract, Global, Mutable, Immutable, Async,
Sequential, Parallel, Joined, Pure, Optional, Singleton}` — likewise parsed
into `FunctionSig.modifiers` and never read in `compiler.rs`/`engine.rs`
**except**: `Abstract` and `Optional`, and only as filters when resolving an
in-file inline `interface` decl's required-member list (§2) — not for
program-level functions at all. `pure` (every example's `verify` function)
has zero compiled effect; there is no purity checking. `private`/`public`
enforce no actual visibility restriction — a "private" helper is callable via
`CALL` exactly like a public one if the caller knows its name.

## 7. Statement forms the parser accepts (`Stmt`, `src/ast.rs:301-330`)

`Let`, `Assign`, `If`, `For` (real, lowers to `while`+`LEN`/`INDEX`, §8),
`While`, `Match` (parses; **non-executing**, §DRIFT-C), `Return`, `Throw`
(parses; **never compiled at all**, any mode), `Emit`, `Opcode(OpcodeStmt)`
(§3.1/3.2 surface forms: `create`/`assign`/`add`/`sub`/`mul`/`div`/`sum`/
`push`/`pop`/`query`/`remember`/`store`/`recall`/`bind`/`unify`/`bind_role`/
`transfer`/`compare`/extended ops), `Expr` (bare expression statement — real
bytecode only for `Call`/`MethodCall`, §DRIFT-C), `TryCatch` (`try`/`catch`
real; `finally` body **never compiled**, §DRIFT-C), `Assert` (real only when
the condition is itself a `Call`/`MethodCall`, §DRIFT-C), `Atomic`/
`Rollback`/`Commit` (real scaffolding; `commit`/`rollback` are single-`SKIP`
VM-level markers, `src/compiler.rs` strict-check doc), `Asm` (the raw
mnemonic escape hatch, §3.1).

## 8. `for` — real, via lowering (not the `LOOP` opcode)

[src, `CUBELANG_OPCODES_PLAN.md` §4, confirmed current in
`src/compiler.rs`'s `strict_check_stmt` doc: *"`for` is NOT in this list...
`for` always lowers to real COMPARE/COND/JMP/LABEL loop scaffolding"*]
`for (let x of arr)` compiles to: `CREATE __i; ASSIGN __i=0; LEN __n, arr;
LABEL loop_top; COMPARE __i,__n,lt; COND->loop_end; INDEX x,arr,__i; <body>;
ADD __i,1; JMP loop_top; LABEL loop_end;`. The `LOOP` opcode byte (0x12)
itself is only emitted as an optional structural/trace marker and never
executes anything (§3.1) — do not confuse "the `for` statement is real" with
"the `LOOP` opcode is real"; they are different things with the same
English name in casual conversation.

## 9. CLI surface (`src/main.rs`)

```
cubelang compile   [--strict] <file.cube> [-o out.cubebin]
cubelang run       <file.cube|file.cubebin> [--strict] [--fn name] [--trace]
                   [--json] [--arg VALUE]... [--knowledge facts.jsonl]
cubelang run-proto                    # length-prefixed protobuf, stdin/stdout
cubelang inspect   <file.cubebin>
cubelang disasm    <file.cubebin>
cubelang check     <file.cube>        # see note below — NOT parse-only
cubelang validate  <file.cube>        # per-function trace-only/stub report
cubelang version
```

- `compile`/`run` are **loose by default**; `--strict` opts in to
  `compile_strict`/`compile_ast_strict` (rejects every construct in
  `docs/DRIFT.md` §C as a hard compile error instead of a silent no-op).
- **`check` is always fully strict** [src, `src/main.rs:325-349`,
  `compiler::compile_ast_strict`], despite `print_usage()`'s own text
  (`src/main.rs:59`: *"Parse and check .cube without compiling"*) — that
  usage line is stale; `check` genuinely compiles the merged (import-
  resolved) AST in strict mode and reports the same diagnostics `compile
  --strict` would. [ran — confirmed: `check examples/gsm8k.cube` fails with
  3 strict violations; a `container`-only file with zero `program` blocks
  reports `"ok (... 0 programs ...)"` because there is nothing to iterate,
  which is a second, distinct trap — see `docs/DRIFT.md`.]
- **`run-proto` is unconditionally strict** [src, `src/main.rs:` doc above
  `run_request_result`, uses `compiler::compile_strict` directly, no flag] —
  there is no loose mode over the protobuf transport at all, unlike `run`.
- `--knowledge facts.jsonl` (`run`) / `knowledge_path` (`RunRequest`, proto
  field 5) load a `KnowledgeStore` for `QUERY` (§3.4); without it every
  `QUERY` returns an empty chunk array (abstains), never an error.
- `run --json` and `run-proto` share one value-mapping convention
  (`value_to_json`); `Value::Null` leaves the protobuf `oneof` **unset**
  rather than becoming a placeholder symbol [src, `proto/reasoning.proto`
  doc comment on `RunResult`].
- ASK/resume over the wire: `RunRequest.answers` is an ordered list of
  JSON-encoded `Value`s; `run-proto` **re-executes the program from
  scratch** and consumes one answer per `ASK` it reaches — no VM state ever
  crosses the wire [src, `proto/reasoning.proto:9-16`].

## 10. VM internals (brief — not exhaustively audited)

- `src/vm/hypervec.rs` (618 lines): bipolar `{-1,+1}^D` hypervector type,
  MAP-Bipolar bind (Hadamard product) / bundle (sum + sign) / permute (cyclic
  shift) / cosine similarity.
- `src/vm/codebook.rs` (156 lines): deterministic FNV-1a-seeded symbol name →
  hypervector mapping (same symbol always maps to the same vector for a
  given seed).
- `src/vm/index.rs` (424 lines): `HammingIndex` (exact, bit-packed linear
  scan) and a second (LSH) backend for nearest-neighbour cleanup.
- `src/vm/memory.rs` (179 lines): hippocampal associative memory —
  `STORE`/`RECALL` on top of `Codebook` + `HammingIndex`; RECALL is genuine
  cosine-nearest cleanup, not an exact-dict lookup (distinct from `QUERY`'s
  exact-key `knowledge.rs` lookup, §3.4 — two different "look something up"
  mechanisms exist side by side, addressed differently and returning
  differently-shaped results: RECALL gives one `(matched_key, value,
  similarity)`, QUERY gives an array of provenance-carrying maps).
- `src/vm/knowledge.rs` (217 lines): exact-key `KnowledgeStore` (`.jsonl`
  facts with `key`/`text`/`source`, plus alias records) backing `QUERY`.

## 11. Two ways to express the same thing (flagged per the task brief)

- **Binding + recovery**: `bind`/`bind_role` (opcode statements) write; the
  *only* two ways to read a binding back out are (a) `use vsa; recover(reg,
  role)` (a registry function call — what every current example and
  `programs.py` actually use) or (b) `asm { UNBIND dst, reg, role; }` (the
  raw mnemonic, reachable by no other surface syntax). These are the same
  underlying VM primitive (`VM::vsa_unbind_cleanup_known`,
  `op::UNBIND`) exposed through two unrelated syntactic doors — one a
  function call, one a block keyword — with nothing in either doc explaining
  why both exist or when to prefer one.
- **Declaring an interface**: `implements ISolver` (resolves to the
  hardcoded registry contract, §2) vs. writing your own
  `interface ISolver { ... }` block in the same file (silently ignored for
  the 4 registry names, live only for a name the registry doesn't
  recognize). Visually identical syntax, two different resolution paths,
  and the file gives no signal at the `implements` site which path a given
  name will take.
- **`override`** — prefix `Modifier::Override` vs. postfix
  `FunctionSig.is_override`, §5. Same keyword, same statement position
  region, unrelated semantics (one inert, one load-bearing).
- **Loop constructs**: `for (let x of arr)` (real, lowers to real
  bytecode) vs. the raw `loop`/`LOOP` opcode keyword (`OpLoop`,
  `src/token.rs`) which is only ever a structural trace marker. A model
  that has learned "for loops work" has no source-level signal that the
  bare `loop` opcode keyword it can also emit does nothing.
