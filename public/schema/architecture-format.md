# Architecture format — draft 0.1

**Status:** Draft contract, `0.1.0-draft.1`. The native Rust ingestion path now validates individual snapshots and previews all seven kinds in an object gallery. Relationship rendering, assembled layout, and publication/revision commands remain unimplemented.

Use a small **typed property graph serialized as JSON**. The agent supplies architectural objects and claims about their relationships. The renderer validates those claims and chooses construction, positioning, connection routing, and interaction. No graph database or general ontology inference engine is required.

The machine-readable shape is [architecture.schema.json](architecture.schema.json). Start with [architecture-minimal.json](../examples/architecture-minimal.json), or see all kinds in [architecture.json](../examples/architecture.json). This document defines the graph rules that JSON Schema alone cannot enforce.

## 1. One representation for each fact

The format has three collections:

- `entities`: addressable architectural objects.
- `relations`: directed, typed claims connecting two entity IDs.
- `evidence`: optional supporting references, without source contents.

Containment, interface ownership, and deployment are relation types too. Do not duplicate them in `parent_id`, `children`, `owner_id`, `dependencies`, or embedded objects. This deliberately replaces the illustrative `parent_id` representation in section 9 of the initial product specification. It gives structural claims their own identity and evidence, and prevents contradictory encodings.

Every entity and relation has a stable `id` and a required `basis`. A name is a label, never a foreign key. Array order is insignificant. Forward references are allowed because the entire snapshot is validated before use. Multiple different relations may connect the same pair of entities.

## 2. Supported objects

These are the seven contract kinds. The native simulator implements a standalone specimen for each kind; it does not yet implement their relationship-driven assembly. Deployment remains optional model content.

| `kind` | Represents | Suggested `subtype` values | Rendering meaning |
| --- | --- | --- | --- |
| `scope` | A system, subsystem, or logical grouping | `system`, `subsystem`, `domain` | A grouping frame; no process or deployment claim |
| `client` | A user-facing application | `web`, `mobile`, `desktop`, `cli` | An outer-facing component |
| `service` | A meaningful compute responsibility | `application`, `gateway`, `worker`, `scheduler` | A compute block; subtype alone does not prove independent deployment |
| `datastore` | A logical data storage resource | `relational`, `graph`, `document`, `key_value`, `object` | A storage assembly; relational and graph are the first specialized forms |
| `interface` | An addressable boundary exposed by a component | `api`, `storage`, `user_interface` | An attachment or port on its owner, not a standalone service |
| `concern` | A responsibility that can span components | `authentication`, `authorization`, `service_identity`, `data_access_control`, `billing_entitlement`, `observability` | A pane or spine touching only explicitly covered objects |
| `deployment` | An execution or hosting context | `environment`, `host`, `container`, `process`, `browser` | An optional deployment boundary, separate from logical grouping |

`kind` is a closed vocabulary. `subtype` is an optional descriptive string with recommended values above; an absent or unrecognized subtype gets the generic form for its kind. It never adds new relationship semantics.

An external payment provider is a `service` with `boundary: "external"`. An external database remains a `datastore`. External status is not an object kind. `boundary` is required and is `internal`, `external`, or `unknown`, relative to the system identified by `system_id`. It describes membership in the modeled system, not network location, operator ownership, or a security trust boundary. For example, a managed database can be internal while hosted by a third party. Containment never supplies a missing boundary classification.

`roles` optionally refines responsibility, for example `gateway`, `worker`, or `shared_substrate`. A gateway remains a service with interfaces. A worker is a service; a schedule can be described, but executable schedules and runtime job instances are outside this draft. A cache can be a datastore with the role `cache`; a Redis technology label alone does not establish that role. Roles are descriptive and cannot command a shape or position. Unknown roles remain inspector text.

`technology` is an optional array of display labels, such as `PostgreSQL`. It creates neither objects nor dependencies. Optional `protocol` is permitted only on interfaces; examples include `https`, `grpc`, `sql`, and `in_process`. It is a label, not a connection string or an executable endpoint.

Queues and event buses need a later profile with `publishes` and `subscribes` semantics. Do not disguise asynchronous exchange as `requests`. People, packages, files, classes, functions, database tables, runtime instances, and payload transformations are also outside this architecture draft. Future versions can add them without asking the renderer to infer their meaning from arbitrary labels.

## 3. How links are created

The agent emits a relation with an ID, kind, source ID, target ID, and basis:

```json
{
  "id": "rel:web-requests-api",
  "kind": "requests",
  "source": "client:web",
  "target": "interface:api",
  "basis": "declared"
}
```

The target interface is another entity. An `exposes` relation identifies its owner. If an interface is not known, a permitted component target is enough; the renderer can attach the channel to a generic surface without inventing a semantic interface.

