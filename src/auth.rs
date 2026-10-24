//! Who is calling (2026-10-24), hardened to the NSA's CNSA 2.0 suite.
//!
//! Signatures. Every identity signs with TWO schemes over the same bytes, ML-DSA-87 (FIPS 204, final;
//! post-quantum, the CNSA 2.0 signature) and Ed25519 (classical). A claim is honoured only if both
//! verify, so a forger must break both. ML-DSA comes from `libcrux-ml-dsa` (Cryspen): its arithmetic,
//! NTT and serialization are formally verified (hax + F*), and it is tested against NIST ACVP and
//! Wycheproof vectors. Digests inside signed messages are SHA-384 (CNSA 2.0).
//!
//! One seed. An identity is ONE 32-byte secret. HKDF-SHA-384 derives the ML-DSA-87 key-generation
//! seed (FIPS 204's ξ, so the seed-to-key mapping is the standard's own, not this crate's) and the
//! Ed25519 seed. The whole identity therefore fits a small QR code that can be printed and kept
//! offline, and is never shown as text. A known-answer test pins the derivation.
//!
//! Key files at rest are sealed: Argon2id (64 MiB, 3 passes, 4 lanes) turns a passphrase of at least
//! 12 characters into an AES-256-GCM key; the header (name, kind, scheme, parameters) is the
//! associated data, so no field can be swapped. A plaintext key file exists only when asked for by
//! name (`Protect::Plaintext`, for test fixtures).
//!
//! Calls. A call signature covers the program's SHA-384, the function, the arguments, the ASK
//! answers, the knowledge file, the caller's name, a timestamp and a nonce. A timestamp outside the
//! window, or a nonce already seen inside it, is refused (no replays); a caller is held to a bounded
//! number of calls per window, so even a valid key cannot grow the verifier's memory without limit.
//!
//! The keyring (loaded by the HOST, never supplied by a request):
//! - `roots`: trust anchors of kind registrar. The host should PIN their fingerprints
//!   (`run-proto --pin-root`), so whoever can write the file cannot add a root;
//! - `keys`: users, agents, models and registrars, each registered (both schemes) by a root or an
//!   earlier registrar; only a root registers a registrar. A registration may carry an expiry;
//! - `revocations`: signed by a root or by the entry's own registrar. A revoked registrar takes
//!   everything it registered with it.
//! A keyring whose chain does not verify is refused whole (fail closed).
//!
//! Every entry and key file names its scheme (`ml-dsa-87+ed25519`): a later scheme is a new entry,
//! never a silent reinterpretation.
//!
//! Encryption keys (2026-10-24). The same seed derives an ML-KEM-1024 key pair (FIPS 203's (d, z))
//! and an X25519 key under other HKDF labels, so the signing keys a seed restores are unchanged. An
//! entry carries the public halves under `kem` (scheme `ml-kem-1024+x25519`), signed by the entry's
//! own keys (`Purpose::Kem`, deterministic, so the entry is a function of the seed); a keyring in
//! which a binding does not verify is refused whole. `crate::seal` seals content to these keys.

use std::collections::{HashMap, HashSet};

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit};
use ed25519_dalek::Signer as _;
use libcrux_ml_dsa::ml_dsa_87;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha384};
use zeroize::{Zeroize, Zeroizing};

/// The one scheme this build signs and verifies with.
pub const SCHEME: &str = "ml-dsa-87+ed25519";
/// How far a call's timestamp may sit from the verifier's clock, either way.
pub const DEFAULT_WINDOW_MS: u64 = 120_000;
/// Calls one caller may make inside one window (about 80 a second over the default window).
pub const MAX_CALLS_PER_WINDOW: usize = 10_000;
/// A passphrase shorter than this does not seal a key file.
pub const MIN_PASSPHRASE_CHARS: usize = 12;

const ML_DSA_87_VK: usize = 2592;
const ML_DSA_87_SIG: usize = 4627;
const ARGON2_M_KIB: u32 = 65_536;
const ARGON2_T: u32 = 3;
const ARGON2_P: u32 = 4;

/// What a signature is for. ML-DSA takes it as its context string; Ed25519 sees it as the tag every
/// message starts with. A signature for one purpose can never pass as another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Call,
    Register,
    Revoke,
    /// An entry's own signature over its encryption keys (2026-10-24).
    Kem,
    /// A sender's signature inside sealed content (2026-10-24, `crate::seal`).
    Seal,
}

impl Purpose {
    fn context(self) -> &'static [u8] {
        match self {
            Purpose::Call => b"cubelang-call-v1",
            Purpose::Register => b"cubelang-register-v1",
            Purpose::Revoke => b"cubelang-revoke-v1",
            Purpose::Kem => b"cubelang-kem-v1",
            Purpose::Seal => b"cubelang-seal-v1",
        }
    }
    pub(crate) fn tag(self) -> &'static [u8] {
        match self {
            Purpose::Call => b"cubelang/call/v1\0",
            Purpose::Register => b"cubelang/register/v1\0",
            Purpose::Revoke => b"cubelang/revoke/v1\0",
            Purpose::Kem => b"cubelang/kem/v1\0",
            Purpose::Seal => b"cubelang/seal/v1\0",
        }
    }
}

/// The encryption scheme (2026-10-24): ML-KEM-1024 (FIPS 203, the CNSA 2.0 key establishment) and
/// X25519, both; content is readable only by breaking both.
pub const KEM_SCHEME: &str = "ml-kem-1024+x25519";
pub(crate) const ML_KEM_1024_EK: usize = 1568;
pub(crate) const ML_KEM_1024_CT: usize = 1568;

/// An entry's encryption keys, bound to its signing keys by its own signature (both schemes,
/// `Purpose::Kem`), so they cannot be swapped by whoever can write the keyring -- a root's included,
/// though a root's registration covers only its signing keys.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KemKeys {
    pub scheme: String,
    /// ML-KEM-1024 encapsulation key, hex.
    pub ml_kem: String,
    /// X25519 public key, hex.
    pub x25519: String,
    pub binding: HybridSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    User,
    Agent,
    Model,
    Registrar,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::User => "user",
            Kind::Agent => "agent",
            Kind::Model => "model",
            Kind::Registrar => "registrar",
        }
    }
    pub fn parse(s: &str) -> Result<Kind, String> {
        match s {
            "user" => Ok(Kind::User),
            "agent" => Ok(Kind::Agent),
            "model" => Ok(Kind::Model),
            "registrar" => Ok(Kind::Registrar),
            _ => Err(format!("kind {:?}: expected user, agent, model or registrar", s)),
        }
    }
}

/// Both signatures over one message (hex in JSON).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HybridSignature {
    #[serde(with = "hex_bytes")]
    pub ml_dsa: Vec<u8>,
    #[serde(with = "hex_bytes")]
    pub ed25519: Vec<u8>,
}

/// A public keyring entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyEntry {
    pub name: String,
    pub kind: Kind,
    pub scheme: String,
    /// ML-DSA-87 verification key, hex.
    pub ml_dsa: String,
    /// Ed25519 verifying key, hex.
    pub ed25519: String,
    /// The weights the key speaks for (sha256, hex), when the host registered them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_sha256: Option<String>,
    /// Who registered this entry, when, until when, and their signatures over it. Roots carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration: Option<Registration>,
    /// The keys content is sealed to (2026-10-24); an entry made before then has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kem: Option<KemKeys>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    pub by: String,
    pub at_ms: u64,
    /// After this instant the entry is no longer honoured (and registers nothing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after_ms: Option<u64>,
    pub signature: HybridSignature,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Revocation {
    /// The revoked entry and the fingerprint of the keys it held (a re-registration under the same
    /// name with new keys is not revoked by this record).
    pub name: String,
    pub fingerprint: String,
    pub by: String,
    pub at_ms: u64,
    #[serde(default)]
    pub reason: String,
    pub signature: HybridSignature,
}

/// The seed, sealed under a passphrase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SealedSeed {
    pub kdf: String,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
    pub salt: String,
    pub aead: String,
    pub nonce: String,
    pub ciphertext: String,
}

/// A private key file: the identity's one seed, sealed (or, only when asked for, plaintext). Never
/// commit one, never put one in a .cube file or a request. (No Debug: a stray `{:?}` must not print
/// a secret; the plaintext seed is wiped from memory when the value drops.)
#[derive(Clone, Serialize, Deserialize)]
pub struct PrivateKeyFile {
    pub name: String,
    pub kind: Kind,
    pub scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<SealedSeed>,
}

impl Drop for PrivateKeyFile {
    fn drop(&mut self) {
        if let Some(s) = self.seed.as_mut() {
            s.zeroize();
        }
    }
}

