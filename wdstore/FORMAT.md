# wdstore: the hot store's files (format 2)

Cubby's hot Wikidata store, CubbyLLM order 0055. Built from the WikiKGQA 2026 challenge's dump (Zenodo 22661413,
CC0), never from a model. Where it lives (the order leaves it to Claude Code): a crate of its own in the cubelang
repository, beside the VM's datastore, with its own workspace and target folder, so building it never touches
cubelang's `target/release`; the VM depends on it when its hops read it (0055 step 3, behind a flag).

## What it keeps, and how exactly

- **Ids** are Wikidata's own: the kind letter and number, packed in a u64 (`Eid`; Q-ids fit in 32 bits and are about
  86% dense, so no id map is built and nothing is ordered by anything but the id).
- **Every statement, at every rank**, flagged: rank, best rank, its statement id (as written), its main value
  (an entity, a string, an IRI, a monolingual text, a typed literal, some value, no value).
- **No float anywhere.** A quantity, a coordinate, a precision is the decimal text the dump wrote; a time keeps its
  literal, and its value node (precision, time zone, calendar) is kept beside it. `tests/store.rs` fails on a float
  type in the store's code.
- **Qualifiers**: the valid-time ones hot (P580 start, P582 end, P585 point in time, P1319 earliest, P1326 latest;
  a build parameter), the rest kept apart (cold). A qualifier's value nodes are listed by property, as the dump
  states them: Turtle does not pair two values of one property with their nodes, so the pairing is made on reading,
  by the node's own value.
- **References** by their node's hash; the reference nodes (their snaks) once each.
- **Recorded time** (the base's side of the two clocks): each entity's revision and timestamp; the dump's identity
  in the manifest. Every base statement is open; an overlay closes one with a recorded end, never overwrites it.
- **Names**: labels, aliases and descriptions in EN, FR and `mul` (a build parameter); **sitelinks**: enwiki and
  frwiki titles.
- **Redirects**: old id -> the id that remains. The dump writes each as its own block (a `data:` header for the old
  id, then `wd:Qold owl:sameAs wd:Qnew`).
- Not kept, because derived from what is: `wdt:`/`wdtn:` (truthy), `psn:`/`pqn:`/`prn:` (normalized values), the
  counts on `data:`, the property declarations (`p:P31 a owl:ObjectProperty`). The manifest counts every skipped
  triple by prefix.

## Files

`manifest.json`: the format version, the code's blake3 (`code_hash()`, over the store's sources), the build
parameters, each input (name, bytes, blake3), each file (name, bytes, blake3; a chunk's first and last id and its
entities), the counts (with the value and reference nodes named but never written: 0 is the line for a full
build), the skipped triples by prefix, the first failed blocks (a block that fails to parse is counted and listed,
never dropped silently). A build step whose input, code and parameter hashes all match is a cached step.

`chunk_NNNNN.wds`, entities sorted by id, ids ascending across chunks:

    "WDSTORE\0" | u32 version | u64 dict_at | u64 index_at | entity records | dictionary | index (id u64, offset u32)*

A statement: the gap from the previous statement's property; a flags byte (rank, 2 bits; best rank; a main value
node; the statement id's kind, 2 bits; has qualifiers; has references); the statement id (16 bytes when it is a UUID
in one case, as written otherwise); the main value (an id as `number << 3 | kind << 1 | has sub-id`); then the main
value node's number, the qualifiers and the references' numbers, as the flags say.

`values.wds`, `refs.wds`: every value and reference node once, sorted by hash and numbered from 0:

    "WDNODES\0" | u32 version | u8 kind | u64 count | u64 dict_at | u64 offsets_at | records | dictionary | offsets

A record is the hash, whether the input wrote the node, then a value node's kind and fields (each the literal the
dump wrote) or a reference's snaks and value-node numbers. The dump may write one node's triples in two places (a
quantity's amount and unit in one block, its `quantityNormalized` link in another): the node is their union.

`redirects.wds`: "WDREDIR\0" | u32 version | u64 count | (old id u64, new id u64)* sorted by the old id. `resolve`
follows at most 8 (a loop is an error, not an answer).

One encoding per value: the same input writes the same bytes. `wdstore check` re-reads every file whole, re-writes
it and compares, and compares each file's hash with the manifest's. One entity is read by a binary search on its
chunk's index; the rest of the chunk is not decoded. The short strings that repeat (datatypes, languages, wikis,
node kinds and field names) sit once in each file's dictionary.

## Measured

One segment of the dump's 1% sample (121,169 entities, 1,108,786 statements; 593 MB of Turtle), 2026-11-10:

| | format 1 | format 2 |
|---|---|---|
| blocks failed | 0 | 0 |
| bytes a statement, entity chunks | 104.6 (nodes inline) | 54.8 |
| bytes a statement, with the node files | 104.6 | 65.5 |
| build | 10 s | 11 s |
| a rebuild writes the same bytes | yes | yes |