No matching by label, folder, proximity, technology, or array position. Do not emit an inverse relation simply for navigation; incoming and outgoing indexes supply both directions. Use separate directed claims if both components actually initiate requests.

### Relation vocabulary and endpoint rules

Here, **component** means `client`, `service`, or `datastore`; **caller** means `client` or `service`.

| `kind` | Source → target | Exact meaning |
| --- | --- | --- |
| `contains` | `scope` → `scope`, component, or `concern` | Logical decomposition, not execution containment or ownership |
| `exposes` | component → `interface` | This component owns this addressable interface |
| `requests` | caller → caller, or interface exposed by a caller | Source initiates a request to target; no timing, success, or response edge is implied |
| `reads` | caller → datastore, or interface exposed by a datastore | Source reads data from target |
| `writes` | caller → datastore, or interface exposed by a datastore | Source writes data to target |
| `depends_on` | component → component or interface | Source relies on target, with no more specific interaction asserted |
| `applies_to` | `concern` → component or interface | The concern covers the target in the required `aspect` |
| `implemented_by` | `concern` → component or interface | Target implements some or all of this concern |
| `deployed_in` | component or `deployment` → `deployment` | Source executes or is hosted within the named context |

`reads` points **reader → store**, even though data returns in the opposite direction. Relationship direction expresses the predicate, not the travel direction of every byte. A future teaching animation must respect that difference.

`applies_to` requires a concise `aspect`, such as `sign_in_interface` or `request_authorization`. It is invalid on other relation kinds. Coverage does not automatically propagate through containment, interfaces, dependencies, or implementation links. There are no wildcard targets; emit one relation per covered target. Coverage is a supplied architectural claim, not proof of successful security enforcement.

`implemented_by` does not imply `applies_to`, and neither implies a network request. An authentication service, an authentication concern, and that concern's coverage are three different things.

### Structural constraints

- `system_id` references an internal `scope` with subtype `system`. It has no incoming `contains` relation.
- Logical containment is acyclic. Each contained entity has at most one incoming `contains`. Internal scopes, components, and concerns other than the root must be reachable from `system_id` through containment. External or unknown objects can stand alone or be grouped logically.
- Every interface has exactly one incoming `exposes`. Its boundary classification matches its owner. Interfaces cannot also be targets of `contains`; the owner supplies their architectural home.
- Deployment contexts are not logical parents. They cannot participate in `contains` or `exposes`. Nest them through `deployed_in`; the deployment graph must be acyclic. Components may have multiple placements, for example staging and production; that does not assert replica counts or communication between environments.
- Direct self-relations are rejected in this draft. Requests or dependencies between distinct entities may form cycles; those are valid architecture, not a validation failure.
- An entity's kind cannot change while retaining its ID within one model. A rename, description change, or technology change can retain identity. Replacing a service with a genuinely different responsibility requires a new ID.

The common `basis` applies to the record's assertions as a whole. If the service is supported but its proposed deployment is inferred, mark the service supported and the separate `deployed_in` relation inferred. Omit speculative optional fields or use the weaker basis with an explanation; do not use one supported reference to launder unrelated assumptions.

## 4. Dependencies and graph traversal

Prefer `requests`, `reads`, or `writes` when the interaction is known. Use `depends_on` when only reliance is known or when it expresses a distinct fact, such as a build dependency. Its optional `dependency_phase` is `runtime`, `build`, `startup`, or `unknown`; its optional `necessity` is `required`, `optional`, or `unknown`. Both default to unknown when omitted. These fields are invalid on other relation kinds.

An optional plain-text `condition` on `requests`, `reads`, `writes`, or `depends_on` describes when the link applies, such as “when exporting invoices.” It is not an expression to evaluate. Absence makes no claim that every execution uses the connection.

The renderer can derive a **uses/dependency overview** from the interaction edges and explicit `depends_on` edges. Resolve an interface endpoint to its owner for a component-level projection, and retain the original relation IDs for inspection. Deduplicate displayed component pairs without merging their meanings or evidence. Do not ask the LLM to emit a redundant dependency edge for every read and request.

Traversal must choose relation kinds explicitly. “What uses this database?” follows incoming `reads`, `writes`, and relevant `depends_on`; it does not follow `contains`, `applies_to`, `exposes`, or `deployed_in` as if they were runtime links. Structural links can resolve an interface owner or a display ancestor but do not themselves add dependency reachability.

Default factual queries include `supported` and `declared` claims. Inferred links can be included with visible qualification. Unknown links are provisional candidates and are excluded from factual traversal by default. Graph reachability means an architectural path may exist. It does not prove execution order, a hard failure cascade, latency, throughput, or transitive enforcement. Preserve conditions and basis when reporting a path; do not infer that all conditional paths can run together.

## 5. Snapshot envelope and identity

