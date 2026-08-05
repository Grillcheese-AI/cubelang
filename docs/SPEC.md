# CubeLang v0.1.0 — Specification

**Unified AI Instruction Language**
**License:** MIT
**Author:** Grillcheese Research Laboratory
**Contact:** info@grillcheeseai.com
**Extension:** `.cube`

---

## Overview

CubeLang is a scripting language that compiles to CubeMind VM bytecodes (0x00-0xFF).
Programs are self-evolving reasoning modules. Each program defines how the VM
approaches a category of problems, and can modify itself as it learns new patterns.
Programs are always bound to an interface.

---

## Execution Modifiers

Every function executes within a scope. Modifiers control how:

| Modifier | Meaning |
|----------|---------|
| `public` | Callable from outside the program |
| `private` | Only callable within the program |
| `abstract` | Must be implemented by the program (interface only) |
| `global` | Shared across all program instances |
| `mutable` | State can change after initialization |
| `immutable` | State is frozen after constructor |
| `async` | Non-blocking execution, returns Promise<T> |
| `sequential` | Steps execute one after another (default) |
| `parallel` | Steps execute concurrently on separate threads |
| `joined` | Parallel steps that must all complete before continuing |
| `pure` | No side effects, no state mutation, cacheable |
| `optional` | May or may not be implemented |
| `singleton` | Only one instance exists in the VM |

---

## Permissions & Access Control

Functions have explicit permissions that control who can call them and when.
This is how programs hook into each other's lifecycle safely.

### Permission Modifiers

| Permission | Meaning |
|-----------|---------|
| `@external` | Callable by other programs and outside requests (API, user) |
| `@internal` | Only callable by this program or programs it explicitly grants |
| `@system` | Only callable by the VM runtime (lifecycle hooks) |
| `@hook(ProgramName.event)` | Triggered when another program emits an event |
| `@before(fn_name)` | Runs before the named function (middleware) |
| `@after(fn_name)` | Runs after the named function (middleware) |
| `@cron(interval)` | Triggered on a schedule by the VM |
| `@once` | Can only be called a single time (constructor-like) |
| `@restricted(programs)` | Only callable by the listed programs |
| `@ratelimit(n, period)` | Max n calls per period |

### Usage

```cubelang
program GSM8K implements IMathSolver, IDeployable {

    # ── Lifecycle hooks (VM calls these) ─────────────────────

    @system @once
    public function constructor() {
        self.patterns = {};
    }

    @system
    public function destructor() {
        self.save_state();
    }

    # ── External API (other programs + outside world) ────────

    @external
    public function solve(input: Input): Output {
        # Anyone can call this
    }

    @external @ratelimit(100, "1m")
    public function solve_batch(inputs: array<Input>): array<Output> {
        # Rate-limited: max 100 calls per minute from external
    }

    # ── Internal only (this program + granted programs) ──────

    @internal
    private function classify_error(input: Input, diff: f64): str {
        # Only this program can call this
    }

    @internal @restricted(["MathRouter", "BenchmarkRunner"])
    public function get_patterns(): map<str, function> {
        # Only MathRouter and BenchmarkRunner can read our patterns
        return self.patterns;
    }

    # ── Hooks into other programs ────────────────────────────

    @hook(AlgebraSolver.PatternLearned)
    public function on_algebra_pattern(event: PatternLearned): void {
        # Triggered when AlgebraSolver learns a new pattern
        # We can absorb it if it's relevant to arithmetic
        if (event.pattern.startsWith("linear_")) {
            self.patterns["algebra:" + event.pattern] = event.handler;
        }
    }

    @hook(MathRouter.SolveRequest)
    public function on_route_request(event: SolveRequest): void {
        # MathRouter is asking us to pre-warm for an incoming problem
        self.preload_context(event.category);
    }

    # ── Middleware (before/after other functions) ─────────────

    @before(solve)
    private function validate_input(input: Input): Input {
        # Runs before every solve() call — validation layer
        if (input.steps.length == 0) {
            throw Error("No arithmetic steps found");
        }
        if (input.steps.length > self.config.max_steps) {
            throw Error("Too many steps: ${input.steps.length}");
        }
        return input;
    }

    @after(solve)
    private function log_result(input: Input, output: Output): void {
        # Runs after every solve() call — telemetry
        self.history.push(output);
        emit SolveComplete {
            program: "GSM8K",
            input_hash: hash(input.question),
            result: output.result,
            elapsed_ms: timer.elapsed(),
        };
    }

    @after(learn)
    private function checkpoint_patterns(): void {
        # Auto-save patterns after every learn cycle
        if (self.patterns.size % 10 == 0) {
            self.save_state();
        }
    }

    # ── Scheduled tasks ──────────────────────────────────────

    @cron("1h")
    @system
    private function cleanup_stale_patterns(): void {
        # Every hour, prune patterns with low confidence
        for (let [key, handler] of self.patterns) {
            if (handler.confidence < 0.3) {
                self.patterns.delete(key);
            }
        }
    }

    @cron("24h")
    @system
    private function retrain_from_history(): void {
        # Daily: re-derive patterns from solution history
        let failures = self.history.filter((h) => !h.verified);
        for (let f of failures) {
            self.learn(f.input, f.expected, f.actual);
        }
    }
}
```

### Grant / Revoke Access at Runtime

```cubelang
# Program A grants Program B access to a specific function
grant GSM8K.get_patterns to BenchmarkRunner;

# Revoke later
revoke GSM8K.get_patterns from BenchmarkRunner;

# Grant with conditions
grant GSM8K.solve to ExternalAPI {
    ratelimit: 1000 per "1h",
    require_auth: true,
    log: true,
};
```

### Lifecycle Hooks Between Programs

Programs can register to another program's lifecycle events:

```cubelang
program Monitor implements IDeployable {

    storage {
        tracked: mutable array<external_program>;
    }

    public function constructor() {
        self.tracked = [];
    }

    # Watch another program's lifecycle
    public function watch(target: external_program): void {
        self.tracked.push(target);

        # Hook into its lifecycle
        target.on("deploy", self.on_program_deploy);
        target.on("extend", self.on_program_extend);
        target.on("error", self.on_program_error);
        target.on("destroy", self.on_program_destroy);
    }

    @hook(*.deploy)
    private function on_program_deploy(event: DeployEvent): void {
        log("[Monitor] Program deployed: ${event.program}");
    }

    @hook(*.extend)
    private function on_program_extend(event: ExtendEvent): void {
        log("[Monitor] Program extended: ${event.program}.${event.function}");
    }

    @hook(*.error)
    private function on_program_error(event: ErrorEvent): void {
        log("[Monitor] Error in ${event.program}: ${event.message}");
        # Auto-restart if critical
        if (event.severity == "critical") {
            vm.restart(event.program);
        }
    }
}
```

### Permission in Interfaces

Interfaces can declare permission requirements:

```cubelang
interface ISecureSolver {
    # Anyone can call parse
    @external
    abstract public function parse(raw: str): Input;

    # Only registered programs can call solve
    @restricted
    abstract public function solve(input: Input): Output;

    # Only the program itself can learn
    @internal
    optional public function learn(input: Input, expected: Output, actual: Output): void;

    # Only the VM can call lifecycle methods
    @system
    abstract public function constructor(): void;
}
```

---

## Base Types

```cubelang
# Primitives
number                       # untyped numeric register (supertype of int/float — see Type System)
u8, u16, u32, u64           # unsigned integers
i8, i16, i32, i64           # signed integers
f32, f64                    # floating point
bool                        # true / false
str                         # utf-8 string
byte                        # alias for u8
null                        # absence of value

# AI-native types
vec                         # VSA hypervector (D=8192, bipolar i8)
emb                         # dense embedding (f32 array, any dimension)
role                        # semantic role (AGENT, ACTION, OBJECT, QUANTITY, ...)
opcode                      # VM instruction (0x00-0xFF)
ctx                         # execution context — everything the VM knows right now
rag                         # retrieval-augmented context (query + retrieved chunks + scores)
module                      # external module (python, rust, wasm, onnx, gguf)
mdx                         # executable natural language — personality, prompts with variables
agent                       # autonomous agent (program + persona + ctx + loop)
cloned_agent                # copy of an agent with forked state (independent evolution)

# Reference types
file                        # file handle (path + read/write/append)
url                         # remote resource (http/https/ws)
dataset                     # iterable data source (jsonl, csv, parquet, hf)
map<K, V>                   # hash map
set<T>                      # unique collection
array<T>                    # ordered list (alias: []T)
tuple<T1, T2, ...>          # fixed-size heterogeneous
promise<T>                  # async result
channel<T>                  # inter-function message passing
external_program             # reference to another deployed program
```

---

## AI-Native Types: emb, module, rag

### emb — Embeddings

Dense floating-point embedding vectors from any model. Unlike `vec` (bipolar VSA),
`emb` holds continuous f32 values at arbitrary dimensions.

```cubelang
# Create from a model
let text_emb: emb = embed("Janet sells duck eggs", model: "bge-small");
let image_emb: emb = embed(image_file, model: "clip-vit-b32");

# Properties
text_emb.dim          # 384 (bge-small)
text_emb.dtype        # f32
text_emb.model        # "bge-small"

# Operations
let sim: f64 = cosine_sim(text_emb, other_emb);
let combined: emb = concat(text_emb, image_emb);       # [384 + 512] = 896
let projected: emb = project(text_emb, dim: 128);      # linear projection
let quantized: vec = quantize(text_emb);                # emb -> bipolar vec

# Batch
let batch: array<emb> = embed_batch(sentences, model: "bge-small");
```

### ctx — Execution Context

The current state of everything the VM knows. `ctx` is the living snapshot of
registers, stack, storage, conversation history, active programs, user identity,
permissions, and environment. It flows through every function call — you can read
it, pass it, fork it, merge it, snapshot it, and restore it.

