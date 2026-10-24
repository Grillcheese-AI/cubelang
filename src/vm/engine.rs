//! CubeLang VM — loads and executes `.cubebin` bytecodes.
//!
//! The VM maintains registers, a stack, and storage. It decodes
//! bytecodes instruction-by-instruction and executes them.

use std::collections::{HashMap, HashSet};
use crate::compiler::{CompiledProgram, CompiledFunction, Access, op};
use crate::vm::memory::HippocampalMemory;
use crate::vm::codebook::Codebook;
use crate::vm::hypervec::Hypervec;

/// Hypervector dimension for the VM's hippocampal memory. This is CubeLang's
/// own working value -- it does NOT match opcode-vsa-rs's own default (8192,
/// `hypervec::DEFAULT_DIM`); cross-repo dimension reconciliation is out of
/// scope here. Quasi-orthogonality of distinct keys holds comfortably at
/// this width.
const MEM_DIM: usize = 4096;
/// Fixed codebook seed so memory addresses are reproducible across runs.
const MEM_SEED: u64 = 0xC0DEB00C;

/// Runtime value.
#[derive(Debug, Clone)]
pub enum Value {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Null,
    Array(Vec<Value>),
    Map(HashMap<String, Value>),
    /// A bipolar hypervector value (VSA role-filler binding result). Lives in
    /// the same {-1,+1}^MEM_DIM space as the hippocampal memory's addresses.
    Hvec(Hypervec),
}

impl Value {
    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(n) => *n,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => if *b { 1 } else { 0 },
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Int(n) => *n as f64,
            Value::Float(f) => *f,
            Value::Bool(b) => if *b { 1.0 } else { 0.0 },
            _ => 0.0,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Value::Str(s) => s.clone(),
            Value::Int(n) => n.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => "null".into(),
            _ => format!("{:?}", self),
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(n) => *n != 0,
            Value::Float(f) => *f != 0.0,
            Value::Null => false,
            Value::Str(s) => !s.is_empty(),
            _ => true,
        }
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(v) => write!(f, "{}", v),
            Value::Str(s) => write!(f, "{}", s),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Null => write!(f, "null"),
            Value::Array(a) => write!(f, "[{} items]", a.len()),
            Value::Map(m) => write!(f, "{{{} entries}}", m.len()),
            Value::Hvec(h) => write!(f, "<hvec d={}>", h.dim()),
        }
    }
}

/// VM execution result.
#[derive(Debug)]
pub enum ExecResult {
    Ok(Value),
    Return(Value),
    /// The program cannot proceed without information only the caller has.
    ///
    /// This is NOT an error and NOT a result: it is a third outcome. A program
    /// that has grounded several real candidates and cannot choose between them
    /// must neither pick one (that is a confabulation) nor fail (nothing went
    /// wrong). It suspends, hands the question outward, and resumes when told.
    ///
    /// Resumption is cheap because registers/storage/memory/accumulator are
    /// VM-global; only the interpreter's *locals* (pc, flag, jump_budget) need
    /// capturing. See `VM::resume`.
    Suspend(Suspension),
    Error(String),
}

/// Everything needed to continue a suspended program, plus what to ask.
#[derive(Debug, Clone)]
pub struct Suspension {
    /// The question, as the program phrased it (ASK's first operand).
    pub question: Value,
    /// Candidate answers the program is choosing between (ASK's remaining operands).
    pub candidates: Vec<Value>,
    /// Interpreter locals, captured so `resume` can pick up mid-loop.
    pub pc: usize,
    pub flag: bool,
    pub jump_budget: u64,
    /// Which function was executing (resume re-enters it).
    pub function: String,
    /// Which program owns that function. Captured because `call()` restores
    /// `current_program` on the way out, so the VM has forgotten it by the time
    /// the host sees the Suspend. A suspension must be self-describing.
    pub program: String,
    /// The callers waiting on the function that asked, innermost first
    /// (2026-10-24). Empty when the host's entry function asked. `resume`
    /// finishes the asker, hands its result to each caller in turn, and
    /// continues each one where its CALL left off -- so a skill can call a
    /// skill that asks.
    pub callers: Vec<CallerFrame>,
    /// The asker's own registers when it is a callee: a callee's registers
    /// are its own, while the entry function's live in the VM.
    inner: Option<FrameState>,
}

/// One caller waiting on a call when the callee suspended: where it resumes
/// (the instruction after its CALL) and the registers it held.
#[derive(Debug, Clone)]
pub struct CallerFrame {
    pub program: String,
    pub function: String,
    pub pc: usize,
    pub flag: bool,
    pub jump_budget: u64,
    /// `None` only for the outermost caller, whose registers are the VM's own.
    state: Option<FrameState>,
}

/// A function's private register scope (registers and the VSA frames that
/// follow them), held aside while it waits.
#[derive(Debug, Clone, Default)]
struct FrameState {
    registers: HashMap<String, Value>,
    vsa_frames: HashMap<String, VsaFrame>,
}

/// The CubeLang virtual machine.
pub struct VM {
    /// Grounded facts, keyed by canonical surface form. Backs `QUERY`.
    /// Empty by default: a VM with no knowledge abstains on every query, which
    /// is the correct behaviour for a VM with no knowledge.
    pub knowledge: crate::vm::knowledge::KnowledgeStore,
    /// Set by `resume()`; consumed by the next `exec_function` entry to restore
    /// interpreter locals (pc/flag/jump_budget) after an ASK suspension.
    resume_state: Option<Suspension>,
    /// Named registers.
    pub registers: HashMap<String, Value>,
    /// Stack for PUSH/POP chaining.
    pub stack: Vec<Value>,
    /// Persistent storage (STORE/RECALL) — exact-key fast path.
    pub storage: HashMap<String, Value>,
    /// Hippocampal associative memory (VSA cleanup) for STORE/RECALL/REMEMBER/FORGET.
    pub memory: HippocampalMemory,
    /// Symbol/role codebook for VSA role-filler binding (BIND_ROLE/UNBIND).
    /// Deterministic symbol -> hypervector at MEM_DIM/MEM_SEED, so bound
    /// structures are reproducible across runs and share the memory's space.
    vsa: Codebook,
    /// WO-1.2: register name -> the fillers bound INTO that register, in bind
    /// order, deduplicated.
    ///
    /// `recover`/UNBIND is a cosine cleanup, and a cleanup always returns its
    /// nearest candidate -- it never returns "nothing". So the candidate pool
    /// IS the semantics. It used to be every symbol the VM had ever seen as a
    /// name, which means a role that was never bound in a frame came back as
    /// whatever unrelated string happened to sit closest to quasi-orthogonal
    /// noise: measured before this change, `recover(frame, LOCATION)` on a
    /// frame holding SUBJECT/OBJECT/VERB returned **"umbrella" at 0.0215** --
    /// a filler belonging to a completely different frame.
    ///
    /// Scoping the pool to the frame's own bindings does not make the absent
    /// role return Null (that is a separate question -- DRIFT.md B11a), but it
    /// does confine the wrong answer to symbols that are actually part of this
    /// structure, and it makes the similarity mean what the tau thresholds
    /// assume it means: "how well does this frame's own content explain the
    /// unbound result", not "what is the nearest thing in the universe".
    ///
    /// Kept beside `registers` and given the same lifetime: `exec_intra_call`
    /// takes and restores both together, so a callee's bindings cannot leak
    /// into the caller's frames.
    ///
    /// VM step 0 (2026-09-29): the frame also records its ROLES, so an absent
    /// role is a structural fact (`RecoverReason::AbsentRole`, Null before any
    /// similarity is computed) rather than a low number the tau thresholds
    /// have to catch. See `vsa_unbind_cleanup_known`.
    vsa_frames: HashMap<String, VsaFrame>,
    /// Accumulator (SUM results).
    pub accumulator: Value,
    /// Event log.
    pub events: Vec<(String, HashMap<String, Value>)>,
    /// Loaded programs.
    programs: HashMap<String, CompiledProgram>,
    /// Execution trace (for debugging).
    pub trace: Vec<String>,
    /// Enable tracing.
    pub trace_enabled: bool,
    /// Name→hash mapping for reverse lookup.
    name_table: HashMap<[u8; 2], String>,
    /// Plan-2: name of the program whose function is currently executing. Set
    /// by `call()`; consulted by op::CALL to resolve sibling-function names.
    current_program: Option<String>,
    /// Plan-2: intra-program call recursion depth. Capped at MAX_CALL_DEPTH
    /// so runaway recursion surfaces as an Error rather than a native overflow.
    call_depth: u32,
    /// Task 3: the VM-internal module/capability registry. Built once at
    /// `VM::new` (cheap, deterministic) and consulted by `op::CALL` for
    /// override/registry-impl resolution. See `vm::registry` for the
    /// deny-by-default access model and the override precedence rule.
    registry: crate::vm::registry::ModuleRegistry,
    /// Task 7 (Feature A, similarity-surfacing): the cosine similarity of
    /// the last winning `vsa_unbind_cleanup` match in this run -- i.e. the
    /// confidence behind the most recent `recover()`/`UNBIND`. `None` means
    /// either no recover has happened yet, or the last one found no match
    /// (the unbound/absent-role case). "The last recover in this run" is a
    /// safe, well-defined reading here because `run_request_result` builds
    /// a fresh `VM::new()` per request and the reasoning program does
    /// exactly one `recover` per function.
    pub last_recover_similarity: Option<f64>,
    /// Branch report (2026-09-28): opcodes executed in this VM, over every
    /// function it ran. What the host reads to compare branches by the work
    /// they did, not only by what they returned.
    pub ops_executed: u64,
    /// Branch report: jumps taken (the part of the 1,000,000 budget spent).
    pub jumps_taken: u64,
    /// Branch report: every QUERY this VM made, in order -- the mention as
    /// asked, and how many facts the store held for it (0 = it abstained).
    /// Facts touched, by name: the provenance a branch's answer rests on.
    pub queried: Vec<(String, usize)>,
    /// The per-run jump budget (2026-09-28): the harness sets it from the
    /// frame; 1,000,000 by default, which is not a budget for a live turn but
    /// is what every earlier program ran under.
    pub jump_budget: u64,
    /// VM step 0 (2026-09-29), budgets on the request: opcodes this run may
    /// execute (0 = unlimited), QUERYs it may make (0 = unlimited), and the
    /// instant after which the next opcode is refused (None = no wall
    /// budget). The harness sets all three from the frame; exceeding one is
    /// an `ExecResult::Error` naming it, never a silent truncation.
    pub op_budget: u64,
    pub query_budget: u64,
    pub deadline: Option<std::time::Instant>,
    /// The wall budget as requested, for the report (the deadline is derived).
    pub wall_budget_ms: u32,
    /// Every UNBIND / `recover()` this VM did, in order, with the runner-up
    /// and the reason -- what the branch report carries per hop.
    pub recovers: Vec<RecoverRecord>,
    /// The runner-up similarity behind `last_recover_similarity` (None when
    /// the pool had one candidate or nothing was recovered).
    pub last_recover_runner_up: Option<f64>,
    /// Why the last recover returned what it did; None before any recover.
    pub last_recover_reason: Option<RecoverReason>,
    /// `@once` functions that have already run in this VM (`program.fn`).
    once_called: HashSet<String>,
    /// Who is calling from outside (2026-10-24): the program name the host
    /// speaks as, checked against `@restricted(...)` at `call_entry`. `None`:
    /// an anonymous caller, which no restricted function admits.
    pub caller: Option<String>,
    /// `program.fn` -> the callers a container's `permissions { grant ... }`
    /// admitted on top of the function's own `@restricted` list (and past
    /// `@internal`, which the SPEC opens to "programs it explicitly grants").
    pub grants: HashMap<String, HashSet<String>>,
    /// `@ratelimit` windows and the `@cron` clock. A resident host keeps one
    /// across requests (`std::mem::take` in, then back out).
    pub governor: crate::vm::governor::Governor,
}

/// What went into a frame register through BIND_ROLE, in bind order and
/// deduplicated: the roles, so an absent role is a structural fact, and the
/// fillers, which are the cleanup pool.
#[derive(Debug, Clone, Default)]
pub struct VsaFrame {
    pub roles: Vec<String>,
    pub fillers: Vec<String>,
}

/// Why a recover returned what it returned. Closed: the host switches on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverReason {
    /// A candidate won; `similarity` says how well and `margin` how clearly.
    Recovered,
    /// The role is not among the frame's bound roles: Null, structurally,
    /// before any similarity is computed. The threshold guards only crosstalk
    /// among roles that ARE present.
    AbsentRole,
    /// The register holds no hypervector (nothing was ever bound there).
    NoFrame,
    /// There was nothing to clean up against.
    EmptyPool,
}

impl RecoverReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoverReason::Recovered => "recovered",
            RecoverReason::AbsentRole => "absent_role",
            RecoverReason::NoFrame => "no_frame",
            RecoverReason::EmptyPool => "empty_pool",
        }
    }
}

/// One UNBIND / `recover()` hop as the branch report carries it: the winner
/// and its similarity, the runner-up's similarity and the margin between
/// them, the pool size, and the reason. A margin near zero is a near-tie the
/// host must not speak as a recovery even when the winner clears tau.
#[derive(Debug, Clone)]
pub struct RecoverRecord {
    pub reg: String,
    pub role: String,
    pub winner: Option<String>,
    pub similarity: Option<f64>,
    pub runner_up: Option<f64>,
    pub margin: Option<f64>,
    pub pool: usize,
    pub reason: RecoverReason,
}

/// Everything `VM::snapshot` captures and `VM::restore` puts back: the
/// program-visible state (registers, stack, storage, the associative memory,
/// the frames, the accumulator) and the knowledge store. NOT the logs (events,
/// trace) and not the counters: a restored branch still happened, and the
/// report says so. The in-language `ctx.snapshot/restore/fork/discard` the
/// SPEC describes will sit on these two methods; today the host calls them.
#[derive(Clone)]
pub struct VmSnapshot {
    registers: HashMap<String, Value>,
    stack: Vec<Value>,
    storage: HashMap<String, Value>,
    memory: HippocampalMemory,
    vsa_frames: HashMap<String, VsaFrame>,
    accumulator: Value,
    knowledge: crate::vm::knowledge::KnowledgeStore,
}

/// Conservative recursion cap. exec_function has a sizeable per-frame
/// footprint (registers/stack clones, operands buffer, label_map clone via
/// scan), so 64 is comfortably below the native test-thread stack limit while
/// being plenty for hand-written CubeLang programs.
const MAX_CALL_DEPTH: u32 = 64;