/// Redacted: the name and kind, never the seed.
impl std::fmt::Debug for PrivateKeyFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateKeyFile").field("name", &self.name).field("kind", &self.kind)
            .field("sealed", &self.sealed.is_some()).finish_non_exhaustive()
    }
}

/// How a new key file holds its seed.
#[derive(Clone, Copy)]
pub enum Protect<'a> {
    Passphrase(&'a str),
    /// Only for test fixtures and throwaway identities: the seed sits in the file as hex.
    Plaintext,
}

fn check_name(name: &str) -> Result<(), String> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(format!("name {:?}: 1-64 characters of A-Z a-z 0-9 _ - .", name))
    }
}

fn check_basics(e: &KeyEntry) -> Result<(), String> {
    check_name(&e.name)?;
    if let Some(h) = &e.model_sha256 {
        if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("{}: model_sha256 must be 64 hex digits", e.name));
        }
    }
    Ok(())
}

fn sha384(parts: &[&[u8]]) -> [u8; 48] {
    let mut h = Sha384::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// The fingerprint of an entry's keys (SHA-384, hex): what a host pins and a revocation names.
pub fn fingerprint(e: &KeyEntry) -> String {
    let ml = from_hex(&e.ml_dsa).unwrap_or_default();
    let ed = from_hex(&e.ed25519).unwrap_or_default();
    to_hex(&sha384(&[b"cubelang/fingerprint/v1\0", e.scheme.as_bytes(), b"\0", &ml, &ed]))
}

/// Decoded public keys, ready to verify.
#[derive(Clone)]
struct PublicKeys {
    ml_dsa: ml_dsa_87::MLDSA87VerificationKey,
    ed25519: ed25519_dalek::VerifyingKey,
}

impl PublicKeys {
    fn verify(&self, purpose: Purpose, sig: &HybridSignature, message: &[u8]) -> Result<(), &'static str> {
        let ml_ok = match <[u8; ML_DSA_87_SIG]>::try_from(sig.ml_dsa.as_slice()) {
            Ok(b) => ml_dsa_87::verify(&self.ml_dsa, message, purpose.context(),
                                       &ml_dsa_87::MLDSA87Signature::new(b)).is_ok(),
            Err(_) => false,
        };
        let ed_ok = match <[u8; 64]>::try_from(sig.ed25519.as_slice()) {
            Ok(b) => self.ed25519.verify_strict(message, &ed25519_dalek::Signature::from_bytes(&b)).is_ok(),
            Err(_) => false,
        };
        match (ml_ok, ed_ok) {
            (true, true) => Ok(()),
            (false, true) => Err("the ML-DSA-87 signature does not verify"),
            (true, false) => Err("the Ed25519 signature does not verify"),
            (false, false) => Err("neither signature verifies"),
        }
    }
}

fn decode_public(e: &KeyEntry) -> Result<PublicKeys, String> {
    if e.scheme != SCHEME {
        return Err(format!("{} uses scheme {:?}; this build verifies {} only", e.name, e.scheme, SCHEME));
    }
    let ml: [u8; ML_DSA_87_VK] = from_hex(&e.ml_dsa)
        .map_err(|x| format!("{}: ml_dsa: {}", e.name, x))?
        .try_into()
        .map_err(|_| format!("{}: an ML-DSA-87 public key is {} bytes", e.name, ML_DSA_87_VK))?;
    let ed: [u8; 32] = from_hex(&e.ed25519)
        .map_err(|x| format!("{}: ed25519: {}", e.name, x))?
        .try_into()
        .map_err(|_| format!("{}: an Ed25519 public key is 32 bytes", e.name))?;
    let ed25519 = ed25519_dalek::VerifyingKey::from_bytes(&ed)
        .map_err(|_| format!("{}: not a valid Ed25519 public key", e.name))?;
    if ed25519.is_weak() {
        return Err(format!("{}: a weak Ed25519 public key", e.name));
    }
    Ok(PublicKeys { ml_dsa: ml_dsa_87::MLDSA87VerificationKey::new(ml), ed25519 })
}

/// The two key seeds one identity seed stands for (HKDF-SHA-384, domain-separated).
fn derive(seed: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let hk = hkdf::Hkdf::<Sha384>::new(Some(b"cubelang identity v1"), seed);
    let mut xi = Zeroizing::new([0u8; 32]);
    let mut ed = Zeroizing::new([0u8; 32]);
    hk.expand(b"ml-dsa-87 xi", xi.as_mut()).expect("32 bytes is a valid HKDF length");
    hk.expand(b"ed25519 seed", ed.as_mut()).expect("32 bytes is a valid HKDF length");
    (xi, ed)
}

/// The two encryption key seeds one identity seed stands for (2026-10-24): FIPS 203's (d, z) for
/// ML-KEM-1024 key generation, and the X25519 secret. Same HKDF, other labels: the signing keys a
/// seed restores are unchanged.
fn derive_kem(seed: &[u8; 32]) -> (Zeroizing<[u8; 64]>, Zeroizing<[u8; 32]>) {
    let hk = hkdf::Hkdf::<Sha384>::new(Some(b"cubelang identity v1"), seed);
    let mut dz = Zeroizing::new([0u8; 64]);
    let mut x = Zeroizing::new([0u8; 32]);
    hk.expand(b"ml-kem-1024 d||z", dz.as_mut()).expect("64 bytes is a valid HKDF length");
    hk.expand(b"x25519 secret", x.as_mut()).expect("32 bytes is a valid HKDF length");
    (dz, x)
}

/// The bytes an entry's own signature over its encryption keys covers: who (name, kind, and the
/// fingerprint of the signing keys), the scheme, and both public keys.
pub fn kem_binding_message(e: &KeyEntry, scheme: &str, ml_kem: &[u8], x25519: &[u8]) -> Vec<u8> {
    let mut m = Purpose::Kem.tag().to_vec();
    field(&mut m, e.name.as_bytes());
    field(&mut m, e.kind.as_str().as_bytes());
    field(&mut m, &from_hex(&fingerprint(e)).unwrap_or_default());
    field(&mut m, scheme.as_bytes());
    field(&mut m, ml_kem);
    field(&mut m, x25519);
    m
}

/// The fingerprint of a pair of encryption keys (SHA-384): what sealed content names its recipient
/// by, and what both key derivation and the sender's signature bind.
pub fn kem_fingerprint(ml_kem: &[u8], x25519: &[u8]) -> [u8; 48] {
    sha384(&[b"cubelang/kem-fingerprint/v1\0", KEM_SCHEME.as_bytes(), b"\0", ml_kem, x25519])
}

/// An entry's encryption keys, decoded and checked: the scheme, the sizes, a valid ML-KEM key, a
/// non-degenerate X25519 key, and the entry's own signature over them.
pub(crate) fn kem_public(e: &KeyEntry) -> Result<([u8; ML_KEM_1024_EK], [u8; 32]), String> {
    kem_checked(e, &decode_public(e)?)
}

/// Check `message` is signed for `purpose` by the keys of `e` (both schemes).
pub fn verify_entry(e: &KeyEntry, purpose: Purpose, sig: &HybridSignature, message: &[u8]) -> Result<(), String> {
    decode_public(e)?.verify(purpose, sig, message).map_err(|why| format!("{}: {}", e.name, why))
}

fn kem_checked(e: &KeyEntry, keys: &PublicKeys) -> Result<([u8; ML_KEM_1024_EK], [u8; 32]), String> {
    let k = e.kem.as_ref().ok_or_else(|| format!("{} has no encryption keys (an entry made before 2026-10-24; make a new one)", e.name))?;
    if k.scheme != KEM_SCHEME {
        return Err(format!("{}: encryption scheme {:?}; this build seals with {} only", e.name, k.scheme, KEM_SCHEME));
    }
    let ek: [u8; ML_KEM_1024_EK] = from_hex(&k.ml_kem)
        .map_err(|x| format!("{}: ml_kem: {}", e.name, x))?
        .try_into()
        .map_err(|_| format!("{}: an ML-KEM-1024 key is {} bytes", e.name, ML_KEM_1024_EK))?;
    let x: [u8; 32] = from_hex(&k.x25519)
        .map_err(|y| format!("{}: x25519: {}", e.name, y))?
        .try_into()
        .map_err(|_| format!("{}: an X25519 key is 32 bytes", e.name))?;
    if !libcrux_ml_kem::mlkem1024::validate_public_key(&libcrux_ml_kem::mlkem1024::MlKem1024PublicKey::from(ek)) {
        return Err(format!("{}: not a valid ML-KEM-1024 key", e.name));
    }
    if x.iter().all(|b| *b == 0) {
        return Err(format!("{}: a degenerate X25519 key", e.name));
    }
    keys.verify(Purpose::Kem, &k.binding, &kem_binding_message(e, &k.scheme, &ek, &x))
        .map_err(|why| format!("{}: its encryption keys are not signed by its own keys ({})", e.name, why))?;
    Ok((ek, x))
}

