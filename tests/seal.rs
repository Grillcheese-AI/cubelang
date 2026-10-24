//! Sealed content and sealed history directories (2026-10-24): `cubelang::seal`.

use std::path::{Path, PathBuf};
use std::process::Command;

use cubelang::auth::{keygen, register, Identity, KeyEntry, Keyring, Kind, Protect, Verifier};
use cubelang::seal::{self, open, open_from, recipient_of, SealedVault, Sender};
use cubelang::vm::history::{History, SearchMode};
use cubelang::vm::segment::Vault;

fn ident(name: &str, kind: Kind) -> (Identity, KeyEntry) {
    let (file, entry) = keygen(name, kind, None, Protect::Plaintext).unwrap();
    (Identity::load(&file, None).unwrap(), entry)
}

/// A keyring with a root and the given entries, all registered by the root.
fn ring_of(entries: &[KeyEntry]) -> Keyring {
    let (root, root_pk) = ident("Root", Kind::Registrar);
    let mut ring = Keyring::default();
    ring.add_root(root_pk).unwrap();
    for e in entries {
        ring.add(register(&root, e.clone(), 1, None).unwrap()).unwrap();
    }
    ring
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("cubelang-seal-{}-{}", std::process::id(), name));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn sealed_content_opens_for_its_recipient_only() {
    let (bob, bob_pk) = ident("Bob", Kind::User);
    let (eve, _) = ident("Eve", Kind::User);
    let body = b"the launch code is in the blue folder";
    let sealed = seal::seal(&bob_pk, body, b"ctx", None).unwrap();
    assert!(!sealed.windows(4).any(|w| w == b"blue"), "no plaintext in the sealed bytes");
    assert_eq!(recipient_of(&sealed).unwrap().0, "Bob");

    let opened = open(&bob, &sealed, b"ctx", None).unwrap();
    assert_eq!(opened.body, body);
    assert_eq!(opened.sender, Sender::Anonymous);
    assert!(open(&eve, &sealed, b"ctx", None).unwrap_err().contains("sealed to Bob, not to Eve"));

    // another identity named Bob (another seed) is not Bob
    let (bob2, _) = ident("Bob", Kind::User);
    assert!(open(&bob2, &sealed, b"ctx", None).unwrap_err().contains("other encryption keys"));

    // the associated data is part of what opens it
    assert!(open(&bob, &sealed, b"other", None).unwrap_err().contains("does not open"));

    // two sealings of the same body share nothing (fresh encapsulation, fresh key)
    let again = seal::seal(&bob_pk, body, b"ctx", None).unwrap();
    assert_ne!(sealed, again);
}

#[test]
fn every_byte_of_sealed_content_is_covered() {
    let (bob, bob_pk) = ident("Bob", Kind::User);
    let sealed = seal::seal(&bob_pk, b"a short message", b"", None).unwrap();
    // flip one byte at a time, through the header, the ML-KEM ciphertext, the X25519 key, the
    // AEAD ciphertext and its tag: not one altered copy opens
    let n = sealed.len();
    let header = 11 + 3 + 48;
    for at in (0..n).step_by(37).chain([8, 9, 11, header, header + 1567, header + 1568, header + 1599, n - 1]) {
        let mut bad = sealed.clone();
        bad[at] ^= 0x01;
        assert!(open(&bob, &bad, b"", None).is_err(), "a flip at byte {} of {} opened", at, n);
    }
    assert!(open(&bob, &sealed[..n - 1], b"", None).is_err(), "truncated");
    assert!(open(&bob, b"CUBESEAL", b"", None).is_err());
    assert!(open(&bob, b"not sealed at all", b"", None).unwrap_err().contains("not sealed content"));
}