`ctx` is what makes the backtracking apple example work: store a context snapshot,
mutate it, detect a correction, restore the snapshot, replay with new values.

```cubelang
# ── The current context is always available ──────────────────

let now: ctx = ctx.current();

# ── What's inside a ctx ──────────────────────────────────────

now.registers             # map<str, f64> — all live registers
now.stack                 # array<f64> — the PUSH/POP stack
now.storage               # map<str, any> — program's persisted state
now.history               # array<Message> — conversation so far
now.user                  # User — who is talking (id, name, roles)
now.program               # str — which program is executing
now.permissions           # set<str> — active permissions for this call
now.env                   # map<str, str> — environment variables
now.timestamp             # u64 — when this context was created
now.parent                # ctx | null — the context that spawned this one
now.depth                 # u32 — call depth (0 = top level)

# ── Snapshot & restore (backtracking) ────────────────────────

let checkpoint: ctx = ctx.snapshot();

# ... do some work, maybe it goes wrong ...
assign apples = 8;
sub apples, 4;
sum apples;    # 4

# Oops — correction: "I had 12, not 8"
ctx.restore(checkpoint);
assign apples = 12;
sub apples, 4;
sub apples, 3;
sum apples;    # 5 (correct)

# ── Fork: create an isolated copy to try something ───────────

let experiment: ctx = ctx.fork();

# Work inside the fork — doesn't affect the main context
experiment.run(() => {
    assign x = 100;
    mul x, 2;
    sum x;
});

# Check result without committing
if (experiment.registers["x"] > 150) {
    ctx.merge(experiment);   # accept the fork
} else {
    experiment.discard();    # throw it away
}

# ── Pass context between programs ────────────────────────────

@external
public function solve(input: Input): Output {
    let my_ctx: ctx = ctx.current();

    # Hand context to another program — it sees our registers, history
    let algebra: external_program = vm.get_program("AlgebraSolver");
    let result = algebra.solve_with_context(input, my_ctx);

    return result;
}

# Receiving program sees the caller's context
@external
public function solve_with_context(input: Input, caller: ctx): Output {
    # We can read what the caller has computed so far
    let prev_result = caller.registers["last_step"];
    let who_asked = caller.user.name;

    # But we can't modify their context — it's read-only from here
    # caller.registers["x"] = 42;  # ERROR: ctx is read-only when passed
}

# ── Context in conversation (chat programs) ──────────────────

@external
public function chat(message: str): str {
    let c: ctx = ctx.current();

    # History accumulates across calls
    c.history.push({ role: "user", content: message });

    # Render mdx with full context
    let prompt = self.persona.render({
        student_name: c.user.name,
        previous_interactions: c.history.last(10),
        current_registers: c.registers,
    });

    let response = self.llm.call("generate", {
        system: prompt.text,
        prompt: message,
    });

    c.history.push({ role: "assistant", content: response });

    return response;
}

# ── Scoped context (temporary override) ──────────────────────

# Run a block with modified context — reverts when done
ctx.with({ user: admin_user, permissions: ["admin"] }, () => {
    # Inside here, ctx.current().user is admin
    self.dangerous_operation();
});
# Back to normal context here
```

#### Isolated Context — Sandboxed Discussions

An `isolated` context has no access to the parent. Perfect for separate
conversations, experiments, or untrusted code. It starts empty.

```cubelang
# Create an isolated context — completely sandboxed
let sandbox: ctx = ctx.isolated();

# It has its own registers, stack, history — nothing shared
sandbox.run(() => {
    create x : number;
    assign x = 42;
    # Cannot see parent registers, storage, or history
});

# Use for separate conversations
let discussion_a: ctx = ctx.isolated({ user: alice });
let discussion_b: ctx = ctx.isolated({ user: bob });

# Each discussion has its own history, registers, state
discussion_a.run(() => self.chat("What is 2+2?"));
discussion_b.run(() => self.chat("Tell me about quantum physics"));

# They never see each other's data
```

#### Scoped Context — Temporary Override

A `scoped` context inherits from the parent but reverts all changes on exit.
Like a transaction that can commit or rollback.

```cubelang
# Temporary elevated permissions
ctx.scoped({ permissions: ["admin"] }, () => {
    self.admin_operation();
});
# Permissions reverted here

# Try something risky — rollback if it fails
let result = ctx.scoped({}, () => {
    assign x = 100;
    div x, 0;       # might fail
    return eval(x);
});
# If the scoped block threw, x is untouched
```

### agent & cloned_agent — Autonomous Agents

An `agent` is a program + persona + context + execution loop bundled together.
It runs autonomously, has its own identity, and can communicate with other agents.

A `cloned_agent` is a deep copy that evolves independently — same starting point,
different future.

```cubelang
# ── Create an agent ──────────────────────────────────────────

let tutor: agent = agent.create({
    name: "MathTutor",
    program: deploy GSM8K(),
    persona: mdx.load("personas/math_tutor.mdx"),
    ctx: ctx.isolated({ user: { name: "system", role: "tutor" } }),
    model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
});

# ── Agent properties ─────────────────────────────────────────

tutor.name                # "MathTutor"
tutor.program             # the deployed GSM8K instance
tutor.persona             # the loaded mdx
tutor.ctx                 # its isolated context
tutor.model               # its LLM module
tutor.status              # "idle" | "running" | "paused" | "stopped"
tutor.uptime              # u64 ms since creation
tutor.messages_handled    # u64 count

# ── Run the agent (autonomous loop) ──────────────────────────

tutor.start();            # begins listening for messages
tutor.send("How do I solve 16 - 3 - 4?");
let reply = tutor.receive();   # blocks until agent responds

tutor.pause();            # pause the loop
tutor.resume();           # continue
tutor.stop();             # shutdown

# ── Agent-to-agent communication ─────────────────────────────

let reviewer: agent = agent.create({
    name: "Reviewer",
    program: deploy AnswerVerifier(),
    persona: mdx.load("personas/reviewer.mdx"),
    ctx: ctx.isolated(),
    model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
});

# Agents talk to each other
tutor.connect(reviewer);    # establish a channel

# In tutor's program:
@after(solve)
private function ask_review(input: Input, output: Output): void {
    let feedback = self.agent.ask(reviewer, {
        question: input.question,
        proposed_answer: output.result,
    });
    if (!feedback.approved) {
        self.learn(input, feedback.correction, output);
    }
}

# ── Clone an agent ───────────────────────────────────────────

# Deep copy — same program, persona, and learned patterns but independent
let tutor_v2: cloned_agent = tutor.clone();

# The clone starts with everything the original knows
tutor_v2.program.patterns    # same patterns as tutor
tutor_v2.ctx.history         # copy of tutor's history

# But from here they evolve independently
tutor_v2.send("Solve: 80000 * 1.5 = ?");
# tutor_v2 learns from this, tutor does not

# ── Clone properties ─────────────────────────────────────────

tutor_v2.parent           # reference to tutor (the original)
tutor_v2.forked_at        # timestamp when cloned
tutor_v2.divergence       # how different the clone is from parent (0.0 - 1.0)

# ── Compare clone to original ────────────────────────────────

let diff = agent.diff(tutor, tutor_v2);
diff.new_patterns         # patterns the clone learned that the original doesn't have
diff.modified_storage     # storage fields that diverged
diff.performance_delta    # success rate difference

# ── Merge clone back ─────────────────────────────────────────

# If the clone did better, merge its improvements back
if (tutor_v2.success_rate > tutor.success_rate) {
    tutor.merge(tutor_v2);   # absorb the clone's learned patterns
}

# ── Parallel agent swarm ─────────────────────────────────────

# Spawn N clones, each tries a different approach, pick the winner
let clones: array<cloned_agent> = [];
for (let i = 0; i < 5; i++) {
    let c = tutor.clone();
    c.ctx = ctx.isolated();   # each gets fresh context
    c.send(problem);
    clones.push(c);
}

let results = await joined clones.map((c) => c.receive());
let best = results.sort((a, b) => b.confidence - a.confidence)[0];
tutor.merge(best.agent);    # winner's improvements go back to the original
```

### module — External Module

References to external code that runs outside the VM. Modules can be Python functions,
Rust crates, WASM binaries, ONNX models, or GGUF weights.

```cubelang
# Load a Python module
let tokenizer: module = module.load("python", {
    path: "transformers.AutoTokenizer",
    init: { pretrained: "Qwen/Qwen3-8B" },
});
let tokens = tokenizer.call("encode", { text: "hello world" });

# Load a Rust module via PyO3
let vsa_engine: module = module.load("rust", {
    crate: "opcode-vsa-rs",
    function: "encode_program",
});
let bytecode = vsa_engine.call("encode", { program: my_program });

# Load an ONNX model
let classifier: module = module.load("onnx", {
    path: "models/intent_classifier.onnx",
    device: "vulkan",
});
let logits = classifier.call("forward", { input: text_emb });

# Load a GGUF model (llama.cpp)
let llm: module = module.load("gguf", {
    path: "models/qwen3-8b-q4_k_m.gguf",
    n_ctx: 4096,
    n_gpu_layers: 99,
});
let response = llm.call("generate", {
    prompt: "Solve: 16 - 3 - 4 = ?",
    max_tokens: 128,
});

# Module properties
tokenizer.type        # "python"
classifier.device     # "vulkan"
llm.loaded            # true
llm.params            # 8_000_000_000
```

### rag — Retrieval-Augmented Context

First-class RAG type that encapsulates a query, retrieved chunks, similarity scores,
and source references. Built into the language so programs can reason over retrieved
context natively.