/// The two shared secrets of one KEM opening (ML-KEM's, X25519's), wiped when dropped.
pub(crate) type SharedSecrets = (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>);

/// An identity ready to sign. It keeps the ML-DSA seed, not the 4,896-byte signing key: the key is
/// regenerated for each signature (FIPS 204 key generation is fast) and wiped right after, so the
/// expanded key exists in memory only while it signs. The same for ML-KEM's decapsulation key
/// (2026-10-24): regenerated to open sealed content, wiped after.
pub struct Identity {
    pub name: String,
    pub kind: Kind,
    xi: Zeroizing<[u8; 32]>,
    ml_dsa_vk: Vec<u8>,
    ed25519: ed25519_dalek::SigningKey,
    kem_dz: Zeroizing<[u8; 64]>,
    ml_kem_ek: Vec<u8>,
    x25519: x25519_dalek::StaticSecret,
}

/// Redacted: the name and kind, never a key.
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Identity").field("name", &self.name).field("kind", &self.kind).finish_non_exhaustive()
    }
}

impl Identity {
    pub fn from_seed(name: &str, kind: Kind, seed: &[u8; 32]) -> Result<Identity, String> {
        check_name(name)?;
        let (xi, ed) = derive(seed);
        let mut kp = ml_dsa_87::generate_key_pair(*xi);
        let ml_dsa_vk = kp.verification_key.as_slice().to_vec();
        kp.signing_key.as_mut_slice().zeroize();
        let (kem_dz, x) = derive_kem(seed);
        let ml_kem_ek = {
            let kp = libcrux_ml_kem::mlkem1024::generate_key_pair(*kem_dz);
            let (sk, pk) = kp.into_parts();
            let ek = pk.as_slice().to_vec();
            let mut sk: [u8; 3168] = sk.into();
            sk.zeroize();
            ek
        };
        Ok(Identity {
            name: name.to_string(), kind, xi, ml_dsa_vk, ed25519: ed25519_dalek::SigningKey::from_bytes(&ed),
            kem_dz, ml_kem_ek, x25519: x25519_dalek::StaticSecret::from(*x),
        })
    }

    /// Open a KEM encapsulation made to this identity: the two shared secrets (ML-KEM's, X25519's).
    /// ML-KEM rejects implicitly (a forged ciphertext yields an unrelated secret, so the AEAD fails);
    /// an X25519 exchange with a low-order point is refused.
    pub(crate) fn kem_open(&self, ct: &[u8; ML_KEM_1024_CT], epk: &[u8; 32]) -> Result<SharedSecrets, String> {
        let kp = libcrux_ml_kem::mlkem1024::generate_key_pair(*self.kem_dz);
        let (sk, _pk) = kp.into_parts();
        let ss = Zeroizing::new(libcrux_ml_kem::mlkem1024::decapsulate(&sk, &libcrux_ml_kem::mlkem1024::MlKem1024Ciphertext::from(*ct)));
        let mut sk_bytes: [u8; 3168] = sk.into();
        sk_bytes.zeroize();
        let shared = self.x25519.diffie_hellman(&x25519_dalek::PublicKey::from(*epk));
        if !shared.was_contributory() {
            return Err("sealed content: a degenerate X25519 key".into());
        }
        Ok((ss, Zeroizing::new(shared.to_bytes())))
    }

    /// The fingerprint of this identity's encryption keys (what content sealed to it names).
    pub fn kem_fingerprint(&self) -> [u8; 48] {
        kem_fingerprint(&self.ml_kem_ek, x25519_dalek::PublicKey::from(&self.x25519).as_bytes())
    }

    /// ML-DSA in FIPS 204's deterministic mode, for a signature that must be the same every time
    /// it is made (the binding of the encryption keys, so `public_entry` is a function of the seed).
    fn sign_deterministic(&self, purpose: Purpose, message: &[u8]) -> Result<HybridSignature, String> {
        if !message.starts_with(purpose.tag()) {
            return Err("refusing to sign: the message is not a CubeLang message of this purpose".into());
        }
        let mut kp = ml_dsa_87::generate_key_pair(*self.xi);
        let signed = ml_dsa_87::sign(&kp.signing_key, message, purpose.context(), [0u8; 32]);
        kp.signing_key.as_mut_slice().zeroize();
        let ml = signed.map_err(|e| format!("ML-DSA-87 signing failed: {:?}", e))?;
        Ok(HybridSignature { ml_dsa: ml.as_slice().to_vec(), ed25519: self.ed25519.sign(message).to_bytes().to_vec() })
    }

    /// Open a key file. A sealed file needs its passphrase; a plaintext one takes none.
    pub fn load(file: &PrivateKeyFile, passphrase: Option<&str>) -> Result<Identity, String> {
        if file.scheme != SCHEME {
            return Err(format!("the key file uses scheme {:?}; this build signs with {} only", file.scheme, SCHEME));
        }
        let seed: Zeroizing<[u8; 32]> = match (&file.seed, &file.sealed) {
            (Some(hex), None) => {
                let raw = Zeroizing::new(from_hex(hex)?);
                Zeroizing::new(raw.as_slice().try_into().map_err(|_| "a seed is 32 bytes".to_string())?)
            }
            (None, Some(s)) => {
                let pass = passphrase.ok_or_else(|| format!("the key file for {} is sealed: it needs its passphrase", file.name))?;
                unseal(file, s, pass)?
            }
            _ => return Err("a key file holds exactly one of `seed` or `sealed`".into()),
        };
        Identity::from_seed(&file.name, file.kind, &seed)
    }

    /// The public entry this identity's keys belong to (unregistered).
    pub fn public_entry(&self, model_sha256: Option<String>) -> KeyEntry {
        let mut e = KeyEntry {
            name: self.name.clone(),
            kind: self.kind,
            scheme: SCHEME.to_string(),
            ml_dsa: to_hex(&self.ml_dsa_vk),
            ed25519: to_hex(self.ed25519.verifying_key().as_bytes()),
            model_sha256: model_sha256.map(|h| h.to_ascii_lowercase()),
            registration: None,
            kem: None,
        };
        let x = x25519_dalek::PublicKey::from(&self.x25519).to_bytes();
        let msg = kem_binding_message(&e, KEM_SCHEME, &self.ml_kem_ek, &x);
        let binding = self.sign_deterministic(Purpose::Kem, &msg)
            .expect("a binding message always starts with its own tag");
        e.kem = Some(KemKeys { scheme: KEM_SCHEME.into(), ml_kem: to_hex(&self.ml_kem_ek), x25519: to_hex(&x), binding });
        e
    }

    /// Both signatures for `purpose` (ML-DSA hedged with fresh randomness). Refuses a message that is
    /// not a CubeLang message of that purpose, so a signing host cannot be used as an oracle.
    pub fn sign(&self, purpose: Purpose, message: &[u8]) -> Result<HybridSignature, String> {
        if !message.starts_with(purpose.tag()) {
            return Err("refusing to sign: the message is not a CubeLang message of this purpose".into());
        }
        let mut kp = ml_dsa_87::generate_key_pair(*self.xi);
        let mut rnd = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut rnd);
        let signed = ml_dsa_87::sign(&kp.signing_key, message, purpose.context(), rnd);
        kp.signing_key.as_mut_slice().zeroize();
        let ml = signed.map_err(|e| format!("ML-DSA-87 signing failed: {:?}", e))?;
        Ok(HybridSignature { ml_dsa: ml.as_slice().to_vec(), ed25519: self.ed25519.sign(message).to_bytes().to_vec() })
    }
}

fn keyfile_aad(name: &str, kind: Kind, scheme: &str, s: &SealedSeed) -> Vec<u8> {
    let mut m = b"cubelang/keyfile/v1\0".to_vec();
    for f in [name, kind.as_str(), scheme, &s.kdf, &s.aead] {
        field(&mut m, f.as_bytes());
    }
    for n in [s.m_kib, s.t, s.p] {
        m.extend_from_slice(&n.to_le_bytes());
    }
    m
}

