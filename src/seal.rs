//! Sealed content (2026-10-24): bytes only one identity can read, optionally signed by another.
//!
//! Two key exchanges, both: ML-KEM-1024 (FIPS 203, the CNSA 2.0 key establishment; Cryspen's
//! formally verified `libcrux-ml-kem`, the same maker as the ML-DSA in `auth`) and X25519. Each
//! sealing makes a fresh ML-KEM encapsulation and a fresh X25519 key; HKDF-SHA-384 combines the two
//! shared secrets with everything public that produced them (the recipient's key fingerprint, which
//! covers both of its public keys, the ML-KEM ciphertext and the ephemeral X25519 key). Reading the
//! content needs both secrets, so it needs breaking both schemes. The key is used once, with
//! AES-256-GCM.
//!
//! The sender, when there is one, is INSIDE the ciphertext: whoever sees the sealed bytes learns
//! who they are for, never who wrote them. The sender's signature (ML-DSA-87 + Ed25519,
//! `Purpose::Seal`) covers the sender's name, the recipient's name and key fingerprint, and the
//! SHA-384 of the associated data and of the body, so a recipient cannot re-seal a signed message
//! to someone else as though it had been written to them.
//!
//! ## Format (integers little-endian)
//!
//! ```text
//! [8]    magic "CUBESEAL"
//! [1]    version (1)
//! [2]    recipient name length, then the name
//! [48]   recipient key fingerprint (`auth::kem_fingerprint`)
//! [1568] ML-KEM-1024 ciphertext
//! [32]   ephemeral X25519 public key
//! [..]   AES-256-GCM ciphertext and its 16-byte tag, of:
//!          [1] flags (bit 0: signed)
//!          signed: [2] sender name length, the name, [4627] ML-DSA-87 signature, [64] Ed25519 signature
//!          the body
//! ```
//!
//! The GCM associated data is the header (everything before the ciphertext) followed by the
//! caller's associated data, length-prefixed: no header field can be changed, and content sealed
//! for one place (a file name, a conversation) does not open in another.

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::aead::Nonce;
use aes_gcm::{Aes256Gcm, KeyInit};
use libcrux_ml_kem::mlkem1024;
use rand::RngCore;
use sha2::{Digest, Sha384};
use zeroize::Zeroizing;

use crate::auth::{self, HybridSignature, Identity, KeyEntry, Kind, Purpose, Verifier};
use crate::vm::segment::Vault;

pub const MAGIC: &[u8; 8] = b"CUBESEAL";
pub const VERSION: u8 = 1;
const FPR: usize = 48;
const CT: usize = 1568;
const EPK: usize = 32;
const TAG: usize = 16;
const SIG_ML: usize = 4627;
const SIG_ED: usize = 64;
const FLAG_SIGNED: u8 = 1;

/// Who sealed the content, as far as `open` could tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sender {
    /// No signature: anyone holding the recipient's public keys could have written it.
    Anonymous,
    /// Signed, not checked (no keyring was given): the name is the content's own claim.
    Unchecked(String),
    /// Signed, and the signature verifies against an entry the keyring admits now.
    Verified { name: String, kind: Kind },
}

/// What `open` found.
#[derive(Debug)]
pub struct Opened {
    pub body: Vec<u8>,
    pub sender: Sender,
}

struct Header {
    recipient: String,
    fpr: [u8; FPR],
    ct: [u8; CT],
    epk: [u8; EPK],
    /// Bytes of the header (where the ciphertext starts).
    len: usize,
}

fn parse_header(sealed: &[u8]) -> Result<Header, String> {
    let short = || "sealed content: truncated".to_string();
    if sealed.len() < 11 || &sealed[..8] != MAGIC {
        return Err("not sealed content (no CUBESEAL header)".into());
    }
    if sealed[8] != VERSION {
        return Err(format!("sealed content version {}; this build reads {}", sealed[8], VERSION));
    }
    let n = u16::from_le_bytes([sealed[9], sealed[10]]) as usize;
    let mut at = 11;
    let take = |at: &mut usize, k: usize| -> Result<&[u8], String> {
        let s = sealed.get(*at..*at + k).ok_or_else(short)?;
        *at += k;
        Ok(s)
    };
    let recipient = std::str::from_utf8(take(&mut at, n)?)
        .map_err(|_| "sealed content: the recipient name is not UTF-8".to_string())?
        .to_string();
    let fpr = take(&mut at, FPR)?.try_into().unwrap();
    let ct = take(&mut at, CT)?.try_into().unwrap();
    let epk = take(&mut at, EPK)?.try_into().unwrap();
    if sealed.len() < at + TAG + 1 {
        return Err(short());
    }
    Ok(Header { recipient, fpr, ct, epk, len: at })
}

