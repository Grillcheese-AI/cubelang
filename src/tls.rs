//! TLS 1.3 for a remote run-proto (2026-10-24): one profile, no options, both ends ours.
//!
//! - **TLS 1.3 only**, one cipher suite: TLS_AES_256_GCM_SHA384.
//! - **One key exchange**: SecP384r1MLKEM1024 (draft-ietf-tls-ecdhe-mlkem, codepoint 0x11ED): ECDH on
//!   P-384 AND ML-KEM-1024, the secret both concatenated, so reading the traffic takes breaking both;
//!   ML-KEM-1024 is the CNSA 2.0 key establishment (the hybrids rustls ships pair ML-KEM-768, a level
//!   lower). Built here from rustls's own P-384 and ML-KEM-1024 groups on AWS-LC.
//! - **Both sides authenticate**, with raw public keys (RFC 7250), not certificates: each side's TLS
//!   key is ECDSA P-384, derived from its CubeLang identity seed (`auth`), and the other side pins its
//!   SHA-384 fingerprint. No certificate authority stands between the two, so none can be fooled.
//! - **No resumption, no 0-RTT**: every connection is a full handshake with a fresh key exchange.
//!
//! The handshake signature is classical (P-384): it has to be broken during the handshake to matter,
//! not later from a recording, and each call carried inside is signed with ML-DSA-87 + Ed25519 anyway
//! (`auth::CallClaim`). The traffic is what a recording would be kept for, and that is post-quantum.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{AlwaysResolvesClientRawPublicKeys, Resumption};
use rustls::crypto::aws_lc_rs as aws;
use rustls::crypto::{
    verify_tls13_signature_with_raw_key, ActiveKeyExchange, CompletedKeyExchange, CryptoProvider, SharedSecret,
    SupportedKxGroup, WebPkiSupportedAlgorithms,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivateSec1KeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{AlwaysResolvesServerRawPublicKeys, NoServerSessionStorage};
use rustls::sign::CertifiedKey;
use rustls::{
    CertificateError, ClientConfig, DigitallySignedStruct, DistinguishedName, Error, NamedGroup, PeerMisbehaved,
    ProtocolVersion, ServerConfig, SignatureScheme,
};
use sha2::{Digest, Sha384};

use crate::auth::{to_hex, Identity};

/// SecP384r1MLKEM1024's TLS codepoint.
pub const SECP384R1MLKEM1024: NamedGroup = NamedGroup::Unknown(0x11ed);
/// The one handshake signature scheme either side accepts.
pub const SIGNATURE: SignatureScheme = SignatureScheme::ECDSA_NISTP384_SHA384;

const P384_SHARE: usize = 97;
const MLKEM1024_EK: usize = 1568;
const MLKEM1024_CT: usize = 1568;

/// P-384 ECDH and ML-KEM-1024, both: the shares and the secret are the classical part then the
/// post-quantum part (the draft's order for its NIST-curve hybrids).
#[derive(Debug)]
struct SecP384r1MlKem1024;

static HYBRID: SecP384r1MlKem1024 = SecP384r1MlKem1024;

fn bad_share() -> Error {
    Error::PeerMisbehaved(PeerMisbehaved::InvalidKeyShare)
}

impl SupportedKxGroup for SecP384r1MlKem1024 {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let ec = aws::kx_group::SECP384R1.start()?;
        let pq = aws::kx_group::MLKEM1024.start()?;
        let share = [ec.pub_key(), pq.pub_key()].concat();
        Ok(Box::new(ActiveHybrid { ec, pq, share }))
    }

    /// The server's half: answer the client's P-384 share, encapsulate to its ML-KEM key.
    fn start_and_complete(&self, client_share: &[u8]) -> Result<CompletedKeyExchange, Error> {
        if client_share.len() != P384_SHARE + MLKEM1024_EK {
            return Err(bad_share());
        }
        let (ec_share, pq_share) = client_share.split_at(P384_SHARE);
        let ec = aws::kx_group::SECP384R1.start_and_complete(ec_share)?;
        let pq = aws::kx_group::MLKEM1024.start_and_complete(pq_share)?;
        Ok(CompletedKeyExchange {
            group: SECP384R1MLKEM1024,
            pub_key: [ec.pub_key.as_slice(), pq.pub_key.as_slice()].concat(),
            secret: SharedSecret::from([ec.secret.secret_bytes(), pq.secret.secret_bytes()].concat()),
        })
    }

    fn name(&self) -> NamedGroup {
        SECP384R1MLKEM1024
    }

    fn usable_for_version(&self, version: ProtocolVersion) -> bool {
        version == ProtocolVersion::TLSv1_3
    }
}