impl VM {
    pub fn new() -> Self {
        let mut vm = Self {
            registers: HashMap::new(),
            stack: Vec::new(),
            storage: HashMap::new(),
            memory: HippocampalMemory::new(MEM_DIM, MEM_SEED),
            vsa: Codebook::with_dim(MEM_SEED, MEM_DIM),
            vsa_frames: HashMap::new(),
            accumulator: Value::Null,
            events: Vec::new(),
            programs: HashMap::new(),
            trace: Vec::new(),
            trace_enabled: false,
            name_table: HashMap::new(),
            current_program: None,
            call_depth: 0,
            resume_state: None,
            knowledge: crate::vm::knowledge::KnowledgeStore::new(),
            registry: crate::vm::registry::ModuleRegistry::new(),
            last_recover_similarity: None,
            ops_executed: 0,
            jumps_taken: 0,
            queried: Vec::new(),
            jump_budget: 1_000_000,
            op_budget: 0,
            query_budget: 0,
            deadline: None,
            wall_budget_ms: 0,
            recovers: Vec::new(),
            last_recover_runner_up: None,
            last_recover_reason: None,
            once_called: HashSet::new(),
            caller: None,
            grants: HashMap::new(),
            governor: crate::vm::governor::Governor::new(),
        };
        // Task 3: pre-register every registry function name into the
        // hash-reversal table, regardless of which modules any given
        // program `use`s. This is NOT a grant of access -- deny-by-default
        // is enforced separately, by op::CALL gating resolution on the
        // calling program's own `used_modules` -- it only makes a name the
        // compiler already hashed (because the source literally called it)
        // reversible to a string the VM can look up in the registry at all.
        let registry_names: Vec<String> =
            vm.registry.all_function_names().map(str::to_string).collect();
        for name in &registry_names {
            vm.register_name(name);
        }
        vm
    }

    /// Capture the program-visible state and the knowledge store. Cheap
    /// enough for a branch per request (a clone of the maps and the memory).
    pub fn snapshot(&self) -> VmSnapshot {
        VmSnapshot {
            registers: self.registers.clone(),
            stack: self.stack.clone(),
            storage: self.storage.clone(),
            memory: self.memory.clone(),
            vsa_frames: self.vsa_frames.clone(),
            accumulator: self.accumulator.clone(),
            knowledge: self.knowledge.clone(),
        }
    }

    /// Put a snapshot back. Logs and counters are kept: a discarded branch
    /// still ran, and the report keeps what it cost.
    pub fn restore(&mut self, snap: VmSnapshot) {
        self.registers = snap.registers;
        self.stack = snap.stack;
        self.storage = snap.storage;
        self.memory = snap.memory;
        self.vsa_frames = snap.vsa_frames;
        self.accumulator = snap.accumulator;
        self.knowledge = snap.knowledge;
    }

    /// Resume a program suspended by ASK, supplying the caller's answer.
    ///
    /// THE CALLER IS THE MODEL, NOT A HUMAN. The trunk renders `susp.question`
    /// and `susp.candidates` into language, the user replies in language, and
    /// the trunk maps that reply back to ONE OF THE CANDIDATES. The VM never
    /// talks to a person, and a person never picks an index.
    ///
    /// That division is what keeps the guarantee: the trunk supplies syntax
    /// (rendering) and selection (mapping), never content. So the selection is
    /// CHECKABLE, and this function checks it -- `answer` must be *identical*
    /// to a candidate, not merely plausible. A trunk that returns 1968 when the
    /// candidates are {1959, 1969} has confabulated a value into a slot that
    /// could only ever hold one of two, and is rejected here rather than
    /// silently believed.
    ///
    /// The answer is PUSHED ONTO THE STACK, matching how CALL delivers a
    /// callee's return value. The resumed program binds it with `pop <reg>`.
    /// (An earlier draft used the accumulator; wrong, because `SUM x` overwrites
    /// the accumulator from register x.)
    ///
    /// Resumption needs no call frames: registers/storage/memory are VM-global,
    /// so `Suspension` carries back only the interpreter locals (pc, flag,
    /// jump_budget).
    pub fn resume(&mut self, susp: Suspension, answer: Value) -> ExecResult {
        // Structural guard: the selection must be one the program offered.
        // Deliberately NOT `derive(PartialEq)` on Value -- Hvec holds floats and
        // "equal hypervectors" is a similarity question, not an equality one.
        //
        // Compared structurally, because a QUERY chunk is a Map: the trunk maps
        // the user's reply back to one of the offered chunks and hands that Map
        // straight back. A scalar-only comparison would reject every possible
        // answer and make ASK unusable on grounded facts.
        fn same_selection(a: &Value, b: &Value) -> bool {
            match (a, b) {
                (Value::Int(x), Value::Int(y)) => x == y,
                (Value::Str(x), Value::Str(y)) => x == y,
                (Value::Bool(x), Value::Bool(y)) => x == y,
                (Value::Float(x), Value::Float(y)) => x == y,
                (Value::Null, Value::Null) => true,
                (Value::Array(x), Value::Array(y)) => {
                    x.len() == y.len()
                        && x.iter().zip(y).all(|(i, j)| same_selection(i, j))
                }
                (Value::Map(x), Value::Map(y)) => {
                    x.len() == y.len()
                        && x.iter().all(|(k, v)| {
                            y.get(k).map_or(false, |w| same_selection(v, w))
                        })
                }
                // Hypervectors are never a *selection*; equality is the wrong
                // question to ask of them.
                _ => false,
            }
        }
        if !susp.candidates.is_empty() && !susp.candidates.iter().any(|c| same_selection(c, &answer)) {
            return ExecResult::Error(format!(
                "resume: answer {} is not among the {} candidates the program offered \
                 (a selection must be chosen, not invented)",
                answer,
                susp.candidates.len()
            ));
        }
        if !susp.callers.is_empty() {
            return self.resume_chain(susp, answer);
        }
        if susp.inner.is_some() {
            return ExecResult::Error("resume: a suspension with a callee's registers but no caller is malformed".into());
        }
        let prog_name = susp.program.clone();
        let func = match self.function_named(&prog_name, &susp.function) {
            Ok(f) => f,
            Err(e) => return ExecResult::Error(e),
        };
        // Deliver the answer the same way CALL delivers a result: via the stack.
        self.stack.push(answer);
        // A nested ASK inside the resumed run must report the same program.
        let prev = self.current_program.replace(prog_name);
        self.resume_state = Some(susp);
        let result = self.exec_function(&func);
        self.current_program = prev;
        result
    }

    fn function_named(&self, program: &str, function: &str) -> Result<CompiledFunction, String> {
        let prog = self.programs.get(program)
            .ok_or_else(|| format!("resume: program not found: {}", program))?;
        prog.functions.iter().find(|f| f.name == function).cloned()
            .ok_or_else(|| format!("resume: function not found: {}.{}", program, function))
    }

    /// Resume an ASK made inside a called function (2026-10-24). The frames
    /// run innermost first: the asker gets the answer on the stack, as at the
    /// top level; each caller then gets its callee's result on the stack, as
    /// CALL delivers it, and continues after its CALL with its own registers.
    /// The outermost frame (the host's entry) runs with the VM's registers.
    ///
    /// A frame that asks again re-suspends with the frames still waiting
    /// above it attached, exactly as if the ASK had happened on the first run:
    /// so the next `resume` continues the same chain.
    fn resume_chain(&mut self, susp: Suspension, answer: Value) -> ExecResult {
        let mut frames: Vec<CallerFrame> = Vec::with_capacity(susp.callers.len() + 1);
        frames.push(CallerFrame {
            program: susp.program.clone(),
            function: susp.function.clone(),
            pc: susp.pc,
            flag: susp.flag,
            jump_budget: susp.jump_budget,
            state: susp.inner,
        });
        frames.extend(susp.callers);
        let n = frames.len();
        // Every waiting frame but the outermost carries its registers; the
        // outermost's are the VM's own.
        if frames.iter().enumerate().any(|(i, f)| (i + 1 < n) != f.state.is_some()) {
            return ExecResult::Error("resume: the suspended call chain is malformed".into());
        }
        let mut funcs = Vec::with_capacity(n);
        for f in &frames {
            match self.function_named(&f.program, &f.function) {
                Ok(func) => funcs.push(func),
                Err(e) => return ExecResult::Error(e),
            }
        }
        frames[n - 1].state = Some(FrameState {
            registers: std::mem::take(&mut self.registers),
            vsa_frames: std::mem::take(&mut self.vsa_frames),
        });
        let prev_program = self.current_program.clone();
        let prev_depth = self.call_depth;
        let mut carry = answer;
        let mut i = 0;
        let outcome = loop {
            let state = frames[i].state.take().expect("checked above");
            self.registers = state.registers;
            self.vsa_frames = state.vsa_frames;
            self.stack.push(carry);
            self.current_program = Some(frames[i].program.clone());
            self.call_depth = prev_depth + (n - 1 - i) as u32;
            self.resume_state = Some(Suspension {
                question: Value::Null,
                candidates: Vec::new(),
                pc: frames[i].pc,
                flag: frames[i].flag,
                jump_budget: frames[i].jump_budget,
                function: frames[i].function.clone(),
                program: frames[i].program.clone(),
                callers: Vec::new(),
                inner: None,
            });
            let r = self.exec_function(&funcs[i]);
            match r {
                ExecResult::Ok(v) | ExecResult::Return(v) if i + 1 < n => {
                    carry = v;
                    i += 1;
                }
                ExecResult::Suspend(mut again) if i + 1 < n => {
                    // As `exec_intra_call` would have: this frame's registers
                    // go into the suspension, then every frame still waiting
                    // above it, the outermost's back into the VM.
                    let state = FrameState {
                        registers: std::mem::take(&mut self.registers),
                        vsa_frames: std::mem::take(&mut self.vsa_frames),
                    };
                    match again.callers.last_mut() {
                        None => again.inner = Some(state),
                        Some(waiting) => waiting.state = Some(state),
                    }
                    for (j, mut waiting) in frames.drain(i + 1..).enumerate() {
                        if i + 1 + j == n - 1 {
                            let outer = waiting.state.take().expect("the outermost frame's registers");
                            self.registers = outer.registers;
                            self.vsa_frames = outer.vsa_frames;
                        }
                        again.callers.push(waiting);
                    }
                    break ExecResult::Suspend(again);
                }
                ExecResult::Error(e) if i + 1 < n => {
                    // Put the outermost frame's registers back, so the VM is
                    // left as an erroring top-level run leaves it.
                    if let Some(outer) = frames[n - 1].state.take() {
                        self.registers = outer.registers;
                        self.vsa_frames = outer.vsa_frames;
                    }
                    break ExecResult::Error(e);
                }
                // the outermost frame finished, asked again or failed: as at the top level
                other => break other,
            }
        };
        self.current_program = prev_program;
        self.call_depth = prev_depth;
        outcome
    }

    /// Load a compiled program into the VM.
    pub fn load(&mut self, prog: CompiledProgram) {
        // Pre-populate name table for reverse lookups
        self.register_name(&prog.name);
        for iface in &prog.implements {
            self.register_name(iface);
        }
        for field in &prog.storage_fields {
            self.register_name(field);
        }
        for func in &prog.functions {
            self.register_name(&func.name);
        }
        // Role + filler symbol names used by BIND_ROLE/UNBIND, so their 2-byte
        // hashes in the bytecode reverse to the real strings the VSA codebook
        // keys on (enabling genuine, human-readable role-filler binding).
        for sym in &prog.symbol_names {
            self.register_name(sym);
        }
        self.programs.insert(prog.name.clone(), prog);
    }

    /// Load from a .cubebin file.
    pub fn load_file(&mut self, path: &std::path::Path) -> Result<String, String> {
        let prog = CompiledProgram::load(path)?;
        let name = prog.name.clone();
        self.load(prog);
        Ok(name)
    }

    /// Run a function by name in a loaded program.
    pub fn call(&mut self, program: &str, function: &str, args: Vec<Value>) -> ExecResult {
        let prog = match self.programs.get(program) {
            Some(p) => p.clone(),
            None => return ExecResult::Error(format!("program not found: {}", program)),
        };

        let func = match prog.functions.iter().find(|f| f.name == function) {
            Some(f) => f.clone(),
            None => return ExecResult::Error(format!("function not found: {}.{}", program, function)),
        };

        // Bind host arguments to registers.
        //
        // Previously this ONLY synthesized `arg0`, `arg1`, ... because
        // CompiledFunction discarded the declared parameter names at compile
        // time. So a function written as `solve(mention: str)` read an UNBOUND
        // register when called from a host -- silently, since an unbound
        // register resolves to Int(0)/Null rather than erroring. Every example
        // in the SPEC hits this.
        //
        // Now: bind by DECLARED NAME (v3 .cubebin carries them), and keep the
        // `argN` alias so v2 files and existing callers still work.
        for (i, arg) in args.into_iter().enumerate() {
            if let Some(param) = func.params.get(i) {
                self.register_name(param);
                self.registers.insert(Self::register_key(param), arg.clone());
            }
            let alias = format!("arg{}", i);
            self.register_name(&alias);
            self.registers.insert(Self::register_key(&alias), arg);
        }

        // Plan-2: remember which program owns the running function so an
        // intra-program CALL can look up siblings by name.
        let prev = self.current_program.replace(program.to_string());
        let result = self.exec_function(&func);
        self.current_program = prev;
        result
    }

    /// The HOST's entry into a program (VM step 0, 2026-09-29): `call` with
    /// the access attributes enforced. `@internal` is "only callable by this
    /// program" and `@system` is "only callable by the VM runtime", so a
    /// request naming either as its entry function is refused here, before
    /// anything runs. The runtime's own constructor call goes through
    /// `call`/`init`, which is the runtime. A function with no attribute is
    /// callable, as it always was.
    pub fn call_entry(&mut self, program: &str, function: &str, args: Vec<Value>) -> ExecResult {
        if let Err(e) = self.admit_entry(program, function, std::time::Instant::now()) {
            return ExecResult::Error(e);
        }
        self.call(program, function, args)
    }