#[test]
fn a_signed_sender_is_verified_against_the_keyring() {
    let (alice, alice_pk) = ident("Alice", Kind::Agent);
    let (bob, bob_pk) = ident("Bob", Kind::User);
    let (mallory, _) = ident("Mallory", Kind::User);
    let v = Verifier::new(ring_of(&[alice_pk.clone(), bob_pk.clone()]), vec![]).unwrap();

    let sealed = seal::seal(&bob_pk, b"report attached", b"", Some(&alice)).unwrap();
    assert!(!sealed.windows(5).any(|w| w == b"Alice"), "the sender is inside the ciphertext");
    let opened = open(&bob, &sealed, b"", Some(&v)).unwrap();
    assert_eq!(opened.sender, Sender::Verified { name: "Alice".into(), kind: Kind::Agent });
    assert_eq!(open(&bob, &sealed, b"", None).unwrap().sender, Sender::Unchecked("Alice".into()));
    assert_eq!(open_from(&bob, &sealed, b"", &alice_pk).unwrap(), b"report attached");

    // a sender the keyring does not know is an error, not an unsigned message
    let from_mallory = seal::seal(&bob_pk, b"trust me", b"", Some(&mallory)).unwrap();
    assert!(open(&bob, &from_mallory, b"", Some(&v)).unwrap_err().contains("not in the keyring"));
    assert!(open_from(&bob, &from_mallory, b"", &alice_pk).unwrap_err().contains("signed by Mallory"));
    let unsigned = seal::seal(&bob_pk, b"x", b"", None).unwrap();
    assert!(open_from(&bob, &unsigned, b"", &alice_pk).unwrap_err().contains("unsigned"));
}

#[test]
fn an_entry_without_signed_encryption_keys_is_not_sealed_to() {
    let (_b, mut bob_pk) = ident("Bob", Kind::User);
    let (_c, carol_pk) = ident("Carol", Kind::User);
    let swapped = KeyEntry { kem: carol_pk.kem.clone(), ..bob_pk.clone() };
    assert!(seal::seal(&swapped, b"x", b"", None).unwrap_err().contains("not signed by its own keys"));
    bob_pk.kem = None;
    assert!(seal::seal(&bob_pk, b"x", b"", None).unwrap_err().contains("no encryption keys"));
}

fn talk() -> History {
    let mut h = History::new();
    h.append("user", "Who invented the printing press?", 1);
    h.append("cubby", "Johannes Gutenberg, around 1440, in Mainz.", 2);
    h.append("user", "And what did Gutenberg print first?", 3);
    h.append("cubby", "The Gutenberg Bible.", 4);
    h
}

fn files_of(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir).unwrap()
        .map(|e| { let e = e.unwrap(); (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap()) })
        .collect();
    v.sort();
    v
}

