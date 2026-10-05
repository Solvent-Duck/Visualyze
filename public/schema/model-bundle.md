# Local semantic bundle v1

Status: implemented additive Rust provider. This format does **not** replace or relax the existing [`0.1.0-draft.1` architecture snapshot](architecture-format.md). The snapshot reader, JSON Schema, fixtures, and limits remain unchanged. Bundle profiles add typed semantics needed for deeper and larger supplied models. They contain no coordinates, shapes, pins, groups, camera state, executable instructions, or source bodies.

The implementation is `visualyze_model::provider`. Start with [`data-messaging.bundle.jsonl`](../examples/data-messaging.bundle.jsonl), a declared synthetic example containing tables, code decomposition, asynchronous messaging, and an explicit deployment instance.

## File envelope

A bundle is one explicitly selected, UTF-8 JSON-lines regular file. Every record occupies one newline-terminated line. Blank lines, trailing content, unrecognized fields, duplicate fields, excessive nesting, and incomplete files are rejected. JSON text may contain escaped newlines. There are no archives, embedded file names, external index paths, relative file references to open, or automatic evidence fetches.

Each record has exactly `record` and `value` fields:

```json
{"record":"manifest","value":{"format":"visualyze.bundle.v1","model_id":"example:orders","revision":1,"root_id":"scope:orders","profiles":["architecture.v1","data.v1"],"coverage":"partial","scope_description":"Declared example","omissions":[]}}
```

Records appear in this order:

1. One `manifest`.
2. Zero or more `evidence` records, sorted by ID.
3. One or more `entity` records, sorted by ID.
4. Zero or more `relation` records, sorted by ID.
5. One `complete` record, with exact `entities`, `relations`, and `evidence` counts.

IDs are unique across the three semantic sections. They use 1–128 lowercase ASCII letters, digits, dots, underscores, colons, or hyphens and begin with a letter. Names are labels, never lookup keys. A bundle's physical order is canonical to allow streaming validation; producers must sort records before emission. Sorting does not create architectural meaning.

`revision` is a positive unsigned integer. `model_id` and `root_id` use the same ID grammar. `coverage` is `partial` or `complete` relative to the supplied `scope_description`; an omitted object is never proof that it does not exist. `omissions` preserves known uncertainty. All five profile names below are exact versioned strings; unsupported versions are rejected. The required `architecture.v1` profile may be accompanied by any of the other profiles, without duplicates.

## Records and claims

An entity record contains `summary` and nullable `detail`:

```json
{"record":"entity","value":{"summary":{"id":"table:orders","kind":"table","label":"Orders","boundary":"internal","claim":{"basis":"declared","evidence_ids":[],"rationale":null}},"detail":{"description":"One explicitly supplied table","subtype":null,"roles":[],"technology":[]}}}
```

Summary fields are `id`, `kind`, `label`, `boundary`, and `claim`. Boundary is `internal`, `external`, or `unknown`; ownership does not invent boundary classification. Detail contains bounded descriptive text only. A null `detail` means detail was **not supplied**. Supplied detail that is currently absent from the provider cache is **unloaded**. Cache eviction does not change the supplied model.

A relation contains `id`, `kind`, `source`, `target`, `claim`, nullable `label`, and nullable `aspect`. Structural ownership is a relation with its own identity and evidence; it is not duplicated in an entity's parent field. Relation endpoints must resolve to entities. Parallel distinct claims and runtime cycles are permitted; direct self-relations are rejected in v1. `aspect` is required only for `applies_to` and forbidden on other predicates.

Every entity and relation carries a `Claim`: `basis`, `evidence_ids`, and nullable `rationale`. Basis is `supported`, `inferred`, `declared`, or `unknown`. Supported claims require at least one resolving evidence reference; inferred claims require a nonblank rationale. Evidence IDs cannot repeat within a claim. The provider preserves these claims and never silently upgrades their certainty.

Evidence records contain `id`, `description`, and nullable opaque `reference` text. References are display data. The provider never opens a reference as a path, fetches a URL, or interprets a string as a command.

## Versioned profiles