    /// Every rule a call from OUTSIDE must pass (2026-10-24), in the order a
    /// refusal is most useful to read: who may call at all (`@system`,
    /// `@cron`, `@internal` / `private`), who by name (`@restricted`, plus a
    /// container's grants), then how often (`@ratelimit`, counted only when
    /// everything before it admitted the call, so a refused caller cannot
    /// spend the window). A function the program does not have falls through
    /// to `call`, which says so.
    pub fn admit_entry(&mut self, program: &str, function: &str, now: std::time::Instant) -> Result<(), String> {
        let Some((access, cron, restricted, ratelimit)) = self.programs.get(program)
            .and_then(|p| p.functions.iter().find(|f| f.name == function))
            .map(|f| (f.access, f.cron, f.restricted.clone(), f.ratelimit))
        else { return Ok(()) };
        let key = format!("{}.{}", program, function);
        let granted = self.caller.as_ref()
            .is_some_and(|c| self.grants.get(&key).is_some_and(|g| g.contains(c)));
        match access {
            Access::System => return Err(format!("{} is @system: only the VM runtime may call it", key)),
            Access::Internal if !granted => return Err(format!(
                "{} is @internal (or private): not callable from outside the program{}",
                key, if self.grants.contains_key(&key) { " except by the programs granted it" } else { "" })),
            _ => {}
        }
        if cron.is_some() {
            return Err(format!("{} is @cron: the VM's scheduler runs it (tick), nothing calls it", key));
        }
        if let Some(allowed) = &restricted {
            let admitted = self.caller.as_ref()
                .is_some_and(|c| allowed.iter().any(|a| a == c)) || granted;
            if !admitted {
                let who = self.caller.as_deref().unwrap_or("an anonymous caller");
                let list = if allowed.is_empty() { "the programs a container grants it".to_string() } else { allowed.join(", ") };
                return Err(format!("{} is @restricted to {}: {} may not call it", key, list, who));
            }
        }
        if let Some((n, period_ms)) = ratelimit {
            // Per caller: one busy agent cannot spend everyone's window. Every anonymous caller
            // shares one window (a public front end limits its visitors before they get here).
            let bucket = format!("{}#{}", key, self.caller.as_deref().unwrap_or("anonymous"));
            self.governor.admit(&bucket, n, std::time::Duration::from_millis(period_ms), now)?;
        }
        Ok(())
    }

    /// The `@cron` functions of `program` due at `now`, in declaration order.
    pub fn due_crons(&self, program: &str, now: std::time::Instant) -> Vec<String> {
        let Some(p) = self.programs.get(program) else { return Vec::new() };
        p.functions.iter()
            .filter_map(|f| f.cron.map(|ms| (f, ms)))
            .filter(|(f, ms)| self.governor.due(&format!("{}.{}", program, f.name),
                                                std::time::Duration::from_millis(*ms), now))
            .map(|(f, _)| f.name.clone())
            .collect()
    }

    /// One tick of the scheduler (2026-10-24): run every `@cron` function of
    /// `program` that is due at `now`, as the runtime (no entry rules apply:
    /// the scheduler is the one caller a cron function admits). Each is
    /// marked as run BEFORE it runs, so a cron that errors waits its full
    /// interval like one that succeeded rather than retrying every tick.
    /// The host drives the clock: `cubelang tick`, a `RunRequest` with
    /// `tick`, or a library caller on its own timer.
    pub fn tick(&mut self, program: &str, now: std::time::Instant) -> Vec<(String, ExecResult)> {
        let mut out = Vec::new();
        for name in self.due_crons(program, now) {
            self.governor.mark_ran(&format!("{}.{}", program, name), now);
            let r = self.call(program, &name, Vec::new());
            out.push((name, r));
        }
        out
    }

    /// Admit `caller` to `program.function` on top of its own rules (what a
    /// container's `grant program.function to caller;` does).
    pub fn grant(&mut self, program: &str, function: &str, caller: &str) {
        self.grants.entry(format!("{}.{}", program, function)).or_default().insert(caller.to_string());
    }

    /// Withdraw a grant (`revoke ... from caller;`). A function's own
    /// `@restricted` list is not touched: a revoke takes back a grant only.
    pub fn revoke(&mut self, program: &str, function: &str, caller: &str) {
        if let Some(g) = self.grants.get_mut(&format!("{}.{}", program, function)) {
            g.remove(caller);
        }
    }

    /// Run the constructor of a program.
    pub fn init(&mut self, program: &str) -> ExecResult {
        self.call(program, "constructor", Vec::new())
    }

