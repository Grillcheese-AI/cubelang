//! The permission contract (2026-10-24). Before this, `@restricted`, `@ratelimit` and `@cron` parsed
//! and did nothing, so a self-written skill tagged `@restricted` was restricted in name only. Now:
//!
//! - `@restricted(A, B)`: only those callers (or a container's grants) may enter;
//! - `@ratelimit(n, "period")`: n entries per period, per caller, kept by the resident host;
//! - `@cron("interval")`: only the scheduler runs it (`tick`), never a caller;
//! - `private`: not callable from outside the program; `@system` not even from inside it;
//! - the contract survives `.cubebin` (v4 trailer);
//! - over `run-proto`, a caller name counts only with ML-DSA-87 + Ed25519 signatures by a keyring
//!   identity, and a locked process runs unsigned requests only against deployed contracts.

use cubelang::auth::{self, CallClaim, Identity, Kind, Keyring, Protect, Purpose};
use cubelang::compiler::{self, Access, CompiledProgram};
use cubelang::reasoning::{Caller, RunRequest, RunResult, run_result};
use cubelang::vm::{ExecResult, VM, Value};
use prost::Message;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const CONTRACT: &str = r#"
program Guarded implements ISolve {
    @external
    public function solve(x: str): str { return x; }
    @restricted(Emitter450m, "Planner")
    public function emit(x: str): str { return x; }
    @ratelimit(2, "1m")
    public function limited(x: str): str { return x; }
    @cron("1h")
    public function nightly(): str { return "swept"; }
    private function helper(): str { return "inner"; }
    public function uses_helper(x: str): str { return helper(); }
    @system
    function lifecycle(x: str): str { return x; }
    public function calls_system(x: str): str { return lifecycle(x); }
}
"#;

fn load(src: &str) -> (VM, String) {
    let mut progs = compiler::compile_strict(src).unwrap_or_else(|e| panic!("{e}"));
    let prog = progs.remove(0);
    let name = prog.name.clone();
    let mut vm = VM::new();
    vm.load(prog);
    (vm, name)
}

fn arg() -> Vec<Value> {
    vec![Value::Str("a".into())]
}

fn ok(r: &ExecResult) -> bool {
    matches!(r, ExecResult::Ok(_) | ExecResult::Return(_))
}

fn err(r: ExecResult) -> String {
    match r {
        ExecResult::Error(e) => e,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// ── in the VM ───────────────────────────────────────────────────────────────

#[test]
fn restricted_admits_only_the_named_callers_and_grants() {
    let (mut vm, p) = load(CONTRACT);
    let e = err(vm.call_entry(&p, "emit", arg()));
    assert!(e.contains("@restricted to Emitter450m, Planner") && e.contains("anonymous"), "{e}");
    for who in ["Emitter450m", "Planner"] {
        vm.caller = Some(who.into());
        assert!(ok(&vm.call_entry(&p, "emit", arg())), "{who} is on the list");
    }
    vm.caller = Some("Intruder".into());
    assert!(err(vm.call_entry(&p, "emit", arg())).contains("Intruder may not call"));
    vm.grant(&p, "emit", "Intruder");
    assert!(ok(&vm.call_entry(&p, "emit", arg())), "a grant admits");
    vm.revoke(&p, "emit", "Intruder");
    assert!(err(vm.call_entry(&p, "emit", arg())).contains("may not call"), "a revoke takes it back");
    // an unrestricted function admits anyone, as before
    vm.caller = None;
    assert!(ok(&vm.call_entry(&p, "solve", arg())));
}

#[test]
fn ratelimit_counts_per_caller_and_refused_calls_do_not_count() {
    let (mut vm, p) = load(CONTRACT);
    let t0 = Instant::now();
    vm.caller = Some("A".into());
    assert!(vm.admit_entry(&p, "limited", t0).is_ok());
    assert!(vm.admit_entry(&p, "limited", t0 + Duration::from_secs(1)).is_ok());
    let e = vm.admit_entry(&p, "limited", t0 + Duration::from_secs(2)).unwrap_err();
    assert!(e.contains("@ratelimit(2, \"1m\")"), "{e}");
    vm.caller = Some("B".into());
    assert!(vm.admit_entry(&p, "limited", t0 + Duration::from_secs(2)).is_ok(), "B has its own window");
    vm.caller = Some("A".into());
    assert!(vm.admit_entry(&p, "limited", t0 + Duration::from_secs(30)).is_err());
    assert!(vm.admit_entry(&p, "limited", t0 + Duration::from_secs(60)).is_ok(),
            "the first call left the window; the refused ones never entered it");
}

#[test]
fn cron_runs_only_from_the_scheduler_and_on_its_interval() {
    let (mut vm, p) = load(CONTRACT);
    assert!(err(vm.call_entry(&p, "nightly", vec![])).contains("@cron"));
    let t0 = Instant::now();
    let runs = vm.tick(&p, t0);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0, "nightly");
    assert!(matches!(&runs[0].1, ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) if s == "swept"),
            "{:?}", runs[0].1);
    assert!(vm.tick(&p, t0 + Duration::from_secs(30 * 60)).is_empty(), "not due yet");
    assert_eq!(vm.tick(&p, t0 + Duration::from_secs(61 * 60)).len(), 1);
}