#[test]
fn a_sealed_history_reads_back_and_says_nothing_on_disk() {
    let dir = temp("hist");
    let (me, _) = ident("Host", Kind::Model);
    let vault = SealedVault::own(me);
    let mut h = talk();
    h.save_dir_in(&dir, Some(&vault)).unwrap();
    h.append("user", "Where is Mainz?", 5);
    h.save_dir_in(&dir, Some(&vault)).unwrap();

    for (name, bytes) in files_of(&dir) {
        assert!(name == "MANIFEST.cubeseal" || (name.starts_with("seg-") && name.ends_with(".cubeseal")), "{name}");
        let text = String::from_utf8_lossy(&bytes).to_lowercase();
        for word in ["gutenberg", "mainz", "printing", "user", "cubby"] {
            assert!(!text.contains(word), "{name} holds {word:?} in the clear");
        }
    }
    let back = History::open_dir_in(&dir, Some(&vault)).unwrap();
    assert_eq!(back.len(), 5);
    assert_eq!(back.generation(), 2);
    let hits = back.search("gutenburg", SearchMode::Fuzzy, 10);
    assert_eq!(hits.iter().map(|x| x.turn.id).collect::<Vec<_>>(), vec![4, 3, 2]);

    // without the key, or with another, it does not open
    assert!(History::open_dir(&dir).unwrap_err().contains("sealed"));
    let (other, _) = ident("Host", Kind::Model);
    assert!(History::open_dir_in(&dir, Some(&SealedVault::own(other))).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sealed_history_refuses_swapped_planted_and_mixed_files() {
    let dir = temp("tamper");
    let (me, me_pk) = ident("Host", Kind::Model);
    let vault = SealedVault::own(me);
    let mut h = talk();
    h.freeze().unwrap();
    h.append("user", "a second segment", 5);
    h.save_dir_in(&dir, Some(&vault)).unwrap();
    let segs: Vec<String> = files_of(&dir).into_iter().map(|f| f.0).filter(|n| n.starts_with("seg-")).collect();
    assert_eq!(segs.len(), 2);

    // two segment files trade names: each is bound to its own name
    let (a, b) = (dir.join(&segs[0]), dir.join(&segs[1]));
    let (ab, bb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    std::fs::write(&a, &bb).unwrap();
    std::fs::write(&b, &ab).unwrap();
    assert!(History::open_dir_in(&dir, Some(&vault)).is_err(), "swapped files open");
    std::fs::write(&a, &ab).unwrap();
    std::fs::write(&b, &bb).unwrap();
    assert!(History::open_dir_in(&dir, Some(&vault)).is_ok());

    // a file sealed to the host's own public keys by someone else (anyone can do that) is refused:
    // a vault file must be signed by the vault's identity
    let (stranger, _) = ident("Stranger", Kind::User);
    let plain = {
        let mut one = History::new();
        one.append("user", "a forged turn", 1);
        let d2 = temp("forge-src");
        one.save_dir(&d2).unwrap();
        let seg = files_of(&d2).into_iter().find(|f| f.0.ends_with(".cubeseg")).unwrap().1;
        let _ = std::fs::remove_dir_all(&d2);
        seg
    };
    let mut aad = b"cubelang/vault/v1\0".to_vec();
    aad.extend_from_slice(segs[0].as_bytes());
    let planted = seal::seal(&me_pk, &plain, &aad, Some(&stranger)).unwrap();
    std::fs::write(&a, &planted).unwrap();
    assert!(History::open_dir_in(&dir, Some(&vault)).unwrap_err().contains("signed by Stranger"));
    std::fs::write(&a, &ab).unwrap();

    // a plain manifest next to the sealed one: refuse to guess
    std::fs::write(dir.join("MANIFEST.json"), br#"{"version":1,"next_id":1,"segments":[]}"#).unwrap();
    assert!(History::open_dir_in(&dir, Some(&vault)).unwrap_err().contains("both"));
    assert!(History::open_dir(&dir).unwrap_err().contains("both"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn forgetting_in_a_sealed_history_leaves_no_file_that_holds_the_turn() {
    let dir = temp("forget");
    let (me, _) = ident("Host", Kind::Model);
    let vault = SealedVault::own(me);
    let mut h = talk();
    h.save_dir_in(&dir, Some(&vault)).unwrap();
    assert!(h.forget(2));
    h.merge_all().unwrap();
    h.save_dir_in(&dir, Some(&vault)).unwrap();
    let removed = History::gc_dir_in(&dir, Some(&vault)).unwrap();
    assert_eq!(removed.len(), 1, "the segment that held the turn is gone");
    // open every file left with the key: not one holds the forgotten words
    for (name, bytes) in files_of(&dir) {
        let plain = vault.open_file(&name, &bytes).unwrap();
        assert!(!String::from_utf8_lossy(&plain).contains("Mainz"), "{name} still holds the forgotten turn");
    }
    assert_eq!(History::open_dir_in(&dir, Some(&vault)).unwrap().len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_plain_history_is_sealed_in_place() {
    let dir = temp("convert");
    let mut h = talk();
    h.save_dir(&dir).unwrap();
    let (me, _) = ident("Host", Kind::Model);
    let vault = SealedVault::own(me);
    assert!(History::open_dir_in(&dir, Some(&vault)).unwrap_err().contains("not sealed"));
    let removed = History::seal_dir(&dir, &vault).unwrap();
    assert!(removed.contains(&"MANIFEST.json".to_string()) && removed.len() == 2, "{removed:?}");
    for (name, bytes) in files_of(&dir) {
        assert!(name.ends_with(".cubeseal"), "{name}");
        assert!(!String::from_utf8_lossy(&bytes).contains("Gutenberg"));
    }
    let back = History::open_dir_in(&dir, Some(&vault)).unwrap();
    assert_eq!(back.len(), 4);
    assert_eq!(back.search("bible", SearchMode::Exact, 5).len(), 1);
    assert!(History::seal_dir(&dir, &vault).unwrap_err().contains("already sealed"));
    let _ = std::fs::remove_dir_all(&dir);
}

fn cli(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_cubelang")).args(args).env_remove("CUBELANG_KEY_PASSPHRASE").output().unwrap();
    (out.status.success(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn the_seal_and_open_commands() {
    let dir = temp("cli");
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let (ok, entry_json, err) = cli(&["keygen", "Bob", "--kind", "user", "--out", &p("bob.key.json"), "--plaintext"]);
    assert!(ok, "{err}");
    std::fs::write(p("bob.entry.json"), &entry_json).unwrap();
    let (ok, _, err) = cli(&["keygen", "Alice", "--kind", "agent", "--out", &p("alice.key.json"), "--plaintext"]);
    assert!(ok, "{err}");
    std::fs::write(p("note.txt"), "meet at the north gate").unwrap();

    let (ok, _, err) = cli(&["seal", "--to", &p("bob.entry.json"), "--from", &p("alice.key.json"), "--aad", "note",
                             &p("note.txt"), &p("note.sealed")]);
    assert!(ok, "{err}");
    assert!(err.contains("sealed 22 bytes to Bob") && err.contains("signed by Alice"), "{err}");
    let (ok, _, err) = cli(&["seal", "--to", &p("bob.entry.json"), &p("note.txt"), &p("note.sealed")]);
    assert!(!ok && err.contains("not writing over it"), "{err}");

    let (ok, out, err) = cli(&["open", "--key", &p("bob.key.json"), "--aad", "note", &p("note.sealed"), "-"]);
    assert!(ok, "{err}");
    assert_eq!(out, "meet at the north gate");
    assert!(err.contains("NOT checked"), "{err}");
    let (ok, _, err) = cli(&["open", "--key", &p("bob.key.json"), &p("note.sealed"), "-"]);
    assert!(!ok && err.contains("does not open"), "without its associated data: {err}");
    let (ok, _, err) = cli(&["open", "--key", &p("alice.key.json"), "--aad", "note", &p("note.sealed"), "-"]);
    assert!(!ok && err.contains("sealed to Bob, not to Alice"), "{err}");
    let (ok, _, err) = cli(&["open", "--key", &p("bob.key.json"), "--signer", "Alice", "--aad", "note", &p("note.sealed"), "-"]);
    assert!(!ok && err.contains("--signer needs --keyring"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_history_cli_with_a_key() {
    let dir = temp("hist-cli");
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let (ok, _, err) = cli(&["keygen", "Host", "--kind", "model", "--out", &p("host.key.json"), "--plaintext"]);
    assert!(ok, "{err}");
    let h = p("h");
    for text in ["Who invented the printing press?", "Johannes Gutenberg, around 1440, in Mainz."] {
        let (ok, _, err) = cli(&["history", "append", &h, text]);
        assert!(ok, "{err}");
    }
    let (ok, out, err) = cli(&["history", "seal", &h, "--key", &p("host.key.json")]);
    assert!(ok, "{err}");
    assert!(out.contains("sealed to Host (2 turns"), "{out}");
    let (ok, _, err) = cli(&["history", "search", &h, "gutenberg"]);
    assert!(!ok && err.contains("sealed"), "{err}");
    let (ok, _, err) = cli(&["history", "append", &h, "--key", &p("host.key.json"), "And", "then?"]);
    assert!(ok, "{err}");
    let (ok, out, _) = cli(&["history", "recent", &h, "--key", &p("host.key.json"), "5"]);
    assert!(ok);
    assert_eq!(out.lines().count(), 3, "{out}");
    let (ok, out, _) = cli(&["history", "search", &h, "gutenburg", "--fuzzy", "--key", &p("host.key.json")]);
    assert!(ok);
    assert_eq!(out.lines().count(), 1, "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

const COUNT: &str = r#"
use history;
program Recall implements ISolve {
    public function solve(input: str): str { return input; }
    @external
    public function count(words: str): number {
        let hits = search(words);
        let n = hits.len();
        return n;
    }
}
"#;

/// One run-proto process, several requests: the results in order.
fn proto_session(extra: &[&str], reqs: Vec<cubelang::reasoning::RunRequest>, between: impl Fn(usize))
                 -> Vec<cubelang::reasoning::RunResult> {
    use prost::Message;
    use std::io::{Read, Write};
    let mut child = Command::new(env!("CARGO_BIN_EXE_cubelang")).arg("run-proto").args(extra)
        .env_remove("CUBELANG_KEY_PASSPHRASE")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let mut out = Vec::new();
    for (i, req) in reqs.into_iter().enumerate() {
        between(i);
        let mut buf = Vec::new();
        req.encode(&mut buf).unwrap();
        stdin.write_all(&(buf.len() as u32).to_be_bytes()).unwrap();
        stdin.write_all(&buf).unwrap();
        stdin.flush().unwrap();
        let mut len = [0u8; 4];
        stdout.read_exact(&mut len).unwrap();
        let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
        stdout.read_exact(&mut body).unwrap();
        out.push(cubelang::reasoning::RunResult::decode(&body[..]).unwrap());
    }
    drop(stdin);
    child.wait().unwrap();
    out
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
    }
}

#[test]
fn run_proto_reads_a_sealed_history_and_refuses_a_rolled_back_one() {
    use cubelang::reasoning::{run_result, RunRequest};
    let dir = temp("proto");
    let p = |n: &str| dir.join(n).to_str().unwrap().to_string();
    let (ok, _, err) = cli(&["keygen", "Host", "--kind", "model", "--out", &p("host.key.json"), "--plaintext"]);
    assert!(ok, "{err}");
    let h = p("h");
    let (ok, _, err) = cli(&["history", "append", &h, "Johannes Gutenberg, around 1440."]);
    assert!(ok, "{err}");
    let (ok, _, err) = cli(&["history", "seal", &h, "--key", &p("host.key.json")]);
    assert!(ok, "{err}");
    copy_dir(Path::new(&h), &dir.join("old"));
    let (ok, _, err) = cli(&["history", "append", &h, "--key", &p("host.key.json"), "Gutenberg again."]);
    assert!(ok, "{err}");

    let count = || RunRequest { program: COUNT.into(), fn_name: "count".into(), args: vec!["gutenberg".into()],
                                history_path: h.clone(), ..Default::default() };
    let symbol = |r: &cubelang::reasoning::RunResult| match &r.result {
        Some(run_result::Result::Symbol(s)) => s.clone(),
        other => format!("{:?}", other),
    };

    // without the key the sealed history is an error
    let r = proto_session(&[], vec![count()], |_| {});
    assert!(!r[0].ok, "a sealed history opened without its key");

    // with it: read, then an older copy put back in its place is refused
    let r = proto_session(&["--history-key", &p("host.key.json")], vec![count(), count(), count()], |i| {
        if i == 1 {
            std::thread::sleep(std::time::Duration::from_millis(50));
            copy_dir(&dir.join("old"), Path::new(&h));
        }
    });
    assert_eq!(symbol(&r[0]), "2", "{:?}", r[0]);
    assert!(!r[1].ok, "the rolled-back copy was served: {:?}", r[1]);
    assert!(format!("{:?}", r[1]).contains("older"), "{:?}", r[1]);
    let _ = std::fs::remove_dir_all(&dir);
}