/// The content key and nonce: HKDF-SHA-384 over both shared secrets, bound to the header.
fn content_key(ss_kem: &[u8; 32], ss_x: &[u8; 32], header: &[u8]) -> (Zeroizing<[u8; 32]>, [u8; 12]) {
    let mut ikm = Zeroizing::new([0u8; 64]);
    ikm[..32].copy_from_slice(ss_kem);
    ikm[32..].copy_from_slice(ss_x);
    let hk = hkdf::Hkdf::<Sha384>::new(Some(b"cubelang/seal/v1"), ikm.as_ref());
    let mut info = b"cubelang/seal/v1 content key\0".to_vec();
    info.extend_from_slice(&Sha384::digest(header));
    let mut okm = Zeroizing::new([0u8; 44]);
    hk.expand(&info, okm.as_mut()).expect("44 bytes is a valid HKDF length");
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(&okm[..32]);
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&okm[32..]);
    (key, nonce)
}

fn gcm_aad(header: &[u8], aad: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(header.len() + 8 + aad.len());
    a.extend_from_slice(header);
    a.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    a.extend_from_slice(aad);
    a
}

fn field(m: &mut Vec<u8>, b: &[u8]) {
    m.extend_from_slice(&(b.len() as u64).to_le_bytes());
    m.extend_from_slice(b);
}

/// The bytes a sender signs: who wrote it, to whom (name and key), and digests of the associated
/// data and the body.
pub fn signed_message(sender: &str, recipient: &str, fpr: &[u8; FPR], aad: &[u8], body: &[u8]) -> Vec<u8> {
    let mut m = Purpose::Seal.tag().to_vec();
    field(&mut m, sender.as_bytes());
    field(&mut m, recipient.as_bytes());
    m.extend_from_slice(fpr);
    m.extend_from_slice(&Sha384::digest(aad));
    m.extend_from_slice(&Sha384::digest(body));
    m
}

/// Seal `body` to `recipient`, bound to `aad`, signed by `sender` when one is given. The recipient
/// entry must carry encryption keys its own signing keys signed (`auth::kem_public`); whether the
/// entry really is who it names is the keyring's question (`Keyring::recipient`).
pub fn seal(recipient: &KeyEntry, body: &[u8], aad: &[u8], sender: Option<&Identity>) -> Result<Vec<u8>, String> {
    seal_inner(recipient, aad, |fpr| {
        let mut inner = Zeroizing::new(Vec::with_capacity(1 + body.len() + SIG_ML + SIG_ED + 70));
        match sender {
            None => inner.push(0),
            Some(s) => {
                let sig = s.sign(Purpose::Seal, &signed_message(&s.name, &recipient.name, fpr, aad, body))?;
                if sig.ml_dsa.len() != SIG_ML || sig.ed25519.len() != SIG_ED {
                    return Err("sealing: an unexpected signature size".into());
                }
                inner.push(FLAG_SIGNED);
                inner.extend_from_slice(&(s.name.len() as u16).to_le_bytes());
                inner.extend_from_slice(s.name.as_bytes());
                inner.extend_from_slice(&sig.ml_dsa);
                inner.extend_from_slice(&sig.ed25519);
            }
        }
        inner.extend_from_slice(body);
        Ok(inner)
    })
}

