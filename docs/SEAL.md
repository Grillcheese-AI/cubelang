# Sealed content

*In plain words: a file, a message or a conversation can be locked so only one person's or agent's
key opens it, and anyone can check who wrote it. Copying the files gets an attacker nothing, today
or once quantum computers arrive.*

`src/seal.rs` (2026-10-24). What it brings to CubbyLLM: a conversation history, a knowledge file or
an agent's message can be stored or sent where others can read the bytes (a cloud disk, a backup, a
shared folder) and stay private, with the author proven. Most model hosts keep conversations as
plain text behind access rules; here the text itself is unreadable without the owner's key.

## The scheme

| Job | Algorithm | Implementation |
|---|---|---|
| Key exchange | **ML-KEM-1024** (FIPS 203, the CNSA 2.0 choice) **and** X25519, both | libcrux-ml-kem (Cryspen, formally verified; the same family as the ML-DSA in use), x25519-dalek |
| Key derivation | HKDF-SHA-384 over both shared secrets and everything public that made them | RustCrypto hkdf |
| Encryption | AES-256-GCM, one key per sealing | RustCrypto aes-gcm |
| Sender's signature | ML-DSA-87 + Ed25519 (`Purpose::Seal`), inside the ciphertext | as for calls |

Reading sealed content takes breaking ML-KEM **and** X25519. Each sealing makes a fresh encapsulation
and a fresh X25519 key, so two sealings of the same text share nothing.

**Encryption keys come from the identity's one seed** (HKDF labels `ml-kem-1024 d||z` and
`x25519 secret`), so the printed seed restores them too; the signing keys a seed gives are unchanged.
A keyring entry carries them under `kem`, signed by the entry's own signing keys
(`Purpose::Kem`, deterministic ML-DSA, so an entry is a function of its seed). A keyring in which
one entry's encryption keys were swapped is refused whole. An entry made before 2026-10-24 has none;
its owner adds them with `cubelang keyring enable-sealing RING --key KEY.json` (the registration
stays as it is: it covers the signing keys only). A known-answer test pins the derivation; it
was cross-checked once against independent code (kyber-py's FIPS 203 key generation, pyca's X25519,
HKDF written out by hand).

## The format

```text
[8]    "CUBESEAL"        [1] version 1
[2]    recipient name length, the name
[48]   recipient key fingerprint: SHA-384 over both public encryption keys
[1568] ML-KEM-1024 ciphertext
[32]   ephemeral X25519 public key
[..]   AES-256-GCM ciphertext + 16-byte tag, of:
         [1] flags (bit 0: signed)
         signed: [2] sender name length, the name, [4627] ML-DSA-87 sig, [64] Ed25519 sig
         the body
```

Key and nonce: HKDF-SHA-384(salt `cubelang/seal/v1`, ML-KEM secret ‖ X25519 secret,
info `cubelang/seal/v1 content key\0` ‖ SHA-384(header)). The cipher's associated data is the header
followed by the caller's associated data (length-prefixed): no header byte can change, and content
sealed for one place (a file name, a conversation) does not open in another. The sender's signature
covers the sender, the recipient's name and key fingerprint, and SHA-384 of the associated data and
of the body, so a recipient cannot forward a signed message to someone else as though it had been
written to them.

The format was checked with an independent opener written from this description alone (Python:
kyber-py, dilithium-py, pyca/cryptography): it opens a file the CLI sealed and verifies both
signatures.

**Cost** (`cargo run --release --example seal_bench`, the development machine): sealing 1 KiB with a
signature takes 0.8 ms, opening and checking it 0.3 ms. The envelope adds 1,676 bytes plus the
recipient's name; a signature adds 4,693 bytes plus the sender's name.

## Use

```
cubelang seal --to NAME --keyring RING.json [--from KEY.json] [--aad TEXT] IN OUT
cubelang seal --to ENTRY.json [--from KEY.json] [--aad TEXT] IN OUT
cubelang open --key KEY.json [--keyring RING.json [--pin-root FP]...] [--signer NAME] [--aad TEXT] IN OUT|-
```

- `--to NAME --keyring`: a name the keyring admits now (registered, not revoked, not expired). With an
  entry file instead, `seal` says the recipient is the file's word: check its fingerprint with its
  owner.
- `open` reports the sender as **unsigned**, **signed but not checked** (no keyring), or **signed and
  checked** against the keyring. A signature that fails is an error, never "unsigned". `--signer NAME`
  insists on one checked sender.
- Neither command writes over an existing file without `--force`; an opened file is owner-only on Unix.

In code: `seal::seal`, `seal::open` (with an optional `Verifier`), `seal::open_from` (one known
sender), `seal::recipient_of`.

## Sealed history

`cubelang history seal DIR --key KEY.json` seals a history directory in place; from then on every
`history` command takes `--key KEY.json`, `run --history DIR --history-key KEY.json` reads it, and
`run-proto --history-key KEY.json` seals every history it opens (a plain one is refused). Details in
`docs/STORE.md`.

## What it does not do

- **The header is visible**: the recipient's name and key fingerprint, and the size. The sender is not.
- **A leaked seed opens old copies.** The key for a sealed file is derived from the recipient's seed
  and the file, so anyone who holds both a copy and the seed can open it. Deleting a file (forget,
  merge, gc) removes it; copies taken before (backups, sync folders) stay readable by that seed.
  Retire a seed you think leaked: make a new identity, re-seal, revoke the old entry.
- **Rollback across restarts.** A sealed history carries a save counter; `run-proto` refuses an older
  copy put back in place for as long as it runs. A host that must catch it across restarts keeps the
  highest generation it served itself.
- **Messages are documents, not a session**: no nonce or timestamp; a copy of a sealed message is the
  same message. A call's freshness is the signed call's job (`docs/CONTRACTS.md`).