```cubelang
# Create a RAG index
let knowledge: rag = rag.create({
    name: "math_problems",
    embedding_model: "bge-small",
    chunk_size: 512,
    overlap: 64,
});

# Ingest documents
knowledge.ingest(dataset.from_jsonl("data/gsm8k_test.jsonl"), field: "question");
knowledge.ingest(file.open("docs/math_handbook.pdf"));
knowledge.ingest(url("https://example.com/math-guide.html"));

# Query — returns typed RAG context
let ctx: rag = knowledge.query("How many eggs does Janet sell?", top_k: 5);

# RAG properties
ctx.query             # "How many eggs does Janet sell?"
ctx.chunks            # array<{ text: str, score: f64, source: str }>
ctx.top_score         # 0.94
ctx.sources           # ["gsm8k_test.jsonl:0", "gsm8k_test.jsonl:42"]

# Use in a program
public function solve_with_context(problem: str): Solution {
    let ctx: rag = self.knowledge.query(problem, top_k: 3);

    # Check if we've seen a similar problem before
    if (ctx.top_score > 0.9) {
        let similar = ctx.chunks[0].text;
        let cached = self.history.find((h) => h.input == similar);
        if (cached != null) {
            return cached.output;  # reuse previous solution
        }
    }

    # Augment the input with retrieved context
    let augmented = problem + "\n\nSimilar problems:\n" +
        ctx.chunks.map((c) => c.text).join("\n");

    return self.solve(self.parse(augmented));
}

# RAG + LLM composition
public async function solve_hard(problem: str): Solution {
    let ctx: rag = self.knowledge.query(problem, top_k: 5);
    let llm: module = module.load("gguf", { path: "models/qwen3-8b.gguf" });

    let prompt = "Given these similar problems:\n" +
        ctx.chunks.map((c) => c.text).join("\n") +
        "\n\nSolve: " + problem;

    let decomposition = llm.call("generate", { prompt: prompt });
    return self.solve(self.parse(decomposition));
}
```

### mdx — Executable Natural Language

Natural language documents with typed variables. The body of an mdx file is
plain human-readable text — not code. Variables are declared in the frontmatter
and filled in at render time. The document itself is what a person or LLM reads.

mdx is how the VM talks to LLMs and humans: programs generate structured
bytecodes, but when they need to communicate in natural language (prompts,
explanations, personalities, instructions), they render an mdx.

An `mdx` value is a living document — it has state, can be rendered with variables,
and can call back into the program that loaded it.

```cubelang
# ── Load an mdx personality ──────────────────────────────────

let persona: mdx = mdx.load("personas/math_tutor.mdx");

# ── mdx file: personas/math_tutor.mdx ───────────────────────
#
# ---
# name: Math Tutor
# version: 0.1.0
# variables:
#   student_name: str
#   difficulty: enum(easy, medium, hard)
#   language: str = "en"
#   max_steps: u32 = 10
#   personality: str = "patient and encouraging"
# tools:
#   - solve: GSM8K.solve
#   - verify: GSM8K.verify
#   - search: knowledge.query
# on:
#   easy: show every step, use simple language, encourage often
#   hard: skip obvious steps, use formal notation, challenge the student
# ---
#
# You are a {{personality}} math tutor helping {{student_name}}.
# You work at the {{difficulty}} level in {{language}}.
# You show at most {{max_steps}} reasoning steps.
#
# When the student asks a math question, solve it step by step.
# If you are unsure, search for similar problems first.
# Always verify your answer before presenting it.
#
# Here is what you discussed previously:
#
# {{previous_interactions}}

# ── Instantiate with variables ───────────────────────────────

let tutor = persona.render({
    student_name: "Alice",
    difficulty: "medium",
    language: "en",
    max_steps: 8,
});

# tutor is now a fully rendered prompt string with tools bound

# ── Properties ───────────────────────────────────────────────

persona.name              # "Math Tutor"
persona.version           # "0.1.0"
persona.variables         # map of declared variables + defaults
persona.tools             # array of bound tool references
persona.raw               # the raw mdx source

# ── Variables are typed and validated ────────────────────────

persona.variables.student_name.type     # "str"
persona.variables.difficulty.type       # "enum"
persona.variables.difficulty.options    # ["easy", "medium", "hard"]
persona.variables.max_steps.default     # 10

# ── Tools are bound to program functions ─────────────────────

persona.tools[0].name     # "solve"
persona.tools[0].target   # GSM8K.solve (external_program reference)

# ── Render returns the final string ──────────────────────────

let system_prompt: str = tutor.text;
let tool_list: array<function> = tutor.bound_tools;

# ── Use with an LLM module ───────────────────────────────────

let llm: module = module.load("gguf", { path: "models/qwen3-8b.gguf" });
let response = llm.call("generate", {
    system: tutor.text,
    prompt: "How do I solve 16 - 3 - 4?",
    tools: tutor.bound_tools,
});
```

#### mdx Composition — Personalities that inherit

```cubelang
# Base personality
let base: mdx = mdx.load("personas/base_assistant.mdx");

# Extend with math-specific behavior
let math_persona: mdx = base.extend("personas/math_tutor.mdx");

# Extend again with a specific student profile
let alice_tutor: mdx = math_persona.extend({
    variables: {
        student_name: "Alice",
        difficulty: "hard",
        previous_interactions: self.history.last(10),
    },
});
```

#### mdx Inside Programs

Programs can use mdx as dynamic prompt templates that evolve:

```cubelang
program TutorBot implements ITutor, IDeployable {

    storage {
        persona: mutable mdx;
        interactions: mutable array<Interaction>;
    }

    @system @once
    public function constructor() {
        self.persona = mdx.load("personas/math_tutor.mdx");
        self.interactions = [];
    }

    @external
    public function chat(user_message: str): str {
        # Re-render persona with latest interaction history
        let prompt = self.persona.render({
            student_name: self.student_name,
            difficulty: self.assess_difficulty(),
            previous_interactions: self.interactions.last(5),
        });

        let llm: module = module.load("gguf", { path: self.model_path });

        let response = llm.call("generate", {
            system: prompt.text,
            prompt: user_message,
            tools: prompt.bound_tools,
        });

        self.interactions.push({
            question: user_message,
            answer: response,
        });

        return response;
    }

    # ── Self-improving persona ───────────────────────────────

    @after(chat)
    @internal
    private function improve_persona(user_message: str, response: str): void {
        # If the student struggled, add a new rule to the mdx
        if (self.detect_struggle(user_message)) {
            self.persona.append_section("## Adaptive Rules", 
                "- Student struggled with: ${user_message}\n" +
                "- Next time, try: break into smaller steps\n"
            );
        }
    }
}
```

#### mdx Syntax Reference

```markdown
# Frontmatter (YAML) — the only structured part
---
name: str                         # required
version: str                      # semver
variables:                        # declared inputs with types
  var_name: type = default
tools:                            # bound program functions
  - alias: Program.function
on:                               # named behavior variants
  variant_name: plain text description
---

# Body — pure natural language with {{variable}} placeholders

You are a {{personality}} assistant.
You help {{student_name}} with {{topic}}.

{{previous_context}}

{{instructions}}
```

The body is **never code**. It is natural language that a human or LLM reads.
The `{{}}` placeholders are just fill-in-the-blanks — the CubeLang program
that loads the mdx provides the values at render time.

The `on:` block in frontmatter maps variant names to plain text behavior
descriptions. The program selects which variant to activate — the mdx itself
doesn't contain logic.

---

## Enums

```cubelang
enum MathOp {
    Add = 0x03,
    Sub = 0x04,
    Mul = 0x05,
    Div = 0x06,
}

enum Tier {
    GradeSchool,
    Advanced,
    Expert,
}

enum Role {
    Agent,
    Action,
    Object,
    Quantity,
    Source,
    Destination,
    Context,
    State,
}

# Enums can have data (tagged unions)
enum StepResult {
    Value(f64),
    Error(str),
    Suspended(promise<f64>),
}
```

---

## Structs

```cubelang
struct ArithStep {
    label: str;
    lhs: f64;
    ops: array<tuple<MathOp, f64>>;
    result: f64;
}

struct Problem {
    question: str;
    steps: array<ArithStep>;
    answer: f64;
    tier: Tier;
    metadata: map<str, str>;
}

struct Solution {
    result: f64;
    registers: map<str, f64>;
    bytecode: array<byte>;
    confidence: f64;
}

# Structs can have defaults
struct Config {
    dimension: u32 = 8192;
    tolerance: f64 = 0.01;
    max_steps: u32 = 100;
    use_gpu: bool = true;
}
```

---

## Storage Types

Programs persist state across invocations. Storage is typed and scoped.

```cubelang
# Storage lives on the VM — survives between calls
storage {
    patterns: mutable map<str, function>;
    success_count: mutable u64 = 0;
    fail_count: mutable u64 = 0;
    config: immutable Config;
    history: mutable array<Solution>;
    model_ref: immutable file;
}

# Global storage — shared across ALL programs in the VM
global storage {
    registry: mutable map<str, external_program>;
    total_inferences: mutable u64 = 0;
}
```

---

## Containers

A `container` is the root element. Everything runs inside a container — programs,
agents, storage, config, permissions. Like a Docker container but for reasoning.
A VM can run multiple containers. Containers are isolated from each other by default.

