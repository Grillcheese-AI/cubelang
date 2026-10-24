# Contracts: who may call what, how often, and proving who is calling

Status 2026-10-24. Every rule below is enforced by the VM and pinned by `tests/contracts.rs`
(VM rules, run-proto, CLI) and `src/auth.rs` (signatures, key files, keyring).

In plain words: a function's tags now mean what they say. A function tagged `@restricted(Emitter450m)`
is closed to everyone else, and over `run-proto` the name `Emitter450m` counts only with that
identity's signatures.

## The tags

| Tag | Enforced as |
|---|---|
| `@external` | callable from outside (the default for an untagged function) |
| `@internal`, `private` | not callable from outside the program; the program's own calls are fine. `private` + `@external` is a compile error |
| `@system` | runtime only: refused from outside **and** from the program's own `CALL` (the constructor is the usual one) |
| `@once` | a second run in the same VM is refused |
| `@restricted(A, B)` | only callers named `A` or `B`, or callers a container grants (`VM::grant` / `revoke`). Anonymous callers are refused |
| `@ratelimit(n, "period")` | at most `n` admitted calls per period, **per caller** (all anonymous callers share one window). A refused call does not count. Periods: `ms`, `s`, `m`, `h`, `d`; a bare number is seconds; zero is a compile error |
| `@cron("interval")` | never callable; the scheduler runs it when due (`cubelang tick`, a `RunRequest` with `tick`, or `VM::tick`) |
| `@hook`, `@before`, `@after` | still not enforced: a strict compile error, a warning under `--loose` |

Order of the checks on entry: `@system`, then `@internal`/`private`, then `@cron`, then `@restricted`,
then `@ratelimit` (counted only when everything before it admitted the call).

The contract survives `.cubebin`: version 4 appends a metadata trailer (used modules, override flags,
the contract, the types), so `use`/`override` programs now serialize too and behave the same after a
reload. Versions 1-3 still load.

## Cryptography (the CNSA 2.0 suite)

| Job | Algorithm | Implementation |
|---|---|---|
| Signatures | **ML-DSA-87** (FIPS 204) **and** Ed25519, both required | libcrux-ml-dsa (Cryspen; formally verified arithmetic, NTT and serialization; ACVP + Wycheproof tested), ed25519-dalek (strict verification, weak keys refused) |
| Digests in signed messages | SHA-384 | RustCrypto sha2 |
| Key derivation | HKDF-SHA-384 | RustCrypto hkdf |
| Key files at rest | Argon2id (64 MiB, 3 passes, 4 lanes) + AES-256-GCM | RustCrypto argon2, aes-gcm |
| Sealed content (2026-10-24, `docs/SEAL.md`) | **ML-KEM-1024** (FIPS 203) **and** X25519, both; AES-256-GCM | libcrux-ml-kem (Cryspen, formally verified), x25519-dalek |
| A remote run-proto (2026-10-24, below) | TLS 1.3 only: **SecP384r1MLKEM1024** (P-384 ECDH **and** ML-KEM-1024), TLS_AES_256_GCM_SHA384, ECDSA P-384 raw keys pinned both ways | rustls on AWS-LC (aws-lc-rs) |

Every identity is **one 32-byte seed**. HKDF-SHA-384 derives FIPS 204's key-generation seed ξ (so the
seed-to-key mapping is the standard's, not ours) and the Ed25519 seed. A known-answer test pins the
derivation, and it was cross-checked once against RustCrypto's independent `ml-dsa`: the same ξ gives
the same public key. The seed fits a small QR code, so a root key can be printed and kept offline.
The same seed also gives the identity's encryption keys (ML-KEM-1024 and X25519; `docs/SEAL.md`),
which its entry carries under `kem`, signed by its own signing keys.

Each signature names its purpose (call, registration, revocation, the binding of encryption keys,
sealed content): ML-DSA takes it as its context
string, Ed25519 sees it as the message's leading tag, so a signature for one purpose never passes as
another. Every entry and key file names its scheme (`ml-dsa-87+ed25519`): a later scheme is a new
entry, never a silent reinterpretation.