    /// Execute a function's bytecode.
    fn exec_function(&mut self, func: &CompiledFunction) -> ExecResult {
        let bc = &func.bytecode;
        let mut pc: usize = 0;

        // Pre-scan pass: resolve LABEL hashes → bytecode offsets so JMP/COND
        // can jump. We record the offset of the LABEL instruction itself
        // (jumping there re-reads the LABEL as a no-op and continues).
        let label_map = scan_labels(bc);
        // Comparison flag set by COMPARE, tested by COND.
        let mut flag: bool = false;
        // Loop-iteration guard: bound total back-edges to prevent infinite
        // loops from a malformed / non-terminating program reaching the VM.
        let mut jump_budget: u64 = self.jump_budget;

        // Resuming a suspended program: restore the interpreter locals that
        // `Suspension` captured. Registers/storage/memory/accumulator are
        // VM-global and were never lost. `pc` already points PAST the ASK
        // instruction, so execution continues with the answer in scope.
        let resuming = self.resume_state.is_some();
        if let Some(r) = self.resume_state.take() {
            pc = r.pc;
            flag = r.flag;
            jump_budget = r.jump_budget;
        }

        // `@once` (VM step 0): a second entry into a once-function in this VM
        // is an error, not a re-run. A resume re-enters the SAME run and is
        // not a second call.
        if func.once && !resuming {
            let key = format!("{}.{}", self.current_program.clone().unwrap_or_default(), func.name);
            if !self.once_called.insert(key.clone()) {
                return ExecResult::Error(format!("{} is @once and has already run in this VM", key));
            }
        }

        while pc < bc.len() {
            let opcode = bc[pc]; pc += 1;
            self.ops_executed += 1;

            // Budgets on the request (VM step 0). Checked before the opcode
            // runs, so an exhausted budget never half-executes a step.
            if self.op_budget > 0 && self.ops_executed > self.op_budget {
                return ExecResult::Error(format!(
                    "op budget exhausted ({} opcodes)", self.op_budget));
            }
            if let Some(deadline) = self.deadline {
                if self.ops_executed & 0x3f == 0 && std::time::Instant::now() > deadline {
                    return ExecResult::Error("wall budget exhausted".into());
                }
            }

            // SKIP / NOP
            if opcode == op::SKIP {
                self.trace_op(pc - 1, "SKIP");
                continue;
            }

            // Read operand count
            if pc >= bc.len() { break; }
            let n_ops = bc[pc] as usize; pc += 1;

            // Decode operands
            let mut operands: Vec<Operand> = Vec::new();
            for _ in 0..n_ops {
                if pc >= bc.len() { break; }
                let (op, consumed) = decode_operand(&bc[pc..]);
                operands.push(op);
                pc += consumed;
            }

            // Execute
            match opcode {
                op::CREATE => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let ty = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("CREATE {} : {}", name, ty));
                    // DRIFT.md §C4 (2026-09-14): a declared container type now
                    // creates an EMPTY CONTAINER rather than Null, so
                    // `create m : map; m["k"] = v;` works without a separate
                    // initialisation step. `Value::Null` is kept for every
                    // other declared type -- `create x : number;` behaves
                    // exactly as before (and `as_i64(Null)` is already 0, so
                    // nothing numeric changes).
                    //
                    // `type_name` (src/compiler.rs) collapses `map<K,V>` to
                    // "map" and `array<T>` to "array", so the two spellings
                    // below are the only ones that ever reach here.
                    let init = match ty.as_str() {
                        "map" => Value::Map(HashMap::new()),
                        "array" => Value::Array(Vec::new()),
                        _ => Value::Null,
                    };
                    self.registers.insert(name, init);
                }

                op::MAKE_ARRAY => {
                    // Plan-3. operands: [dst, n_imm, e0, e1, ..., e_{n-1}].
                    // n is taken from the Imm; we then collect exactly that
                    // many element operands. If the operand stream is short
                    // (malformed bytecode) we fill with Null.
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let n = match operands.get(1) {
                        Some(Operand::Imm(k)) => (*k).max(0) as usize,
                        _ => 0,
                    };
                    let mut items: Vec<Value> = Vec::with_capacity(n);
                    for i in 0..n {
                        items.push(self.resolve_assign_rhs(operands.get(2 + i)));
                    }
                    self.trace_op(pc, &format!("MAKE_ARRAY {} ({} items)", dst, n));
                    self.registers.insert(dst, Value::Array(items));
                }

                op::LEN => {
                    // operands: [dst, arr]
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let arr = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let len = match self.registers.get(&arr) {
                        Some(Value::Array(v)) => v.len() as i64,
                        Some(Value::Str(s)) => s.len() as i64,
                        _ => 0,
                    };
                    self.trace_op(pc, &format!("LEN {} = len({}) → {}", dst, arr, len));
                    self.registers.insert(dst, Value::Int(len));
                }

                op::INDEX => {
                    // operands: [dst, arr, i]. Out-of-range yields Null so
                    // bad indices surface as untrusted data rather than crash.
                    let dst = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let arr = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    // DRIFT.md §C4 (2026-09-14): resolve the key as a VALUE so
                    // maps can be read too. `.as_i64()` here is identical to
                    // the previous `resolve_i64` for every operand kind, so
                    // array indexing is unchanged.
                    let key = self.resolve_value(operands.get(2));
                    let i = key.as_i64();
                    let val = match self.registers.get(&arr) {
                        Some(Value::Array(v)) if i >= 0 && (i as usize) < v.len() => v[i as usize].clone(),
                        // A map read was previously impossible: this arm did
                        // not exist, so `m["k"]` fell through to Null even
                        // when the key was present. Writing to a map was
                        // impossible too (no SET_INDEX), so the two gaps hid
                        // each other.
                        Some(Value::Map(m)) => m.get(&key.as_str()).cloned().unwrap_or(Value::Null),
                        _ => Value::Null,
                    };
                    self.trace_op(pc, &format!("INDEX {} = {}[{}] → {}", dst, arr, key, val));
                    self.registers.insert(dst, val);
                }

                op::SET_INDEX => {
                    // DRIFT.md §C4 fix. operands: [container, key, value].
                    //
                    // `arr[0] = x`, `regs[reg] = v`, `self.patterns[k] = h`
                    // used to compile to ZERO bytecode -- `compile_stmt`'s
                    // Assign arm only handled `self.field` and a bare
                    // identifier, and anything else fell off the end of the
                    // if/else chain emitting nothing at all. It is the
                    // mechanism behind `examples/gsm8k.cube` returning 0.
                    //
                    // Out-of-range and wrong-container-type are ERRORS, not
                    // silent no-ops: replacing a silent drop with a silent
                    // clamp would keep the same class of bug.
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let key = self.resolve_value(operands.get(1));
                    let val = self.resolve_assign_rhs(operands.get(2));
                    self.trace_op(pc, &format!("SET_INDEX {}[{}] = {}", name, key, val));

                    let current = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    let updated = match current {
                        Value::Map(mut m) => {
                            m.insert(key.as_str(), val);
                            Value::Map(m)
                        }
                        Value::Array(mut v) => {
                            let i = key.as_i64();
                            if i < 0 {
                                return ExecResult::Error(format!(
                                    "SET_INDEX: negative index {} into array `{}`", i, name));
                            }
                            let i = i as usize;
                            if i < v.len() {
                                v[i] = val;
                            } else if i == v.len() {
                                v.push(val); // append is the common idiom
                            } else {
                                return ExecResult::Error(format!(
                                    "SET_INDEX: index {} is past the end of array `{}` (len {}); \
                                     assign at index {} to append", i, name, v.len(), v.len()));
                            }
                            Value::Array(v)
                        }
                        // An untyped `let`/`create` leaves Null; pick the
                        // container from the key's type so a caller does not
                        // have to declare one. A string key means a map, an
                        // integer key means an array.
                        Value::Null => match &key {
                            Value::Str(k) => {
                                let mut m = HashMap::new();
                                m.insert(k.clone(), val);
                                Value::Map(m)
                            }
                            _ => {
                                let i = key.as_i64();
                                if i != 0 {
                                    return ExecResult::Error(format!(
                                        "SET_INDEX: cannot create array `{}` at index {} -- the \
                                         first integer-keyed assignment must be index 0", name, i));
                                }
                                Value::Array(vec![val])
                            }
                        },
                        other => {
                            return ExecResult::Error(format!(
                                "SET_INDEX: `{}` is {:?}, not an array or map -- cannot index-assign \
                                 into it", name, other));
                        }
                    };
                    self.registers.insert(name, updated);
                }

                op::RETURN => {
                    // Plan-1: early return. With 0 operands, yield the current
                    // accumulator. With 1 operand, resolve it (literal or
                    // register) into the accumulator before exiting.
                    if let Some(o) = operands.get(0) {
                        self.accumulator = self.resolve_value(Some(o));
                    }
                    self.trace_op(pc, &format!("RETURN {}", self.accumulator));
                    return ExecResult::Return(self.accumulator.clone());
                }

                op::ASSIGN => {
                    // P0-2: a Named/Global operand must resolve to the source
                    // register's VALUE, not the synthesized register-name string.
                    // Without this, `assign b = a` stored `Value::Str("r_xxxx")`,
                    // so arithmetic/sum on b returned 0. Literal Imm/FloatImm
                    // assignment behaviour is preserved (see resolve_assign_rhs).
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.resolve_assign_rhs(operands.get(1));
                    self.trace_op(pc, &format!("ASSIGN {} = {}", name, val));
                    self.registers.insert(name, val);
                }

                op::ADD => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '+');
                    self.trace_op(pc, &format!("ADD {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::SUB => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '-');
                    self.trace_op(pc, &format!("SUB {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::MUL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '*');
                    self.trace_op(pc, &format!("MUL {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::DIV => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let rhs = self.resolve_value(operands.get(1));
                    // Division by zero (int or float) is an error.
                    if rhs.as_f64() == 0.0 {
                        return ExecResult::Error("division by zero".into());
                    }
                    let cur = self.registers.get(&name).cloned().unwrap_or(Value::Int(0));
                    let out = num_binop(&cur, &rhs, '/');
                    self.trace_op(pc, &format!("DIV {} {} → {}", name, rhs, out));
                    self.registers.insert(name, out);
                }

                op::SUM => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("SUM {} → {}", name, val));
                    self.accumulator = val;
                }

                op::PUSH => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("PUSH {} ({})", name, val));
                    self.stack.push(val);
                }

                op::POP => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.stack.pop().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("POP {} ← {}", name, val));
                    self.registers.insert(name, val);
                }

                op::QUERY => {
                    // QUERY <mention>
                    //
                    // Was: read a register, write a trace line, retrieve nothing.
                    // The SPEC has always typed this as
                    //   rag { chunks: array<{text, score, source}> }
                    // -- provenance in the type, waiting for an implementation.
                    //
                    // Now: an EXACT lookup against the VM's knowledge store. The
                    // chunk array is pushed onto the stack (same convention as
                    // CALL and ASK), so `pop ctx` binds it.
                    //
                    // Zero embeddings. A canonical title IS a key: DBpedia's
                    // 4.6M entities normalize to 4,628,421 unique titles at 0.2%
                    // collisions. Embedding the query would cost a forward pass
                    // per lookup -- a linear marginal cost, which is exactly the
                    // economics this architecture exists to avoid.
                    //
                    // A MISS PUSHES AN EMPTY ARRAY. Not an Error (nothing
                    // failed), and never a nearest neighbour -- that is how you
                    // get "the capital of France is Kyiv" at 0.699. Zero chunks
                    // is the program's cue to say it does not know. The caller
                    // branches on `len`:
                    //     0 -> gap, RETURN     1 -> fact, RETURN     N -> ASK
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let mention = match self.registers.get(&reg) {
                        // a register holding a string is the usual case
                        Some(Value::Str(s)) => s.clone(),
                        // otherwise fall back to the operand's own symbol name,
                        // so `query "printing press";` works as a literal
                        _ => self.resolve_symbol_name(operands.get(0)),
                    };
                    // Query budget (VM step 0): refused before the lookup, so
                    // the store is never read past what the frame allowed.
                    if self.query_budget > 0 && self.queried.len() as u64 >= self.query_budget {
                        return ExecResult::Error(format!(
                            "query budget exhausted ({} queries)", self.query_budget));
                    }

                    let chunks: Vec<Value> = self
                        .knowledge
                        .lookup(&mention)
                        .iter()
                        .map(|f| {
                            let mut m = HashMap::new();
                            m.insert("text".to_string(), Value::Str(f.text.clone()));
                            // An exact key match is confidence 1.0, not a
                            // similarity. A cosine neighbour at 0.87 is a guess
                            // and must never wear the same number.
                            m.insert("score".to_string(), Value::Float(1.0));
                            m.insert("source".to_string(), Value::Str(f.source.clone()));
                            Value::Map(m)
                        })
                        .collect();

                    self.trace_op(pc, &format!(
                        "QUERY {:?} -> {} chunk(s)", mention, chunks.len()));
                    self.queried.push((mention.clone(), chunks.len()));
                    self.stack.push(Value::Array(chunks));
                }

                op::REMEMBER => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&name).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("REMEMBER {} ({})", name, val));
                    // Hippocampal store under the register's own name.
                    self.memory.store(&name, val.clone());
                    self.storage.insert(name.clone(), val);
                }

                op::STORE => {
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let key = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&reg).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("STORE {} → {}", reg, key));
                    // Associative store: key-hypervector addresses the payload.
                    self.memory.store(&key, val.clone());
                    self.storage.insert(key, val);
                }

                op::RECALL => {
                    // `recall "key"` (1-arg): operand0 is the key; there is
                    // no explicit destination, so the recalled value parks
                    // in a register addressed by that same key operand.
                    // `recall reg, "key"` (2-arg): operand0 is the
                    // destination register and operand1 is the key --
                    // the SAME layout STORE uses (STORE's `reg` operand0 /
                    // `key` operand1), so select operands by arity the same
                    // way STORE does rather than always reading operand0.
                    let (dest, key) = if operands.len() >= 2 {
                        let dest = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                        let key = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                        (dest, key)
                    } else {
                        let key = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                        (key.clone(), key)
                    };
                    // Associative cleanup: nearest stored key by cosine similarity.
                    match self.memory.recall(&key) {
                        Some((matched, val, sim)) => {
                            self.trace_op(pc, &format!("RECALL {} → {} (sim {:.3}) = {}", key, matched, sim, val));
                            self.registers.insert(dest, val);
                        }
                        None => {
                            self.trace_op(pc, &format!("RECALL {} → (empty)", key));
                            self.registers.insert(dest, Value::Null);
                        }
                    }
                }

                op::FORGET => {
                    let key = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dropped = self.memory.forget(&key);
                    self.storage.remove(&key);
                    self.trace_op(pc, &format!("FORGET {} ({})", key, if dropped { "dropped" } else { "absent" }));
                }

                op::BIND_ROLE => {
                    // Genuine VSA role-filler binding. The register key is the
                    // operand's display token (matching every other op's
                    // register addressing); the role and filler resolve to
                    // their real symbol strings (via the name table) so the
                    // codebook produces stable, human-recoverable vectors.
                    // reg <- bundle(reg, bind(role, filler)).
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = self.resolve_symbol_name(operands.get(1));
                    let filler = self.resolve_symbol_name(operands.get(2));
                    self.trace_op(pc, &format!("BIND_ROLE {} {} {}", reg, role, filler));
                    self.vsa_bind_into(&reg, &role, &filler);
                }

                op::COMPARE => {
                    // Operands: [a, b, kind_imm] (3-arg from a condition) or
                    // [a, b] (2-arg statement-level compare, defaults to eq).
                    let kind = match operands.get(2) {
                        Some(Operand::Imm(k)) => *k,
                        _ => 0, // statement-level `compare a, b` → equality test
                    };

                    // FIXED 2026-09-14 -- string comparison was silently
                    // always TRUE.
                    //
                    // Both sides used to be forced through `resolve_i64`, and
                    // `Value::as_i64` returns 0 for EVERY `Value::Str` (see
                    // its `_ => 0` arm). So any comparison between two strings
                    // compared 0 to 0: `if (intent == "question")` took the
                    // then-branch for EVERY value of `intent`, with no error
                    // and no trace signal. That is precisely the silent
                    // wrong-answer class this VM exists to prevent, and it is
                    // why `tests/str_semantics.rs`'s
                    // `string_equality_against_a_parameter_discriminates` was
                    // pinned `#[ignore]`. It also made real `match` arm
                    // selection impossible to build: every string arm would
                    // have matched, so the first arm would always win.
                    //
                    // `resolve_value` already preserves strings correctly
                    // (Named -> the register's current value; Global -> the
                    // literal's recorded spelling via the name table), so when
                    // BOTH sides resolve to strings, compare them AS strings.
                    //
                    // Deliberately scoped to the both-strings case. A mixed
                    // Str/Int comparison keeps the numeric path, and
                    // `va.as_i64()` is value-identical to the old
                    // `resolve_i64(op)` for every operand kind (Imm/FloatImm
                    // pass through; a Named register resolves to the same
                    // value; anything unresolved is 0 either way) -- so no
                    // existing numeric comparison changes behaviour.
                    //
                    // KNOWN REMAINING GAP: kind 6 (truthiness) still tests
                    // `as_i64() != 0`, so a non-empty string is still falsy.
                    // Left alone on purpose -- it is a separate change with a
                    // different blast radius, and the test that would cover it
                    // (`str_contains_evaluates_on_a_register_string`) is
                    // blocked on `str.contains` being a stub, not on this.
                    let va = self.resolve_value(operands.get(0));
                    let vb = if kind == 6 {
                        Value::Int(0)
                    } else {
                        self.resolve_value(operands.get(1))
                    };

                    let cmp_word;
                    match (&va, &vb) {
                        (Value::Str(x), Value::Str(y)) => {
                            flag = match kind {
                                0 => x == y,
                                1 => x != y,
                                2 => x < y,
                                3 => x <= y,
                                4 => x > y,
                                5 => x >= y,
                                _ => false,
                            };
                            cmp_word = match kind {
                                0 | 1 => if x == y { "equal" }
                                         else if x < y { "less" }
                                         else { "greater" },
                                _ => if flag { "true" } else { "false" },
                            };
                            self.trace_op(pc, &format!(
                                "COMPARE {:?} ? {:?} (kind {}, str) → {}", x, y, kind, flag));
                        }
                        _ => {
                            let a = va.as_i64();
                            let b = vb.as_i64();
                            flag = match kind {
                                0 => a == b,
                                1 => a != b,
                                2 => a < b,
                                3 => a <= b,
                                4 => a > b,
                                5 => a >= b,
                                6 => a != 0, // truthiness
                                _ => false,
                            };
                            cmp_word = match kind {
                                0 | 1 => if a == b { "equal" }
                                         else if a < b { "less" }
                                         else { "greater" },
                                _ => if flag { "true" } else { "false" },
                            };
                            self.trace_op(pc, &format!(
                                "COMPARE {} ? {} (kind {}) → {}", a, b, kind, flag));
                        }
                    }
                    // Expose the comparison result in a conventional register too.
                    self.registers.insert("__cmp".into(), Value::Str(cmp_word.into()));
                    self.accumulator = Value::Bool(flag);
                }

                op::LABEL => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("LABEL {}", name));
                    // Runtime no-op; targets were resolved in the pre-scan.
                }

                op::JMP => {
                    let key = operands.get(0).and_then(|o| o.label_key());
                    match key.and_then(|k| label_map.get(&k)) {
                        Some(&target) => {
                            self.trace_op(pc, &format!("JMP → 0x{:04x}", target));
                            if jump_budget == 0 {
                                return ExecResult::Error("jump budget exhausted (possible infinite loop)".into());
                            }
                            jump_budget -= 1;
                            self.jumps_taken += 1;
                            pc = target;
                        }
                        None => {
                            self.trace_op(pc, "JMP (unresolved label, falling through)");
                        }
                    }
                }

                op::COND => {
                    // Conditional jump: if the comparison flag is FALSE, jump to
                    // the target label (skip the then-body / exit the loop).
                    let key = operands.get(0).and_then(|o| o.label_key());
                    if !flag {
                        match key.and_then(|k| label_map.get(&k)) {
                            Some(&target) => {
                                self.trace_op(pc, &format!("COND false → jump 0x{:04x}", target));
                                if jump_budget == 0 {
                                    return ExecResult::Error("jump budget exhausted (possible infinite loop)".into());
                                }
                                jump_budget -= 1;
                                self.jumps_taken += 1;
                                pc = target;
                            }
                            None => {
                                self.trace_op(pc, "COND false (unresolved label)");
                            }
                        }
                    } else {
                        self.trace_op(pc, "COND true → fall through");
                    }
                }

                op::LOOP => {
                    // Structural loop marker (for-each). The VM executes the body
                    // as a single structured pass; the exit label is carried for
                    // well-formedness. (Iterating a value-level collection source
                    // is a documented follow-up.)
                    self.trace_op(pc, "LOOP (structural marker)");
                }

                op::CALL => {
                    // Plan-2 intra-program CALL, extended by Task 3 with
                    // `use`/`override` resolution.
                    //   operands[0] = global(fn_name); operands[1..] = arg values.
                    //
                    // Resolution precedence (design doc, "THE CRUX"):
                    //   1. the program's own function, if it's a COMPILE-TIME-
                    //      VALIDATED `override` (CompiledFunction::is_override)
                    //      AND a `use`'d module still exposes `fn_name` as
                    //      overridable  -> the program's override wins.
                    //   2. else, a `use`'d module exposing `fn_name` at all
                    //      -> the registry's native implementation.
                    //   3. else, the program's own function (today's
                    //      pre-Task-3 behaviour, unchanged) -> intra-program
                    //      bytecode call.
                    //   4. else -> error. Deny-by-default: a module NOT
                    //      named in `prog.used_modules` is invisible at step
                    //      2 even if the registry otherwise knows it
                    //      (`ModuleRegistry::resolve`).
                    let key = operands.get(0).and_then(|o| match o {
                        Operand::Global(h) | Operand::Named(h) => Some(*h),
                        _ => None,
                    });
                    let fn_name = match key.and_then(|h| self.name_table.get(&h).cloned()) {
                        Some(n) => n,
                        None => {
                            self.trace_op(pc, "CALL (unresolved name)");
                            return ExecResult::Error("CALL: unresolved function name".into());
                        }
                    };

                    if self.call_depth >= MAX_CALL_DEPTH {
                        return ExecResult::Error(format!(
                            "call depth exceeded ({}); recursion guard tripped at {}",
                            MAX_CALL_DEPTH, fn_name));
                    }

                    let prog_name = match self.current_program.clone() {
                        Some(p) => p,
                        None => return ExecResult::Error(
                            "CALL: no current program context".into()),
                    };
                    let prog = match self.programs.get(&prog_name) {
                        Some(p) => p.clone(),
                        None => return ExecResult::Error(format!(
                            "CALL: program `{}` not loaded", prog_name)),
                    };

                    // Resolve args up front -- shared by every branch below.
                    // Each operand becomes a Value via the same path used by
                    // ASSIGN's RHS (Imm/Float/Named/Global).
                    let mut arg_values: Vec<Value> = Vec::new();
                    for i in 1..operands.len() {
                        arg_values.push(self.resolve_assign_rhs(Some(&operands[i])));
                    }

                    let own_fn = prog.functions.iter().find(|f| f.name == fn_name).cloned();
                    // 2026-10-24: `@system` is "only callable by the VM runtime" -- the
                    // program's own code is not the runtime. (`@internal`, `private`,
                    // `@restricted`, `@ratelimit` govern OUTSIDE callers and are
                    // admitted here: a program may always call itself.)
                    if own_fn.as_ref().is_some_and(|f| f.access == Access::System) {
                        return ExecResult::Error(format!(
                            "{}.{} is @system: only the VM runtime may call it, not the program's own code",
                            prog_name, fn_name));
                    }
                    // (bool, fn ptr) is Copy: pulling both out of the
                    // registry entry here ends the immutable borrow of
                    // `self.registry` before any branch below needs
                    // `&mut self` again.
                    let module_hit: Option<(bool, crate::vm::registry::NativeFn)> = self
                        .registry
                        .resolve(&prog.used_modules, &fn_name)
                        .map(|entry| (entry.overridable, entry.call));
                    let is_valid_override = own_fn.as_ref().is_some_and(|f| f.is_override)
                        && module_hit.is_some_and(|(overridable, _)| overridable);

                    if is_valid_override {
                        let callee = own_fn.expect("is_valid_override implies own_fn is Some");
                        self.trace_op(pc, &format!(
                            "CALL {} (program override, {} arg(s))", fn_name, arg_values.len()));
                        match self.exec_intra_call(&callee, arg_values) {
                            None => {}
                            Some(ExecResult::Suspend(s)) => return self.caller_waits(s, func, pc, flag, jump_budget),
                            Some(other) => return other,
                        }
                    } else if let Some((_, native)) = module_hit {
                        // Task 6: a registry (native) function gets its OWN
                        // argument resolution, `resolve_registry_arg` --
                        // deliberately NOT the shared `arg_values` above
                        // (still used unchanged by the other two branches).
                        // See that method's doc for why: a native module
                        // like `vsa` is symbol- and register-KEY aware by
                        // nature (`recover(frame, SUBJECT)` names a
                        // register and a role, it does not want their
                        // resolved VALUES), matching how BIND_ROLE/UNBIND's
                        // own bytecode arms already read their operands.
                        let native_args: Vec<Value> = (1..operands.len())
                            .map(|i| self.resolve_registry_arg(Some(&operands[i])))
                            .collect();
                        self.trace_op(pc, &format!(
                            "CALL {} (registry impl, {} arg(s))", fn_name, native_args.len()));
                        let result = native(self, &native_args);
                        self.stack.push(result);
                    } else if let Some(callee) = own_fn {
                        self.trace_op(pc, &format!("CALL {} ({} arg(s))", fn_name, arg_values.len()));
                        match self.exec_intra_call(&callee, arg_values) {
                            None => {}
                            Some(ExecResult::Suspend(s)) => return self.caller_waits(s, func, pc, flag, jump_budget),
                            Some(other) => return other,
                        }
                    } else {
                        return ExecResult::Error(format!(
                            "CALL: function `{}` not found in program `{}` (and no `use`'d \
                             module exposes it)", fn_name, prog_name));
                    }
                }

                // ── Data-movement opcodes (value-level semantics) ──────────

                op::DESTROY => {
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.registers.remove(&name);
                    self.trace_op(pc, &format!("DESTROY {}", name));
                }

                op::COPY => {
                    let src = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dst = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let val = self.registers.get(&src).cloned().unwrap_or(Value::Null);
                    self.trace_op(pc, &format!("COPY {} → {}", src, dst));
                    self.registers.insert(dst, val);
                }

                op::TRANSFER => {
                    // TRANSFER src, dst, amount: move `amount` from src to dst.
                    let src = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let dst = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    let amount = self.resolve_i64(operands.get(2));
                    let s = self.registers.get(&src).map(|v| v.as_i64()).unwrap_or(0);
                    let d = self.registers.get(&dst).map(|v| v.as_i64()).unwrap_or(0);
                    self.registers.insert(src.clone(), Value::Int(s - amount));
                    self.registers.insert(dst.clone(), Value::Int(d + amount));
                    self.trace_op(pc, &format!("TRANSFER {} → {} ({})", src, dst, amount));
                }

                op::UNBIND => {
                    // Genuine VSA unbind + cleanup. Recover the filler bound to
                    // `role` in the register's bundled hypervector and leave the
                    // recovered symbol (as a Str) in the register. Candidates are
                    // the frame's OWN bound fillers (WO-1.2; the global name
                    // table only as a fallback for a frame with no recorded
                    // bindings); the cosine-nearest wins. A register that never
                    // held an Hvec is left untouched.
                    let reg = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let role = self.resolve_symbol_name(operands.get(1));
                    // WO-1.2: go through `vsa_unbind_cleanup_known` rather than
                    // building the candidate pool inline. This arm used to
                    // assemble `name_table.values()` itself, which meant the
                    // "what do we clean up against" policy lived in two places
                    // and the registry module's `recover` and the raw UNBIND
                    // opcode could drift apart. One definition now.
                    match self.vsa_unbind_cleanup_known(&reg, &role) {
                        Some((sym, sim)) => {
                            self.trace_op(pc, &format!("UNBIND {} {} → {} (sim {:.3})", reg, role, sym, sim));
                            self.registers.insert(reg, Value::Str(sym));
                        }
                        None => {
                            // VM step 0: an absent role leaves Null in the
                            // register, structurally -- the register no longer
                            // holds the frame, and a later read sees Null, not
                            // a hypervector that resolves to 0. A register
                            // that never held a frame is left untouched.
                            let reason = self.last_recover_reason
                                .map(|r| r.as_str()).unwrap_or("no hvec");
                            self.trace_op(pc, &format!("UNBIND {} {} → Null ({})", reg, role, reason));
                            if self.last_recover_reason == Some(RecoverReason::AbsentRole) {
                                self.registers.insert(reg, Value::Null);
                            }
                        }
                    }
                }

                op::NEWVAR => {
                    // Fresh type/value variable — create an uninitialised register.
                    let name = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    self.registers.entry(name.clone()).or_insert(Value::Null);
                    self.trace_op(pc, &format!("NEWVAR {}", name));
                }

                // ── Type-inference & reasoning opcodes ─────────────────────
                // These operate at the VSA / semantic level (the trunk binding
                // head + arena per the architecture), not at the VM's value
                // level. The VM records them as well-formed structural steps so
                // a thought-program executes and verifies cleanly; it does not
                // fabricate value-level semantics it cannot faithfully provide.

                op::UNIFY => {
                    let a = operands.get(0).map(|o| o.as_name()).unwrap_or_default();
                    let b = operands.get(1).map(|o| o.as_name()).unwrap_or_default();
                    self.trace_op(pc, &format!("UNIFY {} {}", a, b));
                }
                op::INST => self.trace_structural(pc, "INST", &operands),
                op::GEN => self.trace_structural(pc, "GEN", &operands),
                op::SEQ => self.trace_structural(pc, "SEQ", &operands),
                op::DIFF => self.trace_structural(pc, "DIFF", &operands),
                op::DETECT_PATTERN => self.trace_structural(pc, "DETECT_PATTERN", &operands),
                op::PREDICT => self.trace_structural(pc, "PREDICT", &operands),
                op::MATCH => self.trace_structural(pc, "MATCH", &operands),
                op::DEBATE => self.trace_structural(pc, "DEBATE", &operands),
                op::DISCOVER => self.trace_structural(pc, "DISCOVER", &operands),
                op::DECODE => self.trace_structural(pc, "DECODE", &operands),
                op::SCORE => self.trace_structural(pc, "SCORE", &operands),
                op::SPECIALIZE => self.trace_structural(pc, "SPECIALIZE", &operands),
                op::REWARD => self.trace_structural(pc, "REWARD", &operands),
                op::INFER => self.trace_structural(pc, "INFER", &operands),
                op::MERGE => self.trace_structural(pc, "MERGE", &operands),
                op::SPLIT => self.trace_structural(pc, "SPLIT", &operands),
                op::FILTER => self.trace_structural(pc, "FILTER", &operands),
                op::MAP_ROLES => self.trace_structural(pc, "MAP_ROLES", &operands),
                op::REDUCE => self.trace_structural(pc, "REDUCE", &operands),
                op::TEMPORAL_BIND => self.trace_structural(pc, "TEMPORAL_BIND", &operands),
                op::ANALOGY => self.trace_structural(pc, "ANALOGY", &operands),
                op::BROADCAST => self.trace_structural(pc, "BROADCAST", &operands),
                op::EXPLORE => self.trace_structural(pc, "EXPLORE", &operands),
                op::FORGE => self.trace_structural(pc, "FORGE", &operands),
                op::ASK => {
                    // ASK <question>, <candidate>...
                    //
                    // Suspend: hand the question outward and stop. This is the
                    // THIRD outcome, alongside Ok and Error. A program holding
                    // several grounded candidates must not pick one (that is a
                    // confabulation) nor fail (nothing went wrong). It asks.
                    //
                    // `pc` already points past this instruction, so `resume`
                    // continues from here; the answer arrives on the STACK
                    // (same convention as CALL), bound by a following `pop`.
                    // The question is a string literal, which compiles to
                    // Operand::Global (a 2-byte hash). `resolve_symbol_name`
                    // reverses it via the name table populated on load.
                    // Falls back to the `g_xxxx` token if it was never
                    // registered -- deterministic, just not human-readable.
                    let question = Value::Str(self.resolve_symbol_name(operands.get(0)));
                    let mut candidates: Vec<Value> = operands
                        .iter()
                        .skip(1)
                        .map(|o| self.resolve_value(Some(o)))
                        .collect();
                    // `ask "which?", hits;` where `hits` is the array QUERY
                    // pushed means "choose among these", not "here is one
                    // candidate that happens to be a list". Flatten it, or the
                    // caller can never name a selection and `resume`'s guard
                    // rejects every possible answer.
                    if candidates.len() == 1 {
                        if let Value::Array(items) = &candidates[0] {
                            candidates = items.clone();
                        }
                    }
                    self.trace_op(pc, &format!("ASK {} ({} candidates)", question, candidates.len()));
                    return ExecResult::Suspend(Suspension {
                        question,
                        candidates,
                        pc,
                        flag,
                        jump_budget,
                        function: func.name.clone(),
                        program: self.current_program.clone().unwrap_or_default(),
                        callers: Vec::new(),
                        inner: None,
                    });
                }
                op::SYNC => self.trace_structural(pc, "SYNC", &operands),

                _ => {
                    // A truly unknown opcode byte means malformed bytecode — a
                    // real invariant violation, not something to skip silently.
                    self.trace_op(pc, &format!("UNKNOWN 0x{:02x}", opcode));
                    return ExecResult::Error(format!("unknown opcode 0x{:02x} at pc {}", opcode, pc));
                }
            }
        }

        // Return the accumulator as the result
        ExecResult::Ok(self.accumulator.clone())
    }

    /// Execute an intra-program bytecode callee exactly as `op::CALL` always
    /// has (Plan-2, factored out unchanged by Task 3 so the SAME snapshot/
    /// restore/depth-guard logic backs both call sites that need it now: a
    /// program's own plain function, and a validated `override`). Installs a
    /// fresh `arg0..argN` register scope, executes, restores the caller's
    /// registers, and on success pushes the result onto `self.stack` (the
    /// caller delivers it into a binding via POP) -- matching how CALL
    /// always has.
    ///
    /// Returns `None` on success (result already on the stack), or the
    /// callee's `Error` or `Suspend` for the caller to return.
    ///
    /// Arguments bind to the callee's DECLARED parameter names, as `call()`
    /// binds a host's arguments, plus the `argN` alias. (Until 2026-10-24
    /// only `argN` was bound, so `helper(x)` read an unbound `x` inside
    /// `helper` and returned Int(0) -- silently.)
    ///
    /// A callee that suspends (ASK) is no longer an error (2026-10-24): the
    /// callee's registers go into the suspension (as its `inner` scope when it
    /// asked itself, or as the state of the innermost waiting caller when it
    /// was waiting on a deeper call), the caller's registers are restored, and
    /// the CALL arm adds the caller's own resume point (`caller_waits`).
    fn exec_intra_call(&mut self, callee: &CompiledFunction, arg_values: Vec<Value>) -> Option<ExecResult> {
        let caller_regs = std::mem::take(&mut self.registers);
        // WO-1.2: the frame map is register-keyed, so it has to follow the
        // registers exactly. Without this a callee's `bind frame, ...` would
        // leave its fillers in the caller's pool for a register of the same
        // name -- reintroducing cross-frame contamination through the back
        // door, which is the very thing the scoping exists to remove.
        let caller_frames = std::mem::take(&mut self.vsa_frames);
        for (i, v) in arg_values.into_iter().enumerate() {
            if let Some(param) = callee.params.get(i) {
                self.register_name(param);
                self.registers.insert(Self::register_key(param), v.clone());
            }
            let alias = format!("arg{}", i);
            self.register_name(&alias);
            self.registers.insert(Self::register_key(&alias), v);
        }

        self.call_depth += 1;
        let r = self.exec_function(callee);
        self.call_depth -= 1;

        // Restore caller-visible registers, and the frame map with them,
        // keeping the callee's in case it suspended.
        let callee_regs = std::mem::replace(&mut self.registers, caller_regs);
        let callee_frames = std::mem::replace(&mut self.vsa_frames, caller_frames);

        match r {
            ExecResult::Ok(v) | ExecResult::Return(v) => {
                self.stack.push(v);
                None
            }
            ExecResult::Error(e) => Some(ExecResult::Error(e)),
            ExecResult::Suspend(mut s) => {
                let state = FrameState { registers: callee_regs, vsa_frames: callee_frames };
                match s.callers.last_mut() {
                    None => s.inner = Some(state),
                    Some(waiting) => waiting.state = Some(state),
                }
                Some(ExecResult::Suspend(s))
            }
        }
    }

    /// A callee suspended inside this function's CALL: record where this
    /// function resumes (`pc` is already past the CALL) and pass the
    /// suspension up. Its registers are recorded one level up, by the
    /// `exec_intra_call` that called it -- or stay in the VM when it is the
    /// host's entry function.
    fn caller_waits(&self, mut s: Suspension, func: &CompiledFunction, pc: usize, flag: bool,
                    jump_budget: u64) -> ExecResult {
        s.callers.push(CallerFrame {
            program: self.current_program.clone().unwrap_or_default(),
            function: func.name.clone(),
            pc,
            flag,
            jump_budget,
            state: None,
        });
        ExecResult::Suspend(s)
    }

    /// Trace a reasoning/VSA opcode as a well-formed structural step. The
    /// operands are rendered for trace fidelity but no value-level mutation is
    /// performed (these resolve in the trunk's hyperdimensional machinery).
    fn trace_structural(&mut self, pc: usize, name: &str, operands: &[Operand]) {
        if self.trace_enabled {
            let args: Vec<String> = operands.iter().map(|o| o.as_name()).collect();
            self.trace.push(format!("0x{:04x}: {} [{}]", pc, name, args.join(", ")));
        }
    }

    fn trace_op(&mut self, offset: usize, desc: &str) {
        if self.trace_enabled {
            self.trace.push(format!("0x{:04x}: {}", offset, desc));
        }
    }

    /// Resolve an operand to an i64 value: immediates pass through; named
    /// registers are looked up in the register file (0 if unset).
    fn resolve_i64(&self, op: Option<&Operand>) -> i64 {
        match op {
            Some(Operand::Imm(n)) => *n,
            Some(Operand::FloatImm(f)) => *f as i64,
            Some(o) => self.registers.get(&o.as_name()).map(|v| v.as_i64()).unwrap_or(0),
            None => 0,
        }
    }

    /// Resolve an operand to a Value, preserving int vs float so arithmetic
    /// can promote correctly. Named operands resolve to the register's value.
    /// Global operands (string literals -- e.g. `return "ok";`) resolve via
    /// `resolve_symbol_name`, the same name-table path ASK's question text
    /// and BIND_ROLE/UNBIND's role/filler symbols already use to reverse a
    /// 2-byte hash to its real spelling. Without this arm, RETURN fell
    /// through to the wildcard below and silently returned `Int(0)` for any
    /// string literal. `resolve_symbol_name` falls back to the `g_xxxx`
    /// token only for a genuinely unrecorded literal -- not the normal case,
    /// since `emit_global` always records the spelling before emitting the
    /// operand.
    fn resolve_value(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o @ Operand::Named(_)) => {
                self.registers.get(&o.as_name()).cloned().unwrap_or(Value::Int(0))
            }
            Some(o @ Operand::Global(_)) => Value::Str(self.resolve_symbol_name(Some(o))),
            _ => Value::Int(0),
        }
    }

    /// Resolve an ASSIGN right-hand-side -- also the resolver shared by
    /// MAKE_ARRAY's element list and intra-program CALL's argument list (see
    /// `op::MAKE_ARRAY` and `op::CALL`'s `arg_values`). Imm/FloatImm pass
    /// through as literal values; Named copies the source register's value.
    /// Global operands (string literals -- e.g. `assign x = "hello"`)
    /// resolve via `resolve_symbol_name`, the same name-table path
    /// `resolve_value` (RETURN/ADD/SUB/...) and BIND_ROLE/UNBIND's
    /// role/filler symbols already use to reverse a 2-byte hash to its real
    /// spelling. Without this arm, a Global operand fell back to the raw
    /// `g_xxxx` token whenever nothing happened to be bound at that
    /// register key -- the normal case for a fresh literal, since nothing
    /// ever assigns INTO a hash-keyed register -- so ASSIGN, MAKE_ARRAY
    /// string elements, and string-literal CALL args all silently carried
    /// the hash token instead of the literal's real text.
    /// `resolve_symbol_name` falls back to `g_xxxx` only for a genuinely
    /// unrecorded literal -- not the normal case, since `emit_global`
    /// always records the spelling via `record_symbol` before emitting the
    /// operand.
    fn resolve_assign_rhs(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o @ Operand::Named(_)) => {
                self.registers.get(&o.as_name()).cloned().unwrap_or(Value::Null)
            }
            Some(o @ Operand::Global(_)) => Value::Str(self.resolve_symbol_name(Some(o))),
            Some(other) => other.to_value(),
            None => Value::Null,
        }
    }

    fn register_name(&mut self, name: &str) {
        let h = blake3::hash(name.as_bytes());
        self.name_table.insert([h.as_bytes()[0], h.as_bytes()[1]], name.to_string());
    }

    /// The register key a symbol compiles to: `r_` + the first two bytes of its
    /// blake3 hash, matching `Operand::Named(h).as_name()`. Hosts must use this
    /// to reach a register the bytecode refers to by name.
    pub fn register_key(name: &str) -> String {
        let h = blake3::hash(name.as_bytes());
        format!("r_{:02x}{:02x}", h.as_bytes()[0], h.as_bytes()[1])
    }

    /// Reverse a Named/Role/Global operand's 2-byte hash to the original symbol
    /// string via the name table (populated from the program's symbol_names on
    /// load). Falls back to the operand's display token (`role_xxyy` etc.) when
    /// the name was never registered — so binding is still deterministic, just
    /// not human-readable. Used by BIND_ROLE/UNBIND to drive the VSA codebook.
    fn resolve_symbol_name(&self, op: Option<&Operand>) -> String {
        match op {
            Some(Operand::Role(h)) | Some(Operand::Named(h)) | Some(Operand::Global(h)) => {
                self.name_table.get(h).cloned().unwrap_or_else(|| {
                    op.map(|o| o.as_name()).unwrap_or_default()
                })
            }
            Some(o) => o.as_name(),
            None => String::new(),
        }
    }

    /// Task 6: resolve a CALL argument bound for a REGISTRY (native)
    /// function -- distinct from `resolve_assign_rhs` (still used, unchanged,
    /// for ASSIGN and for intra-program/`override` CALL args -- see
    /// `op::CALL`'s dispatch, which computes a SEPARATE argument list only
    /// for the registry-native branch via this method).
    ///
    /// A native module is symbol- and register-KEY aware by nature:
    /// `recover(frame, SUBJECT)` means "the register named frame, the role
    /// named SUBJECT", not "frame's current value, SUBJECT's current
    /// value" -- exactly how `BIND_ROLE`/`UNBIND`'s own bytecode arms
    /// already read their operands (`resolve_symbol_name`, never a
    /// register-value lookup, for a role/reg operand). Reusing that same
    /// resolution here is what lets `vsa::recover` genuinely perform
    /// UNBIND+cleanup instead of receiving `Value::Null` for a bareword
    /// like `SUBJECT` that was never meant to be a register in the first
    /// place -- `resolve_assign_rhs`'s plain register-value lookup has no
    /// way to produce that string, since nothing ever binds a register
    /// literally named `SUBJECT`.
    ///
    /// Imm/FloatImm literals still pass through as real Int/Float values
    /// (unlike `resolve_symbol_name` alone, whose fallback would stringify
    /// them as `imm_5`/`fimm_5`) -- kept general enough for a future
    /// registry function that wants an actual number, not just names.
    fn resolve_registry_arg(&self, op: Option<&Operand>) -> Value {
        match op {
            Some(Operand::Imm(n)) => Value::Int(*n),
            Some(Operand::FloatImm(f)) => Value::Float(*f),
            Some(o) => Value::Str(self.resolve_symbol_name(Some(o))),
            None => Value::Null,
        }
    }

    /// Get the value of a register.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.registers.get(name)
    }

    /// Get the accumulator (last SUM result).
    pub fn result(&self) -> &Value {
        &self.accumulator
    }

    /// Reset the VM state.
    pub fn reset(&mut self) {
        self.registers.clear();
        self.stack.clear();
        self.accumulator = Value::Null;
        self.trace.clear();
        self.events.clear();
    }

    // ── Genuine VSA role-filler binding ──────────────────────────────────
    // These give BIND_ROLE/UNBIND real hyperdimensional semantics: a register
    // holds a *bundled hypervector* of bind(role, filler) terms, and a role is
    // recovered by unbind + codebook cleanup -- the bind(sentence, inv(ROLE))
    // round-trip from the VSA/HDC literature. Built on the bind/bundle/cosine
    // primitives in hypervec.rs; the codebook (self.vsa) maps role and filler
    // symbols to deterministic quasi-orthogonal hypervectors.

    /// The hypervector for a filler symbol (deterministic via the codebook).
    pub fn vsa_symbol_vec(&mut self, symbol: &str) -> Hypervec {
        self.vsa.get_cloned(symbol)
    }

    /// The hypervector for a role symbol. Roles and fillers share one codebook;
    /// a `role:` prefix keeps the role namespace from colliding with fillers of
    /// the same spelling (so a filler "SUBJECT" and the role SUBJECT differ).
    pub fn vsa_role_vec(&mut self, role: &str) -> Hypervec {
        self.vsa.get_cloned(&format!("role:{}", role))
    }

    /// BIND_ROLE: bundle `bind(role, filler)` into register `reg`.
    ///
    /// If `reg` is empty/non-hvec it becomes exactly `bind(role, filler)`;
    /// otherwise the new pair is bundled (majority-vote superposition) with the
    /// existing structure, so multiple roles compose into one vector:
    ///   reg = bundle(reg, bind(role_i, filler_i))
    pub fn vsa_bind_into(&mut self, reg: &str, role: &str, filler: &str) {
        let rv = self.vsa_role_vec(role);
        let fv = self.vsa_symbol_vec(filler);
        let pair = rv.bind(&fv);
        let combined = match self.registers.get(reg) {
            Some(Value::Hvec(existing)) => Hypervec::bundle(&[existing, &pair]),
            _ => pair,
        };
        self.registers.insert(reg.to_string(), Value::Hvec(combined));
        // WO-1.2: remember WHAT went into this frame, so the cleanup can be
        // scoped to it. Deduplicated and order-preserving -- the order is the
        // tie-break when two candidates score identically, and bind order is
        // the least surprising one.
        let frame = self.vsa_frames.entry(reg.to_string()).or_default();
        if !frame.fillers.iter().any(|f| f == filler) {
            frame.fillers.push(filler.to_string());
        }
        // VM step 0: the roles too, so "was this role ever bound here" is a
        // lookup, not a similarity.
        if !frame.roles.iter().any(|r| r == role) {
            frame.roles.push(role.to_string());
        }
    }

    /// The roles bound into `reg` so far (empty for a register with no
    /// recorded frame). What `vsa_unbind_cleanup_known` consults before it
    /// computes anything.
    pub fn frame_roles(&self, reg: &str) -> Vec<String> {
        self.vsa_frames.get(reg).map(|f| f.roles.clone()).unwrap_or_default()
    }

    /// The register's declared name where the name table knows it, else the
    /// hashed key itself -- for the report, which a person reads.
    fn readable_reg(&self, reg: &str) -> String {
        let hex = reg.strip_prefix("r_").or_else(|| reg.strip_prefix("g_"));
        let Some(hex) = hex else { return reg.to_string() };
        if hex.len() != 4 { return reg.to_string() }
        let (Ok(a), Ok(b)) = (u8::from_str_radix(&hex[..2], 16), u8::from_str_radix(&hex[2..], 16)) else {
            return reg.to_string();
        };
        self.name_table.get(&[a, b]).cloned().unwrap_or_else(|| reg.to_string())
    }

    fn record_recover(&mut self, reg: &str, role: &str, winner: Option<&str>,
                      similarity: Option<f64>, runner_up: Option<f64>, pool: usize,
                      reason: RecoverReason) {
        let margin = match (similarity, runner_up) {
            (Some(s), Some(r)) => Some(s - r),
            _ => None,
        };
        self.last_recover_similarity = similarity;
        self.last_recover_runner_up = runner_up;
        self.last_recover_reason = Some(reason);
        self.recovers.push(RecoverRecord {
            reg: self.readable_reg(reg),
            role: role.to_string(),
            winner: winner.map(str::to_string),
            similarity,
            runner_up,
            margin,
            pool,
            reason,
        });
    }

    /// UNBIND + cleanup: recover the filler bound to `role` in register `reg`.
    ///
    /// Computes `unbind(reg_hvec, role)` (= bind, self-inverse) and returns the
    /// candidate symbol whose codebook vector is cosine-nearest to the result,
    /// with that similarity. `None` if `reg` holds no hypervector. A high
    /// similarity means a real binding was present; near-zero means the role
    /// was not bound (the unbind yields quasi-orthogonal noise).
    ///
    /// Task 7 (Feature A, similarity-surfacing): also mirrors its own
    /// winning similarity (or `None`) into `self.last_recover_similarity`.
    /// This is the one chokepoint both `op::UNBIND` and the `vsa` registry
    /// module's `recover()` funnel through, so neither call site needs its
    /// own bookkeeping to make the score observable to callers upstream.
    pub fn vsa_unbind_cleanup(
        &mut self,
        reg: &str,
        role: &str,
        candidates: &[&str],
    ) -> Option<(String, f64)> {
        let bound = match self.registers.get(reg) {
            Some(Value::Hvec(h)) => h.clone(),
            _ => {
                self.record_recover(reg, role, None, None, None, candidates.len(), RecoverReason::NoFrame);
                return None;
            }
        };
        if candidates.is_empty() {
            self.record_recover(reg, role, None, None, None, 0, RecoverReason::EmptyPool);
            return None;
        }
        let rv = self.vsa_role_vec(role);
        let recovered = bound.unbind(&rv);
        // VM step 0: keep the runner-up too. The margin between the two is
        // what tells a clean recovery from a near-tie the threshold alone
        // would let through.
        let mut best: Option<(String, f64)> = None;
        let mut second: Option<f64> = None;
        for &cand in candidates {
            let cv = self.vsa_symbol_vec(cand);
            let sim = recovered.cosine_sim(&cv);
            match &best {
                Some((_, bs)) if *bs >= sim => {
                    if second.map_or(true, |s| sim > s) { second = Some(sim); }
                }
                _ => {
                    if let Some((_, bs)) = &best { second = Some(*bs); }
                    best = Some((cand.to_string(), sim));
                }
            }
        }
        let (winner, sim) = best.as_ref().map(|(w, s)| (w.as_str(), *s)).unzip();
        self.record_recover(reg, role, winner, sim, second, candidates.len(), RecoverReason::Recovered);
        best
    }

    /// `vsa_unbind_cleanup` against the frame's OWN bound fillers.
    ///
    /// WO-1.2 (2026-09-14). This used to clean up against every symbol the VM
    /// had ever seen as a name. The pool is the semantics here, because the
    /// cleanup always returns its nearest candidate and never returns
    /// "nothing": with a global pool, a role that was never bound in this
    /// frame comes back as whatever unrelated string sits closest to
    /// quasi-orthogonal noise.
    ///
    /// Measured on a frame holding SUBJECT/OBJECT/VERB, with a second frame's
    /// fillers also in the VM's name table:
    ///
    /// | | winner | similarity |
    /// |---|---|---|
    /// | genuine `SUBJECT`, global pool | `"cat"` | 0.2700 |
    /// | genuine `SUBJECT`, frame pool  | `"cat"` | 0.2700 |
    /// | absent `LOCATION`, global pool | `"umbrella"` (another frame's) | 0.0215 |
    /// | absent `LOCATION`, frame pool  | one of this frame's own | see `tests/recover_scope.rs` |
    ///
    /// A genuine recovery is unchanged -- same unbind, same true filler, and
    /// the true filler is always in its own frame's pool -- so the tau_vm
    /// values CubbyLLM calibrates against (1.0 / 0.4736328125 / 0.22021484375)
    /// do not move for any answer that was going to be spoken. What changes is
    /// only which wrong symbol wins when the role is absent.
    ///
    /// **Fallback, deliberate:** a register holding a hypervector with no
    /// recorded bindings falls back to the global pool. That happens for
    /// hand-assembled bytecode and for a frame restored from outside
    /// `vsa_bind_into`, where scoping to an empty pool would silently turn a
    /// working recovery into `None`. Narrowing behaviour is worth having; a
    /// silent regression to Null is not.
    ///
    /// **VM step 0 (2026-09-29), the absent role is structural.** A register
    /// with a recorded frame answers "was `role` ever bound here" by lookup
    /// BEFORE any unbind: not among the frame's roles => `None`, reason
    /// `AbsentRole`, no similarity computed. The tau thresholds now guard
    /// only crosstalk among roles that are present. The global-pool fallback
    /// above has no role record, so it keeps the old behaviour (a similarity
    /// the caller must read).
    pub fn vsa_unbind_cleanup_known(&mut self, reg: &str, role: &str) -> Option<(String, f64)> {
        let frame = self.vsa_frames.get(reg).cloned().unwrap_or_default();
        if !frame.roles.is_empty() && !frame.roles.iter().any(|r| r == role) {
            let pool = frame.fillers.len();
            self.record_recover(reg, role, None, None, None, pool, RecoverReason::AbsentRole);
            return None;
        }
        let candidates: Vec<String> = if frame.fillers.is_empty() {
            self.name_table.values().cloned().collect()
        } else {
            frame.fillers
        };
        let cand_refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
        self.vsa_unbind_cleanup(reg, role, &cand_refs)
    }
}