struct ActiveHybrid {
    ec: Box<dyn ActiveKeyExchange>,
    pq: Box<dyn ActiveKeyExchange>,
    share: Vec<u8>,
}

impl ActiveKeyExchange for ActiveHybrid {
    /// The client's half: finish ECDH with the server's P-384 share, decapsulate its ciphertext.
    fn complete(self: Box<Self>, server_share: &[u8]) -> Result<SharedSecret, Error> {
        if server_share.len() != P384_SHARE + MLKEM1024_CT {
            return Err(bad_share());
        }
        let (ec_share, pq_ct) = server_share.split_at(P384_SHARE);
        let ec = self.ec.complete(ec_share)?;
        let pq = self.pq.complete(pq_ct)?;
        Ok(SharedSecret::from([ec.secret_bytes(), pq.secret_bytes()].concat()))
    }

    fn pub_key(&self) -> &[u8] {
        &self.share
    }

    fn group(&self) -> NamedGroup {
        SECP384R1MLKEM1024
    }
}

/// AWS-LC, held to the profile: TLS_AES_256_GCM_SHA384 and SecP384r1MLKEM1024, nothing else.
pub fn provider() -> Arc<CryptoProvider> {
    let mut p = aws::default_provider();
    p.cipher_suites = vec![aws::cipher_suite::TLS13_AES_256_GCM_SHA384];
    p.kx_groups = vec![&HYBRID];
    Arc::new(p)
}

fn sha384(b: &[u8]) -> [u8; 48] {
    Sha384::digest(b).into()
}

/// An identity's TLS key, ready to sign handshakes, and the SHA-384 of its public key (its pin).
fn tls_key(id: &Identity, provider: &CryptoProvider) -> Result<(Arc<CertifiedKey>, [u8; 48]), String> {
    let der = id.tls_key_sec1();
    let key = provider.key_provider
        .load_private_key(PrivateKeyDer::Sec1(PrivateSec1KeyDer::from(der.to_vec())))
        .map_err(|e| format!("{}'s TLS key: {}", id.name, e))?;
    let spki = key.public_key().ok_or_else(|| format!("{}'s TLS key has no public key", id.name))?.as_ref().to_vec();
    let pin = sha384(&spki);
    Ok((Arc::new(CertifiedKey::new(vec![CertificateDer::from(spki)], key)), pin))
}

/// The pin of an identity's TLS key (hex): what the other side passes as `--server-pin` or
/// `--client-pin`.
pub fn pin(id: &Identity) -> Result<String, String> {
    Ok(to_hex(&tls_key(id, &aws::default_provider())?.1))
}

/// A pin given as hex.
pub fn parse_pin(hex: &str) -> Result<[u8; 48], String> {
    let wrong = || format!("a TLS pin is 96 hex digits (SHA-384; `cubelang tls-pin` prints one); got {:?}", hex);
    let hex = hex.trim();
    if hex.len() != 96 {
        return Err(wrong());
    }
    crate::auth::from_hex(hex).map_err(|_| wrong())?.try_into().map_err(|_| wrong())
}

/// The peers one side accepts: a raw P-384 public key whose SHA-384 is pinned, and a handshake
/// signed by it with ECDSA P-384 SHA-384. Serves as the client's server verifier and the server's
/// client verifier.
#[derive(Debug)]
struct Pinned {
    pins: Vec<[u8; 48]>,
    algs: WebPkiSupportedAlgorithms,
}

impl Pinned {
    fn check_key(&self, key: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>]) -> Result<(), Error> {
        if !intermediates.is_empty() || !self.pins.contains(&sha384(key.as_ref())) {
            return Err(Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure));
        }
        Ok(())
    }

    fn check_signature(&self, message: &[u8], key: &CertificateDer<'_>, dss: &DigitallySignedStruct)
                       -> Result<HandshakeSignatureValid, Error> {
        if dss.scheme != SIGNATURE {
            return Err(Error::PeerMisbehaved(PeerMisbehaved::SignedHandshakeWithUnadvertisedSigScheme));
        }
        verify_tls13_signature_with_raw_key(message, &SubjectPublicKeyInfoDer::from(key.as_ref()), dss, &self.algs)
    }
}