| Profile | Additional entity kinds | Explicit relationships |
| --- | --- | --- |
| `architecture.v1` | `scope`, `client`, `service`, `datastore`, `interface`, `concern`, `deployment` | `contains`, `exposes`, `requests`, `reads`, `writes`, `depends_on`, `applies_to`, `implemented_by`, `deployed_in` |
| `data.v1` | `table`, `column` | `owns_table`: datastore → table; `owns_column`: table → column; `table_references`: table → table or column → column |
| `code.v1` | `code_repository`, `code_package`, `code_file`, `code_type`, `code_function` | `owns_code`: repository → package/file, package → package/file, file → type/function, type → type/function; `code_references`: code object → code object |
| `messaging.v1` | `message_channel`, `message_type` | `publishes`: caller → channel; `subscribes`: caller → channel; `carries`: channel → message type |
| `instances.v1` | `deployment_instance` | `instance_of`: instance → client/service/datastore; `deployed_in`: instance → deployment context |

Here a component is a client, service, or datastore; a caller is a client or service. A message channel can be described as a queue, topic, or bus using `detail.subtype`, but that descriptive field does not invent delivery guarantees. `subscribes` points from the subscriber to its channel; this predicate direction does not depict the physical direction of payload travel. `table_references` asserts a supplied dependency/reference only, without inferring cardinality or SQL constraints. `code_references` does not imply an executed function call.

Architecture endpoint rules are explicit:

- `contains`: scope → scope, component, concern, code repository, message channel, or message type.
- `exposes`: component → interface.
- `requests`: caller → caller or interface owned by a caller.
- `reads` / `writes`: caller → datastore, table, or interface owned by a datastore. Direction remains reader/writer → data resource.
- `depends_on`: component → component or interface.
- `applies_to` / `implemented_by`: concern → component or interface. These are distinct predicates; neither implies the other or a request.
- `deployed_in`: component, deployment context, or deployment instance → deployment context. A component may have several supplied contexts. A deployment context or concrete instance has at most one context parent in this profile.

An interface, table, column, or non-root code unit must have exactly one permitted owner. Each instance requires exactly one `instance_of` and one `deployed_in` claim. The logical ownership forest and deployment-context forest are validated independently and must be acyclic. Instances and deployment contexts are separate from logical ownership. The declared root is an unowned internal scope. Every other internal logical object must be reachable from it through supplied ownership relations. Ownership other than logical `contains` requires equal owner/child boundary classifications; `contains` may group objects with different explicit boundary values.

Structural ownership is factual supplied content. Suggested visual grouping belongs exclusively to private view state and must not be serialized as supplied `contains` claims.

These are deliberately small initial profiles. They do not represent executable schedules, arbitrary metadata, SQL expression semantics, payload transformations, trace timing, deployment discovery, or code-to-runtime implementation mappings. Subtype strings cannot add these semantics. Future profile revisions must define them explicitly.

## Provider API and indexing

`BundleWriter::create(path, manifest, limits)` creates a **new** file with exclusive creation. `push(&BundleRecord)` serializes one semantic record at a time and enforces order and local bounds. `finish()` writes the completion marker, flushes the file, then fully validates and indexes it before returning `LocalBundle`. `finish_with_progress` supports cancellation during validation/indexing. A failed or dropped writer leaves its output available for diagnosis; incomplete or semantically invalid files cannot be opened as providers.

`LocalBundle::open(path, limits)` and `open_with_progress` scan and validate the file before exposing any records. Initial indexing is a separate phase from layout and has a progress callback every 1,024 semantic records, before indexing, and around final graph validation. Returning `ControlFlow::Break(())` cancels without publishing a partial provider.

The index retains identities, kinds, labels for bounded search, record offsets/checksums, ownership preorder intervals, compact relation endpoint indices, and incident-relation lists. It **does not** retain all detail descriptions. Record detail is decoded transiently during validation and summary reads, then discarded unless explicitly requested for the bounded detail cache. The on-disk format does not yet persist a reusable index: opening a bundle rebuilds its metadata index in O(entities + relations + evidence) work, plus child ordering. Very large, high-degree models remain subject to the index-memory budget.