// ── Operand decoding ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Operand {
    Named([u8; 2]),
    Imm(i64),
    FloatImm(f64),
    Type([u8; 2]),
    Role([u8; 2]),
    Global([u8; 2]),
    None,
}

impl Operand {
    fn as_name(&self) -> String {
        match self {
            Operand::Named(h) => format!("r_{:02x}{:02x}", h[0], h[1]),
            Operand::Global(h) => format!("g_{:02x}{:02x}", h[0], h[1]),
            Operand::Type(h) => format!("t_{:02x}{:02x}", h[0], h[1]),
            Operand::Role(h) => format!("role_{:02x}{:02x}", h[0], h[1]),
            Operand::Imm(n) => format!("imm_{}", n),
            Operand::FloatImm(f) => format!("fimm_{}", f),
            Operand::None => "_".into(),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Operand::Imm(n) => Value::Int(*n),
            Operand::FloatImm(f) => Value::Float(*f),
            Operand::Named(h) => Value::Str(format!("r_{:02x}{:02x}", h[0], h[1])),
            Operand::Global(h) => Value::Str(format!("g_{:02x}{:02x}", h[0], h[1])),
            _ => Value::Null,
        }
    }

    /// The raw 2-byte hash key for a Global/Named operand (used as a label /
    /// jump-target identifier). `None` for immediates and absent operands.
    fn label_key(&self) -> Option<[u8; 2]> {
        match self {
            Operand::Global(h) | Operand::Named(h) => Some(*h),
            _ => None,
        }
    }
}

