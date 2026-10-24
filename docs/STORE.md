# The layered store

2026-10-24. The knowledge store (`QUERY`) and the conversation history (`use history;`) are layered
the way a log-structured database is: immutable, shared **segments** below a small mutable **top
layer**.

*In plain words: the facts and the past conversation are written once into a compact, read-only
index that every request shares, so asking a question or exploring a "what if" no longer means
copying everything Cubby knows. Old conversation is searchable by word, prefix or typo in about a
millisecond, and a turn the user asks to forget is gone at once.*

## What changed

| | before | after |
|---|---|---|
| knowledge per request | a full copy of the store (a hash map) | a pointer to the shared segment, plus the request's own changes |
| a branch ("X did not happen") | the copy, then a scan of every key for the excluded key's aliases | a mask in the branch's top layer; the aliases come from the key's record |
| knowledge file | `.jsonl`, parsed by every process | `.jsonl`, or a prebuilt `.cubeseg` opened without parsing |
| history | none in the VM | `use history;`: `search`, `search_prefix`, `search_fuzzy`, `turn`, `recent` |

## Segments

`src/vm/segment.rs`. A segment is a finite-state transducer (BurntSushi's `fst`) from each key to an
offset in a value blob, in one file:

```text
"CUBESEG\0" | version | kind (1 knowledge, 2 history) | fst length | blob length | fst | blob | BLAKE3
```

The checksum catches a damaged file before anything reads it. It is not a signature, and the file is
not encrypted: sealing segments at rest is task #118.

A process reads a segment once and shares it through an `Arc`; forks clone the pointer. (Memory
mapping the file instead, so separate processes share pages and a large file loads lazily, is a
one-function change if it is wanted.)

## Knowledge

`src/vm/knowledge.rs`. A key's facts are its facts in every segment, oldest first (a segment that
*replaced* the key starts the list over), then the top layer's; a key the top layer masks shows only
the top layer's. Keys with identical facts share one record, which lists them all, so an alias costs
a key in the index rather than a copy of the facts, and `remove` finds a key's aliases without a scan.