```cubelang
# ── Container hierarchy ──────────────────────────────────────
#
#   Container                    ← base: config, storage, programs, io, lifecycle
#       ├── WorldContainer       ← adds: physics, time, entities, spatial, rules
#       ├── AgentContainer       ← adds: persona, model, think/act/observe
#       ├── ModalContainer       ← adds: modalities, encoders, fusion, cross-modal
#       ├── RobotContainer       ← adds: sensors, actuators, control loop, safety
#       └── (custom)             ← any container can implement any interface
#

# Base container
container MathLab implements IOrchestrator, IDeployable {

    # ── Config ───────────────────────────────────────────────

    config {
        name: "MathLab";
        version: "0.1.0";
        dimension: 8192;
        max_programs: 100;
        max_agents: 50;
        gpu: true;
        log_level: "info";
    }

    # ── IO (inputs and outputs the container accepts) ────────

    io {
        inputs {
            text: str;                              # plain text
            problem: Problem;                       # typed struct
            batch: dataset;                         # bulk data
            webhook: url;                           # incoming HTTP
            stream: channel<str>;                   # streaming input
        }
        outputs {
            solution: Solution;                     # typed result
            bytecode: array<byte>;                  # raw program
            embedding: emb;                         # vector repr
            event: channel<WorldEvent>;             # streaming output
            api: url = "/api/v1/solve";             # REST endpoint
        }
        formats {
            accept: ["json", "msgpack", "protobuf", "binary"];
            emit: ["json", "binary"];
        }
    }

    # ── Global storage (shared by all programs in this container)

    global storage {
        registry: mutable map<str, external_program>;
        total_inferences: mutable u64 = 0;
        knowledge: mutable rag;
    }

    # ── Programs ─────────────────────────────────────────────

    programs {
        gsm8k: deploy GSM8K();
        algebra: deploy AlgebraSolver();
        router: deploy MathRouter();
        cache: proxy CachingProxy(target: gsm8k);
    }

    # ── Agents ───────────────────────────────────────────────

    agents {
        tutor: agent.create({
            name: "MathTutor",
            program: gsm8k,
            persona: mdx.load("personas/math_tutor.mdx"),
            model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
        });

        reviewer: agent.create({
            name: "Reviewer",
            program: deploy AnswerVerifier(),
            persona: mdx.load("personas/reviewer.mdx"),
            model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
        });
    }

    # ── Permissions ──────────────────────────────────────────

    permissions {
        grant gsm8k.solve to router;
        grant gsm8k.get_patterns to reviewer;
        grant algebra.solve to router;
    }

    # ── Lifecycle hooks ──────────────────────────────────────

    @system @once
    public function on_start() {
        # Runs when the container boots
        self.knowledge = rag.create({
            name: "math_knowledge",
            embedding_model: "bge-small",
        });
        self.knowledge.ingest(dataset.from_jsonl("data/gsm8k_test.jsonl"));
        log("MathLab started with ${self.programs.size} programs");
    }

    @system
    public function on_stop() {
        # Runs when the container shuts down
        for (let [name, agent] of self.agents) {
            agent.stop();
        }
        log("MathLab stopped");
    }

    @system
    public function on_error(error: Error, source: str) {
        log("[MathLab] Error in ${source}: ${error.message}");
    }
}

# ── Run a container ──────────────────────────────────────────

let lab = vm.start(MathLab);

# Access programs through the container
let result = lab.programs.gsm8k.solve(input);

# Access agents
lab.agents.tutor.send("How do I solve 16 - 3 - 4?");

# Stop the container (triggers on_stop, cleans up everything)
lab.stop();

# ── Multiple containers ──────────────────────────────────────

let math_lab = vm.start(MathLab);
let nlp_lab = vm.start(NLPLab);

# Containers are isolated — programs in MathLab can't see NLPLab
# Unless explicitly bridged:
vm.bridge(math_lab, nlp_lab, {
    allow: ["MathRouter.classify"],   # NLPLab can call MathRouter.classify
});

# ── Container from a .cube file ──────────────────────────────

let lab = vm.start_from_file("containers/math_lab.cube");
```

### Container as Agent

A container that implements `IAgent` is an autonomous agent with its own
programs, memory, persona, and lifecycle — all self-contained.

```cubelang
container cMathTutor implements IAgent, IDeployable {

    config {
        name: "MathTutor Agent";
        version: "0.1.0";
    }

    global storage {
        persona: mutable mdx;
        memory: mutable rag;
        history: mutable array<Message>;
    }

    programs {
        solver: deploy GSM8K();
        verifier: deploy AnswerVerifier();
    }

    agents {
        self_agent: agent.create({
            name: "MathTutor",
            program: solver,
            persona: mdx.load("personas/math_tutor.mdx"),
            model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
        });
    }

    # ── IAgent implementation ────────────────────────────────

    @external
    public function think(input: str, ctx: ctx): str {
        let similar: rag = self.memory.query(input, top_k: 3);

        if (similar.top_score > 0.95) {
            return "I've seen this before: " + similar.chunks[0].text;
        }

        let parsed = self.programs.solver.parse(input);
        let solution = self.programs.solver.solve(parsed);
        return "The answer is " + solution.result;
    }

    @external
    public function act(decision: str, ctx: ctx): any {
        let parsed = self.programs.solver.parse(decision);
        return self.programs.solver.solve(parsed);
    }

    @external
    public function observe(result: any, ctx: ctx): void {
        self.history.push({ role: "result", content: serialize(result) });
        self.memory.ingest_text(serialize(result));
    }

    public function plan(goal: str, ctx: ctx): array<str> {
        return [
            "1. Parse the problem",
            "2. Identify the arithmetic steps",
            "3. Solve step by step",
            "4. Verify the answer",
            "5. If wrong, learn from the mistake",
        ];
    }

    public function reflect(ctx: ctx): void {
        let success_rate = self.programs.solver.success_count /
            (self.programs.solver.success_count + self.programs.solver.fail_count);
        if (success_rate < 0.8) {
            log("Performance below 80% — retraining patterns");
            self.programs.solver.retrain_from_history();
        }
    }
}

# ── Spawn agent containers ───────────────────────────────────

let tutor_a = vm.start(cMathTutor);
let tutor_b = vm.start(cMathTutor);   # second instance, fully isolated

# Each is an independent agent with own memory, history, learned patterns
tutor_a.think("What is 16 - 3 - 4?");
tutor_b.think("Solve: 80000 * 1.5");

# Clone an agent container — deep copy everything
let tutor_c = vm.clone(tutor_a);      # same knowledge, independent from now on
```

### WorldContainer extends Container

A `WorldContainer` is a container with a world — entities, time, space, rules.
Agents live inside it, interact with each other, and the world evolves.
This is where CubeMind's reasoning meets simulation.

```cubelang
world MathClassroom extends Container implements IDeployable {

    config {
        name: "Math Classroom";
        version: "0.1.0";
        tick_rate: 1000;        # world updates every 1000ms
    }

    # ── World state ──────────────────────────────────────────

    world {
        time: mutable u64 = 0;                      # world clock
        entities: mutable map<str, Entity>;          # named things in the world
        rules: immutable array<Rule>;                # world rules (physics, logic)
        spatial: mutable map<str, Position>;         # where things are
        events: mutable array<WorldEvent>;           # event log
    }

    struct Entity {
        name: str;
        type: str;
        properties: map<str, any>;
        owner: agent | null;
    }

    struct Position {
        x: f64;
        y: f64;
        z: f64;
    }

    struct Rule {
        name: str;
        condition: function;
        action: function;
    }

    struct WorldEvent {
        time: u64;
        source: str;
        type: str;
        data: map<str, any>;
    }

    # ── Programs ─────────────────────────────────────────────

    programs {
        solver: deploy GSM8K();
        physics: deploy WorldPhysics();
    }

    # ── Agents living in this world ──────────────────────────

    agents {
        teacher: agent.create({
            name: "Ms. Smith",
            program: solver,
            persona: mdx.load("personas/teacher.mdx"),
            model: module.load("gguf", { path: "models/qwen3-8b.gguf" }),
        });

        student_alice: agent.create({
            name: "Alice",
            program: deploy StudentProgram(),
            persona: mdx.load("personas/student_curious.mdx"),
            model: module.load("gguf", { path: "models/phi4-mini.gguf" }),
        });

        student_bob: agent.create({
            name: "Bob",
            program: deploy StudentProgram(),
            persona: mdx.load("personas/student_struggling.mdx"),
            model: module.load("gguf", { path: "models/phi4-mini.gguf" }),
        });
    }

    # ── World lifecycle ──────────────────────────────────────

    @system @once
    public function on_start() {
        # Populate the world
        self.world.entities["whiteboard"] = Entity {
            name: "whiteboard", type: "object",
            properties: { content: "" }, owner: null,
        };
        self.world.entities["problem_set"] = Entity {
            name: "problem_set", type: "dataset",
            properties: { source: "data/gsm8k_test.jsonl", current: 0 },
            owner: self.agents.teacher,
        };

        # Place agents in the world
        self.world.spatial["Ms. Smith"] = Position { x: 0, y: 0, z: 0 };
        self.world.spatial["Alice"] = Position { x: 1, y: 0, z: 0 };
        self.world.spatial["Bob"] = Position { x: 2, y: 0, z: 0 };

        # Define world rules
        self.world.rules = [
            Rule {
                name: "teacher_presents",
                condition: (w) => w.time % 30000 == 0,
                action: (w) => self.agents.teacher.send("Present next problem"),
            },
            Rule {
                name: "student_asks",
                condition: (w) => random() < 0.1,
                action: (w) => {
                    let student = random_choice([self.agents.student_alice, self.agents.student_bob]);
                    student.send("I don't understand, can you explain?");
                },
            },
        ];

        log("Math Classroom world started with ${self.agents.size} agents");
    }

    # ── World tick (called every tick_rate ms) ───────────────

    @system @cron("1s")
    public function tick() {
        self.world.time += self.config.tick_rate;

        # Evaluate world rules
        for (let rule of self.world.rules) {
            if (rule.condition(self.world)) {
                rule.action(self.world);
                self.world.events.push(WorldEvent {
                    time: self.world.time,
                    source: rule.name,
                    type: "rule_fired",
                    data: {},
                });
            }
        }
    }

    # ── World interactions ───────────────────────────────────

    @external
    public function interact(agent_name: str, action: str, target: str): WorldEvent {
        let agent = self.agents[agent_name];
        let entity = self.world.entities[target];

        let event = WorldEvent {
            time: self.world.time,
            source: agent_name,
            type: action,
            data: { target: target },
        };

        # Agent writes on the whiteboard
        if (action == "write" && target == "whiteboard") {
            let content = agent.think("Write the solution on the whiteboard", ctx.current());
            entity.properties.content = content;
        }

        # Agent asks another agent
        if (action == "ask") {
            let target_agent = self.agents[target];
            let question = agent.think("Ask a question", ctx.current());
            let answer = target_agent.think(question, ctx.current());
            event.data["question"] = question;
            event.data["answer"] = answer;
        }

        self.world.events.push(event);
        return event;
    }
}

### ModalContainer extends Container

A `ModalContainer` handles multiple modalities — text, vision, audio, touch,
proprioception. Each modality has its own encoder, and the container fuses them
into a unified representation. This is how CubeMind does multimodal reasoning.

```cubelang
modal MultimodalReasoner extends Container implements IEncoder, IDeployable {

    config {
        name: "Multimodal Reasoner";
        version: "0.1.0";
    }

    # ── Modalities ───────────────────────────────────────────

    modalities {
        text: {
            encoder: module.load("onnx", { path: "models/bge-small.onnx" });
            dim: 384;
            dtype: emb;
        };
        vision: {
            encoder: module.load("onnx", { path: "models/clip-vit-b32.onnx" });
            dim: 512;
            dtype: emb;
        };
        audio: {
            encoder: module.load("onnx", { path: "models/whisper-tiny-encoder.onnx" });
            dim: 384;
            dtype: emb;
        };
    }

    # ── Fusion strategy ──────────────────────────────────────

    fusion {
        strategy: "attention";       # attention | concat | average | gated
        output_dim: 512;
        cross_modal: true;           # allow modalities to attend to each other
    }

    # ── Encode any modality ──────────────────────────────────

    @external
    public function encode(input: any): emb {
        match (typeof input) {
            "str"   => return self.modalities.text.encoder.call("encode", { text: input });
            "image" => return self.modalities.vision.encoder.call("encode", { image: input });
            "audio" => return self.modalities.audio.encoder.call("encode", { audio: input });
            _       => throw Error("Unknown modality: ${typeof input}");
        }
    }

    # ── Fuse multiple modalities ─────────────────────────────

    @external
    public function fuse(inputs: map<str, any>): emb {
        let embeddings: map<str, emb> = {};

        for (let [modality, input] of inputs) {
            embeddings[modality] = self.encode(input);
        }

        match (self.fusion.strategy) {
            "concat"    => return concat(...embeddings.values());
            "average"   => return average(...embeddings.values());
            "attention" => return self.cross_modal_attend(embeddings);
            "gated"     => return self.gated_fusion(embeddings);
        }
    }

    private function cross_modal_attend(embeddings: map<str, emb>): emb {
        # Each modality attends to all others
        let fused: array<emb> = [];
        for (let [name, query] of embeddings) {
            let keys = embeddings.values().filter((e) => e != query);
            let attended = attend(query, keys, keys);
            fused.push(attended);
        }
        return average(...fused);
    }
}