/// Numeric binary operation with int/float promotion. If either operand is a
/// float, compute in f64 and return a Float (collapsing to Int when the result
/// is integral, so whole-valued results print cleanly). Otherwise integer math.
fn num_binop(a: &Value, b: &Value, op: char) -> Value {
    let a_is_float = matches!(a, Value::Float(_));
    let b_is_float = matches!(b, Value::Float(_));
    if a_is_float || b_is_float {
        let x = a.as_f64();
        let y = b.as_f64();
        let r = match op {
            '+' => x + y,
            '-' => x - y,
            '*' => x * y,
            '/' => x / y,
            _ => 0.0,
        };
        // Collapse integral floats back to Int for clean output / comparison.
        if r.fract() == 0.0 && r.is_finite() {
            Value::Int(r as i64)
        } else {
            Value::Float(r)
        }
    } else {
        // Integer operands. Division uses TRUE division (matching GSM8K's
        // `<<60/100=0.6>>` semantics): if the quotient is non-integral, promote
        // to Float; otherwise stay Int. +,-,* stay integer.
        let x = a.as_i64();
        let y = b.as_i64();
        match op {
            '+' => Value::Int(x + y),
            '-' => Value::Int(x - y),
            '*' => Value::Int(x * y),
            '/' => {
                if y != 0 && x % y == 0 {
                    Value::Int(x / y)
                } else if y != 0 {
                    Value::Float(x as f64 / y as f64)
                } else {
                    Value::Int(0)
                }
            }
            _ => Value::Int(0),
        }
    }
}