`ModelProvider` exposes:

- `manifest()` for model identity, revision, profiles, scope, and omissions.
- `summary(expected_revision, id)` for one entity's semantic summary.
- `relation(expected_revision, id)` for a specific original claim, including a collapsed internal membership ID.
- `children(expected_revision, id, PageRequest)` for supplied logical ownership children.
- `incident(expected_revision, id, PageRequest)` for original incoming and outgoing relation records, including structural, deployment, and instance claims.
- `children_at(expected_revision, id, offset, limit)` and `child_count` for direct bounded child windows without walking previous pages.
- `region_relations(expected_revision, RegionRequest, cancelled)` for original relations crossing visible representatives of a focused ownership subtree, including hidden descendants. A sorted endpoint index queries only the requested ownership intervals, without visiting unrelated relations or loading descendant entity records. Each call examines at most 8,192 indexed endpoints and returns at most the requested record/byte budget; empty pages may advance a scan cursor. A relationship whose two endpoints are in the region consumes two index visits but is emitted or counted only once, including across disjoint group roots. The nearest visible ancestor represents an internal endpoint. An external endpoint is represented by the child of its common containing assembly with the focus; binary-depth ancestor jumps bound this lookup to O(log²(depth)) steps with cancellation checks. Original endpoints, predicate, evidence, and identity remain in every returned relation. Visible ownership claims also remain available. Previously saved global-scan cursors keep their original traversal until exhausted; new queries use the local index.
- `search(expected_revision, query, PageRequest, cancelled)` for case-insensitive ID/label search ranked by exact match, then prefix, then substring, with at most 8,192 index entries examined per page and bounded matching summary reads. Ranking uses three disjoint index passes, so an exact package label cannot be hidden by earlier file substring matches.
- `detail_state` and `detail` to distinguish unsupplied, unloaded, and loaded detail.
- `evidence(expected_revision, id)` for a supplied evidence record.

Every data query rejects a mismatched revision. Page cursors are opaque, model/revision/query-bound values; a cursor for children cannot be reused for incident relations or another parent/model. Children are returned in child ID order; incident relations in relation ID order. Queries return all original kinds, endpoints, claim bases, and evidence references. Consumers select the appropriate relationship types for a view; the provider does not flatten deployment and logical containment into one tree.

Region pages also return exact internal-relation counts for the examined index slice and a bounded sample of original membership IDs. Counts can be summed across disjoint scan pages. They are not complete until the continuation is exhausted. The layout adapter keeps at most 4,096 membership IDs, aggregates visible connectors only for matching directed endpoint pairs, predicate, and aspect, and retains every loaded original claim for inspection. Internal membership beyond that sample stays in the provider; entering the corresponding region reveals its original records. A projection that exhausts its relation, reference, visible-object, or scan budget reports explicit incompleteness and continuation metadata. A camera view is never represented as proof of model completeness.

Lookup of a missing summary/evidence ID returns `None`; traversal or detail access for an unknown entity returns `NotFound`. Missing records and unsupplied detail are not silently treated as unloaded. A page is capped by both record count and serialized bytes. Returned values belong to the caller; holding many pages or `Arc` detail handles is the caller's responsibility and lies outside the provider cache budget.

The detail cache evicts least-recently-used records and is limited by count and charged bytes. Charges include serialized size plus a conservative 4 KiB allowance for bounded text containers, allocation overhead, and cache bookkeeping. `clear_detail_cache()` supports explicit eviction. `ProviderStats` reports counts, estimated index bytes, and charged cache residency, not total process RSS.

Selected inputs must be regular files, not directories or final-component symbolic links. macOS/Linux opens also use nonblocking/no-follow flags to avoid a FIFO or symlink replacement race. Modification metadata is checked on queries; loaded records are checked against their index-time checksum. A changed source requires reopening/reindexing. Checksums provide change detection, not cryptographic authentication or protection against a malicious concurrent writer. Atomically replacing the path leaves an already-open provider on its original file/revision; applications explicitly open the replacement to switch revisions.