fn argon2_key(pass: &str, salt: &[u8], m_kib: u32, t: u32, p: u32) -> Result<Zeroizing<[u8; 32]>, String> {
    let params = argon2::Params::new(m_kib, t, p, Some(32)).map_err(|e| format!("argon2: {}", e))?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut key = Zeroizing::new([0u8; 32]);
    a.hash_password_into(pass.as_bytes(), salt, key.as_mut()).map_err(|e| format!("argon2: {}", e))?;
    Ok(key)
}

fn seal(name: &str, kind: Kind, seed: &[u8; 32], pass: &str) -> Result<SealedSeed, String> {
    if pass.chars().count() < MIN_PASSPHRASE_CHARS {
        return Err(format!("a passphrase seals a key file only with at least {} characters", MIN_PASSPHRASE_CHARS));
    }
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut salt);
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let mut s = SealedSeed {
        kdf: "argon2id".into(), m_kib: ARGON2_M_KIB, t: ARGON2_T, p: ARGON2_P, salt: to_hex(&salt),
        aead: "aes-256-gcm".into(), nonce: to_hex(&nonce), ciphertext: String::new(),
    };
    let key = argon2_key(pass, &salt, s.m_kib, s.t, s.p)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "aes-256-gcm: bad key".to_string())?;
    let aad = keyfile_aad(name, kind, SCHEME, &s);
    let ct = cipher
        .encrypt(&aes_gcm::aead::Nonce::<Aes256Gcm>::from(nonce), Payload { msg: seed, aad: &aad })
        .map_err(|_| "aes-256-gcm: sealing failed".to_string())?;
    s.ciphertext = to_hex(&ct);
    Ok(s)
}

fn unseal(file: &PrivateKeyFile, s: &SealedSeed, pass: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    if s.kdf != "argon2id" || s.aead != "aes-256-gcm" {
        return Err(format!("key file: {} + {} is not a sealing this build opens", s.kdf, s.aead));
    }
    // a file edited down to weaker parameters is refused, not opened
    if s.m_kib < ARGON2_M_KIB || s.t < ARGON2_T || s.p < 1 {
        return Err("key file: its Argon2id parameters are below this build's minimum".into());
    }
    let salt = from_hex(&s.salt)?;
    let nonce: [u8; 12] = from_hex(&s.nonce)?.try_into().map_err(|_| "key file: the nonce is 12 bytes".to_string())?;
    let key = argon2_key(pass, &salt, s.m_kib, s.t, s.p)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| "aes-256-gcm: bad key".to_string())?;
    let aad = keyfile_aad(&file.name, file.kind, &file.scheme, s);
    let pt = Zeroizing::new(
        cipher
            .decrypt(&aes_gcm::aead::Nonce::<Aes256Gcm>::from(nonce), Payload { msg: &from_hex(&s.ciphertext)?, aad: &aad })
            .map_err(|_| format!("the key file for {} does not open: a wrong passphrase, or the file was altered", file.name))?,
    );
    Ok(Zeroizing::new(pt.as_slice().try_into().map_err(|_| "key file: the seed is 32 bytes".to_string())?))
}

/// The key file and public entry a seed stands for (what restoring a printed backup does).
pub fn identity_from_seed(name: &str, kind: Kind, seed: &[u8; 32], model_sha256: Option<String>, protect: Protect)
                          -> Result<(PrivateKeyFile, KeyEntry), String> {
    let public = Identity::from_seed(name, kind, seed)?.public_entry(model_sha256);
    check_basics(&public)?;
    let file = match protect {
        Protect::Plaintext => PrivateKeyFile { name: name.into(), kind, scheme: SCHEME.into(), seed: Some(to_hex(seed)), sealed: None },
        Protect::Passphrase(p) => PrivateKeyFile { name: name.into(), kind, scheme: SCHEME.into(), seed: None, sealed: Some(seal(name, kind, seed, p)?) },
    };
    Ok((file, public))
}

/// A fresh identity from a random seed.
pub fn keygen(name: &str, kind: Kind, model_sha256: Option<String>, protect: Protect) -> Result<(PrivateKeyFile, KeyEntry), String> {
    let mut seed = Zeroizing::new([0u8; 32]);
    rand::rngs::OsRng.fill_bytes(seed.as_mut());
    identity_from_seed(name, kind, &seed, model_sha256, protect)
}

fn field(m: &mut Vec<u8>, b: &[u8]) {
    m.extend_from_slice(&(b.len() as u64).to_le_bytes());
    m.extend_from_slice(b);
}

/// The `function` a scheduler tick signs in place of an entry name (no function can be named so).
pub const TICK: &str = "\0tick";

/// One call, as its signature covers it: what runs (the program's SHA-384, the function, the
/// arguments), what it is told (the ASK answers, the knowledge file, the history), who asks, and when. A request's
/// `branch` and `budget` are the host's settings, not the caller's words, and are not covered (TLS
/// protects them on a network leg).
#[derive(Debug, Clone, Copy)]
pub struct CallClaim<'a> {
    pub program_source: &'a str,
    pub function: &'a str,
    pub args: &'a [String],
    pub answers: &'a [String],
    pub knowledge_path: &'a str,
    pub caller: &'a str,
    pub timestamp_ms: u64,
    pub nonce: u64,
    /// The history directory the call is told (2026-10-24); empty for none.
    pub history_path: &'a str,
}

impl CallClaim<'_> {
    /// The bytes signed. Every field is length-prefixed, so no two different calls encode to the same
    /// bytes (an argument cannot be re-split, or moved into the answers).
    pub fn message(&self) -> Vec<u8> {
        let mut m = Purpose::Call.tag().to_vec();
        m.extend_from_slice(&program_digest(self.program_source));
        field(&mut m, self.function.as_bytes());
        for list in [self.args, self.answers] {
            m.extend_from_slice(&(list.len() as u64).to_le_bytes());
            for a in list {
                field(&mut m, a.as_bytes());
            }
        }
        field(&mut m, self.knowledge_path.as_bytes());
        field(&mut m, self.caller.as_bytes());
        m.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        m.extend_from_slice(&self.nonce.to_le_bytes());
        // 2026-10-24: a tagged trailer, present only when set, so a call that names no history
        // signs exactly the bytes it always did (every field above is fixed-size or
        // length-prefixed, so the trailer cannot be confused with them).
        if !self.history_path.is_empty() {
            m.extend_from_slice(b"\0history");
            field(&mut m, self.history_path.as_bytes());
        }
        m
    }
}

/// SHA-384 of a program's source: what a call signs and how a deployed contract is recognised.
pub fn program_digest(source: &str) -> [u8; 48] {
    sha384(&[b"cubelang/program/v1\0", source.as_bytes()])
}

/// The bytes a registration covers: every field of the entry except the registration itself, then
/// who registers it, when, and until when.
pub fn registration_message(entry: &KeyEntry, by: &str, at_ms: u64, not_after_ms: Option<u64>) -> Vec<u8> {
    let mut m = Purpose::Register.tag().to_vec();
    field(&mut m, entry.name.as_bytes());
    field(&mut m, entry.kind.as_str().as_bytes());
    field(&mut m, entry.scheme.as_bytes());
    field(&mut m, &from_hex(&entry.ml_dsa).unwrap_or_default());
    field(&mut m, &from_hex(&entry.ed25519).unwrap_or_default());
    field(&mut m, &entry.model_sha256.as_deref().map(|h| from_hex(h).unwrap_or_default()).unwrap_or_default());
    field(&mut m, by.as_bytes());
    m.extend_from_slice(&at_ms.to_le_bytes());
    match not_after_ms {
        Some(t) => { m.push(1); m.extend_from_slice(&t.to_le_bytes()); }
        None => m.push(0),
    }
    m
}

pub fn revocation_message(name: &str, fingerprint: &str, by: &str, at_ms: u64, reason: &str) -> Vec<u8> {
    let mut m = Purpose::Revoke.tag().to_vec();
    field(&mut m, name.as_bytes());
    field(&mut m, &from_hex(fingerprint).unwrap_or_default());
    field(&mut m, by.as_bytes());
    m.extend_from_slice(&at_ms.to_le_bytes());
    field(&mut m, reason.as_bytes());
    m
}

/// Register `entry` as `registrar`, optionally until `not_after_ms`.
pub fn register(registrar: &Identity, mut entry: KeyEntry, at_ms: u64, not_after_ms: Option<u64>) -> Result<KeyEntry, String> {
    check_basics(&entry)?;
    if registrar.kind != Kind::Registrar {
        return Err(format!("{} is a {}; only a registrar registers", registrar.name, registrar.kind.as_str()));
    }
    if not_after_ms.is_some_and(|t| t <= at_ms) {
        return Err("a registration must end after it starts".into());
    }
    entry.registration = None;
    let msg = registration_message(&entry, &registrar.name, at_ms, not_after_ms);
    let signature = registrar.sign(Purpose::Register, &msg)?;
    entry.registration = Some(Registration { by: registrar.name.clone(), at_ms, not_after_ms, signature });
    Ok(entry)
}