- `insert`, `alias`, `remove`, `lookup`, `keys`, `load_jsonl`: as before (lookups answer exactly as
  the map did; pinned by the store's tests).
- `freeze`: the top layer becomes a new segment. `compact`: every layer into one.
- `KnowledgeStore::open(path)` / `to_segment()` / `KnowledgeSegment::write(path)`.
- `cubelang knowledge build FACTS.jsonl -o OUT.cubeseg`, `info`, `lookup`, `keys PREFIX`.
- `run-proto`: `knowledge_path` takes a `.cubeseg` or a `.jsonl` (parsed once and frozen); either way
  the resident process caches it by mtime and every request shares it.

## History

`src/vm/history.rs`. Turns (`id`, `at_ms`, `speaker`, `text`) are appended to the top layer and frozen
into a segment at the end of a turn. A history segment holds its turns, an id-to-record table, and a
term index: each normalized word to the turns holding it. Searches walk the index:

- **exact**: the word, normalized as knowledge keys are (case, punctuation);
- **prefix**: words starting with it;
- **fuzzy**: within edit distance 0 (words up to 3 letters), 1 (up to 7) or 2 (longer) — Levenshtein,
  so a swap of two letters counts as two edits.

Every word of the query must match (AND); results are newest first.

Merging works like a binary counter: while the newest segment is at least half the size of the one
before it, the two are merged. A conversation of n turns sits in about log2(n) segments, and each turn
is rewritten about log2(n) times over its life.

**Forgetting.** `forget(id)` hides a turn from every search and read at once; the next `merge_all`
drops it from the segments, and `gc_dir` removes the files that still held it. Until then its text is
still on disk: forgetting is complete after `history merge` and `history gc`.

**On disk.** A history directory holds `MANIFEST.json` (the segment list and the next id) and the
segment files. The manifest is replaced atomically, so a reader sees the old history or the new one.
One writer at a time.

- `cubelang history append DIR [--speaker S] [--at MS] TEXT` (append, freeze, merge, save: one call
  per turn), `search DIR WORDS [--prefix|--fuzzy] [--limit N]`, `recent DIR [N]`, `forget DIR ID`,
  `merge DIR`, `gc DIR`, `info DIR`.
- `cubelang run --history DIR`.
- **Sealed** (2026-10-24, `docs/SEAL.md`): `history seal DIR --key KEY.json` seals a directory in place
  (the plain files are removed; copies made before are not reached). A sealed directory holds
  `MANIFEST.cubeseal` and `seg-<random>.cubeseal` files, each sealed to the key's identity and signed
  by it, each bound to its file name; the manifest names every segment by file and BLAKE3 of its
  content, and counts saves (`generation`). So the files say nothing to whoever copies them, a file
  cannot be swapped, renamed or planted (sealing *to* the host is open to anyone; signing as it is not),
  and a sealed and a plain manifest side by side are refused. Every `history` command then takes
  `--key KEY.json`; `run --history DIR --history-key KEY.json`; `run-proto --history-key KEY.json`
  seals every history it opens, refuses a plain one, and refuses an older copy of one it served.
  Cost (`examples/seal_bench.rs`, release, the development machine): a CLI append (open, append,
  save) takes 13.2 ms sealed against 8.9 ms plain (file writes dominate either way); opening three
  sealed segments takes 4.0 ms against 1.2 ms.
- `run-proto`: `RunRequest.history_path` (field 10), cached like knowledge (by the manifest's mtime).
  A signed call covers it (`CallClaim.history_path`; a call naming no history signs the same bytes as
  before). In locked mode an unsigned or user request may not name a history at all: a history is
  someone's conversation.

**In a program.**

```cubelang
use history;
program Recall implements ISolve {
    public function solve(words: str): number {
        let hits = search_fuzzy(words, 5);   # the 5 newest turns holding every word
        return hits;                         # [{turn, at, speaker, text, matched}, ...]
    }
}
```

The module is sealed (a program cannot replace it) and read-only. Each search counts against the
run's query budget and shows in the branch report as `history:<words>`.

## Measured

`cargo run --release --example store_bench -- 1000000 100000` on the development machine (Windows,
release build). Synthetic data shaped like the real thing: multi-word titles, sentence-length facts
with a source, a quarter as many aliases; turns of 6 to 24 words from a 5,000-word Zipf vocabulary.

**Knowledge: 1,000,000 facts, 250,000 aliases (1,239,732 keys)**

| | before (one map, copied per request) | after (shared segment + top layer) |
|---|---|---|
| memory | ≈436 MiB per copy | 163 MiB, once per process |
| a request's copy (fork) | 1230 ms | 0.05 µs |
| a branch (fork, exclude 1 key, assume 1 fact) | 1054 ms | 3.2 µs |
| exact lookup | 1.25 µs | 1.42 µs |
| build | load 4.8 s | + freeze 7.5 s, once (or offline with `cubelang knowledge build`) |

**History: 100,000 turns (13 MiB of text) in 9 segments (17 MiB)**, appended, frozen and merged one
turn at a time: 353 µs a turn.

| search | hits | before (read every turn) | after, every hit | after, the newest 20 |
|---|---|---|---|---|
| a rare word | 49 | 366 ms | 0.09 ms | 0.02 ms |
| a common word | 34,333 | 369 ms | 19.8 ms | 13.3 ms |
| two words | 127 | 390 ms | 5.8 ms | 3.0 ms |
| a prefix (3 letters) | 73,525 | 399 ms | 122 ms | 51 ms |
| a typo | 294 | 1190 ms | 1.9 ms | 1.0 ms |

The benchmark checks that the layered store answers every lookup as the map did, and that each index
search finds exactly what the full scan finds.

## Not done

- Knowledge segments are checksummed, not sealed (they are published facts; a history is sealed, above).
- Program storage stays per-program registers (small, compile-time constants, immutability enforced
  at compile time); it is not stored in segments.
- A history directory has one writer; there is no lock file.