/// The envelope around an inner record `make_inner` builds for the recipient's key fingerprint.
fn seal_inner(recipient: &KeyEntry, aad: &[u8], make_inner: impl FnOnce(&[u8; FPR]) -> Result<Zeroizing<Vec<u8>>, String>)
              -> Result<Vec<u8>, String> {
    let (ek, x) = auth::kem_public(recipient)?;
    if recipient.name.len() > u16::MAX as usize {
        return Err("a recipient name that long is not a keyring name".into());
    }
    let fpr = auth::kem_fingerprint(&ek, &x);
    let mut rnd = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(rnd.as_mut());
    let (ct, ss_kem) = mlkem1024::encapsulate(&mlkem1024::MlKem1024PublicKey::from(ek), *rnd);
    let ss_kem = Zeroizing::new(ss_kem);
    let eph = x25519_dalek::EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let epk = x25519_dalek::PublicKey::from(&eph);
    let shared = eph.diffie_hellman(&x25519_dalek::PublicKey::from(x));
    if !shared.was_contributory() {
        return Err(format!("{}: a degenerate X25519 key", recipient.name));
    }

    let mut out = Vec::with_capacity(11 + recipient.name.len() + FPR + CT + EPK);
    out.extend_from_slice(MAGIC);
    out.push(VERSION);
    out.extend_from_slice(&(recipient.name.len() as u16).to_le_bytes());
    out.extend_from_slice(recipient.name.as_bytes());
    out.extend_from_slice(&fpr);
    out.extend_from_slice(ct.as_slice());
    out.extend_from_slice(epk.as_bytes());
    let (key, nonce) = content_key(&ss_kem, shared.as_bytes(), &out);
    let inner = make_inner(&fpr)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("a 32-byte key");
    let sealed = cipher
        .encrypt(&Nonce::<Aes256Gcm>::from(nonce), Payload { msg: inner.as_slice(), aad: &gcm_aad(&out, aad) })
        .map_err(|_| "sealing: AES-256-GCM failed".to_string())?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Who sealed content is for: the recipient's name and key fingerprint (hex). Reads the header
/// only; says nothing about whether the content is intact.
pub fn recipient_of(sealed: &[u8]) -> Result<(String, String), String> {
    let h = parse_header(sealed)?;
    Ok((h.recipient, auth::to_hex(&h.fpr)))
}

/// Decrypt sealed content addressed to `me`: the inner plaintext and the header.
fn decrypt(me: &Identity, sealed: &[u8], aad: &[u8]) -> Result<(Zeroizing<Vec<u8>>, Header), String> {
    let h = parse_header(sealed)?;
    if h.recipient != me.name {
        return Err(format!("sealed to {}, not to {}", h.recipient, me.name));
    }
    if h.fpr != me.kem_fingerprint() {
        return Err(format!("sealed to {} under other encryption keys than this key file's", h.recipient));
    }
    let (ss_kem, ss_x) = me.kem_open(&h.ct, &h.epk)?;
    let (key, nonce) = content_key(&ss_kem, &ss_x, &sealed[..h.len]);
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).expect("a 32-byte key");
    let inner = cipher
        .decrypt(&Nonce::<Aes256Gcm>::from(nonce), Payload { msg: &sealed[h.len..], aad: &gcm_aad(&sealed[..h.len], aad) })
        .map_err(|_| "sealed content does not open: it was altered, or sealed with other associated data".to_string())?;
    Ok((Zeroizing::new(inner), h))
}

/// The inner plaintext: the signer and signature (when signed) and where the body starts.
fn split_inner(inner: &[u8]) -> Result<(Option<(String, HybridSignature)>, usize), String> {
    let bad = || "sealed content: a malformed inner record".to_string();
    match inner.first() {
        Some(0) => Ok((None, 1)),
        Some(&FLAG_SIGNED) => {
            let n = u16::from_le_bytes(inner.get(1..3).ok_or_else(bad)?.try_into().unwrap()) as usize;
            let name = std::str::from_utf8(inner.get(3..3 + n).ok_or_else(bad)?).map_err(|_| bad())?.to_string();
            let at = 3 + n;
            let ml_dsa = inner.get(at..at + SIG_ML).ok_or_else(bad)?.to_vec();
            let ed25519 = inner.get(at + SIG_ML..at + SIG_ML + SIG_ED).ok_or_else(bad)?.to_vec();
            Ok((Some((name, HybridSignature { ml_dsa, ed25519 })), at + SIG_ML + SIG_ED))
        }
        Some(f) => Err(format!("sealed content: unknown flags {:#04x}", f)),
        None => Err(bad()),
    }
}

/// Open content sealed to `me` with `aad`. A signature is checked against `verifier` when one is
/// given (an admitted sender, both schemes), and a signature that fails is an error, not a
/// downgrade to "unsigned". Without a verifier the sender is reported as unchecked.
pub fn open(me: &Identity, sealed: &[u8], aad: &[u8], verifier: Option<&Verifier>) -> Result<Opened, String> {
    let (inner, h) = decrypt(me, sealed, aad)?;
    let (signer, at) = split_inner(&inner)?;
    let body = &inner[at..];
    let sender = match (signer, verifier) {
        (None, _) => Sender::Anonymous,
        (Some((name, _)), None) => Sender::Unchecked(name),
        (Some((name, sig)), Some(v)) => {
            let msg = signed_message(&name, &me.name, &h.fpr, aad, body);
            let kind = v.check_sender(&name, &sig, &msg, auth::now_ms())?;
            Sender::Verified { name, kind }
        }
    };
    Ok(Opened { body: body.to_vec(), sender })
}