/// Revoke `entry` (its current keys) as `by`: a root, or the registrar that registered it.
pub fn revoke(by: &Identity, entry: &KeyEntry, at_ms: u64, reason: &str) -> Result<Revocation, String> {
    let fp = fingerprint(entry);
    let msg = revocation_message(&entry.name, &fp, &by.name, at_ms, reason);
    let signature = by.sign(Purpose::Revoke, &msg)?;
    Ok(Revocation { name: entry.name.clone(), fingerprint: fp, by: by.name.clone(), at_ms, reason: reason.into(), signature })
}

/// A keyring entry the chain admits: its keys, its kind and its expiry.
#[derive(Clone)]
struct Admitted {
    keys: PublicKeys,
    kind: Kind,
    not_after_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Keyring {
    #[serde(default)]
    pub roots: Vec<KeyEntry>,
    #[serde(default)]
    pub keys: Vec<KeyEntry>,
    #[serde(default)]
    pub revocations: Vec<Revocation>,
}

impl Keyring {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let ring: Keyring = serde_json::from_str(text).map_err(|e| format!("keyring: {}", e))?;
        ring.validate()?;
        Ok(ring)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The whole chain, checked: see the module doc.
    pub fn validate(&self) -> Result<(), String> {
        self.admitted().map(|_| ())
    }

    /// Names and kinds of the entries the chain admits now (revoked ones left out).
    pub fn admitted_names(&self) -> Result<Vec<(String, Kind)>, String> {
        let a = self.admitted()?;
        Ok(self.roots.iter().chain(self.keys.iter())
            .filter(|e| a.contains_key(&e.name))
            .map(|e| (e.name.clone(), e.kind))
            .collect())
    }

    fn admitted(&self) -> Result<HashMap<String, Admitted>, String> {
        let mut names = HashSet::new();
        let mut all: HashMap<String, Admitted> = HashMap::new();
        let mut roots = HashSet::new();
        let mut registrars = HashSet::new();
        for r in &self.roots {
            check_basics(r).map_err(|e| format!("keyring: {}", e))?;
            if !names.insert(r.name.clone()) {
                return Err(format!("keyring: {} appears twice", r.name));
            }
            if r.kind != Kind::Registrar {
                return Err(format!("keyring: root {} must be of kind registrar", r.name));
            }
            if r.registration.is_some() {
                return Err(format!("keyring: root {} carries a registration; a root is an anchor the host's owner places", r.name));
            }
            let keys = decode_public(r).map_err(|e| format!("keyring: {}", e))?;
            if r.kem.is_some() {
                kem_checked(r, &keys).map_err(|e| format!("keyring: {}", e))?;
            }
            all.insert(r.name.clone(), Admitted { keys, kind: r.kind, not_after_ms: None });
            roots.insert(r.name.clone());
        }
        for k in &self.keys {
            check_basics(k).map_err(|e| format!("keyring: {}", e))?;
            if !names.insert(k.name.clone()) {
                return Err(format!("keyring: {} appears twice", k.name));
            }
            let keys = decode_public(k).map_err(|e| format!("keyring: {}", e))?;
            let reg = k.registration.as_ref()
                .ok_or_else(|| format!("keyring: {} has no registration (only roots stand unsigned)", k.name))?;
            let by_root = roots.contains(&reg.by);
            if !by_root && !registrars.contains(&reg.by) {
                return Err(format!("keyring: {} is registered by {:?}, which is not a root or an earlier registrar", k.name, reg.by));
            }
            if k.kind == Kind::Registrar && !by_root {
                return Err(format!("keyring: registrar {} is registered by {}; only a root registers a registrar", k.name, reg.by));
            }
            if reg.not_after_ms.is_some_and(|t| t <= reg.at_ms) {
                return Err(format!("keyring: the registration of {} ends before it starts", k.name));
            }
            let signer = &all[&reg.by];
            if signer.not_after_ms.is_some_and(|t| reg.at_ms > t) {
                return Err(format!("keyring: {} was registered by {} after {}'s own registration expired", k.name, reg.by, reg.by));
            }
            signer.keys
                .verify(Purpose::Register, &reg.signature, &registration_message(k, &reg.by, reg.at_ms, reg.not_after_ms))
                .map_err(|e| format!("keyring: the registration of {} by {}: {}", k.name, reg.by, e))?;
            // encryption keys, when the entry has them, must carry the entry's own signature: a
            // keyring with one swapped key pair is refused whole, like a broken chain
            if k.kem.is_some() {
                kem_checked(k, &keys).map_err(|e| format!("keyring: {}", e))?;
            }
            if k.kind == Kind::Registrar {
                registrars.insert(k.name.clone());
            }
            all.insert(k.name.clone(), Admitted { keys, kind: k.kind, not_after_ms: reg.not_after_ms });
        }
        // Revocations: each signed by a root or by the revoked entry's own registrar.
        let mut revoked: HashSet<String> = HashSet::new();
        for r in &self.revocations {
            let target = self.keys.iter().find(|k| k.name == r.name)
                .ok_or_else(|| format!("keyring: a revocation names {}, which is not a registered entry (a root is removed by its owner, not revoked)", r.name))?;
            let registrar = target.registration.as_ref().map(|g| g.by.as_str()).unwrap_or("");
            if !roots.contains(&r.by) && r.by != registrar {
                return Err(format!("keyring: {} may not revoke {} (only a root or its registrar, {})", r.by, r.name, registrar));
            }
            let signer = all.get(&r.by)
                .ok_or_else(|| format!("keyring: the revocation of {} is signed by {}, who is not in the keyring", r.name, r.by))?;
            signer.keys
                .verify(Purpose::Revoke, &r.signature, &revocation_message(&r.name, &r.fingerprint, &r.by, r.at_ms, &r.reason))
                .map_err(|e| format!("keyring: the revocation of {} by {}: {}", r.name, r.by, e))?;
            if fingerprint(target) == r.fingerprint {
                revoked.insert(r.name.clone());
            }
        }
        // A revoked registrar takes what it registered with it (registrars come before their entries).
        for k in &self.keys {
            if k.registration.as_ref().is_some_and(|g| revoked.contains(&g.by)) {
                revoked.insert(k.name.clone());
            }
        }
        all.retain(|n, _| !revoked.contains(n));
        Ok(all)
    }

    pub fn get(&self, name: &str) -> Option<&KeyEntry> {
        self.roots.iter().chain(self.keys.iter()).find(|k| k.name == name)
    }

    /// The entry content is sealed to when it is addressed to `name` (2026-10-24): the chain
    /// admits it now (registered, not revoked, not expired at `now_ms`) and it has encryption keys.
    pub fn recipient(&self, name: &str, now_ms: u64) -> Result<&KeyEntry, String> {
        let admitted = self.admitted()?;
        let a = admitted.get(name).ok_or_else(|| format!("{} is not admitted by the keyring (unknown or revoked)", name))?;
        if a.not_after_ms.is_some_and(|t| now_ms > t) {
            return Err(format!("{}: the registration expired", name));
        }
        let e = self.get(name).expect("an admitted name is in the keyring");
        kem_public(e)?;
        Ok(e)
    }

    /// Give `me`'s entry its encryption keys (2026-10-24): for an entry made before entries carried
    /// them. Only the key's owner can: the keys are bound by the entry's own signature, and the
    /// registration (which covers the signing keys only) stays as it is.
    pub fn enable_sealing(&mut self, me: &Identity) -> Result<(), String> {
        let mine = me.public_entry(None);
        let e = self.roots.iter_mut().chain(self.keys.iter_mut()).find(|k| k.name == me.name)
            .ok_or_else(|| format!("{} is not in the keyring", me.name))?;
        if fingerprint(e) != fingerprint(&mine) {
            return Err(format!("the keyring's {} holds other signing keys than this key file's", me.name));
        }
        let before = e.kem.replace(mine.kem.expect("a public entry carries encryption keys"));
        if let Err(err) = self.validate() {
            let e = self.roots.iter_mut().chain(self.keys.iter_mut()).find(|k| k.name == me.name).unwrap();
            e.kem = before;
            return Err(err);
        }
        Ok(())
    }

    /// Append a registered entry, checked exactly as a load would check it.
    pub fn add(&mut self, entry: KeyEntry) -> Result<(), String> {
        self.keys.push(entry);
        if let Err(e) = self.validate() {
            self.keys.pop();
            return Err(e);
        }
        Ok(())
    }