/// Pre-scan the bytecode for LABEL opcodes, mapping each label's 2-byte hash
/// to the byte offset of the LABEL instruction itself. JMP/COND resolve their
/// target label through this map. Mirrors the main loop's opcode/operand
/// stride so offsets line up exactly.
fn scan_labels(bc: &[u8]) -> std::collections::HashMap<[u8; 2], usize> {
    let mut map = std::collections::HashMap::new();
    let mut pc = 0usize;
    while pc < bc.len() {
        let here = pc;
        let opcode = bc[pc]; pc += 1;
        if opcode == op::SKIP {
            continue;
        }
        if pc >= bc.len() { break; }
        let n_ops = bc[pc] as usize; pc += 1;
        let mut first_key: Option<[u8; 2]> = None;
        for i in 0..n_ops {
            if pc >= bc.len() { break; }
            let (operand, consumed) = decode_operand(&bc[pc..]);
            if i == 0 {
                first_key = operand.label_key();
            }
            pc += consumed;
        }
        if opcode == op::LABEL {
            if let Some(k) = first_key {
                map.insert(k, here);
            }
        }
    }
    map
}

fn decode_operand(bytes: &[u8]) -> (Operand, usize) {
    if bytes.is_empty() {
        return (Operand::None, 0);
    }
    match bytes[0] {
        0x01 => { // Named
            if bytes.len() >= 3 {
                (Operand::Named([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x02 => { // Imm (i64 LE)
            if bytes.len() >= 9 {
                let val = i64::from_le_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4],
                    bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                (Operand::Imm(val), 9)
            } else {
                (Operand::None, 1)
            }
        }
        0x03 => { // Type
            if bytes.len() >= 3 {
                (Operand::Type([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x04 => { // Role
            if bytes.len() >= 3 {
                (Operand::Role([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x05 => { // Global
            if bytes.len() >= 3 {
                (Operand::Global([bytes[1], bytes[2]]), 3)
            } else {
                (Operand::None, 1)
            }
        }
        0x06 => { // FloatImm (f64 LE)
            if bytes.len() >= 9 {
                let val = f64::from_le_bytes([
                    bytes[1], bytes[2], bytes[3], bytes[4],
                    bytes[5], bytes[6], bytes[7], bytes[8],
                ]);
                (Operand::FloatImm(val), 9)
            } else {
                (Operand::None, 1)
            }
        }
        0x00 => { // None
            if bytes.len() >= 3 {
                (Operand::None, 3)
            } else {
                (Operand::None, 1)
            }
        }
        _ => (Operand::None, 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Task 4: these fixtures dropped a decorative `implements ISolver` that
    // predates real `implements` validation -- every test here exercises VM
    // opcode/execution semantics unrelated to interfaces, and none defined
    // parse/verify, so the claim was already false before this task, just
    // never checked.

    fn compile_and_load(source: &str) -> VM {
        let programs = crate::compiler::compile(source).unwrap();
        let mut vm = VM::new();
        for prog in programs {
            vm.load(prog);
        }
        vm
    }

    #[test]
    fn test_vm_basic_arithmetic() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    query x;
                    remember x;
                }
            }
        "#);

        vm.trace_enabled = true;
        let result = vm.call("Test", "solve", Vec::new());

        // Print trace
        for line in &vm.trace {
            println!("{}", line);
        }

        match result {
            ExecResult::Ok(val) => {
                println!("Result: {}", val);
                assert_eq!(val.as_i64(), 9, "16 - 3 - 4 = 9");
            }
            ExecResult::Error(e) => panic!("execution error: {}", e),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn test_vm_push_pop_chain() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    push x;
                    create y : number;
                    pop y;
                    mul y, 2;
                    sum y;
                    query y;
                    remember y;
                }
            }
        "#);

        let result = vm.call("Test", "solve", Vec::new());

        match result {
            ExecResult::Ok(val) => {
                println!("Result: {}", val);
                // x = 16-3-4 = 9, push 9, pop into y, y*2 = 18
                assert_eq!(val.as_i64(), 18, "chain: (16-3-4)*2 = 18");
            }
            ExecResult::Error(e) => panic!("execution error: {}", e),
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn test_vm_division_by_zero() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    div x, 0;
                }
            }
        "#);

        let result = vm.call("Test", "solve", Vec::new());
        match result {
            ExecResult::Error(e) => assert!(e.contains("division by zero")),
            _ => panic!("expected division by zero error"),
        }
    }

    #[test]
    fn test_vm_store_recall() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 42;
                    remember x;
                }
            }
        "#);

        vm.call("Test", "solve", Vec::new());

        // The register should be in storage after REMEMBER
        assert!(vm.storage.values().any(|v| v.as_i64() == 42));
    }

    #[test]
    fn test_vm_multiple_functions() {
        let mut vm = compile_and_load(r#"
            program Calc implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 10;
                    add x, 20;
                    sum x;
                }
                public function mul_numbers(): void {
                    create y : number;
                    assign y = 5;
                    mul y, 6;
                    sum y;
                }
            }
        "#);

        let r1 = vm.call("Calc", "solve", Vec::new());
        match r1 {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 30),
            _ => panic!("expected ok"),
        }

        let r2 = vm.call("Calc", "mul_numbers", Vec::new());
        match r2 {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 30),
            _ => panic!("expected ok"),
        }
    }

    #[test]
    fn test_vm_load_cubebin() {
        // Compile, save to cubebin, load from cubebin, execute
        let programs = crate::compiler::compile(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 100;
                    sub x, 37;
                    sum x;
                }
            }
        "#).unwrap();

        let path = std::env::temp_dir().join("test_vm_exec.cubebin");
        programs[0].save(&path).unwrap();

        let mut vm = VM::new();
        let name = vm.load_file(&path).unwrap();
        assert_eq!(name, "Test");

        let result = vm.call("Test", "solve", Vec::new());
        match result {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 63, "100 - 37 = 63"),
            _ => panic!("expected ok"),
        }

        let _ = std::fs::remove_file(&path);
    }

    // Plan-1 (RETURN). Early `return v` must halt execution AND yield v.
    // Probe: a "poison" `sum poison` after the return rewrites the accumulator
    // if fall-through happens, so the returned value distinguishes halt vs fall.
    #[test]
    fn return_early_halts_with_value() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create r : number;
                    create poison : number;
                    assign r = 1;
                    assign poison = 99;
                    return r;
                    sum poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 1,
                    "early return must yield 1, not the poisoned 99 (= fall-through)"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn return_early_inside_if_skips_rest() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create r : number;
                    create poison : number;
                    assign r = 5;
                    assign poison = 99;
                    if (r > 0) {
                        return r;
                    }
                    sum poison;
                    return poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 5,
                "return inside if must halt before sum poison runs"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn return_bare_yields_current_accumulator() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    create poison : number;
                    assign x = 7;
                    assign poison = 99;
                    sum x;
                    return;
                    sum poison;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 7,
                "bare return must yield the accumulator set by sum x, not the poisoned sum"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-4 (LOOP for-each). `for (let x of arr) { body }` lowers to an
    // index-based while using LEN/INDEX/COMPARE/COND/JMP. The loop must
    // iterate every element, terminate on empty arrays, and nest correctly.
    #[test]
    fn for_each_sums_three_elements() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, x;
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 60),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn for_each_empty_array_runs_zero_iterations() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        add total, 999;
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 0),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn for_each_nested_loops_visit_every_pair() {
        // sum over (xi + yj) for xi in [1,2], yj in [10,20] = 4*1 + 4*2 ... actually
        // = (1+10)+(1+20)+(2+10)+(2+20) = 11+21+12+22 = 66.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [1, 2];
                    let ys = [10, 20];
                    create total : number;
                    assign total = 0;
                    for (let x of xs) {
                        for (let y of ys) {
                            add total, x;
                            add total, y;
                        }
                    }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 66),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-3 (MAKE_ARRAY / LEN / INDEX). Array literals must construct a
    // real Value::Array, `arr.len()` must return its length, and `arr[i]`
    // must index it. Out-of-range indexing yields Null.
    #[test]
    fn array_literal_len_is_three() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let n = xs.len();
                    sum n;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 3),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn array_index_returns_element() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let x = xs[1];
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 20),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn array_index_out_of_range_is_null() {
        // Out-of-range indexing produces Null; sum yields 0 (as_i64 of Null).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    let xs = [10, 20, 30];
                    let x = xs[99];
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) => assert_eq!(v.as_i64(), 0,
                "out-of-range index must be Null, not panic"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    // Plan-2 (CALL). Intra-program call must transfer arg0..argN, execute the
    // callee, and deliver its return value to the caller via the stack.
    #[test]
    fn call_returns_value_via_stack() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function dbl(): number {
                    create r : number;
                    assign r = 0;
                    add r, arg0;
                    add r, arg0;
                    return r;
                }
                public function solve(input: str): void {
                    let y = dbl(21);
                    sum y;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 42, "dbl(21) must return 42"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_caller_locals_isolated() {
        // The callee writes to a register with the same name as a caller local.
        // After return, the caller's value must be intact.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function helper(): number {
                    create r : number;
                    assign r = 99;
                    return r;
                }
                public function solve(input: str): void {
                    create r : number;
                    assign r = 7;
                    let _t = helper();
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 7,
                    "caller's local r must still be 7 after helper() (got {:?})", v),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_recursion_factorial() {
        // fact(n) = if n <= 1 then 1 else n * fact(n-1)
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function fact(): number {
                    create n : number;
                    assign n = 0;
                    add n, arg0;
                    if (n > 1) {
                        create m : number;
                        assign m = 0;
                        add m, n;
                        sub m, 1;
                        let sub = fact(m);
                        create out : number;
                        assign out = 0;
                        add out, n;
                        mul out, sub;
                        return out;
                    }
                    return 1;
                }
                public function solve(input: str): void {
                    let r = fact(5);
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) | ExecResult::Return(v) =>
                assert_eq!(v.as_i64(), 120, "fact(5) must return 120"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn call_depth_guard_returns_clean_error() {
        // Unbounded self-recursion should produce a clean Error, not a native
        // stack overflow. fact() without a base case keeps calling.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function loop_forever(): number {
                    let r = loop_forever();
                    return r;
                }
                public function solve(input: str): void {
                    let r = loop_forever();
                    sum r;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Error(e) => assert!(e.to_lowercase().contains("depth"),
                "expected depth-guard error, got: {}", e),
            other => panic!("expected depth-guard Error, got {:?}", other),
        }
    }

    // P0-2 regression: `assign b = a` must copy a's value, not store the
    // register-name token. Before the fix, ASSIGN called `to_value()` on a
    // Named operand and stored "r_xxxx" (a string) — sum returned 0 / a name.
    #[test]
    fn assign_register_to_register_copies_value() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    create b : number;
                    assign a = 5;
                    assign b = a;
                    sum b;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 5,
                "assign b = a must propagate the value 5 (got {:?})", v),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn assign_register_to_register_then_arithmetic() {
        // Compose with arithmetic so a regression also breaks the chain.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    create b : number;
                    assign a = 7;
                    assign b = a;
                    add b, 3;
                    sum b;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 10),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn assign_literal_int_unchanged() {
        // Guard: the bug fix must NOT regress literal assignment.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    assign a = 42;
                    sum a;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 42),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_trace() {
        let mut vm = compile_and_load(r#"
            program Test implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 5;
                    add x, 3;
                    sum x;
                }
            }
        "#);

        vm.trace_enabled = true;
        vm.call("Test", "solve", Vec::new());

        assert!(!vm.trace.is_empty());
        assert!(vm.trace.iter().any(|t| t.contains("CREATE")));
        assert!(vm.trace.iter().any(|t| t.contains("ASSIGN")));
        assert!(vm.trace.iter().any(|t| t.contains("ADD")));
        assert!(vm.trace.iter().any(|t| t.contains("SUM")));
    }

    #[test]
    fn test_vm_if_then_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 5;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 100, "x=5, x>3 true → then-branch → 100"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_if_else_branch() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create x : number;
                    assign x = 2;
                    if (x > 3) { assign x = 100; } else { assign x = 200; }
                    sum x;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 200, "x=2, x>3 false → else-branch → 200"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_while_loop_terminates_and_accumulates() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create total : number;
                    assign total = 0;
                    create i : number;
                    assign i = 5;
                    while (i > 0) { add total, 10; sub i, 1; }
                    sum total;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 50, "5 iterations × 10 = 50"),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_compare_sets_cmp_register() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create a : number;
                    assign a = 7;
                    create b : number;
                    assign b = 7;
                    compare a, b;
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());
        assert_eq!(vm.get("__cmp").map(|v| v.as_str()), Some("equal".to_string()));
    }

    #[test]
    fn test_vm_float_arithmetic() {
        // 80000 * 1.5 = 120000 (real fractional multiply, integral result).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 80000;
                    mul v, 1.5;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 120000),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_float_nonintegral_result() {
        // 7 / 2 = 3.5 — non-integral float preserved.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 7;
                    div v, 2.0;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 3.5).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_true_division_int_operands() {
        // 60 / 100 = 0.6 (true division on integer operands, not floor → 0).
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 60;
                    div v, 100;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert!((v.as_f64() - 0.6).abs() < 1e-9, "got {}", v.as_f64()),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_exact_int_division_stays_int() {
        // 16 / 2 = 8 stays integer.
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create v : number;
                    assign v = 16;
                    div v, 2;
                    sum v;
                }
            }
        "#);
        match vm.call("T", "solve", Vec::new()) {
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 8),
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn test_vm_cubebin_save_load_run() {
        // Save to cubebin, load, and run -- the roundtrip this test has always been
        // about. It used `examples/gsm8k.cube` as its fixture until 2026-09-14, when
        // the C11 operand fix made that file correctly refuse to compile (its
        // constructor binds a MapLit, its solve returns a StructLit). gsm8k was never
        // computing anything here anyway -- this test's own comment said it "won't
        // compute correctly without real input parsing" -- so the fixture is now a
        // self-contained program that does compile, exercising the same VM paths:
        // storage, a constructor, and a solve that returns a real register.
        let source = r#"
            program Bin implements ISolver {
                storage {
                    count: mutable u64 = 0;
                }
                public function constructor() {
                    self.count = 0;
                }
                @external
                public function solve(input: Input): Output {
                    create x : number;
                    assign x = 16;
                    sub x, 3;
                    sub x, 4;
                    sum x;
                    query x;
                    remember x;
                    return x;
                }
                @external
                public function parse(raw: str): Input { return raw; }
                @external
                public pure function verify(input: Input, output: Output): bool { return true; }
            }
        "#;
        let programs = crate::compiler::compile(source).unwrap();

        let path = std::env::temp_dir().join("bin_vm_test.cubebin");
        programs[0].save(&path).unwrap();

        let mut vm = VM::new();
        vm.load_file(&path).unwrap();

        // Run constructor
        vm.call("Bin", "constructor", Vec::new());

        // Run solve -- unlike the old gsm8k fixture, this one computes a real value
        vm.trace_enabled = true;
        let result = vm.call("Bin", "solve", Vec::new());

        println!("Bin.solve() trace ({} ops):", vm.trace.len());
        for line in &vm.trace {
            println!("  {}", line);
        }

        match result {
            // 16 - 3 - 4 = 9: the fixture computes, so assert it rather than
            // accepting any outcome the way the gsm8k fixture had to.
            ExecResult::Ok(v) => assert_eq!(v.as_i64(), 9, "Bin.solve() should compute 16-3-4"),
            ExecResult::Error(e) => panic!("Bin.solve() error: {}", e),
            _ => {}
        }

        let _ = std::fs::remove_file(&path);
    }
}


