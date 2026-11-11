//! run-proto over the network (2026-10-24): TLS 1.3, SecP384r1MLKEM1024, AES-256-GCM, both sides
//! pinned (`cubelang::tls`).

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use cubelang::auth::{Identity, PrivateKeyFile};
use cubelang::reasoning::{run_result, RunRequest, RunResult};
use prost::Message;

const PROGRAM: &str = r#"
program Echo implements ISolve {
    public function solve(input: str): str { return input; }
}
"#;

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cubelang-tls-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn exe() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_cubelang"));
    c.env_remove("CUBELANG_KEY_PASSPHRASE");
    c
}

fn cli(args: &[&str]) -> (bool, String, String) {
    let out = exe().args(args).output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

/// A throwaway identity: its key file and its TLS pin.
fn identity(dir: &std::path::Path, name: &str, kind: &str) -> (String, String) {
    let key = dir.join(format!("{name}.key.json")).to_str().unwrap().to_string();
    let (ok, _, err) = cli(&["keygen", name, "--kind", kind, "--out", &key, "--plaintext"]);
    assert!(ok, "{err}");
    let (ok, pin, err) = cli(&["tls-pin", "--key", &key]);
    assert!(ok, "{err}");
    (key, pin.trim().to_string())
}

fn load(key: &str) -> Identity {
    let file: PrivateKeyFile = serde_json::from_str(&std::fs::read_to_string(key).unwrap()).unwrap();
    Identity::load(&file, None).unwrap()
}

/// A listening run-proto; its address, and its stderr lines as they come.
struct Host {
    child: Child,
    addr: String,
    log: mpsc::Receiver<String>,
}

impl Host {
    fn start(key: &str, client_pins: &[&str]) -> Host {
        let mut args = vec!["run-proto", "--listen", "127.0.0.1:0", "--tls-key", key];
        for p in client_pins {
            args.extend(["--client-pin", p]);
        }
        let mut child = exe().args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().unwrap();
        let (tx, log) = mpsc::channel();
        let stderr = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                let _ = tx.send(line);
            }
        });
        let first = log.recv_timeout(Duration::from_secs(30)).expect("the host printed nothing");
        let addr = first.split("listening on ").nth(1).and_then(|r| r.split(' ').next())
            .unwrap_or_else(|| panic!("no address in {first:?}")).to_string();
        Host { child, addr, log }
    }

    /// The log lines so far.
    fn lines(&self) -> Vec<String> {
        std::thread::sleep(Duration::from_millis(200));
        self.log.try_iter().collect()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn request(arg: &str) -> RunRequest {
    RunRequest { program: PROGRAM.into(), fn_name: "solve".into(), args: vec![arg.into()], ..Default::default() }
}

/// Requests through `run-proto --remote`: the results, whether it succeeded, its stderr.
fn relay(addr: &str, key: &str, server_pin: &str, reqs: &[RunRequest]) -> (Vec<RunResult>, bool, String) {
    let mut child = exe().args(["run-proto", "--remote", addr, "--tls-key", key, "--server-pin", server_pin])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in reqs {
        let mut buf = Vec::new();
        r.encode(&mut buf).unwrap();
        // a relay that has already failed closes its stdin: keep writing as far as it goes
        let _ = stdin.write_all(&(buf.len() as u32).to_be_bytes()).and_then(|_| stdin.write_all(&buf));
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let mut results = Vec::new();
    let mut at = 0;
    while at + 4 <= out.stdout.len() {
        let len = u32::from_be_bytes(out.stdout[at..at + 4].try_into().unwrap()) as usize;
        results.push(RunResult::decode(&out.stdout[at + 4..at + 4 + len]).unwrap());
        at += 4 + len;
    }
    (results, out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn symbol(r: &RunResult) -> String {
    match &r.result {
        Some(run_result::Result::Symbol(s)) => s.clone(),
        other => panic!("expected a symbol, got {:?}", other),
    }
}

#[test]
fn a_remote_run_proto_answers_over_tls() {
    let dir = temp("ok");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (client_key, client_pin) = identity(&dir, "Bridge", "agent");
    assert_eq!(host_pin.len(), 96);
    let host = Host::start(&host_key, &[&client_pin]);
    let (results, ok, err) = relay(&host.addr, &client_key, &host_pin, &[request("first"), request("second")]);
    assert!(ok, "{err}");
    assert_eq!(results.iter().map(symbol).collect::<Vec<_>>(), vec!["first", "second"]);
    let log = host.lines().join("\n");
    assert!(log.contains(&format!("connected (client {})", &client_pin[..16])), "{log}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_wrong_host_and_an_unpinned_client_are_refused() {
    let dir = temp("refused");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (client_key, client_pin) = identity(&dir, "Bridge", "agent");
    let (other_key, other_pin) = identity(&dir, "Other", "agent");
    let host = Host::start(&host_key, &[&client_pin]);

    // the relay expects another host's key: it stops at the handshake
    let (results, ok, err) = relay(&host.addr, &client_key, &other_pin, &[request("x")]);
    assert!(!ok && results.is_empty() && err.contains("handshake failed"), "{err}");

    // a client the host does not pin gets nothing
    let (results, ok, err) = relay(&host.addr, &other_key, &host_pin, &[request("x")]);
    assert!(!ok && results.is_empty(), "an unpinned client was served: {err}");
    assert!(err.contains("pinned there"), "{err}");
    assert!(host.lines().iter().any(|l| l.contains("refused")));

    // and the host serves on
    let (results, ok, err) = relay(&host.addr, &client_key, &host_pin, &[request("still here")]);
    assert!(ok, "{err}");
    assert_eq!(symbol(&results[0]), "still here");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Handshake in-process with `cfg`: the negotiated connection, or the error.
fn handshake(addr: &str, cfg: Arc<rustls::ClientConfig>) -> Result<rustls::ClientConnection, String> {
    let mut sock = std::net::TcpStream::connect(addr).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut conn = rustls::ClientConnection::new(cfg, cubelang::tls::server_name()).unwrap();
    while conn.is_handshaking() {
        conn.complete_io(&mut sock).map_err(|e| e.to_string())?;
    }
    // the host's verdict on our key arrives after our Finished: read once to hear it
    let mut tls = rustls::Stream::new(&mut conn, &mut sock);
    let _ = tls.write_all(&0u32.to_be_bytes());
    let mut b = [0u8; 4];
    tls.read(&mut b).map_err(|e| e.to_string())?;
    Ok(conn)
}

#[test]
fn the_profile_is_the_only_one_accepted() {
    use rustls::crypto::aws_lc_rs as aws;
    let dir = temp("profile");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (client_key, client_pin) = identity(&dir, "Bridge", "agent");
    let host = Host::start(&host_key, &[&client_pin]);
    let me = load(&client_key);
    let pin = cubelang::tls::parse_pin(&host_pin).unwrap();

    // ours: TLS 1.3, AES-256-GCM, the P-384 + ML-KEM-1024 hybrid
    let conn = handshake(&host.addr, cubelang::tls::client_config(&me, pin).unwrap()).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(conn.protocol_version(), Some(rustls::ProtocolVersion::TLSv1_3));
    assert_eq!(conn.negotiated_cipher_suite().unwrap().suite(), rustls::CipherSuite::TLS13_AES_256_GCM_SHA384);
    assert_eq!(conn.negotiated_key_exchange_group().unwrap().name(), cubelang::tls::SECP384R1MLKEM1024);

    // a stock client (X25519MLKEM768, X25519, P-256, P-384; AES-128 and ChaCha too) finds no group
    let stock = Arc::new(aws::default_provider());
    assert!(handshake(&host.addr, cubelang::tls::client_config_with(&me, pin, stock).unwrap()).is_err(),
            "a client without the hybrid was accepted");

    // the hybrid, but only AES-128-GCM: no cipher suite in common
    let mut weak = aws::default_provider();
    weak.kx_groups = cubelang::tls::provider().kx_groups.clone();
    weak.cipher_suites = vec![aws::cipher_suite::TLS13_AES_128_GCM_SHA256];
    assert!(handshake(&host.addr, cubelang::tls::client_config_with(&me, pin, Arc::new(weak)).unwrap()).is_err(),
            "AES-128 was accepted");

    // classical P-384 alone (what a recording could be broken from later): refused
    let mut classical = aws::default_provider();
    classical.kx_groups = vec![aws::kx_group::SECP384R1];
    classical.cipher_suites = vec![aws::cipher_suite::TLS13_AES_256_GCM_SHA384];
    assert!(handshake(&host.addr, cubelang::tls::client_config_with(&me, pin, Arc::new(classical)).unwrap()).is_err(),
            "a classical key exchange was accepted");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn garbage_and_plain_tcp_do_not_stop_the_host() {
    let dir = temp("garbage");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (client_key, client_pin) = identity(&dir, "Bridge", "agent");
    let host = Host::start(&host_key, &[&client_pin]);
    // a plain run-proto frame, no TLS
    let mut buf = Vec::new();
    request("plain").encode(&mut buf).unwrap();
    let mut s = std::net::TcpStream::connect(&host.addr).unwrap();
    let _ = s.write_all(&(buf.len() as u32).to_be_bytes());
    let _ = s.write_all(&buf);
    let mut reply = Vec::new();
    let _ = s.read_to_end(&mut reply);
    assert!(!reply.windows(5).any(|w| w == b"plain"), "a plaintext request was answered");
    // random bytes, then a connection that says nothing
    let mut s = std::net::TcpStream::connect(&host.addr).unwrap();
    let _ = s.write_all(&[0x16, 0x03, 0x01, 0xff, 0xff, 1, 2, 3]);
    drop(s);
    let _idle = std::net::TcpStream::connect(&host.addr).unwrap();
    let (results, ok, err) = relay(&host.addr, &client_key, &host_pin, &[request("after")]);
    assert!(ok, "{err}");
    assert_eq!(symbol(&results[0]), "after");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_port_check_is_logged_as_one_and_not_as_a_refusal() {
    let dir = temp("portcheck");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (client_key, client_pin) = identity(&dir, "Bridge", "agent");
    let host = Host::start(&host_key, &[&client_pin]);
    // what a supervisor does to see that the port answers: connect, then close without a byte
    drop(std::net::TcpStream::connect(&host.addr).unwrap());
    let lines = host.lines();
    assert!(lines.iter().any(|l| l.contains("closed before the handshake")), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("refused")), "{lines:?}");
    // bytes that are no handshake are still refused, and the host serves on
    let mut s = std::net::TcpStream::connect(&host.addr).unwrap();
    let _ = s.write_all(&[0x16, 0x03, 0x01, 0xff, 0xff, 1, 2, 3]);
    drop(s);
    let lines = host.lines();
    assert!(lines.iter().any(|l| l.contains("refused")), "{lines:?}");
    let (results, ok, err) = relay(&host.addr, &client_key, &host_pin, &[request("after")]);
    assert!(ok, "{err}");
    assert_eq!(symbol(&results[0]), "after");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_network_leg_without_tls_or_pins_does_not_start() {
    let dir = temp("cli");
    let (host_key, host_pin) = identity(&dir, "Host", "model");
    let (ok, _, err) = cli(&["run-proto", "--listen", "127.0.0.1:0"]);
    assert!(!ok && err.contains("TLS 1.3 or nothing"), "{err}");
    let (ok, _, err) = cli(&["run-proto", "--listen", "127.0.0.1:0", "--tls-key", &host_key]);
    assert!(!ok && err.contains("at least one --client-pin"), "{err}");
    let (ok, _, err) = cli(&["run-proto", "--remote", "127.0.0.1:1", "--tls-key", &host_key]);
    assert!(!ok && err.contains("--server-pin"), "{err}");
    let (ok, _, err) = cli(&["run-proto", "--remote", "127.0.0.1:1", "--tls-key", &host_key, "--server-pin", &host_pin,
                             "--keyring", "ring.json"]);
    assert!(!ok && err.contains("its own options"), "{err}");
    let (ok, _, err) = cli(&["run-proto", "--listen", "127.0.0.1:0", "--tls-key", &host_key, "--client-pin", "abc"]);
    assert!(!ok && err.contains("96 hex digits"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