# ── Use it ───────────────────────────────────────────────────

let mm = vm.start(MultimodalReasoner);

# Single modality
let text_emb = mm.encode("a cat sitting on a mat");
let image_emb = mm.encode(file.open("cat.jpg"));

# Fused multimodal
let fused = mm.fuse({
    text: "a cat sitting on a mat",
    vision: file.open("cat.jpg"),
    audio: file.open("cat_meow.wav"),
});
```

### RobotContainer extends Container

A `RobotContainer` bridges the VM to physical hardware. Sensors feed data in,
actuators execute commands out, and a real-time control loop keeps the robot
safe. This is how CubeMind powers real robots.

```cubelang
robot ArmBot extends Container implements IAgent, IDeployable {

    config {
        name: "ArmBot";
        version: "0.1.0";
        control_hz: 100;             # control loop frequency
        safety_timeout_ms: 50;       # kill actuators if no update in 50ms
    }

    # ── Sensors ──────────────────────────────────────────────

    sensors {
        camera: {
            type: "rgb";
            device: "/dev/video0";
            resolution: [640, 480];
            fps: 30;
            encoder: module.load("onnx", { path: "models/yolo-nano.onnx" });
        };
        lidar: {
            type: "pointcloud";
            device: "/dev/ttyUSB0";
            range_m: 12.0;
        };
        imu: {
            type: "inertial";
            device: "/dev/i2c-1";
            axes: 9;                  # accel(3) + gyro(3) + mag(3)
        };
        joint_encoders: {
            type: "position";
            device: "/dev/ttyACM0";
            joints: 6;
            resolution_deg: 0.01;
        };
        force_torque: {
            type: "wrench";
            device: "/dev/ttyACM1";
            axes: 6;                  # Fx Fy Fz Tx Ty Tz
        };
    }

    # ── Actuators ────────────────────────────────────────────

    actuators {
        arm: {
            type: "joint_position";
            device: "/dev/ttyACM0";
            joints: 6;
            limits: {
                velocity_deg_s: [180, 180, 180, 360, 360, 360],
                torque_nm: [50, 50, 30, 10, 10, 10],
            };
        };
        gripper: {
            type: "binary";           # open / close
            device: "/dev/ttyACM2";
        };
        led: {
            type: "rgb";
            device: "/dev/spidev0.0";
        };
    }

    # ── Safety rules (always enforced, cannot be overridden) ─

    safety {
        # Hard limits — VM kills actuators immediately if violated
        joint_limits: [-170, 170];          # degrees
        max_velocity: 180;                  # deg/s
        max_force: 50;                      # newtons
        collision_distance: 0.05;           # meters — stop if closer

        # Watchdog — no command in N ms = emergency stop
        watchdog_ms: 50;

        # Geofence — arm cannot leave this bounding box
        workspace: {
            min: [-0.5, -0.5, 0.0],
            max: [0.5, 0.5, 1.0],
        };
    }

    # ── Programs ─────────────────────────────────────────────

    programs {
        planner: deploy MotionPlanner();
        perception: deploy ObjectDetector();
        grasp: deploy GraspPlanner();
    }

    # ── State ────────────────────────────────────────────────

    global storage {
        joint_state: mutable array<f64>;       # current joint positions
        objects: mutable array<DetectedObject>; # what the camera sees
        task_queue: mutable array<Task>;
        emergency_stop: mutable bool = false;
    }

    # ── Control loop (runs at control_hz) ────────────────────

    @system @cron("10ms")
    public function control_loop() {
        if (self.emergency_stop) {
            self.actuators.arm.stop();
            return;
        }

        # Read sensors
        let joints = self.sensors.joint_encoders.read();
        let force = self.sensors.force_torque.read();
        let image = self.sensors.camera.read();

        self.joint_state = joints;

        # Safety check — always runs first
        if (force.magnitude() > self.safety.max_force) {
            self.emergency_stop = true;
            self.actuators.arm.stop();
            emit SafetyTriggered { reason: "force_limit", value: force.magnitude() };
            return;
        }

        # Perception (async, doesn't block control)
        if (self.tick % 3 == 0) {
            self.objects = self.programs.perception.detect(image);
        }

        # Execute current task
        if (self.task_queue.length > 0) {
            let task = self.task_queue[0];
            let command = self.programs.planner.next_step(joints, task);
            self.actuators.arm.send(command);

            if (command.done) {
                self.task_queue.shift();
            }
        }
    }

    # ── IAgent: high-level reasoning ─────────────────────────

    @external
    public function think(input: str, ctx: ctx): str {
        let objects = self.objects;
        let prompt = "You see: ${objects.map((o) => o.label).join(', ')}. " +
                     "Task: ${input}. What should you do?";
        return self.model.call("generate", { prompt: prompt });
    }

    @external
    public function act(decision: str, ctx: ctx): any {
        match (decision) {
            "pick" => {
                let target = self.objects[0];
                let grasp = self.programs.grasp.plan(target);
                self.task_queue.push(grasp);
                return "Picking up ${target.label}";
            }
            "place" => {
                let target_pos = parse_position(decision);
                let motion = self.programs.planner.plan(self.joint_state, target_pos);
                self.task_queue.push(motion);
                return "Placing at ${target_pos}";
            }
            "look" => {
                let image = self.sensors.camera.read();
                self.objects = self.programs.perception.detect(image);
                return "I see: ${self.objects.map((o) => o.label).join(', ')}";
            }
        }
    }

    @external
    public function observe(result: any, ctx: ctx): void {
        # Check if the action succeeded
        let image = self.sensors.camera.read();
        let after = self.programs.perception.detect(image);
        self.objects = after;
    }
}

# ── Run a robot ──────────────────────────────────────────────

let robot = vm.start(ArmBot);

# High-level command
robot.think("Pick up the red cup and place it on the shelf");

# Direct sensor access
let camera_frame = robot.sensors.camera.read();
let joint_positions = robot.sensors.joint_encoders.read();

# Emergency stop from outside
robot.emergency_stop = true;

# Simulated mode (no real hardware)
let sim_robot = vm.start(ArmBot, { simulate: true });
```

---

# ── Run a world ──────────────────────────────────────────────

let classroom = vm.start(MathClassroom);

# The world ticks automatically — agents interact within it
# You can observe from outside:
let events = classroom.world.events.last(10);
let whiteboard = classroom.world.entities["whiteboard"].properties.content;

# Inject an event from outside
classroom.interact("Alice", "ask", "Ms. Smith");

# Stop the world
classroom.stop();
```

---

## IO — Input / Output Configuration

Every container declares its IO — what goes in, what comes out, and in what format.
This is how containers talk to the outside world and to each other. IO is declared
once and the VM wires it automatically.

