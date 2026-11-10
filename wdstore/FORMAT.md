# wdstore: the hot store's files (format 1)

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
parameters, each input (name, bytes, blake3), each chunk (file, first and last id, entities, bytes, blake3), the
counts, the skipped triples, the first failed blocks (a block that fails to parse is counted and listed, never
dropped silently). A build step whose input, code and parameter hashes all match is a cached step.

`chunk_NNNNN.wds`: entities sorted by id, ids ascending across chunks:

    "WDSTORE\0" | u32 version | u64 dict_at | u64 index_at | u64 nodes_at | u64 refs_at | u64 redirects_at
    entity records | dictionary | index (id u64, offset u64)* | value nodes | reference nodes | redirects

Varints, length-prefixed UTF-8, one encoding per value: the same input writes the same bytes (`wdstore check`
re-reads every chunk, re-writes it and compares, and checks each chunk's hash against the manifest). One entity is
read by a binary search on the index; the rest of the chunk is not decoded. The short strings that repeat
(datatypes, languages, wikis, node kinds and field names) sit once in the chunk's dictionary.

## Measured (format 1, 2026-11-10)

One segment of the dump's 1% sample (121,169 entities, 1,108,786 statements; 593 MB of Turtle): 0 blocks failed;
116.0 MB, **104.6 bytes a statement**, built in 10 s; a rebuild writes the same bytes. The dump's last 21.7 MB:
7,223 entities, 43 redirects, 97.9 bytes a statement. At about 1.72B statements, format 1 would be about 180 GB:
twice the order's 90 GB line, before scholarly articles go cold.

## Next (format 2)

Where format 1's bytes go, by its own layout: the statement id as text (36-41 bytes, about a third), each reference
and value node as a 16-20 byte hash, every string inline. Format 2: statement ids as 16 bytes with a case flag;
value and reference nodes in one sorted node file each, referenced by a 32-bit number; a reverse index (property,
value entity) -> subjects; the names FST (exact and accent-folded keys, a bounded typo path, never ordered by
fame); then the build over the whole dump in id-range buckets, resumable chunk by chunk.