    /// Add a trust anchor (a registrar's public entry, unregistered).
    pub fn add_root(&mut self, entry: KeyEntry) -> Result<(), String> {
        self.roots.push(entry);
        if let Err(e) = self.validate() {
            self.roots.pop();
            return Err(e);
        }
        Ok(())
    }

    pub fn add_revocation(&mut self, r: Revocation) -> Result<(), String> {
        self.revocations.push(r);
        if let Err(e) = self.validate() {
            self.revocations.pop();
            return Err(e);
        }
        Ok(())
    }
}

/// The verifier a resident host keeps across requests: the admitted keys (decoded once), the pinned
/// root fingerprints, and the nonces seen inside the window, per caller.
pub struct Verifier {
    keyring: Keyring,
    admitted: HashMap<String, Admitted>,
    pins: Vec<String>,
    pub window_ms: u64,
    seen: HashMap<String, HashMap<u64, u64>>,
    checks: u64,
}

fn check_pins(ring: &Keyring, pins: &[String]) -> Result<(), String> {
    if pins.is_empty() {
        return Ok(());
    }
    if ring.roots.is_empty() {
        return Err("keyring: roots are pinned but the keyring has none".into());
    }
    for r in &ring.roots {
        let fp = fingerprint(r);
        if !pins.iter().any(|p| p.eq_ignore_ascii_case(&fp)) {
            return Err(format!("keyring: root {} (fingerprint {}) is not one of the pinned roots", r.name, fp));
        }
    }
    Ok(())
}

impl Verifier {
    /// `pins`: the root fingerprints the host trusts. Empty: the file's roots are trusted as found.
    pub fn new(keyring: Keyring, pins: Vec<String>) -> Result<Self, String> {
        check_pins(&keyring, &pins)?;
        let admitted = keyring.admitted()?;
        Ok(Self { keyring, admitted, pins, window_ms: DEFAULT_WINDOW_MS, seen: HashMap::new(), checks: 0 })
    }

    pub fn keyring(&self) -> &Keyring {
        &self.keyring
    }

    /// The kind of an admitted caller (None: unknown, revoked or never admitted).
    pub fn kind_of(&self, name: &str) -> Option<Kind> {
        self.admitted.get(name).map(|a| a.kind)
    }

    /// Check a signature on sealed content (`Purpose::Seal`, 2026-10-24): the signer is admitted
    /// now and both schemes verify. Sealed content carries no nonce -- it is a document, and a
    /// copy of a document is the same document -- so nothing is recorded.
    pub fn check_sender(&self, name: &str, signature: &HybridSignature, message: &[u8], now_ms: u64) -> Result<Kind, String> {
        let who = self.admitted.get(name)
            .ok_or_else(|| format!("sender {:?} is not in the keyring (or was revoked)", name))?;
        if who.not_after_ms.is_some_and(|t| now_ms > t) {
            return Err(format!("sender {}: the registration expired", name));
        }
        who.keys.verify(Purpose::Seal, signature, message).map_err(|e| format!("sender {}: {}", name, e))?;
        Ok(who.kind)
    }

    /// Swap in a reloaded keyring, keeping the pins and the nonces seen (a reload must not reopen a
    /// replay). A keyring that fails its chain or its pins is refused and the old one stays.
    pub fn replace_keyring(&mut self, keyring: Keyring) -> Result<(), String> {
        check_pins(&keyring, &self.pins)?;
        let admitted = keyring.admitted()?;
        self.keyring = keyring;
        self.admitted = admitted;
        Ok(())
    }