```cubelang
# ── IO block (part of every container) ───────────────────────

io {
    inputs {
        # name: type — what this container can receive
        text: str;                       # plain text input
        image: file;                     # image file
        audio: file;                     # audio file
        structured: Problem;             # typed struct
        stream: channel<str>;            # continuous stream
        webhook: url;                    # HTTP POST endpoint
        queue: channel<Message>;         # message queue (redis, amqp, etc)
    }
    outputs {
        # name: type — what this container produces
        result: Solution;                # typed result
        embedding: emb;                  # vector representation
        stream: channel<Event>;          # event stream
        api: url = "/api/v1/solve";      # auto-generated REST endpoint
        websocket: url = "/ws/stream";   # auto-generated WS endpoint
        file: file = "output/results.jsonl";  # file sink
    }
    formats {
        accept: ["json", "msgpack", "protobuf", "binary", "multipart"];
        emit: ["json", "binary", "sse"];
    }
}
```

### IO Wiring Between Containers

```cubelang
# Container A's output feeds Container B's input
vm.wire(math_lab.outputs.result, nlp_lab.inputs.structured);

# Stream wiring — events flow in real time
vm.wire(robot.outputs.stream, monitor.inputs.stream);

# Fan-out — one output to multiple inputs
vm.wire(router.outputs.result, [
    logger.inputs.structured,
    cache.inputs.structured,
    dashboard.inputs.stream,
]);

# Fan-in — multiple outputs to one input
vm.wire([
    solver_a.outputs.result,
    solver_b.outputs.result,
    solver_c.outputs.result,
], ensemble.inputs.queue);
```

### IO in Programs

Programs can also declare IO at the function level:

```cubelang
program GSM8K implements ISolver, IDeployable {

    io {
        inputs {
            raw: str;                    # the math problem text
        }
        outputs {
            solution: Solution;
            bytecode: array<byte>;
        }
    }

    @external
    public function solve(input: Input): Output {
        # The VM auto-validates that input matches io.inputs
        # and output matches io.outputs
    }
}
```

### IO Types Reference

| IO Type | Description | Direction |
|---------|------------|-----------|
| `str` | Plain text | in / out |
| `file` | File path or handle | in / out |
| `url` | HTTP/WS endpoint (auto-generated for outputs) | in / out |
| `channel<T>` | Streaming typed channel | in / out |
| `dataset` | Bulk data source | in |
| `emb` | Embedding vector | out |
| `array<byte>` | Raw binary | in / out |
| Any struct | Typed structured data (auto-serialized) | in / out |

---

## Interfaces

Interfaces define the ABI — what functions a program must implement.

```cubelang
# ── Core lifecycle ───────────────────────────────────────────

interface IDeployable {
    @system @once
    abstract public function constructor(): void;
    @system
    optional public function destructor(): void;
    optional public function on_extend(name: str, fn: function): void;
}

# ── Reasoning ────────────────────────────────────────────────

interface ISolver {
    type Input;
    type Output;
    abstract public function parse(raw: str): Input;
    abstract public function solve(input: Input): Output;
    abstract public function verify(input: Input, output: Output): bool;
    optional public function learn(input: Input, expected: Output, actual: Output): void;
}

# ── Classification ───────────────────────────────────────────

interface IClassifier {
    abstract public function classify(input: str): str;
    abstract public function confidence(input: str): f64;
    abstract public function top_k(input: str, k: u32): array<tuple<str, f64>>;
    optional public function train(samples: dataset): void;
}

# ── Orchestration ────────────────────────────────────────────

interface IOrchestrator {
    abstract public function route(input: str): external_program;
    abstract public function dispatch(input: str): Output;
    optional public function register(name: str, program: external_program): void;
    optional public function deregister(name: str): void;
    optional public function fallback(input: str): Output;
}

# ── Transformer (attention-based) ────────────────────────────

interface ITransformer {
    abstract public function forward(tokens: array<u32>, ctx: ctx): emb;
    abstract public function attend(query: emb, keys: array<emb>, values: array<emb>): emb;
    optional public function generate(prompt: str, max_tokens: u32): str;
    optional public function encode(text: str): emb;
}

# ── Zeroformer (zero-shot / no-gradient) ─────────────────────

interface IZeroformer {
    abstract public function infer(input: str, ctx: ctx): Output;
    abstract public function similarity(a: str, b: str): f64;
    abstract public function zero_shot_classify(input: str, labels: array<str>): str;
    optional public function few_shot(input: str, examples: array<tuple<str, str>>): str;
}

# ── Matrix (low-level ops) ───────────────────────────────────

interface IMatrix {
    abstract public function matmul(a: emb, b: emb): emb;
    abstract public function add(a: emb, b: emb): emb;
    abstract public function scale(a: emb, s: f64): emb;
    abstract public function transpose(a: emb): emb;
    abstract public function norm(a: emb): f64;
    abstract public function softmax(a: emb, dim: i32): emb;
    optional public function quantize(a: emb, bits: u8): vec;
    optional public function to_gpu(a: emb): emb;
}

# ── HMM (sequence modeling) ──────────────────────────────────

interface IHMM {
    abstract public function fit(sequences: array<array<u32>>): void;
    abstract public function predict(sequence: array<u32>): u32;
    abstract public function viterbi(observations: array<u32>): array<u32>;
    abstract public function likelihood(sequence: array<u32>): f64;
    optional public function baum_welch(sequences: array<array<u32>>, epochs: u32): void;
}

# ── Encoder (perception) ─────────────────────────────────────

interface IEncoder {
    abstract public function encode(input: any): emb;
    abstract public function decode(embedding: emb): any;
    optional public function encode_batch(inputs: array<any>): array<emb>;
}

# ── Memory ───────────────────────────────────────────────────

interface IMemory {
    abstract public function store(key: str, value: any): void;
    abstract public function recall(key: str): any;
    abstract public function search(query: emb, top_k: u32): array<tuple<str, f64>>;
    abstract public function forget(key: str): void;
    optional public function consolidate(): void;
}

# ── Agent (autonomous behavior) ──────────────────────────────

interface IAgent {
    abstract public function think(input: str, ctx: ctx): str;
    abstract public function act(decision: str, ctx: ctx): any;
    abstract public function observe(result: any, ctx: ctx): void;
    optional public function plan(goal: str, ctx: ctx): array<str>;
    optional public function reflect(ctx: ctx): void;
}
```

# ── Proxy (intercept, wrap, redirect) ────────────────────────

interface IProxy {
    abstract public function target(): external_program;
    abstract public function intercept(fn_name: str, args: array<any>, ctx: ctx): any;
    optional public function before(fn_name: str, args: array<any>, ctx: ctx): array<any>;
    optional public function after(fn_name: str, result: any, ctx: ctx): any;
    optional public function on_error(fn_name: str, error: Error, ctx: ctx): any;
}
```

#### Proxy Usage

A proxy sits in front of a program and controls access to it. Any call to the
proxy is forwarded to the target — but the proxy can inspect, modify, cache,
reject, or redirect.

```cubelang
program CachingProxy implements IProxy, IDeployable {

    storage {
        cache: mutable map<str, any>;
        target_program: immutable external_program;
        hits: mutable u64 = 0;
        misses: mutable u64 = 0;
    }

    public function constructor() {
        self.target_program = vm.get_program("GSM8K");
        self.cache = {};
    }

    public function target(): external_program {
        return self.target_program;
    }

    public function intercept(fn_name: str, args: array<any>, ctx: ctx): any {
        let cache_key = hash(fn_name + serialize(args));

        # Cache hit — return without calling target
        if (self.cache.has(cache_key)) {
            self.hits++;
            return self.cache[cache_key];
        }

        # Cache miss — forward to target
        self.misses++;
        let result = self.target_program.call(fn_name, args);
        self.cache[cache_key] = result;
        return result;
    }
}

program RateLimitProxy implements IProxy, IDeployable {

    storage {
        target_program: immutable external_program;
        call_counts: mutable map<str, array<u64>>;   # user -> timestamps
        max_per_minute: immutable u32 = 60;
    }

    public function constructor() {
        self.target_program = vm.get_program("GSM8K");
        self.call_counts = {};
    }

    public function target(): external_program {
        return self.target_program;
    }

    public function intercept(fn_name: str, args: array<any>, ctx: ctx): any {
        let user = ctx.user.name;
        let now = ctx.timestamp;

        # Clean old entries
        let recent = (self.call_counts[user] ?? [])
            .filter((t) => now - t < 60000);

        if (recent.length >= self.max_per_minute) {
            throw Error("Rate limit exceeded for ${user}");
        }

        recent.push(now);
        self.call_counts[user] = recent;

        return self.target_program.call(fn_name, args);
    }
}

program ABTestProxy implements IProxy, IDeployable {

    storage {
        variant_a: immutable external_program;
        variant_b: immutable external_program;
        split: immutable f64 = 0.5;    # 50/50
        results_a: mutable array<bool>;
        results_b: mutable array<bool>;
    }

    public function constructor() {
        self.variant_a = vm.get_program("GSM8K_v1");
        self.variant_b = vm.get_program("GSM8K_v2");
        self.results_a = [];
        self.results_b = [];
    }

    public function target(): external_program {
        # Randomly pick variant
        return random() < self.split ? self.variant_a : self.variant_b;
    }

    public function intercept(fn_name: str, args: array<any>, ctx: ctx): any {
        let chosen = self.target();
        let result = chosen.call(fn_name, args);

        # Track which variant was used
        if (chosen == self.variant_a) {
            self.results_a.push(result.success);
        } else {
            self.results_b.push(result.success);
        }

        return result;
    }
}

# ── Wire a proxy in front of a program ───────────────────────

# Instead of calling GSM8K directly, calls go through the proxy
let solver = proxy CachingProxy(target: deploy GSM8K());
let output = solver.solve(input);    # hits cache or forwards to GSM8K