#[cfg(test)]
mod vsa_binding_tests {
    //! Genuine VSA role-filler binding over hypervectors.
    //!
    //! These drive the VM's hyperdimensional capability directly (not via
    //! bytecode): a register can hold a bundled hypervector built from
    //! bind(role, filler) terms, and a role can be unbound + cleaned up against
    //! a candidate filler set to recover the original symbol -- the
    //! bind(sentence, inverse(ROLE)) round-trip. The primitives live in
    //! hypervec.rs; this wires them into the VM as Value::Hvec + helper methods.
    use super::*;

    // A bound register must hold an actual hypervector, not a string key.
    #[test]
    fn bind_role_produces_hypervector_value() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sentence", "SUBJECT", "cat");
        match vm.get("sentence") {
            Some(Value::Hvec(h)) => assert_eq!(h.dim(), MEM_DIM,
                "bound register must hold a MEM_DIM hypervector"),
            other => panic!("expected Value::Hvec, got {:?}", other),
        }
    }

    // Single role: unbind(bind(ROLE, filler), ROLE) ~= filler, recovered by
    // cleanup against the candidate set at similarity 1.0 (self-inverse bind).
    #[test]
    fn single_role_unbinds_to_filler() {
        let mut vm = VM::new();
        vm.vsa_bind_into("s", "SUBJECT", "cat");
        let candidates = ["cat", "dog", "mouse", "tree"];
        let (sym, sim) = vm.vsa_unbind_cleanup("s", "SUBJECT", &candidates)
            .expect("must recover a filler");
        assert_eq!(sym, "cat", "unbinding SUBJECT must recover 'cat'");
        assert!(sim > 0.99, "single-term unbind must be near-exact, got sim={sim:.4}");
    }

    // The headline example: cat chases mouse, bundled into one vector, each
    // role recoverable. With 3 bundled terms the recovered filler is the
    // cosine-nearest candidate (noisy but correct -- the cleanup memory's job).
    #[test]
    fn three_role_sentence_recovers_each_filler() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sent", "SUBJECT", "cat");
        vm.vsa_bind_into("sent", "VERB", "chases");
        vm.vsa_bind_into("sent", "OBJECT", "mouse");

        let cand = ["cat", "dog", "chases", "eats", "mouse", "tree", "runs"];
        let (s, ss) = vm.vsa_unbind_cleanup("sent", "SUBJECT", &cand).unwrap();
        let (v, vs) = vm.vsa_unbind_cleanup("sent", "VERB", &cand).unwrap();
        let (o, os) = vm.vsa_unbind_cleanup("sent", "OBJECT", &cand).unwrap();
        assert_eq!(s, "cat", "SUBJECT must clean up to cat (sim {ss:.3})");
        assert_eq!(v, "chases", "VERB must clean up to chases (sim {vs:.3})");
        assert_eq!(o, "mouse", "OBJECT must clean up to mouse (sim {os:.3})");
    }

    // Querying a role the sentence does not contain must NOT confidently
    // return a bound filler: the best cleanup similarity should be low
    // (quasi-orthogonal noise), distinguishing "absent" from a real hit.
    #[test]
    fn absent_role_yields_low_similarity() {
        let mut vm = VM::new();
        vm.vsa_bind_into("sent", "SUBJECT", "cat");
        vm.vsa_bind_into("sent", "OBJECT", "mouse");
        let cand = ["cat", "mouse", "dog"];
        // LOCATION was never bound -> unbinding it yields noise vs all candidates.
        let best = vm.vsa_unbind_cleanup("sent", "LOCATION", &cand);
        if let Some((_, sim)) = best {
            assert!(sim < 0.3,
                "unbinding an absent role must be low-similarity noise, got {sim:.4}");
        }
    }

    // Determinism: same symbols -> same hypervector across VM instances
    // (fixed codebook seed), so bound structures are reproducible across runs.
    #[test]
    fn role_and_symbol_vectors_are_deterministic() {
        let mut vm1 = VM::new();
        let mut vm2 = VM::new();
        let a = vm1.vsa_symbol_vec("cat");
        let b = vm2.vsa_symbol_vec("cat");
        assert_eq!(a, b, "same symbol must map to same hypervector across VMs");
        let r1 = vm1.vsa_role_vec("SUBJECT");
        let r2 = vm2.vsa_role_vec("SUBJECT");
        assert_eq!(r1, r2, "same role must map to same hypervector across VMs");
        // Distinct symbols are quasi-orthogonal (so cleanup can separate them).
        assert!(a.cosine_sim(&vm1.vsa_symbol_vec("dog")).abs() < 0.1,
            "distinct fillers must be quasi-orthogonal");
    }
}


#[cfg(test)]
mod vsa_opcode_wiring_tests {
    //! End-to-end: `bind` statements in real CubeLang source must produce
    //! genuine VSA hypervector bindings in the target register (not string-key
    //! inserts), recoverable by unbinding the role. This drives the compiler
    //! to retain role/filler symbol names (so the VM can reverse the 2-byte
    //! hashes the bytecode carries) and the BIND_ROLE/UNBIND opcode arms to
    //! call the vsa_* methods.
    use super::*;

    fn compile_and_load(source: &str) -> VM {
        let programs = crate::compiler::compile(source).unwrap();
        let mut vm = VM::new();
        for prog in programs {
            vm.load(prog);
        }
        vm
    }

    // After `bind sentence, SUBJECT, "cat"`, the register `sentence` must hold
    // a Value::Hvec -- a real bound hypervector -- not a Str/Null.
    #[test]
    fn bind_statement_produces_hypervector_register() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create sentence : frame;
                    bind sentence, SUBJECT, "cat";
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());
        // The register key is the hashed name; find the single Hvec value.
        let has_hvec = vm.registers.values()
            .any(|v| matches!(v, Value::Hvec(_)));
        assert!(has_hvec,
            "after `bind`, some register must hold a Value::Hvec (genuine VSA bind)");
    }

    // The headline round-trip from source: bind three roles into one register,
    // then each role unbinds+cleans-up to the right filler. We reach into the
    // VM's vsa methods for cleanup using the SAME register the bytecode wrote.
    #[test]
    fn bound_frame_recovers_fillers_via_unbind() {
        let mut vm = compile_and_load(r#"
            program T implements ISolve {
                public function solve(input: str): void {
                    create frame : frame;
                    bind frame, SUBJECT, "cat";
                    bind frame, VERB, "chases";
                    bind frame, OBJECT, "mouse";
                }
            }
        "#);
        vm.call("T", "solve", Vec::new());

        // Resolve the register the compiler used for `frame` (hashed name key).
        let reg_key = vm.registers.iter()
            .find(|(_, v)| matches!(v, Value::Hvec(_)))
            .map(|(k, _)| k.clone())
            .expect("a bound hypervector register must exist");

        let cand = ["cat", "dog", "chases", "eats", "mouse", "tree"];
        let (s, _) = vm.vsa_unbind_cleanup(&reg_key, "SUBJECT", &cand).unwrap();
        let (v, _) = vm.vsa_unbind_cleanup(&reg_key, "VERB", &cand).unwrap();
        let (o, _) = vm.vsa_unbind_cleanup(&reg_key, "OBJECT", &cand).unwrap();
        assert_eq!(s, "cat", "SUBJECT must recover cat");
        assert_eq!(v, "chases", "VERB must recover chases");
        assert_eq!(o, "mouse", "OBJECT must recover mouse");
    }

    // UNBIND opcode (engine-level): after binding, executing UNBIND on the role
    // must remove/replace the bound structure with the recovered filler symbol
    // in the target register. We hand-build the bytecode since no surface
    // syntax emits UNBIND yet, mirroring the compiler's operand layout.
    #[test]
    fn unbind_opcode_recovers_filler_into_register() {
        use crate::compiler::{op, CompiledProgram, CompiledFunction};

        // Bytecode for: BIND_ROLE frame, SUBJECT, "cat"; UNBIND frame, SUBJECT
        // operand tags: NAMED=0x01, ROLE=0x04, GLOBAL=0x05
        fn h(s: &str) -> [u8; 2] {
            let d = blake3::hash(s.as_bytes());
            [d.as_bytes()[0], d.as_bytes()[1]]
        }
        let frame = h("frame");
        let subj = h("SUBJECT");
        let cat = h("cat");

        let mut bc: Vec<u8> = Vec::new();
        // BIND_ROLE (3 operands): NAMED frame, ROLE SUBJECT, GLOBAL "cat"
        bc.push(op::BIND_ROLE); bc.push(3);
        bc.push(0x01); bc.push(frame[0]); bc.push(frame[1]);
        bc.push(0x04); bc.push(subj[0]);  bc.push(subj[1]);
        bc.push(0x05); bc.push(cat[0]);   bc.push(cat[1]);
        // UNBIND (2 operands): NAMED frame, ROLE SUBJECT
        bc.push(op::UNBIND); bc.push(2);
        bc.push(0x01); bc.push(frame[0]); bc.push(frame[1]);
        bc.push(0x04); bc.push(subj[0]);  bc.push(subj[1]);

        let func = CompiledFunction {
            name: "run".to_string(),
            params: Vec::new(),
            bytecode: bc,
            labels: Vec::new(),
            debug_lines: Vec::new(),
            is_override: false,
            access: Access::External,
            once: false,
            restricted: None,
            ratelimit: None,
            cron: None,
            param_types: Vec::new(),
            return_type: crate::types::TypeSpec::Any,
        };
        let prog = CompiledProgram {
            name: "T".to_string(),
            implements: vec!["ISolver".to_string()],
            functions: vec![func],
            storage_fields: Vec::new(),
            symbol_names: vec![
                "frame".to_string(), "SUBJECT".to_string(), "cat".to_string(),
            ],
            used_modules: Vec::new(),
            types: Default::default(),
        };

        let mut vm = VM::new();
        vm.load(prog);
        vm.call("T", "run", Vec::new());

        // After UNBIND, the frame register should carry the recovered filler
        // symbol "cat" (as a Str), the cleanup result of unbinding SUBJECT.
        let frame_key = format!("r_{:02x}{:02x}", frame[0], frame[1]);
        match vm.get(&frame_key) {
            Some(Value::Str(s)) => assert_eq!(s, "cat",
                "UNBIND must leave the recovered filler 'cat' in the register"),
            other => panic!("expected recovered Str(\"cat\"), got {:?}", other),
        }
    }
}
