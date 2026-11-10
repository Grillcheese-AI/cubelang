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

## Next

The reverse index (property, value entity) -> subjects; the names FST (exact and accent-folded keys, a bounded typo
path, never ordered by fame); the scholarly split (the size line's question in 0055); the build over the whole dump
in id-range buckets, resumable chunk by chunk, memory-mapped reads, and the lookup latencies the order's lines ask
for (1 ms p95 from SSD).