At about 1.72B statements, format 2 is about 113 GB for the whole dump; with the scholarly articles cold (55% of the
statements in the JSON count), about 50-55 GB, under the order's 90 GB line. The node files will not scale with
the sample (151M distinct references in the dump, estimated): the full build measures them. 6,198 of the segment's
45,309 value nodes and 5,308 of its 86,719 references are named but written elsewhere in the dump.

## The reverse index and the names (2026-11-10)

`reverse.wds`: for each (property, value entity), the entities whose statement has that value, in id order, each
with its statement's rank and best-rank flag (a sorted key table of 24-byte entries, then varint gaps). Main values
only. `names_exact.fst`, `names_folded.fst`, `names.post`: every label and alias kept (EN, FR, `mul`) under an exact
key (lowercase, spaces collapsed) and a folded one (accents and punctuation dropped too), each mapping to its
entities in id order with the name's language and kind. A lookup answers from the first stage with a hit: exact,
folded, then a typo path over the folded keys (one edit from 5 characters, two from 9, none below), all candidates,
never ordered by fame; over 64 near keys, it says so instead of guessing. `check` rebuilds both from the entities
and compares the bytes.

Same segment: reverse 4.8 MB (136,280 keys), names 11.5 MB (276,522 names, about 42 bytes a name); 80.2 bytes a
statement in all. Lookups from memory (`wdstore bench`, 3,275 ids spread over the store): an entity by id p50 9.2 us,
p95 15.4 us, p99 22.5 us; a name p50 2.2 us, p95 5.8 us, p99 8.4 us. The order's lines are 1 ms p95 for a fact and
50 ms p95 for a name, from SSD: measured on the full build, memory-mapped.

## The full build (format 3)

The dump is about 1 TB of Turtle in 121 GB of bzip2, in 8 shards one after another; within a shard the ids rise
but not strictly (34 descents in one sample segment), so the build never sorts the entities:

- **Read:** the 113 parts as one file, decoded on every core (`bz2par`: 238 MB/s out on 12 threads, about 70 min
  for the dump from a local disk), cut into entity blocks, ingested on every core, handed on in the dump's order.
- **Entities:** written in dump order, a chunk at a time (each chunk sorted inside); `ids.wds`, a dense table per
  kind (one 8-byte slot per Q-number: chunk, offset; about 1.1 GB), finds any entity in one read.
- **Nodes:** numbered by first sighting (a reference or a definition), the hash -> number map in memory (about
  10 GB at 200M nodes); every definition appended as a record; a node the dump writes in two places is two records,
  merged on reading; a node never defined is written by hash alone at the end and counted.
- **Reverse rows and name rows:** sorted in runs on a scratch disk, merged at the end into `reverse.wds` and the two
  FSTs.
- **Redirects:** collected, sorted at the end.
- **Resumable:** every output is append-only; a checkpoint (every few minutes) records each file's length, the
  bzip2 block the text resumes from and the decoded offset of the next entity. A stopped build truncates its files
  to the checkpoint and goes on from that block: nothing is decoded twice.
- **Hot and cold:** entities of the bulk classes (scholarly articles, P31 Q13442814, by default; a build
  parameter) go to a cold store of the same format beside the hot one, as the synthesis says.
- **Deterministic:** one writer, in the dump's order; a second build gives the same bytes (the order's acceptance).

Measured on the dump's last part (1.05 GB of bzip2, 8.5 GB of Turtle), 2026-11-10: 1,235,092 hot and 614,755 cold
entities, 16.5M statements, 32,919 redirects; 87 s on 12 threads; a build stopped after 8 checkpoints and run again
gives the same 72 files, byte for byte, as one never stopped; 1.2 GB written in all (hot and cold chunks, nodes,
indexes and the build's logs). The dump at this rate: about 2.7 h and about 135 GB, hot store about 75-80 GB.

## The store's state, by one hash (CheezML v0.2; 0055's snapshot addendum)

- **The base:** the hash of every file's name and blake3 as the manifest lists them, sorted. A rebuild writing the
  same bytes names the same state, whatever its timing. The manifest also names the dump as a release (its download
  manifest: name, size, md5, the Zenodo record, its date and licence) and the dump's own date
  (`wikibase:Dump schema:dateModified`), the as-of an undated question uses on a store pinned to the dump.
- **The change log** (`changes.log`, empty until the first overlay change): one JSON object a line, each starting
  with the blake3 of the line before (`{"prev": "...", ...}`), so the last line's hash stands for the whole log and a
  line changed anywhere breaks the chain.
- **`wdstore snapshot <dir> [--at N]`:** the base hash, the log's length and head, and one snapshot hash over them;
  `--at N` is the state after the log's first N changes (a pin). A read against a named snapshot that the store
  does not hold fails with `snapshot_not_held`, never reads the latest instead.

## Next

The scholarly split (the size line's question in 0055); the build over the whole dump
in id-range buckets, resumable chunk by chunk, memory-mapped reads, and the lookup latencies the order's lines ask
for (1 ms p95 from SSD).