/// Open content sealed to `me` that must be signed by `sender` (one known entry, no keyring).
pub fn open_from(me: &Identity, sealed: &[u8], aad: &[u8], sender: &KeyEntry) -> Result<Vec<u8>, String> {
    let (inner, h) = decrypt(me, sealed, aad)?;
    let (signer, at) = split_inner(&inner)?;
    let Some((name, sig)) = signer else {
        return Err(format!("sealed content is unsigned; it must be signed by {}", sender.name));
    };
    if name != sender.name {
        return Err(format!("sealed content is signed by {}; it must be signed by {}", name, sender.name));
    }
    let body = &inner[at..];
    auth::verify_entry(sender, Purpose::Seal, &sig, &signed_message(&name, &me.name, &h.fpr, aad, body))?;
    Ok(body.to_vec())
}

/// A store directory sealed to one identity and signed by it (2026-10-24): what `history --key`
/// and `run-proto --history-key` use. A file opens only if this identity sealed it, under this
/// name, so whoever can write the directory but does not hold the key can neither read a file nor
/// plant one.
pub struct SealedVault {
    me: Identity,
    entry: KeyEntry,
}

impl SealedVault {
    pub fn own(me: Identity) -> Self {
        let entry = me.public_entry(None);
        SealedVault { me, entry }
    }

    pub fn name(&self) -> &str {
        &self.me.name
    }
}

fn file_aad(name: &str) -> Vec<u8> {
    let mut a = b"cubelang/vault/v1\0".to_vec();
    a.extend_from_slice(name.as_bytes());
    a
}

impl Vault for SealedVault {
    fn seal_file(&self, name: &str, plain: &[u8]) -> Result<Vec<u8>, String> {
        seal(&self.entry, plain, &file_aad(name), Some(&self.me)).map_err(|e| format!("{}: {}", name, e))
    }

    fn open_file(&self, name: &str, sealed: &[u8]) -> Result<Vec<u8>, String> {
        open_from(&self.me, sealed, &file_aad(name), &self.entry).map_err(|e| format!("{}: {}", name, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{keygen, register, Keyring, Protect};

    fn ident(name: &str, kind: Kind) -> (Identity, KeyEntry) {
        let (file, entry) = keygen(name, kind, None, Protect::Plaintext).unwrap();
        (Identity::load(&file, None).unwrap(), entry)
    }

    /// Surreptitious forwarding: Bob receives a message Alice signed for him and seals the same
    /// inner record (her signature included) to Carol. Carol's open must not report Alice.
    #[test]
    fn a_signed_message_forwarded_to_someone_else_does_not_verify() {
        let (alice, alice_pk) = ident("Alice", Kind::User);
        let (bob, bob_pk) = ident("Bob", Kind::User);
        let (carol, carol_pk) = ident("Carol", Kind::User);
        let (root, root_pk) = ident("Root", Kind::Registrar);
        let mut ring = Keyring::default();
        ring.add_root(root_pk).unwrap();
        for e in [alice_pk, bob_pk.clone(), carol_pk.clone()] {
            ring.add(register(&root, e, 1, None).unwrap()).unwrap();
        }
        let v = Verifier::new(ring, vec![]).unwrap();

        let to_bob = seal(&bob_pk, b"meet at noon", b"", Some(&alice)).unwrap();
        let opened = open(&bob, &to_bob, b"", Some(&v)).unwrap();
        assert_eq!(opened.sender, Sender::Verified { name: "Alice".into(), kind: Kind::User });

        let (inner, _) = decrypt(&bob, &to_bob, b"").unwrap();
        let forwarded = seal_inner(&carol_pk, b"", |_| Ok(inner)).unwrap();
        let err = open(&carol, &forwarded, b"", Some(&v)).unwrap_err();
        assert!(err.contains("sender Alice"), "{err}");
        // and without a keyring the claim is only reported as unchecked
        assert_eq!(open(&carol, &forwarded, b"", None).unwrap().sender, Sender::Unchecked("Alice".into()));
        assert!(open_from(&carol, &forwarded, b"", &bob_pk).is_err());
    }
}
