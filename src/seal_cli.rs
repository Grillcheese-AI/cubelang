//! `cubelang seal` and `cubelang open` (2026-10-24): files only one identity can read
//! (`cubelang::seal`).

use std::io::Write;
use std::path::Path;
use std::process;

use cubelang::auth::{KeyEntry, Keyring, Verifier};
use cubelang::seal::{self, Sender};

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {}", msg);
    process::exit(1)
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).map(|i| args.get(i + 1).cloned().unwrap_or_else(|| die(format!("{} needs a value", name))))
}

fn flags(args: &[String], name: &str) -> Vec<String> {
    args.iter().enumerate().filter(|(_, a)| *a == name).filter_map(|(i, _)| args.get(i + 1).cloned()).collect()
}

/// The arguments that are not flags or flag values.
fn positional(args: &[String], with_values: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if with_values.contains(&args[i].as_str()) {
            i += 2;
            continue;
        }
        if !args[i].starts_with("--") || args[i] == "-" {
            out.push(args[i].clone());
        }
        i += 1;
    }
    out
}

/// Write `bytes` to `path`: never over an existing file unless `force`, and (on Unix) readable by
/// the owner only, since an opened file is plaintext.
fn write_out(path: &str, bytes: &[u8], force: bool) {
    if path == "-" {
        let mut out = std::io::stdout().lock();
        out.write_all(bytes).and_then(|_| out.flush()).unwrap_or_else(|e| die(format!("stdout: {}", e)));
        return;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true);
    if force {
        opts.create(true).truncate(true);
    } else {
        opts.create_new(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path).unwrap_or_else(|e| {
        if e.kind() == std::io::ErrorKind::AlreadyExists {
            die(format!("{} exists; not writing over it (--force to replace it)", path))
        }
        die(format!("{}: {}", path, e))
    });
    f.write_all(bytes).and_then(|_| f.sync_all()).unwrap_or_else(|e| die(format!("{}: {}", path, e)));
}

fn read_in(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| die(format!("{}: {}", path, e)))
}

fn keyring(path: &str) -> Keyring {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| die(format!("--keyring {}: {}", path, e)));
    Keyring::from_json(&text).unwrap_or_else(|e| die(format!("--keyring {}: {}", path, e)))
}

/// `cubelang seal --to ENTRY.json|NAME [--keyring RING.json] [--from KEY.json] [--aad TEXT] IN OUT`
pub fn cmd_seal(args: &[String]) {
    let usage = "usage: cubelang seal --to NAME --keyring RING.json | --to ENTRY.json  [--from KEY.json] \
                 [--aad TEXT] [--force] IN OUT\n\
                 (sealed --from key files: --passphrase-file P or CUBELANG_KEY_PASSPHRASE)";
    let with_values = ["--to", "--keyring", "--from", "--aad", "--passphrase-file"];
    let Some(to) = flag(args, "--to") else { die(usage) };
    let files = positional(args, &with_values);
    let [input, output] = files.as_slice() else { die(usage) };
    let recipient: KeyEntry = match flag(args, "--keyring") {
        // a name the keyring admits now: registered by its chain, not revoked, not expired
        Some(ring) => keyring(&ring).recipient(&to, cubelang::auth::now_ms()).unwrap_or_else(|e| die(e)).clone(),
        None if to.ends_with(".json") && Path::new(&to).is_file() => {
            let text = std::fs::read_to_string(&to).unwrap_or_else(|e| die(format!("{}: {}", to, e)));
            serde_json::from_str(&text).unwrap_or_else(|e| die(format!("{}: {}", to, e)))
        }
        None => die(format!("--to {}: a name needs --keyring RING.json (or give an entry file, ENTRY.json)", to)),
    };
    let sender = flag(args, "--from").map(|k| crate::open_identity(&k, flag(args, "--passphrase-file").as_deref()));
    let aad = flag(args, "--aad").unwrap_or_default();
    let body = read_in(input);
    let sealed = seal::seal(&recipient, &body, aad.as_bytes(), sender.as_ref()).unwrap_or_else(|e| die(e));
    write_out(output, &sealed, args.iter().any(|a| a == "--force"));
    let (_, fpr) = seal::recipient_of(&sealed).unwrap_or_else(|e| die(e));
    eprintln!("sealed {} bytes to {} (encryption keys {}), {}{}", body.len(), recipient.name, &fpr[..16],
              match &sender { Some(s) => format!("signed by {}", s.name), None => "unsigned".to_string() },
              if flag(args, "--keyring").is_none() {
                  "; the recipient is the entry file's word: check its fingerprint with its owner"
              } else { "" });
}

/// `cubelang open --key KEY.json [--keyring RING.json [--pin-root FP]...] [--signer NAME]
/// [--aad TEXT] IN OUT|-`
pub fn cmd_open(args: &[String]) {
    let usage = "usage: cubelang open --key KEY.json [--keyring RING.json [--pin-root FP]...] [--signer NAME] \
                 [--aad TEXT] [--force] IN OUT|-\n\
                 (sealed key files: --passphrase-file P or CUBELANG_KEY_PASSPHRASE)";
    let with_values = ["--key", "--keyring", "--pin-root", "--signer", "--aad", "--passphrase-file"];
    let Some(key) = flag(args, "--key") else { die(usage) };
    let files = positional(args, &with_values);
    let [input, output] = files.as_slice() else { die(usage) };
    let signer = flag(args, "--signer");
    let verifier = match flag(args, "--keyring") {
        Some(ring) => {
            let pins = flags(args, "--pin-root").into_iter().map(|p| p.trim().to_ascii_lowercase()).collect();
            Some(Verifier::new(keyring(&ring), pins).unwrap_or_else(|e| die(format!("--keyring {}: {}", ring, e))))
        }
        None if signer.is_some() => die("--signer needs --keyring RING.json (the signature is checked against it)"),
        None => None,
    };
    let me = crate::open_identity(&key, flag(args, "--passphrase-file").as_deref());
    let aad = flag(args, "--aad").unwrap_or_default();
    let opened = seal::open(&me, &read_in(input), aad.as_bytes(), verifier.as_ref()).unwrap_or_else(|e| die(e));
    let said = match &opened.sender {
        Sender::Anonymous => "unsigned: anyone holding your public keys could have sealed it".to_string(),
        Sender::Unchecked(n) => format!("signed by {} -- NOT checked (pass --keyring to check it)", n),
        Sender::Verified { name, kind } => format!("signed by {} (a {}), checked against the keyring", name, kind.as_str()),
    };
    if let Some(want) = &signer {
        if !matches!(&opened.sender, Sender::Verified { name, .. } if name == want) {
            die(format!("the content must be signed by {}; it is {}", want, said));
        }
    }
    write_out(output, &opened.body, args.iter().any(|a| a == "--force"));
    eprintln!("opened {} bytes; {}", opened.body.len(), said);
}