#[test]
fn private_is_inside_only_and_system_is_runtime_only() {
    let (mut vm, p) = load(CONTRACT);
    assert!(err(vm.call_entry(&p, "helper", vec![])).contains("private"));
    // (A no-argument helper: CALL v1 passes arguments wrongly -- `return helper(x);` comes back
    // Int(0) -- which the call-frame work, task 113, fixes. Access is what this test is about.)
    match vm.call_entry(&p, "uses_helper", arg()) {
        ExecResult::Return(Value::Str(s)) | ExecResult::Ok(Value::Str(s)) => assert_eq!(s, "inner"),
        other => panic!("the program itself may call its private function: {other:?}"),
    }
    assert!(err(vm.call_entry(&p, "calls_system", arg())).contains("@system"),
            "not even the program calls a @system function");
}

#[test]
fn a_malformed_contract_does_not_compile() {
    for (src, says) in [
        ("@external private function f(x: str): str { return x; }", "both `private` and `@external`"),
        ("@ratelimit(0, \"1m\") public function f(x: str): str { return x; }", "at least 1"),
        ("@ratelimit(5, \"soon\") public function f(x: str): str { return x; }", "period"),
        ("@cron(\"0s\") public function f(): str { return \"x\"; }", "@cron"),
    ] {
        let program = format!("program P implements ISolve {{\n    {src}\n}}\n");
        for e in [compiler::compile_strict(&program).err(), compiler::compile(&program).err().map(|e| e.to_string())] {
            let e = e.unwrap_or_else(|| panic!("must not compile: {src}"));
            assert!(e.contains(says), "{src}: {e}");
        }
    }
}