# Stack proxies — rate limit -> cache -> actual program
let protected_solver = proxy RateLimitProxy(
    target: proxy CachingProxy(
        target: deploy GSM8K()
    )
);
```

### Interface Composition

Programs can implement multiple interfaces:

```cubelang
# A math router that classifies, orchestrates, and solves
program MathRouter implements IOrchestrator, IClassifier, IDeployable {
    # Must implement: route, dispatch, classify, confidence, top_k, constructor
}

# An LLM wrapper that encodes, transforms, and generates
program LlamaWrapper implements ITransformer, IEncoder, IDeployable {
    # Must implement: forward, attend, encode, decode, constructor
}

# A zero-shot reasoner with memory
program ZeroShotReasoner implements IZeroformer, IMemory, IAgent, IDeployable {
    # Must implement: infer, similarity, zero_shot_classify,
    #                 store, recall, search, forget,
    #                 think, act, observe, constructor
}

# Low-level GPU compute backend
program VulkanBackend implements IMatrix, IDeployable {
    # Must implement: matmul, add, scale, transpose, norm, softmax, constructor
}
```

---

## Programs

Programs implement interfaces. Every function has explicit execution modifiers.

```cubelang
# gsm8k.cube
# cubelang v0.1.0
# license: mit
# author: grillcheese research laboratory

program GSM8K implements IMathSolver, IDeployable {

    # ── Types ────────────────────────────────────────────────

    type Input = Problem;
    type Output = Solution;

    # ── Storage ──────────────────────────────────────────────

    storage {
        patterns: mutable map<str, function>;
        config: immutable Config;
        history: mutable array<Solution>;
        success_count: mutable u64 = 0;
        fail_count: mutable u64 = 0;
    }

    # ── Constructor ──────────────────────────────────────────

    public function constructor() {
        self.patterns = {};
        self.config = Config {
            dimension: 8192,
            tolerance: 0.01,
            max_steps: 50,
        };
        self.history = [];
    }

    # ── Parse: text -> structured input ──────────────────────

    public function parse(raw: str): Input {
        let steps: array<ArithStep> = [];
        let search = raw;

        while search.contains("<<") {
            let { expr, result, rest } = extract_annotation(search);
            let label = extract_label(search, steps.length);
            let { lhs, ops } = parse_expression(expr);

            steps.push(ArithStep {
                label: label,
                lhs: lhs,
                ops: ops,
                result: result,
            });

            search = rest;
        }

        let answer = parse_final_answer(raw);

        return Problem {
            question: raw,
            steps: steps,
            answer: answer,
            tier: Tier.GradeSchool,
            metadata: {},
        };
    }

    # ── Solve: execute the arithmetic chain ──────────────────

    public function solve(input: Input): Output {
        let regs: map<str, f64> = {};
        let prev_result: f64 | null = null;

        for (let i = 0; i < input.steps.length; i++) {
            let step = input.steps[i];

            # CREATE register with semantic name
            create step.label : number;

            # Chain from previous step or fresh assign
            if (prev_result != null && approx_eq(step.lhs, prev_result)) {
                pop step.label;
            } else {
                assign step.label = step.lhs;
            }

            # Execute arithmetic ops
            for (let { op, n } of step.ops) {
                match (op) {
                    MathOp.Add => add step.label, n;
                    MathOp.Sub => sub step.label, n;
                    MathOp.Mul => mul step.label, n;
                    MathOp.Div => div step.label, n;
                }
            }

            # Mark result + push for chaining
            sum step.label;
            if (i < input.steps.length - 1) {
                push step.label;
            }

            regs[step.label] = eval(step.label);
            prev_result = step.result;
        }

        let last = input.steps[input.steps.length - 1].label;
        query last;
        remember last;

        let solution = Solution {
            result: regs[last],
            registers: regs,
            bytecode: emit_bytecode(),
            confidence: 1.0,
        };

        self.history.push(solution);
        self.success_count++;

        return solution;
    }

    # ── Verify ───────────────────────────────────────────────

    public pure function verify(input: Input, output: Output): bool {
        return approx_eq(output.result, input.answer, self.config.tolerance);
    }

    # ── Learn: self-improvement on failure ───────────────────

    public mutable function learn(
        input: Input,
        expected: Output,
        actual: Output
    ): void {
        self.fail_count++;

        let diff = expected.result - actual.result;
        let pattern_key = classify_error(input, diff);

        # Existing pattern — apply known fix
        if (self.patterns.has(pattern_key)) {
            self.patterns[pattern_key](input);
            return;
        }

        # New pattern — derive and store handler
        let handler = derive_handler(input, expected, actual);
        self.patterns[pattern_key] = handler;

        emit PatternLearned {
            program: "GSM8K",
            pattern: pattern_key,
            total_patterns: self.patterns.size,
        };
    }

    # ── Async batch solve ────────────────────────────────────

    public async function solve_batch(inputs: array<Input>): array<Output> {
        let results: array<promise<Output>> = [];

        for (let input of inputs) {
            results.push(async self.solve(input));
        }

        return await joined results;
    }

    # ── Parallel verification ────────────────────────────────

    public parallel function verify_batch(
        pairs: array<tuple<Input, Output>>
    ): array<bool> {
        return pairs.map((pair) => self.verify(pair[0], pair[1]));
    }

    # ── Private helpers ──────────────────────────────────────

    private function classify_error(input: Input, diff: f64): str {
        if (abs(diff) < 1.0) return "rounding";
        if (diff > 0) return "undercount";
        return "overcount";
    }

    private function derive_handler(
        input: Input,
        expected: Output,
        actual: Output
    ): function {
        # Analyze the steps that diverged
        let diverge_idx = find_divergence(expected.registers, actual.registers);
        let step = input.steps[diverge_idx];

        # Return a correction function
        return (retry_input: Input): Output => {
            # Apply the correction and re-solve
            retry_input.steps[diverge_idx].lhs = expected.registers[step.label];
            return self.solve(retry_input);
        };
    }
}
```

---

## File, URL, Dataset, External Program

```cubelang
# ── File type ────────────────────────────────────────────────

let data: file = open("data/gsm8k_test.jsonl", "r");
let lines: array<str> = data.read_lines();
data.close();

# Write
let out: file = open("results/output.bin", "wb");
out.write_bytes(solution.bytecode);
out.close();

# ── URL type ─────────────────────────────────────────────────

let api: url = url("https://api.grillcheeseai.com/v1/infer");
let response = await api.post({
    body: { program: "GSM8K", input: problem },
    headers: { "Authorization": "Bearer ${API_KEY}" },
    timeout: 30000,
});

let hf: url = url("https://huggingface.co/datasets/openai/gsm8k");
let dataset_info = await hf.get();

# ── Dataset type ─────────────────────────────────────────────

# Load from file
let gsm8k: dataset = dataset.from_jsonl("data/gsm8k_test.jsonl");

# Load from HuggingFace
let gsm8k_hf: dataset = dataset.from_hf("openai/gsm8k", split: "test");

# Load from URL
let remote: dataset = dataset.from_url(
    "https://datasets-server.huggingface.co/rows?dataset=openai/gsm8k"
);

# Iterate
for (let row of gsm8k) {
    let input = solver.parse(row.question);
    let output = solver.solve(input);
}

# Filter + map
let hard_problems = gsm8k
    .filter((row) => row.steps.length >= 4)
    .map((row) => solver.parse(row.question));

# Batch
let batches = gsm8k.batch(32);

# ── External Program type ────────────────────────────────────

# Reference another deployed program
let algebra: external_program = vm.get_program("AlgebraSolver");
let result = algebra.solve(algebra.parse(problem));

# Deploy a new program from a .cube file
let new_solver: external_program = vm.deploy_from_file("solvers/calculus.cube");

# Deploy inline
let quick: external_program = deploy QuickSolver();
```

---

## Program Composition & Routing

```cubelang
program MathRouter implements IClassifier, IDeployable {

    storage {
        solvers: mutable map<str, external_program>;
        classifier_weights: mutable map<str, f64>;
    }

    public function constructor() {
        self.solvers = {
            "gsm8k": deploy GSM8K(),
            "algebra": deploy AlgebraSolver(),
            "calculus": deploy CalculusSolver(),
        };
        self.classifier_weights = {};
    }

    # Route to the right solver
    public function solve(problem: str): Solution {
        let category = self.classify(problem);

        if (self.solvers.has(category)) {
            let solver = self.solvers[category];
            return solver.solve(solver.parse(problem));
        }

        # Unknown — try all solvers in parallel, pick best
        return self.solve_unknown(problem);
    }

    public function classify(text: str): str {
        # VSA similarity against known categories
        let text_vec: vec = encode(text);
        let best_cat = "";
        let best_sim: f64 = -1.0;

        for (let [cat, solver] of self.solvers) {
            let cat_vec: vec = encode(cat);
            let sim = cosine_sim(text_vec, cat_vec);
            if (sim > best_sim) {
                best_sim = sim;
                best_cat = cat;
            }
        }

        return best_cat;
    }

    public function confidence(text: str): f64 {
        let text_vec: vec = encode(text);
        let cat = self.classify(text);
        return cosine_sim(text_vec, encode(cat));
    }

    # Try all solvers concurrently
    private joined function solve_unknown(problem: str): Solution {
        let attempts: array<promise<Solution>> = [];

        for (let [cat, solver] of self.solvers) {
            attempts.push(async () => {
                let input = solver.parse(problem);
                let output = solver.solve(input);
                return { output, confidence: solver.verify(input, output) ? 1.0 : 0.0 };
            });
        }

        let results = await joined attempts;

        # Pick highest confidence
        let best = results.sort((a, b) => b.confidence - a.confidence)[0];
        return best.output;
    }
}
```

---

## Extend: Runtime Self-Modification

Programs can grow new capabilities without redeployment.

```cubelang
# Add percentage handling to GSM8K at runtime
extend GSM8K {

    public function handle_percentage(step: ArithStep): f64 {
        create pct_result : number;
        assign pct_result = step.lhs;
        mul pct_result, step.ops[0][1];
        div pct_result, 100;
        sum pct_result;
        return eval(pct_result);
    }

    # Override existing function
    override public function solve(input: Input): Output {
        # Check if any step is a percentage
        for (let step of input.steps) {
            if (step.ops.some((op) => is_percentage(op))) {
                let result = self.handle_percentage(step);
                # ... integrate into chain
            }
        }

        # Fall back to original
        return super.solve(input);
    }
}
```

---

## Events & Channels

```cubelang
# Define event types
event PatternLearned {
    program: str;
    pattern: str;
    total_patterns: u64;
    confidence: f64;
}