| Field | Required | Meaning |
| --- | --- | --- |
| `schema_version` | Yes | Exact contract version: `0.1.0-draft.1` |
| `model_id` | Yes | Stable identity of this modeled system and extraction scope |
| `revision` | Yes | Positive model revision; increases for each replacement snapshot |
| `system_id` | Yes | Root logical system entity |
| `snapshot` | Yes | Snapshot ID, source revision, coverage, scope description, and known omissions |
| `entities` | Yes | Unordered entity array |
| `relations` | Yes | Unordered relation array; may be empty |
| `evidence` | Yes | Unordered evidence array; may be empty |

All entity, relation, and evidence IDs are unique across those three collections. They use 1–128 lowercase ASCII letters, digits, dots, underscores, colons, or hyphens and start with a letter. Prefixes such as `service:` are recommended for readability, not parsed for semantics. `model_id` and snapshot IDs use the same syntax in separate identity namespaces. Labels, paths, and array indexes must not be used to reconstruct identity.

Reuse existing IDs when refreshing the same architecture. Keep IDs opaque when preserving them: renaming “Orders API” to “Commerce API” does not require changing `service:orders`. Relation IDs survive label or evidence changes, but a change of kind, source, or target creates a new relation ID. Evidence IDs identify immutable references; a changed locator or revision needs a new ID.

`snapshot.source_revision` is a nullable opaque source revision string, not the model revision. `coverage` is `partial` or `complete`, always relative to `scope_description`; it never silently means “the entire repository.” `omissions` lists known exclusions or uncertainty and may be empty. A missing entity or edge does not prove absence in the software.

Publication is atomic replacement of a full snapshot, including when its extraction coverage is partial. No implicit merge or patch in this version. A producer refreshing a model must retain previously modeled content it is not intentionally replacing. An auth-only projection is a view over existing IDs, not a smaller replacement snapshot. An independently extracted subset uses a separate model ID and declared scope.

The later publication command should carry `expected_revision` outside the model: `null` for creation, otherwise the revision being replaced. The receiver atomically rejects stale replacements and requires the new revision to be the prior revision plus one. An exact retry of the same snapshot and request is idempotent; reusing a snapshot ID with different contents is an error. Loading a saved historical snapshot is a separate read operation, not a new publication. These are command requirements, not implemented APIs.

## 6. Evidence and uncertainty

Each entity and relation declares one basis:

| `basis` | Rule |
| --- | --- |
| `supported` | At least one resolving `evidence_ids` entry; supplied evidence supports the claim |
| `inferred` | A required `rationale`; evidence references are optional |
| `declared` | An explicit authored or hypothetical claim, appropriate for synthetic examples |
| `unknown` | An explicitly unresolved claim or candidate; no numeric confidence invented |

Entity evidence supports the entity record, not its relationships. A supported service can still have an inferred outgoing call. Uncertainty belongs to individual claims rather than being hidden in a model-wide confidence score.

Evidence records contain `id`, `description`, and an optional `locator`. A locator is either a `source` location with a repository-relative `path` and optional positive `start_line`/`end_line`, or an opaque `reference` with a `value`. When both lines are present, end must be at least start; end requires start. Source locators refer to this snapshot's source revision. Cross-repository or differently versioned sources use opaque references for now.

References are display data. Validation resolves evidence IDs but does not open files, verify claims against source, fetch URLs, or execute anything. Reject absolute source paths, backslashes, empty path segments, `.`/`..` segments, and NULs. Labels, descriptions, technology, protocols, and opaque references are never instructions or automatically fetched resources. Source bodies, credentials, connection strings, and arbitrary assets are not fields in this contract.

## 7. What the renderer derives

Build indexes by entity ID, relation ID, relation kind, incoming/outgoing endpoint, logical parent, interface owner, and deployment context. Then compile semantic relationships into presentation:

| Input fact | Renderer responsibility |
| --- | --- |
| Scope and `contains` | Nested frames, peer grouping, collapse/expand behavior |
| Kind, subtype, role | Suitable cuboid construction and generic fallbacks |
| Interface and `exposes` | Ports/faces attached to the owning object |
| `requests`, `reads`, `writes`, `depends_on` | Distinguishable orthogonal channels with truthful direction |
| Concern and `applies_to` | A span engaging only the supplied targets/aspects |
| Boundary classification | Distinguish internal, external, and unknown membership |
| `deployed_in` | A separate deployment view or overlay when requested |
| Stable IDs and prior local layout | Spatial continuity across refreshes |

When a scope is collapsed, connections may terminate at a display proxy on that scope. This does not create a new semantic interface or relation. A bundled channel reports all underlying relation IDs. Multiple fragments of one concern retain that concern's ID. Display-only selection objects must resolve to those IDs.