fn tls12_refused() -> Error {
    Error::General("TLS 1.2 is refused".into())
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>],
                          _server_name: &ServerName<'_>, _ocsp: &[u8], _now: UnixTime) -> Result<ServerCertVerified, Error> {
        self.check_key(end_entity, intermediates)?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &DigitallySignedStruct)
                              -> Result<HandshakeSignatureValid, Error> {
        Err(tls12_refused())
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct)
                              -> Result<HandshakeSignatureValid, Error> {
        self.check_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SIGNATURE]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

impl ClientCertVerifier for Pinned {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }

    fn verify_client_cert(&self, end_entity: &CertificateDer<'_>, intermediates: &[CertificateDer<'_>], _now: UnixTime)
                          -> Result<ClientCertVerified, Error> {
        self.check_key(end_entity, intermediates)?;
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &DigitallySignedStruct)
                              -> Result<HandshakeSignatureValid, Error> {
        Err(tls12_refused())
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct)
                              -> Result<HandshakeSignatureValid, Error> {
        self.check_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SIGNATURE]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

/// The listening side: `id`'s key, and only the clients whose keys are pinned in `client_pins`.
pub fn server_config(id: &Identity, client_pins: &[[u8; 48]]) -> Result<Arc<ServerConfig>, String> {
    if client_pins.is_empty() {
        return Err("a listening run-proto needs at least one --client-pin: nobody else connects".into());
    }
    let provider = provider();
    let (key, _) = tls_key(id, &provider)?;
    let verifier = Arc::new(Pinned { pins: client_pins.to_vec(), algs: provider.signature_verification_algorithms });
    let mut cfg = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("TLS: {}", e))?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(Arc::new(AlwaysResolvesServerRawPublicKeys::new(key)));
    cfg.session_storage = Arc::new(NoServerSessionStorage {});
    cfg.send_tls13_tickets = 0;
    cfg.max_early_data_size = 0;
    Ok(Arc::new(cfg))
}

/// The connecting side: `id`'s key, and only the server whose key is `server_pin`.
pub fn client_config(id: &Identity, server_pin: [u8; 48]) -> Result<Arc<ClientConfig>, String> {
    client_config_with(id, server_pin, provider())
}

/// `client_config` on another provider (tests use it to offer what the profile refuses).
pub fn client_config_with(id: &Identity, server_pin: [u8; 48], provider: Arc<CryptoProvider>)
                          -> Result<Arc<ClientConfig>, String> {
    let (key, _) = tls_key(id, &provider)?;
    let verifier = Arc::new(Pinned { pins: vec![server_pin], algs: provider.signature_verification_algorithms });
    let mut cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("TLS: {}", e))?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_cert_resolver(Arc::new(AlwaysResolvesClientRawPublicKeys::new(key)));
    cfg.resumption = Resumption::disabled();
    cfg.enable_early_data = false;
    Ok(Arc::new(cfg))
}

/// The name a client sends (SNI). The server's key is pinned, so no name is checked.
pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("run-proto.cubelang").expect("a valid DNS name")
}

/// A connected peer's pin (hex), once the handshake is done.
pub fn peer_pin(certs: Option<&[CertificateDer<'_>]>) -> Option<String> {
    certs.and_then(|c| c.first()).map(|k| to_hex(&sha384(k.as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Kind;

    /// The TLS pin `[7u8; 32]` derives (HKDF-SHA-384 → a P-384 scalar), pinned 2026-10-24: a host's
    /// pin, written into its clients' configuration, must survive an upgrade. (Cross-checked once:
    /// the same scalar from HKDF written out by hand, its key and SubjectPublicKeyInfo from pyca's
    /// `cryptography`, give the same pin.)
    const KAT_SEED7_TLS_PIN: &str =
        "1ad964b80639a605aad93d35e7482bcf8cb021ee4a9585ddf903118d456f93c1f874b6b5f62eece2bf260386d429042b";

    #[test]
    fn the_tls_key_comes_from_the_seed() {
        let id = Identity::from_seed("A", Kind::Agent, &[7u8; 32]).unwrap();
        assert_eq!(pin(&id).unwrap(), KAT_SEED7_TLS_PIN, "the TLS key derivation changed: every pin would break");
        let other = Identity::from_seed("A", Kind::Agent, &[8u8; 32]).unwrap();
        assert_ne!(pin(&other).unwrap(), KAT_SEED7_TLS_PIN);
        assert!(parse_pin(&pin(&id).unwrap()).is_ok());
    }
}