Secrets in memory: the seed is wiped when dropped; the 4,896-byte ML-DSA signing key is regenerated
for each signature and wiped right after; `Debug` on a key prints only its name and kind.

A call signature covers the program's SHA-384, the function (or the tick marker), the arguments, the
ASK answers, the knowledge file, the caller's name, a timestamp and a nonce. A timestamp outside the
window (2 minutes either way) or a nonce seen inside it is refused (no replays); a caller gets at most
10,000 calls per window. A request's `branch` and `budget` are host settings and are not covered.

## Key files

`keygen` seals the seed with a passphrase of at least 12 characters, read from
`CUBELANG_KEY_PASSPHRASE` or `--passphrase-file` (never from the command line, where it would sit in
the process list and the shell history). The file's header (name, kind, scheme, parameters) is the
cipher's associated data, so no field can be swapped, and a file edited down to weaker parameters is
refused. A plaintext key file exists only with `--plaintext`, for throwaway test identities.

## The keyring

A JSON file the host loads (never one a request supplies):

- `roots`: trust anchors, kind `registrar`, placed by the host's owner. **Pin them**:
  `run-proto --pin-root FINGERPRINT` (repeatable; `keyring check` prints the fingerprints). With pins,
  a keyring whose roots are not all pinned is refused, at start and on reload. Without pins the process
  warns that whoever can write the file can add a root;
- `keys`: `user`, `agent`, `model` and `registrar` entries, each registered (both schemes) by a root
  or an earlier registrar. Only a root registers a registrar. A registration may end
  (`--days N`), and an expired registrar registers nothing;
- `revocations`: signed by a root or by the entry's own registrar, naming the entry and the
  fingerprint of its keys. A revoked registrar takes everything it registered with it.

The whole chain is verified at load, encryption keys included: an entry's `kem` must carry the
entry's own signature, so whoever can write the file cannot give one entry another's encryption keys
(a registration covers the signing keys only). An entry made before 2026-10-24 has no `kem`: it still
signs and verifies, and refuses to be sealed to until its owner runs `keyring enable-sealing`. A keyring that does not verify is refused whole: `run-proto`
does not start (fail closed), and a reload that does not verify keeps the previous keyring.
`run-proto` reloads the file when its modification time changes, keeping the nonces it has seen.

## run-proto

- **No options:** as before. A local pipe serves its parent, and a request that claims a caller is
  refused (there is no keyring to check it against).
- **`--keyring ring.json` and/or `--contracts DIR`** lock the process. A request signed by a `model` or
  an `agent` may run any program. Anyone else (anonymous, a `user`):
  - may run only a **deployed contract**: a `.cube` in `DIR`, matched by the SHA-384 of its source;
  - may name a knowledge file only inside `DIR`;
  - runs under the host's caps (1,000,000 jumps, 5,000,000 ops, 1,000 queries, 5 s), lower if it asks.
- A frame over 16 MiB stops the process before anything is allocated.
- The resident process keeps the `@ratelimit` windows and the `@cron` clock across requests (each
  request still runs on a fresh VM). A `tick` request runs the due `@cron` functions and reports them
  in `RunResult.crons`.

## Over a network

*In plain words: when the brain and the VM sit on different machines, the line between them is
locked so that nobody listening can read it, today or with a quantum computer later, and each end
only talks to the one other end it was told to trust.*

`run-proto` itself never opens a port unless told to, and then only one way (`src/tls.rs`):

- **TLS 1.3 only**, one cipher suite (TLS_AES_256_GCM_SHA384), one key exchange:
  **SecP384r1MLKEM1024** (draft-ietf-tls-ecdhe-mlkem, codepoint 0x11ED): P-384 ECDH and ML-KEM-1024
  together, so reading a recording takes breaking both. ML-KEM-1024 is the CNSA 2.0 level; the
  hybrids rustls ships pair ML-KEM-768, a level lower, so this one is built from rustls's own P-384
  and ML-KEM-1024 on AWS-LC. A client that offers anything else finds nothing in common.