Evidence basis stays visible on structural representations too: an inferred containment claim or unknown interface ownership cannot appear verified merely because it affects grouping or attachment. If a renderer cannot represent a provisional structural claim honestly, it must report the limitation instead of silently upgrading the claim.

The agent never supplies coordinates, sizes, axes, colors, materials, meshes, camera poses, layout forces, or scene-building code. Presentation choices live in local view state. Initial placement should be reproducible for a canonicalized graph and renderer version; subsequent placement can also depend on prior local layout state. Reordering JSON arrays must not shuffle the assembly.

Keep three representations separate: **architecture snapshot**, **view/teaching intent**, and **private render scene**. Focus, hidden IDs, expanded IDs, annotations, walkthrough order, and camera framing are not architecture facts. A walkthrough references existing entity/relation IDs at an expected model revision. Step order is explanatory order unless separately supported; it cannot invent connections or masquerade as a live execution trace. Formal view and walkthrough schemas remain a later slice.

## 8. Validation and compatibility

Validation runs before replacing any current model:

1. Enforce input limits before expensive processing; reject duplicate JSON object keys rather than accepting parser-dependent values.
2. Check JSON Schema: version, fields, enums, lengths, required data, and claim-local evidence rules.
3. Check global ID uniqueness and every reference, then endpoint-kind rules, interface ownership, boundary consistency, containment/deployment cycles, root reachability, evidence paths/line ranges, and identity compatibility with the prior revision.
4. Check negotiated runtime capabilities. Unsupported required entity/relation kinds reject the snapshot; an unrecognized descriptive subtype or role can use a generic form with a warning. Never silently drop a relationship whose meaning is unsupported.
5. Apply the whole validated snapshot atomically; retain the last valid scene on failure.

Two distinct relations with the same predicate, endpoints, aspect, condition, and dependency qualifiers should produce a duplicate-claim warning. Multiple predicates between the same endpoints are valid. Empty logical scopes, unplaced deployments, generic subtype fallbacks, or absent runtime links are not evidence that the extraction failed.

Initial limits: 2 MiB UTF-8 payload, JSON nesting depth 16 (root object is depth 1), 512 entities, 4,096 relations, 4,096 evidence records, 32 evidence IDs per claim, 120-character labels, and 2,000-character descriptions/rationales/conditions. All other field limits are in the schema. These are defensive draft budgets, not a claim that 512 objects are readable simultaneously. Capability discovery may advertise stricter limits.

Diagnostics should include a stable code, a JSON Pointer, and an actionable message, for example `interface_owner_missing` at `/entities/3` or `invalid_relation_target` at `/relations/2/target`. The later validator should accumulate bounded independent errors rather than returning only “invalid model.”

Unknown top-level and record fields are rejected. No arbitrary `metadata` bag or custom executable extension mechanism in this draft. New semantic kinds require an advertised schema/profile version. Do not silently coerce `queue` into `service` or omit unsupported edges. If a model is reduced for an older renderer, the producer must describe the omission in coverage and publish an honestly scoped model.

The schema uses [JSON Schema Draft 2020-12](https://json-schema.org/draft/2020-12/json-schema-core). It is the canonical full validation shape; provider-specific structured-output adapters can simplify its presentation but must validate generated JSON against this contract afterward. Passing JSON Schema alone does not establish graph validity or factual accuracy.

## 9. Generation recipe for the user's LLM

1. Read capabilities and existing model IDs before extraction or refresh.
2. Identify the requested architectural scope, coverage, and omissions. Keep source access in the user's agent.
3. Create the root scope, concrete components, and any meaningful logical subscopes. Reuse stable IDs.
4. Add interfaces only where they improve the explanation; connect each to exactly one owner using `exposes`.
5. Add the most specific supported interaction links; use generic dependencies only where appropriate.
6. Model cross-cutting concerns separately from their implementations, with explicit per-target coverage.
7. Add deployment contexts only when supported or clearly marked inferred. Never deduce one from a directory name.
8. Attach basis and evidence to each claim. Leave missing information unknown.
9. Validate locally, repair diagnostics, and publish a complete replacement with the expected revision.
10. Request focus or a walkthrough separately using the resulting semantic IDs.

## 10. Implementation boundary

The Rust `architecture` module now supplies bounded parsing, schema validation, semantic graph checks, typed snapshots, and shared executable conformance cases. The native simulator loads embedded, file, or stdin snapshots and renders each entity as a separate specimen. The [seven-object fixture](../examples/objects.json) contains only required logical containment and interface ownership; no connections are drawn. The historical database-only type remains separate.

Revision storage/publication checks, TypeScript validation, MCP tools, formal view/walkthrough schemas, relationship rendering, and assembled layout remain later slices. The single-snapshot loader does not claim to enforce identity compatibility against a previously published revision. Those slices should use this contract and shared conformance cases rather than creating independent interpretations.