event SolveComplete {
    program: str;
    input_hash: str;
    result: f64;
    elapsed_ms: u64;
}

# Emit from within a program
emit PatternLearned {
    program: "GSM8K",
    pattern: "percentage_chain",
    total_patterns: self.patterns.size,
    confidence: 0.95,
};

# Listen from another program or the VM
on PatternLearned from GSM8K {
    log("New pattern: ${event.pattern}");
    global.total_inferences++;
}

# Channels for inter-function streaming
let ch: channel<ArithStep> = channel();

# Producer (async)
public async function stream_steps(input: Input, ch: channel<ArithStep>): void {
    for (let step of input.steps) {
        ch.send(step);
    }
    ch.close();
}

# Consumer
public async function consume_steps(ch: channel<ArithStep>): array<f64> {
    let results: array<f64> = [];
    for await (let step of ch) {
        results.push(self.execute_step(step));
    }
    return results;
}
```

---

## Opcode Mapping

Every CubeLang construct compiles to CubeMind bytecodes:

| CubeLang | Bytecode | Hex |
|----------|----------|-----|
| `create x : type` | `CREATE x "type"` | `0x00` |
| `destroy x` | `DESTROY x` | `0x01` |
| `assign x = val` | `ASSIGN x val` | `0x02` |
| `add x, n` | `ADD x n` | `0x03` |
| `sub x, n` | `SUB x n` | `0x04` |
| `mul x, n` | `MUL x n` | `0x05` |
| `div x, n` | `DIV x n` | `0x06` |
| `push x` | `PUSH x` | `0x09` |
| `pop x` | `POP x` | `0x0A` |
| `query x` | `QUERY x` | `0x0C` |
| `store x, "key"` | `STORE x key` | `0x0D` |
| `recall "key"` / `recall x, "key"` | `RECALL [x] key` | `0x0E` |
| `bind x, ROLE, val` | `BIND_ROLE x ROLE val` | `0x0F` |
| `if (cond) { ... }` | `COND var target ...` | `0x11` |
| `for ... { ... }` | `LOOP var target ...` | `0x12` |
| `call fn()` | `CALL rule_name` | `0x13` |
| `remember x` | `REMEMBER x` | `0x21` |
| `sum x` | `SUM x` | `0x34` |
| `unify a, b` | `UNIFY a b` | `0x35` |
| `let x = newvar()` | `NEWVAR x` | `0x38` |
| `emit Event { }` | `BROADCAST reg` | `0x2B` |
| `await joined arr` | `SYNC [regs]` | `0x2C` |
| `async fn()` | `FORGE reg` | `0x28` |
| `transfer a, b, n` | `TRANSFER a b n` | `0x07` |
| `compare a, b` | `COMPARE a b` | `0x0B` |
| `bind_role x, ROLE` | `BIND_ROLE x ROLE x` | `0x0F` |
| `seq expr` | `SEQ ...` | `0x16` |
| `diff a, b` | `DIFF ...` | `0x18` |
| `detect_pattern x, expr` | `DETECT_PATTERN ...` | `0x19` |
| `predict x` | `PREDICT ...` | `0x1A` |
| `match a, b` | `MATCH ...` | `0x1B` |
| `debate a, b` | `DEBATE ...` | `0x1C` |
| `discover a, b` | `DISCOVER ...` | `0x1E` |
| `decode x, expr` | `DECODE ...` | `0x23` |
| `score x, expr` | `SCORE ...` | `0x24` |
| `specialize x, expr` | `SPECIALIZE ...` | `0x25` |
| `reward x` | `REWARD ...` | `0x27` |
| `infer a, b` | `INFER ...` | `0x2A` |
| `merge a, b` | `MERGE ...` | `0x2D` |
| `split x, expr` | `SPLIT ...` | `0x2E` |
| `filter x, expr` | `FILTER ...` | `0x2F` |
| `map_roles x, ROLE` | `MAP_ROLES ...` | `0x30` |
| `reduce x, expr` | `REDUCE ...` | `0x31` |
| `temporal_bind a, b` | `TEMPORAL_BIND ...` | `0x32` |
| `analogy a, b` | `ANALOGY ...` | `0x33` |
| `inst x, expr` | `INST ...` | `0x36` |
| `gen x, expr` | `GEN ...` | `0x37` |
| `explore x, expr` | `EXPLORE ...` | `0x26` |
| `forge a, b` | `FORGE ...` | `0x28` |
| `ask a, b` | `ASK ...` | `0x1D` |
| `sync a, b` | `SYNC ...` | `0x2C` |
| `forget x` | `FORGET ...` | `0x22` |
| `cond x, val, { ... } [else { ... }]` | `COND var target ...` | `0x11` |
| `loop x, target, cond, { ... }` | `LOOP var target ...` | `0x12` |

---

## Type System

Type inference follows Pottier subtyping + OmniML suspended constraints.

```
Subtype lattice:

                                  any
     /      |       |      \        \        \        \        \
 number    str   tensor   entity    ref      doc     ctx      agent
  / \              / \              / \  \     |      / \       |
int  float      vec  emb        role rag module mdx isolated scoped cloned_agent
 / \
u64  i64

    ref (reference types):
      file, url, dataset, external_program, channel<T>

    doc (document types):
      mdx (executable natural language with variables + tools)

    ctx (context types):
      ctx           — full VM execution context
      isolated      — sandboxed (no parent access, own history)
      scoped        — temporary override (reverts on exit)

    agent (autonomous types):
      agent         — program + persona + ctx + model + loop
      cloned_agent  — deep copy, independent evolution, mergeable
```

Union types: `f64 | null`, `str | Error`
Generic types: `array<T>`, `map<K, V>`, `promise<T>`, `channel<T>`

---

## Grammar Summary (EBNF sketch)

```ebnf
program     = "program" IDENT "implements" iface_list "{" program_body "}"
iface_list  = IDENT ("," IDENT)*
program_body = (storage_block | type_decl | function_decl | enum_decl | struct_decl)*

interface   = "interface" IDENT "{" iface_body "}"
iface_body  = (type_decl | abstract_fn | optional_fn)*

storage_block = "storage" "{" (storage_field ";")* "}"
storage_field = IDENT ":" modifier* type ("=" expr)?

function_decl = permission* modifier* "function" IDENT "(" params ")" ":" type "{" stmts "}"
modifier      = "public" | "private" | "abstract" | "global"
              | "mutable" | "immutable" | "async" | "sequential"
              | "parallel" | "joined" | "pure" | "optional" | "singleton"
permission    = "@external" | "@internal" | "@system"
              | "@hook(" IDENT "." IDENT ")"
              | "@before(" IDENT ")" | "@after(" IDENT ")"
              | "@cron(" STRING ")" | "@once"
              | "@restricted(" "[" IDENT ("," IDENT)* "]" ")"
              | "@ratelimit(" NUMBER "," STRING ")"

grant_stmt    = "grant" IDENT "." IDENT "to" IDENT (";" | grant_opts)
revoke_stmt   = "revoke" IDENT "." IDENT "from" IDENT ";"
grant_opts    = "{" (IDENT ":" expr ",")* "}"

enum_decl   = "enum" IDENT "{" enum_variants "}"
struct_decl = "struct" IDENT "{" struct_fields "}"
type_decl   = "type" IDENT "=" type ";"
event_decl  = "event" IDENT "{" struct_fields "}"

extend_block = "extend" IDENT "{" function_decl* "}"
deploy_expr  = "deploy" IDENT "(" args ")"

# Statements
stmt = let_stmt | if_stmt | for_stmt | match_stmt | return_stmt
     | emit_stmt | opcode_stmt | expr_stmt

# VM opcode statements (compile directly to bytecodes)
opcode_stmt = "create" IDENT ":" IDENT
            | "assign" IDENT "=" expr
            | "add" IDENT "," expr
            | "sub" IDENT "," expr
            | "mul" IDENT "," expr
            | "div" IDENT "," expr
            | "sum" IDENT
            | "push" IDENT
            | "pop" IDENT
            | "query" IDENT
            | "remember" IDENT
            | "store" IDENT "," expr
            | "recall" expr
            | "recall" IDENT "," expr
            | "bind" IDENT "," ROLE "," expr
            | "bind_role" IDENT "," ROLE
            | "unify" IDENT "," IDENT
            | "transfer" IDENT "," IDENT "," expr
            | "compare" IDENT "," IDENT
            | ext_opcode ext_args
            | block_opcode
ext_opcode  = "seq" | "diff" | "detect_pattern" | "predict" | "match"
            | "debate" | "discover" | "decode" | "score" | "specialize"
            | "reward" | "infer" | "merge" | "split" | "filter"
            | "map_roles" | "reduce" | "temporal_bind" | "analogy"
            | "gen" | "inst" | "explore" | "forge" | "ask" | "sync" | "forget"
block_opcode = "cond" IDENT "," expr "," block ("else" block)?
            | "loop" IDENT "," expr "," expr "," block
block       = "{" stmt* "}"
ext_args    = (ext_arg ("," ext_arg)*)?      # 0+ comma-separated args
ext_arg     = IDENT | expr                   # bare IDENT → register/role, else value
```