Revision publication, cross-revision identity reconciliation, saved indexes, snapshot-to-bundle migration, and background scheduling are consumer responsibilities and are not provider APIs. No repository scan, network listener, architecture upload, or automatic source dereference is introduced.

## Resource limits

| Resource | v1 default |
| --- | ---: |
| Bundle file | 16 GiB maximum |
| One record including newline | 64 KiB maximum |
| JSON nesting | 16 levels maximum |
| Entities / relations / evidence | 2,000,000 / 16,000,000 / 2,000,000 |
| Conservative metadata index budget | 1 GiB |
| Returned page | 256 records and 1 MiB serialized bytes |
| Cached detail | 256 records and 4 MiB charged bytes |
| Claim evidence references | 32 |
| Label / rationale / detail description | 512 / 4,096 / 32,768 UTF-8 bytes |

Limits can be tightened. The index budget can reject a model before record-count maxima are reached. Query page limits cannot exceed 256 records or 16 MiB, and a page byte budget must accommodate the allowed maximum record. Record and file maxima cannot be raised above the v1 format bounds. Arrays and text fields have additional typed limits enforced by the reader. Unknown fields and duplicate typed fields are rejected rather than ignored, including rendering instructions disguised as extra properties.

## Synthetic measurements

Measured on September 29, 2026, on Apple M5, 16 GiB RAM, macOS 26.6.2 (25G83), Rust 1.96.0. Native optimized development build; no extra dependencies. Each run creates a 32-way ownership tree with one description per entity and one `contains` relation per non-root entity, writes it sequentially, validates/indexes it, and queries a root page and 100 details. It then deletes its temporary bundle. Each size runs in a fresh process. The operating-system file cache is warm from writing; these are single-run observations, not disk-cold latency percentiles.

```sh
cargo build -p visualyze-model --example bundle_bench --locked
/usr/bin/time -l target/debug/examples/bundle_bench 10000
/usr/bin/time -l target/debug/examples/bundle_bench 100000
/usr/bin/time -l target/debug/examples/bundle_bench 1000000
```

| Entities | Relations | Bundle bytes | Streaming write | Finalize + index + validation | Peak RSS | Root page (32 summaries) |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 9,999 | 5,558,510 | 0.020 s | 0.030 s | 10,502,144 bytes | 62 µs |
| 100,000 | 99,999 | 55,682,888 | 0.203 s | 0.247 s | 78,102,528 bytes | 64 µs |
| 1,000,000 | 999,999 | 557,826,640 | 1.969 s | 2.876 s | 833,077,248 bytes | 72 µs |

These measurements include descendant intervals, compact relation endpoints, indexed labels and separate semantic-lens children used by production projection/search. They predate cooperative cancellation inside finalization and the September 30 subtree incidence indexes and ancestor jumps; current measurements are reported separately in the placement validation document. They replace the earlier 744,161,280-byte million-entity RSS observation from the smaller index. `BundleWriter::finish()` includes the final file flush/sync in addition to indexing/validation. [Raw measured fields](../../docs/measurements/provider-lenses-2026-09-29.json) distinguish measured RSS from the provider's conservative index estimate (967,388,649 bytes at one million entities).

No details are cached immediately after indexing. The query sample retains 100 details, charged at approximately 0.43 MiB. The resident index remains proportional to the graph; full detail is never held resident by this provider. This benchmark establishes provider-scale behavior for a synthetic tree only. It does not measure 3D placement, scene rendering, high-degree distributed graphs, comprehension, or actual Google/OpenAI architecture, and it does not establish all product release gates.

## Validation

`cargo test -p visualyze-model --locked` preserves the snapshot conformance suite and adds provider coverage for all profiles, endpoint and ownership semantics, evidence, revision checks, cross-query cursors, unsupplied versus unloaded detail, count/byte cache eviction, page-byte bounds, runtime cycles and parallel claims, structural cycles, cancellation during indexing, file mutation, incomplete records, malformed/duplicate/unknown fields, excessive nesting, symlinks, and streaming-writer non-overwrite behavior.

`cargo clippy -p visualyze-model --all-targets --locked -- -D warnings` checks the provider and benchmark. Native viewer/render integration is evaluated separately from this public model boundary.