    /// The verified caller name, or why the claim is refused. A refusal records nothing, so a forged
    /// call cannot burn a legitimate caller's nonce.
    pub fn check(&mut self, name: &str, signature: &HybridSignature, timestamp_ms: u64, nonce: u64,
                 message: &[u8], now_ms: u64) -> Result<String, String> {
        let window = self.window_ms;
        self.checks += 1;
        if self.checks % 1024 == 0 {
            for m in self.seen.values_mut() {
                m.retain(|_, ts| now_ms.abs_diff(*ts) <= window);
            }
            self.seen.retain(|_, m| !m.is_empty());
        }
        let who = self.admitted.get(name)
            .ok_or_else(|| format!("caller {:?} is not in the keyring (or was revoked)", name))?;
        if who.not_after_ms.is_some_and(|t| now_ms > t) {
            return Err(format!("caller {}: the registration expired", name));
        }
        if now_ms.abs_diff(timestamp_ms) > window {
            return Err(format!("caller {}: the timestamp is {} ms from now (the window is {} ms)",
                               name, now_ms.abs_diff(timestamp_ms), window));
        }
        let mine = self.seen.entry(name.to_string()).or_default();
        mine.retain(|_, ts| now_ms.abs_diff(*ts) <= window);
        if mine.contains_key(&nonce) {
            return Err(format!("caller {}: nonce {} was already used (a replay)", name, nonce));
        }
        if mine.len() >= MAX_CALLS_PER_WINDOW {
            return Err(format!("caller {}: {} calls inside the window; wait for it to move", name, MAX_CALLS_PER_WINDOW));
        }
        who.keys.verify(Purpose::Call, signature, message).map_err(|e| format!("caller {}: {}", name, e))?;
        mine.insert(nonce, timestamp_ms);
        Ok(name.to_string())
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

pub fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

pub fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err("hex: odd length or not ASCII".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| format!("hex: bad digit at {}", i)))
        .collect()
}

mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&super::to_hex(b))
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        super::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(name: &str, kind: Kind) -> (Identity, KeyEntry) {
        let (file, entry) = keygen(name, kind, None, Protect::Plaintext).unwrap();
        (Identity::load(&file, None).unwrap(), entry)
    }

    fn call_message(program: &str, function: &str, args: &[String], caller: &str, ts: u64, nonce: u64) -> Vec<u8> {
        CallClaim { program_source: program, function, args, answers: &[], knowledge_path: "", caller,
                    timestamp_ms: ts, nonce, history_path: "" }.message()
    }

    /// root → registrar R → the given entries, all registered.
    fn chain(entries: &[KeyEntry]) -> (Keyring, Identity, Identity) {
        let (root, root_pk) = ident("Root", Kind::Registrar);
        let (r, r_pk) = ident("R", Kind::Registrar);
        let mut ring = Keyring::default();
        ring.add_root(root_pk).unwrap();
        ring.add(register(&root, r_pk, 1, None).unwrap()).unwrap();
        for e in entries {
            ring.add(register(&r, e.clone(), 2, None).unwrap()).unwrap();
        }
        (ring, root, r)
    }

    #[test]
    fn a_seed_always_derives_the_same_identity() {
        let seed = [7u8; 32];
        let (f1, pk1) = identity_from_seed("A", Kind::Agent, &seed, None, Protect::Plaintext).unwrap();
        let (_f2, pk2) = identity_from_seed("A", Kind::Agent, &seed, None, Protect::Plaintext).unwrap();
        assert_eq!(pk1, pk2);
        assert_eq!(f1.seed.as_deref(), Some(to_hex(&seed).as_str()));
        // Known answer: printed seeds must keep restoring the same keys across crate upgrades.
        assert_eq!(fingerprint(&pk1), KAT_SEED7_FINGERPRINT,
                   "the seed derivation changed: printed backups would no longer restore");
        let (_f, other) = identity_from_seed("A", Kind::Agent, &[8u8; 32], None, Protect::Plaintext).unwrap();
        assert_ne!(other.ml_dsa, pk1.ml_dsa);
        assert_ne!(other.ed25519, pk1.ed25519);
    }

    /// The fingerprint `[7u8; 32]` derives (HKDF-SHA-384 → FIPS 204 ξ / Ed25519 seed), pinned
    /// 2026-10-24. Changing it orphans every printed seed: make a new scheme instead.
    /// (Cross-checked once against RustCrypto's independent `ml-dsa` 0.1.1: the same ξ gives the
    /// same ML-DSA-87 public key.)
    const KAT_SEED7_FINGERPRINT: &str =
        "7e075fbbcc01783b0afeec6c22a44aabcb30eb207de37fb05b01e17fc133df3e62774e1cdb991b7d4e8ed09c3c8eca0a";

    #[test]
    fn a_sealed_key_file_opens_only_with_its_passphrase_and_unaltered() {
        let (file, entry) = keygen("Sealed", Kind::Agent, None, Protect::Passphrase("correct horse battery")).unwrap();
        assert!(file.seed.is_none(), "a sealed file holds no plaintext seed");
        let json = serde_json::to_string(&file).unwrap();
        let back: PrivateKeyFile = serde_json::from_str(&json).unwrap();
        let id = Identity::load(&back, Some("correct horse battery")).unwrap();
        assert_eq!(id.public_entry(None), entry);
        assert!(Identity::load(&back, Some("wrong horse battery")).unwrap_err().contains("does not open"));
        assert!(Identity::load(&back, None).unwrap_err().contains("needs its passphrase"));
        let mut renamed = back.clone();
        renamed.name = "Other".into();
        assert!(Identity::load(&renamed, Some("correct horse battery")).unwrap_err().contains("does not open"),
                "the header is bound to the ciphertext");
        let mut weak = back.clone();
        weak.sealed.as_mut().unwrap().m_kib = 1024;
        assert!(Identity::load(&weak, Some("correct horse battery")).unwrap_err().contains("minimum"));
        assert!(keygen("Short", Kind::Agent, None, Protect::Passphrase("short")).unwrap_err().contains("at least 12"));
    }

    #[test]
    fn a_signed_call_verifies_and_nothing_else_does() {
        let (sk, pk) = ident("Emitter450m", Kind::Model);
        let (ring, _, _) = chain(&[pk]);
        let mut v = Verifier::new(ring, vec![]).unwrap();
        let now = now_ms();
        let args = vec!["Paris".to_string()];
        let msg = call_message("program P {}", "solve", &args, "Emitter450m", now, 7);
        let sig = sk.sign(Purpose::Call, &msg).unwrap();
        assert_eq!(v.check("Emitter450m", &sig, now, 7, &msg, now).unwrap(), "Emitter450m");
        assert!(v.check("Emitter450m", &sig, now, 7, &msg, now).unwrap_err().contains("replay"));
        let other = call_message("program P {}", "solve", &["Rome".to_string()], "Emitter450m", now, 8);
        assert!(v.check("Emitter450m", &sig, now, 8, &other, now).unwrap_err().contains("neither"));
        assert!(v.check("Impostor", &sig, now, 9, &msg, now).unwrap_err().contains("not in the keyring"));
        let old = now - DEFAULT_WINDOW_MS - 1;
        let msg_old = call_message("program P {}", "solve", &args, "Emitter450m", old, 10);
        let sig_old = sk.sign(Purpose::Call, &msg_old).unwrap();
        assert!(v.check("Emitter450m", &sig_old, old, 10, &msg_old, now).unwrap_err().contains("window"));
    }

    #[test]
    fn one_good_signature_is_not_enough() {
        let (sk, pk) = ident("A", Kind::Agent);
        let (ring, _, _) = chain(&[pk]);
        let mut v = Verifier::new(ring, vec![]).unwrap();
        let now = now_ms();
        let msg = call_message("p", "f", &[], "A", now, 1);
        let good = sk.sign(Purpose::Call, &msg).unwrap();
        let elsewhere = sk.sign(Purpose::Call, &call_message("p", "g", &[], "A", now, 1)).unwrap();
        let half_ml = HybridSignature { ml_dsa: elsewhere.ml_dsa.clone(), ed25519: good.ed25519.clone() };
        let half_ed = HybridSignature { ml_dsa: good.ml_dsa.clone(), ed25519: elsewhere.ed25519.clone() };
        assert!(v.check("A", &half_ml, now, 1, &msg, now).unwrap_err().contains("ML-DSA-87"));
        assert!(v.check("A", &half_ed, now, 1, &msg, now).unwrap_err().contains("Ed25519"));
        assert!(v.check("A", &good, now, 1, &msg, now).is_ok());
    }

    #[test]
    fn a_key_of_one_caller_cannot_sign_for_another() {
        let (sk_a, pk_a) = ident("A", Kind::Agent);
        let (_sk_b, pk_b) = ident("B", Kind::Agent);
        let (ring, _, _) = chain(&[pk_a, pk_b]);
        let mut v = Verifier::new(ring, vec![]).unwrap();
        let now = now_ms();
        let msg = call_message("p", "f", &[], "B", now, 1);
        let sig = sk_a.sign(Purpose::Call, &msg).unwrap();
        assert!(v.check("B", &sig, now, 1, &msg, now).is_err());
    }

    #[test]
    fn a_signature_for_one_purpose_is_refused_for_another() {
        let (sk, _) = ident("A", Kind::Agent);
        let call = call_message("p", "f", &[], "A", 1, 1);
        assert!(sk.sign(Purpose::Register, &call).unwrap_err().contains("refusing"));
        assert!(sk.sign(Purpose::Revoke, &call).unwrap_err().contains("refusing"));
    }

    #[test]
    fn every_field_of_a_call_is_covered() {
        let base = CallClaim { program_source: "p", function: "f", args: &[], answers: &["1".into()],
                               knowledge_path: "k.jsonl", caller: "A", timestamp_ms: 1, nonce: 1,
                               history_path: "" };
        for other in [
            CallClaim { answers: &["2".into()], ..base },
            CallClaim { knowledge_path: "evil.jsonl", ..base },
            CallClaim { history_path: "h", ..base },
            CallClaim { function: TICK, ..base },
            CallClaim { program_source: "p ", ..base },
            CallClaim { caller: "B", ..base },
            CallClaim { timestamp_ms: 2, ..base },
            CallClaim { nonce: 2, ..base },
        ] {
            assert_ne!(base.message(), other.message());
        }
        let a = CallClaim { args: &["x".into()], answers: &[], ..base };
        let b = CallClaim { args: &[], answers: &["x".into()], ..base };
        assert_ne!(a.message(), b.message(), "an argument cannot move into the answers");
        let c = CallClaim { args: &["ab".into(), "c".into()], ..base };
        let d = CallClaim { args: &["a".into(), "bc".into()], ..base };
        assert_ne!(c.message(), d.message(), "arguments cannot be re-split");
        // a call naming no history signs what it signed before the field existed: no trailer
        assert!(!base.message().windows(8).any(|w| w == b"\0history"));
        assert!(CallClaim { history_path: "h", ..base }.message().starts_with(&base.message()));
    }

    #[test]
    fn the_registration_chain_is_checked_whole() {
        let (_a, pk_a) = ident("Agent1", Kind::Agent);
        let (ring, root, r) = chain(&[pk_a.clone()]);
        assert_eq!(Keyring::from_json(&ring.to_json()).unwrap(), ring, "a JSON round trip");
        let (_x, pk_x) = ident("X", Kind::Agent);
        let mut forged = ring.clone();
        forged.keys[1].ed25519 = pk_x.ed25519.clone();
        assert!(forged.validate().unwrap_err().contains("registration of Agent1"));
        let (_s, pk_r2) = ident("R2", Kind::Registrar);
        let mut ring2 = ring.clone();
        assert!(ring2.add(register(&r, pk_r2.clone(), 3, None).unwrap()).unwrap_err().contains("only a root"));
        assert!(ring2.add(register(&root, pk_r2, 3, None).unwrap()).is_ok());
        let (outsider, _) = ident("Outsider", Kind::Registrar);
        let (_s, pk_b) = ident("B", Kind::User);
        assert!(ring2.add(register(&outsider, pk_b.clone(), 4, None).unwrap()).unwrap_err().contains("not a root"));
        assert!(ring2.add(pk_b).unwrap_err().contains("no registration"));
        let mut swapped = ring.clone();
        swapped.keys.swap(0, 1);
        assert!(swapped.validate().is_err(), "a registrar comes before what it registers");
        let (agent, _) = ident("Agent2", Kind::Agent);
        let (_s, pk_c) = ident("C", Kind::User);
        assert!(register(&agent, pk_c, 5, None).unwrap_err().contains("only a registrar"));
    }

    #[test]
    fn a_registration_can_expire_and_an_expired_registrar_registers_nothing() {
        let (sk, pk) = ident("Temp", Kind::Agent);
        let (root, root_pk) = ident("Root", Kind::Registrar);
        let (r, r_pk) = ident("R", Kind::Registrar);
        let mut ring = Keyring::default();
        ring.add_root(root_pk).unwrap();
        ring.add(register(&root, r_pk, 1, Some(1_000)).unwrap()).unwrap();
        let now = now_ms();
        let (_s, late) = ident("Late", Kind::User);
        assert!(ring.add(register(&r, late, 2_000, None).unwrap()).unwrap_err().contains("expired"));
        ring.add(register(&r, pk, 500, Some(900)).unwrap()).unwrap();
        let mut v = Verifier::new(ring, vec![]).unwrap();
        let msg = call_message("p", "f", &[], "Temp", now, 1);
        let sig = sk.sign(Purpose::Call, &msg).unwrap();
        assert!(v.check("Temp", &sig, now, 1, &msg, now).unwrap_err().contains("expired"));
    }

    #[test]
    fn a_revocation_removes_an_entry_and_everything_its_registrar_made() {
        let (sk_a, pk_a) = ident("A", Kind::Agent);
        let (root, root_pk) = ident("Root", Kind::Registrar);
        let (r, r_pk) = ident("R", Kind::Registrar);
        let (_sk_b, pk_b) = ident("B", Kind::Model);
        let mut ring = Keyring::default();
        ring.add_root(root_pk).unwrap();
        ring.add(register(&root, r_pk.clone(), 1, None).unwrap()).unwrap();
        ring.add(register(&r, pk_a.clone(), 2, None).unwrap()).unwrap();
        ring.add(register(&r, pk_b.clone(), 3, None).unwrap()).unwrap();
        // the registrar revokes A
        let mut one = ring.clone();
        one.add_revocation(revoke(&r, &pk_a, 4, "key left the host").unwrap()).unwrap();
        let names: Vec<String> = one.admitted_names().unwrap().into_iter().map(|(n, _)| n).collect();
        assert!(!names.contains(&"A".to_string()) && names.contains(&"B".to_string()));
        let mut v = Verifier::new(one, vec![]).unwrap();
        let now = now_ms();
        let msg = call_message("p", "f", &[], "A", now, 1);
        assert!(v.check("A", &sk_a.sign(Purpose::Call, &msg).unwrap(), now, 1, &msg, now).unwrap_err().contains("revoked"));
        // a root revokes the registrar: A and B fall with it
        let mut all = ring.clone();
        all.add_revocation(revoke(&root, &r_pk, 5, "registrar retired").unwrap()).unwrap();
        let names: Vec<String> = all.admitted_names().unwrap().into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, vec!["Root".to_string()]);
        // B may not revoke A; an altered revocation does not verify
        let (b_id, _) = ident("B", Kind::Model);
        let mut bad = ring.clone();
        assert!(bad.add_revocation(revoke(&b_id, &pk_a, 6, "").unwrap()).unwrap_err().contains("may not revoke"));
        let mut altered = revoke(&r, &pk_a, 7, "x").unwrap();
        altered.reason = "y".into();
        assert!(bad.add_revocation(altered).unwrap_err().contains("revocation of A"));
    }

    #[test]
    fn pinned_roots_refuse_a_keyring_with_another_root() {
        let (_a, pk_a) = ident("A", Kind::Agent);
        let (ring, _root, _r) = chain(&[pk_a]);
        let pin = fingerprint(&ring.roots[0]);
        assert!(Verifier::new(ring.clone(), vec![pin.clone()]).is_ok());
        let (_o, other_root) = ident("Other", Kind::Registrar);
        let mut swapped = ring.clone();
        swapped.add_root(other_root).unwrap();
        assert!(Verifier::new(swapped.clone(), vec![pin.clone()]).err().unwrap().contains("not one of the pinned"));
        let mut v = Verifier::new(ring, vec![pin]).unwrap();
        assert!(v.replace_keyring(swapped).unwrap_err().contains("pinned"), "a reload cannot add a root either");
    }

    #[test]
    fn a_keyring_refuses_unknown_schemes_duplicates_and_bad_roots() {
        let (_sk, mut pk) = ident("A", Kind::Registrar);
        let dup = serde_json::to_string(&Keyring { roots: vec![pk.clone(), pk.clone()], ..Default::default() }).unwrap();
        assert!(Keyring::from_json(&dup).unwrap_err().contains("twice"));
        let mut user = pk.clone();
        user.kind = Kind::User;
        let bad_root = serde_json::to_string(&Keyring { roots: vec![user], ..Default::default() }).unwrap();
        assert!(Keyring::from_json(&bad_root).unwrap_err().contains("kind registrar"));
        pk.scheme = "fn-dsa-512+ed25519".into();
        let other = serde_json::to_string(&Keyring { roots: vec![pk], ..Default::default() }).unwrap();
        assert!(Keyring::from_json(&other).unwrap_err().contains("ml-dsa-87+ed25519 only"));
    }

    #[test]
    fn a_reload_keeps_the_nonces_seen() {
        let (sk, pk) = ident("A", Kind::Agent);
        let (ring, _, _) = chain(&[pk]);
        let mut v = Verifier::new(ring.clone(), vec![]).unwrap();
        let now = now_ms();
        let msg = call_message("p", "f", &[], "A", now, 1);
        let sig = sk.sign(Purpose::Call, &msg).unwrap();
        assert!(v.check("A", &sig, now, 1, &msg, now).is_ok());
        v.replace_keyring(ring).unwrap();
        assert!(v.check("A", &sig, now, 1, &msg, now).unwrap_err().contains("replay"));
    }

    /// The fingerprint of the encryption keys `[7u8; 32]` derives (HKDF-SHA-384 → FIPS 203 (d, z) /
    /// X25519 secret), pinned 2026-10-24: a printed seed must keep opening what was sealed to it.
    /// (Cross-checked once against independent code: kyber-py's pure-Python FIPS 203 key generation
    /// from the same (d, z), pyca/cryptography's X25519, and HKDF written out by hand.)
    const KAT_SEED7_KEM: &str =
        "3fc17258c68cd3728220be0e211ea8d25bc3329be8baacded09287957a6472acdaafbd25847b442084d3efc88dc3aff2";

    #[test]
    fn encryption_keys_come_from_the_seed_and_carry_its_signature() {
        let seed = [7u8; 32];
        let (_f, pk) = identity_from_seed("A", Kind::Agent, &seed, None, Protect::Plaintext).unwrap();
        let (ek, x) = kem_public(&pk).unwrap();
        assert_eq!(to_hex(&kem_fingerprint(&ek, &x)), KAT_SEED7_KEM,
                   "the encryption key derivation changed: sealed content would no longer open");
        assert_eq!(fingerprint(&pk), KAT_SEED7_FINGERPRINT, "the signing keys did not move");
        let id = Identity::from_seed("A", Kind::Agent, &seed).unwrap();
        assert_eq!(id.kem_fingerprint(), kem_fingerprint(&ek, &x));
        // an entry is a function of its seed, binding included (deterministic ML-DSA)
        assert_eq!(id.public_entry(None), id.public_entry(None));
    }

    #[test]
    fn a_keyring_with_swapped_encryption_keys_is_refused_whole() {
        let (_a, a_pk) = ident("A", Kind::User);
        let (_b, b_pk) = ident("B", Kind::User);
        let (ring, _root, r) = chain(&[a_pk, b_pk.clone()]);
        assert!(ring.recipient("A", now_ms()).is_ok());

        // whoever writes the file gives A's entry B's encryption keys: A's registration still
        // verifies (it covers the signing keys), the binding does not
        let mut forged = ring.clone();
        let i = forged.keys.iter().position(|k| k.name == "A").unwrap();
        forged.keys[i].kem = b_pk.kem.clone();
        let err = Keyring::from_json(&forged.to_json()).unwrap_err();
        assert!(err.contains("not signed by its own keys"), "{err}");

        // an entry from before encryption keys still loads, and refuses to be sealed to
        let mut old = ring.clone();
        old.keys[i].kem = None;
        let old = Keyring::from_json(&old.to_json()).unwrap();
        assert!(old.recipient("A", now_ms()).unwrap_err().contains("no encryption keys"));

        // a revoked entry is no recipient
        let mut revoked = ring.clone();
        revoked.add_revocation(revoke(&r, ring.get("A").unwrap(), 5, "lost").unwrap()).unwrap();
        assert!(revoked.recipient("A", now_ms()).unwrap_err().contains("not admitted"));
        assert!(revoked.recipient("B", now_ms()).is_ok());
    }

    #[test]
    fn only_its_owner_gives_an_old_entry_encryption_keys() {
        let (a, a_pk) = ident("A", Kind::User);
        let (ring, _root, _r) = chain(&[KeyEntry { kem: None, ..a_pk }]);
        assert!(ring.recipient("A", now_ms()).unwrap_err().contains("no encryption keys"));

        let (impostor, _) = ident("A", Kind::User);
        let mut tried = ring.clone();
        assert!(tried.enable_sealing(&impostor).unwrap_err().contains("other signing keys"));
        assert_eq!(tried, ring, "a refused change leaves the keyring as it was");

        let mut ring = ring;
        ring.enable_sealing(&a).unwrap();
        let reloaded = Keyring::from_json(&ring.to_json()).unwrap();
        assert!(reloaded.recipient("A", now_ms()).is_ok(), "the registration still verifies");
    }
}