#[test]
fn the_contract_survives_cubebin() {
    let progs = compiler::compile_strict(CONTRACT).unwrap();
    let path = std::env::temp_dir().join(format!("contracts_round_trip_{}.cubebin", std::process::id()));
    progs[0].save(&path).unwrap();
    let back = CompiledProgram::load(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    let f = |n: &str| back.functions.iter().find(|f| f.name == n).unwrap().clone();
    assert_eq!(f("emit").restricted, Some(vec!["Emitter450m".to_string(), "Planner".to_string()]));
    assert_eq!(f("limited").ratelimit, Some((2, 60_000)));
    assert_eq!(f("nightly").cron, Some(3_600_000));
    assert_eq!(f("helper").access, Access::Internal);
    assert_eq!(f("lifecycle").access, Access::System);
    let mut vm = VM::new();
    let name = back.name.clone();
    vm.load(back);
    assert!(err(vm.call_entry(&name, "emit", arg())).contains("@restricted"), "still enforced after a reload");
}

// ── over run-proto ──────────────────────────────────────────────────────────

/// One resident run-proto process: every request in order on one pipe, every result back.
fn proto(args: &[&str], reqs: Vec<RunRequest>) -> Vec<RunResult> {
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let mut child = Command::new(exe).arg("run-proto").args(args)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let n = reqs.len();
    for r in reqs {
        let mut buf = Vec::new();
        r.encode(&mut buf).unwrap();
        stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
        stdin.write_all(&buf).unwrap();
    }
    drop(stdin);
    let mut out = Vec::new();
    child.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    let mut errtext = String::new();
    child.stderr.take().unwrap().read_to_string(&mut errtext).unwrap();
    assert!(child.wait().unwrap().success(), "run-proto failed: {errtext}");
    let mut results = Vec::new();
    let mut at = 0;
    while at < out.len() {
        let len = u32::from_be_bytes(out[at..at + 4].try_into().unwrap()) as usize;
        results.push(RunResult::decode(&out[at + 4..at + 4 + len]).unwrap());
        at += 4 + len;
    }
    assert_eq!(results.len(), n);
    results
}

fn error_of(r: &RunResult) -> String {
    match &r.result {
        Some(run_result::Result::Error(e)) => e.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn symbol_of(r: &RunResult) -> String {
    assert!(r.ok, "{r:?}");
    match &r.result {
        Some(run_result::Result::Symbol(s)) => s.clone(),
        other => panic!("expected a symbol, got {other:?}"),
    }
}

fn request(program: &str, function: &str) -> RunRequest {
    RunRequest { program: program.into(), fn_name: function.into(), args: vec!["a".into()], ..Default::default() }
}

static NONCE: AtomicU64 = AtomicU64::new(1);

fn signed(id: &Identity, mut req: RunRequest) -> RunRequest {
    let ts = auth::now_ms();
    let nonce = NONCE.fetch_add(1, Ordering::SeqCst);
    let message = CallClaim {
        program_source: &req.program,
        function: if req.tick { auth::TICK } else { &req.fn_name },
        args: &req.args,
        answers: &req.answers,
        knowledge_path: &req.knowledge_path,
        caller: &id.name,
        timestamp_ms: ts,
        nonce,
    }.message();
    let sig = id.sign(Purpose::Call, &message).unwrap();
    req.caller = Some(Caller { name: id.name.clone(), ml_dsa: sig.ml_dsa, timestamp_ms: ts, nonce, ed25519: sig.ed25519 });
    req
}

/// A throwaway test identity (plaintext seed: never for a real key).
fn ident(name: &str, kind: Kind, model: Option<String>) -> (Identity, auth::KeyEntry) {
    let (file, entry) = auth::keygen(name, kind, model, Protect::Plaintext).unwrap();
    (Identity::load(&file, None).unwrap(), entry)
}

struct Fixture {
    dir: PathBuf,
    model: Identity,
    visitor: Identity,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A root, a registrar it registered, and two identities the registrar registered (a model and a
/// user); the keyring and one deployed contract on disk.
fn fixture(tag: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("cubelang_contracts_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("contracts")).unwrap();
    std::fs::write(dir.join("contracts").join("guarded.cube"), CONTRACT).unwrap();
    let (root, root_pk) = ident("Root", Kind::Registrar, None);
    let (reg, reg_pk) = ident("Registrar", Kind::Registrar, None);
    let (model, model_pk) = ident("Emitter450m", Kind::Model, Some("ab".repeat(32)));
    let (visitor, visitor_pk) = ident("Visitor", Kind::User, None);
    let mut ring = Keyring::default();
    ring.add_root(root_pk).unwrap();
    ring.add(auth::register(&root, reg_pk, 1, None).unwrap()).unwrap();
    ring.add(auth::register(&reg, model_pk, 2, None).unwrap()).unwrap();
    ring.add(auth::register(&reg, visitor_pk, 3, None).unwrap()).unwrap();
    std::fs::write(dir.join("ring.json"), ring.to_json()).unwrap();
    std::fs::write(dir.join("root.pin"), auth::fingerprint(&ring.roots[0])).unwrap();
    Fixture { model, visitor, dir }
}

fn locked_args(f: &Fixture) -> Vec<String> {
    vec!["--keyring".into(), path_str(&f.dir.join("ring.json")),
         "--pin-root".into(), std::fs::read_to_string(f.dir.join("root.pin")).unwrap(),
         "--contracts".into(), path_str(&f.dir.join("contracts"))]
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

const OTHER_PROGRAM: &str = r#"
program Skill implements ISolve {
    @external
    public function solve(x: str): str { return x; }
}
"#;

#[test]
fn a_locked_process_checks_every_caller() {
    let f = fixture("locked");
    let args = locked_args(&f);
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let emit = signed(&f.model, request(CONTRACT, "emit"));
    // an identity outside the ring that signs under the model's name
    let (outsider, _) = ident("Emitter450m", Kind::Model, None);
    let impostor = signed(&outsider, request(CONTRACT, "emit"));
    let results = proto(&args, vec![
        request(CONTRACT, "solve"),                              // 0 anonymous, deployed, open
        request(CONTRACT, "emit"),                               // 1 anonymous, restricted
        emit.clone(),                                            // 2 the model, signed
        emit,                                                    // 3 the same bytes again
        impostor,                                                // 4 the right name, the wrong keys
        request(OTHER_PROGRAM, "solve"),                         // 5 anonymous, not deployed
        signed(&f.model, request(OTHER_PROGRAM, "solve")),       // 6 a model may submit a program
        signed(&f.visitor, request(OTHER_PROGRAM, "solve")),     // 7 a user may not
        signed(&f.visitor, request(CONTRACT, "solve")),          // 8 a user may run a deployed one
        request(CONTRACT, "limited"),                            // 9  the anonymous window: 1
        request(CONTRACT, "limited"),                            // 10 2
        request(CONTRACT, "limited"),                            // 11 3: refused, across requests
        RunRequest { program: CONTRACT.into(), tick: true, ..Default::default() },  // 12 the cron runs
        RunRequest { program: CONTRACT.into(), tick: true, ..Default::default() },  // 13 not due again
    ]);
    assert_eq!(symbol_of(&results[0]), "a");
    let capped = results[0].report.as_ref().unwrap().budget.clone().unwrap();
    assert_eq!((capped.max_wall_ms, capped.max_queries), (5_000, 1_000), "an untrusted request runs under the host's caps");
    let open = results[6].report.as_ref().unwrap().budget.clone().unwrap();
    assert_eq!(open.max_wall_ms, 0, "a signed model's request is not capped");
    assert!(error_of(&results[1]).contains("@restricted"), "{:?}", results[1]);
    assert_eq!(symbol_of(&results[2]), "a");
    assert!(error_of(&results[3]).contains("replay"), "{:?}", results[3]);
    assert!(error_of(&results[4]).contains("signature"), "{:?}", results[4]);
    assert!(error_of(&results[5]).contains("deployed contract"), "{:?}", results[5]);
    assert_eq!(symbol_of(&results[6]), "a");
    assert!(error_of(&results[7]).contains("only a signed model or agent"), "{:?}", results[7]);
    assert_eq!(symbol_of(&results[8]), "a");
    assert_eq!(symbol_of(&results[9]), "a");
    assert_eq!(symbol_of(&results[10]), "a");
    assert!(error_of(&results[11]).contains("@ratelimit"), "{:?}", results[11]);
    assert!(results[12].ok, "{:?}", results[12]);
    assert_eq!(results[12].crons.len(), 1);
    assert_eq!((results[12].crons[0].function.as_str(), results[12].crons[0].value.as_str()), ("nightly", "swept"));
    assert!(results[13].crons.is_empty(), "the host's clock says it already ran");
}

#[test]
fn a_caller_claim_without_a_keyring_is_refused_and_unlocked_mode_is_unchanged() {
    let f = fixture("unlocked");
    let results = proto(&[], vec![
        signed(&f.model, request(CONTRACT, "emit")),
        request(OTHER_PROGRAM, "solve"),
        request(CONTRACT, "emit"),
    ]);
    assert!(error_of(&results[0]).contains("no keyring"), "{:?}", results[0]);
    assert_eq!(symbol_of(&results[1]), "a", "a local pipe serves its parent's programs as before");
    assert!(error_of(&results[2]).contains("@restricted"), "a restricted function stays closed to anonymous");
}

#[test]
fn a_locked_process_keeps_knowledge_files_inside_the_contracts_directory() {
    let f = fixture("knowledge");
    let outside = f.dir.join("outside.jsonl");
    std::fs::write(&outside, "").unwrap();
    let inside = f.dir.join("contracts").join("facts.jsonl");
    std::fs::write(&inside, "").unwrap();
    let args = locked_args(&f);
    let args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let with = |p: &Path| RunRequest { knowledge_path: path_str(p), ..request(CONTRACT, "solve") };
    let results = proto(&args, vec![with(&outside), with(&inside)]);
    assert!(error_of(&results[0]).contains("inside the contracts directory"), "{:?}", results[0]);
    assert_eq!(symbol_of(&results[1]), "a");
}

#[test]
fn a_keyring_that_does_not_verify_stops_the_process() {
    let f = fixture("tampered");
    let ring_path = f.dir.join("ring.json");
    let mut ring: Keyring = serde_json::from_str(&std::fs::read_to_string(&ring_path).unwrap()).unwrap();
    let (_s, other) = ident("X", Kind::Model, None);
    ring.keys[1].ml_dsa = other.ml_dsa; // the model's key swapped after registration
    std::fs::write(&ring_path, serde_json::to_string(&ring).unwrap()).unwrap();
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let out = Command::new(exe).args(["run-proto", "--keyring", &path_str(&ring_path)])
        .stdin(Stdio::null()).output().unwrap();
    assert!(!out.status.success(), "fail closed");
    assert!(String::from_utf8_lossy(&out.stderr).contains("registration of Emitter450m"));
}

#[test]
fn a_keyring_with_an_unpinned_root_stops_the_process() {
    let f = fixture("pins");
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let wrong = "00".repeat(48);
    let out = Command::new(exe)
        .args(["run-proto", "--keyring", &path_str(&f.dir.join("ring.json")), "--pin-root", &wrong])
        .stdin(Stdio::null()).output().unwrap();
    assert!(!out.status.success(), "fail closed");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not one of the pinned roots"));
}

// ── the CLI ─────────────────────────────────────────────────────────────────

#[test]
fn keygen_register_check_and_sign_from_the_command_line() {
    let dir = std::env::temp_dir().join(format!("cubelang_cli_keys_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let exe = env!("CARGO_BIN_EXE_cubelang");
    let p = |name: &str| path_str(&dir.join(name));
    let pass = p("root.pass");
    std::fs::write(&pass, "a long root passphrase\n").unwrap();
    let run = |args: &[&str]| {
        let out = Command::new(exe).args(args).env_remove("CUBELANG_KEY_PASSPHRASE").output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    // the root's key file is sealed with a passphrase; a key with no passphrase is refused
    let refused = Command::new(exe).args(["keygen", "Root", "--kind", "registrar", "--out", &p("nopass.key.json")])
        .env_remove("CUBELANG_KEY_PASSPHRASE").output().unwrap();
    assert!(!refused.status.success() && String::from_utf8_lossy(&refused.stderr).contains("seals the key file"));
    let root_pub = run(&["keygen", "Root", "--kind", "registrar", "--out", &p("root.key.json"), "--passphrase-file", &pass]);
    std::fs::write(dir.join("root.pub.json"), &root_pub).unwrap();
    let root_file = std::fs::read_to_string(dir.join("root.key.json")).unwrap();
    assert!(root_file.contains("\"sealed\"") && !root_file.contains("\"seed\""), "{root_file}");
    let agent_pub = run(&["keygen", "Helper", "--kind", "agent", "--out", &p("agent.key.json"), "--plaintext"]);
    std::fs::write(dir.join("agent.pub.json"), &agent_pub).unwrap();
    // the seed never reaches the terminal
    let key: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("agent.key.json")).unwrap()).unwrap();
    assert!(!agent_pub.contains(key["seed"].as_str().unwrap()));
    // and a key file is never overwritten
    let again = Command::new(exe).args(["keygen", "Helper", "--kind", "agent", "--out", &p("agent.key.json"), "--plaintext"]).output().unwrap();
    assert!(!again.status.success());
    let added = run(&["keyring", "add-root", &p("ring.json"), &p("root.pub.json")]);
    let pin = added.rsplit("--pin-root ").next().unwrap().trim().to_string();
    assert_eq!(pin.len(), 96, "a SHA-384 fingerprint: {added}");
    // registering with the sealed root needs its passphrase
    let locked = Command::new(exe).args(["keyring", "register", &p("ring.json"), "--by", &p("root.key.json"), &p("agent.pub.json")])
        .env_remove("CUBELANG_KEY_PASSPHRASE").output().unwrap();
    assert!(!locked.status.success() && String::from_utf8_lossy(&locked.stderr).contains("needs its passphrase"));
    run(&["keyring", "register", &p("ring.json"), "--by", &p("root.key.json"), "--passphrase-file", &pass,
          "--days", "30", &p("agent.pub.json")]);
    let check = run(&["keyring", "check", &p("ring.json")]);
    assert!(check.contains("the chain verifies") && check.contains("Helper -- agent (registered by Root)"), "{check}");
    assert!(check.contains(&pin), "check prints the fingerprints to pin: {check}");
    // sign one call and send it
    std::fs::write(dir.join("skill.cube"), OTHER_PROGRAM).unwrap();
    let caller: serde_json::Value = serde_json::from_str(&run(&[
        "sign", "--key", &p("agent.key.json"), "--program", &p("skill.cube"), "--fn", "solve", "--arg", "a",
    ])).unwrap();
    let hex = |k: &str| auth::from_hex(caller[k].as_str().unwrap()).unwrap();
    let req = RunRequest {
        caller: Some(Caller {
            name: caller["name"].as_str().unwrap().into(),
            ml_dsa: hex("ml_dsa"),
            timestamp_ms: caller["timestamp_ms"].as_u64().unwrap(),
            nonce: caller["nonce"].as_u64().unwrap(),
            ed25519: hex("ed25519"),
        }),
        ..request(OTHER_PROGRAM, "solve")
    };
    let ring_arg = p("ring.json");
    let results = proto(&["--keyring", &ring_arg, "--pin-root", &pin], vec![req.clone()]);
    assert_eq!(symbol_of(&results[0]), "a", "an agent signed by the CLI may submit its own program");
    // revoked by its registrar, the same agent is refused
    run(&["keyring", "revoke", &p("ring.json"), "--by", &p("root.key.json"), "--passphrase-file", &pass,
          "--reason", "test", "Helper"]);
    assert!(run(&["keyring", "check", &p("ring.json")]).contains("REVOKED"));
    let fresh = RunRequest { caller: req.caller.clone().map(|mut c| { c.nonce += 1; c }), ..req };
    let results = proto(&["--keyring", &ring_arg, "--pin-root", &pin], vec![fresh]);
    assert!(error_of(&results[0]).contains("revoked"), "{:?}", results[0]);
    let _ = std::fs::remove_dir_all(&dir);
}