- **Both sides prove who they are**, with raw public keys (RFC 7250): each side's TLS key is ECDSA
  P-384, derived from its CubeLang identity seed (no TLS key file to keep), and the other side pins its
  SHA-384 fingerprint (`cubelang tls-pin --key KEY.json`). No certificate authority is involved, so
  none can be tricked into vouching for an impostor. The handshake signature is classical: it has to
  be forged during the handshake to matter, and every call inside is signed with ML-DSA-87 + Ed25519
  anyway.
- **No resumption, no 0-RTT**: each connection is a full handshake with a fresh key exchange.
- The host: `run-proto --listen ADDR --tls-key HOST.key.json --client-pin PIN...` (at least one pin;
  with `--keyring`/`--contracts`/`--history-key` as on a pipe). Connections are read on their own
  threads (at most 64; 10 s to finish a handshake, 10 min idle); requests run one at a time against the
  same host state as the pipe. A connection that sends a bad frame is closed; the host serves on.
- The client: `run-proto --remote ADDR --tls-key KEY.json --server-pin PIN` relays stdin to the host and
  the answers back, so a program that spawns `run-proto` (CubbyLLM) reaches a remote host by changing
  its arguments, with no TLS stack of its own (Python's OpenSSL 3.0 cannot offer this group).

Checked against an independent implementation: OpenSSL 3.5's `s_client` negotiates SecP384r1MLKEM1024
and AES-256-GCM with the host, raw keys both ways, and is admitted when its key is pinned.

Cost (`cargo run --release --example tls_bench`, loopback, the development machine): a full handshake
takes 8.7 ms; a 1 KiB round trip on an open connection, 36 µs. The relay holds one connection for all
of a session's requests, so the handshake is paid once.

## CLI

```
cubelang keygen NAME --kind user|agent|model|registrar --out key.json [--model-sha256 HEX] [--passphrase-file P | --plaintext]
cubelang keyring add-root RING ENTRY.json            # prints the fingerprint to pin
cubelang keyring register RING --by REGISTRAR.key.json [--days N] ENTRY.json
cubelang keyring revoke RING --by KEY.json NAME [--reason TEXT]
cubelang keyring check RING                          # the chain, fingerprints, REVOKED marks
cubelang keyring enable-sealing RING --key KEY.json  # give an entry made before 2026-10-24 its encryption keys
cubelang sign --key key.json --program FILE (--fn NAME | --tick) [--arg V]... [--answer J]... [--knowledge PATH]
cubelang run-proto --keyring RING --pin-root FINGERPRINT --contracts DIR
cubelang run FILE --as NAME                          # a host-asserted caller for local testing (no signature)
cubelang tick FILE [--every 1s] [--ticks N]
cubelang seal --to NAME --keyring RING [--from KEY.json] [--aad TEXT] IN OUT      # docs/SEAL.md
cubelang open --key KEY.json [--keyring RING] [--signer NAME] [--aad TEXT] IN OUT|-
cubelang run-proto ... --history-key KEY.json        # every history it opens is sealed to KEY
cubelang tls-pin --key KEY.json                      # the pin of this identity's TLS key
cubelang run-proto --listen ADDR --tls-key HOST.key.json --client-pin PIN [...]
cubelang run-proto --remote ADDR --tls-key KEY.json --server-pin PIN
```

`keygen` never writes over an existing file (owner-only on Unix) and prints only the public entry.
Key files never go in a repository, a `.cube` file or a request.

## Not yet

- The browser leg of a public demo is the demo's web server, not `run-proto`: browsers offer
  X25519MLKEM768, not this profile. Containers (grants declared in source), sealed content and a
  remote `run-proto` landed 2026-10-24.
- For certification (FIPS 140-3), the primitives move to a validated module (aws-lc-rs); the scheme
  tags make that a swap, not a format change.
