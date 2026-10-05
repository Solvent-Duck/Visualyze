# Draft architecture contract conformance

Target: `0.1.0-draft.1`. Single-snapshot Rust validation is implemented. [architecture-cases.json](architecture-cases.json) contains 76 shared mutation cases executed by `cargo test -p visualyze-model --locked`. Each case starts with a named synthetic fixture and applies JSON Patch `add`/`remove` operations; `expected_error: null` means acceptance. Other values identify an expected diagnostic code. Additional Rust tests cover resource limits and raw JSON parsing. TypeScript parity and the interpretation/publication cases below remain future work.

The [JSON Schema](../schema/architecture.schema.json) enforces record shape. The [format document](../schema/architecture-format.md) defines graph and publication rules. Both layers are required before accepting a model.

## Positive fixtures

- [objects.json](../examples/objects.json): exactly seven entities, one per kind, with only required logical containment and interface ownership. Default simulator input.

- [architecture-minimal.json](../examples/architecture-minimal.json): root system, one declared datastore, one containment link.
- [architecture.json](../examples/architecture.json): all seven entity kinds and all nine relation kinds, interface ownership, independent storage read/write claims, explicit concern coverage, an external service, and deployment nesting.

Both are synthetic declarations. Neither asserts that a real repository was inspected. The older `database.json` remains the separate Phase 0 renderer fixture and is intentionally not valid under this draft envelope.

## Schema cases

| Variation | Expected |
| --- | --- |
| Either positive fixture | Accept shape |
| Reorder entities, relations, or evidence | Accept shape |
| Omit optional subtype, roles, technology, or protocol | Accept shape |
| Unknown descriptive subtype or role | Accept shape; runtime can warn and use a generic form |
| Unknown entity kind or relation kind | Reject |
| Missing envelope, identity, label, boundary, or basis field | Reject |
| Wrong schema version | Reject |
| Null in a field other than nullable source revision | Reject |
| Empty/whitespace-only label, description, or rationale | Reject |
| ID contains whitespace, uppercase letters, slashes, or exceeds 128 characters | Reject |
| Revision is zero, fractional, or outside the safe integer range | Reject |
| Coordinates, styles, arbitrary metadata, or unknown fields | Reject |
| Supported claim with missing/empty evidence IDs | Reject |
| Inferred claim without rationale | Reject |
| Supported claim with a syntactically valid but unresolved evidence ID | Accept shape; semantic rejection |
| `applies_to` without aspect, or aspect on another predicate | Reject |
| Dependency qualifiers on anything other than `depends_on` | Reject |
| Condition on a structural or coverage relation | Reject |
| Protocol on a non-interface entity | Reject |
| Evidence end line without start, or a nonpositive line | Reject |
| Counts or field lengths exceed schema limits | Reject |

## Graph and resource cases

| Variation | Expected |
| --- | --- |
| Duplicate IDs within or across collections | Reject |
| Missing source, target, system, or evidence reference | Reject |
| Root is not an internal system scope, or is contained by something | Reject |
| Internal logical entity has no containment path to root | Reject |
| Two logical parents or cyclic logical containment | Reject |
| Interface has zero/two owners, or a boundary different from its owner | Reject |
| Interface is logically contained as well as exposed | Reject |
| Datastore exposes a SQL interface; service reads that interface | Accept |
| Service reads a service-owned interface or a concern | Reject endpoint kinds |
| `applies_to` source is not a concern, or target is a scope | Reject endpoint kinds |
| Deployment is used as a logical parent | Reject |
| Cycle of nested deployments | Reject |
| Service has placements in staging and production | Accept; no replica or cross-environment traffic inference |
| Two distinct services request/depend on each other | Accept; dependency cycles are legal |
| Direct self-relation | Reject in this draft |
| Read and write between the same endpoints | Accept as separate claims |
| Repeated identical predicate/endpoints/qualifiers | Warn about duplicate claim |
| Source path is absolute, traverses `..`, or has forbidden segments | Reject |
| Evidence end line precedes start line | Reject |
| Payload exceeds bytes/depth limit; duplicate JSON object keys | Reject before publication |

## Interpretation and publication cases

- Collapsing a scope preserves all underlying entity/relation IDs. A display proxy does not add facts.
- Following an interface connection reaches its owner in the component projection while retaining the original interface and edge IDs.
- Concern coverage touches exactly its declared targets. Implementation links do not expand coverage.
- The datastore is not covered by authentication in the worked fixture, and the model makes no authorization or database-enforcement claim.
- Requests/read/write direction denotes the caller/reader/writer's action; return data does not reverse the semantic relation.
- Filtering by relation kind keeps containment, deployment, and coverage out of dependency traversal.
- Unknown claims do not become confirmed dependencies. Inferred paths retain their qualification and conditions.
- A complete claim means complete within declared extraction scope, not proof of repository-wide coverage.
- A focus-only view cannot remove objects from the canonical snapshot.
- Missing objects in a replacement snapshot are removals from that model, not proof of software deletion.
- Stale expected revision, changed content under the same snapshot ID, or changed entity kind under the same ID is rejected.
- Exact idempotent retry returns the prior acknowledgement; it does not increment revision again.
- Failed shape, graph, or capability validation leaves the prior model/view usable.
- Unknown required semantic kinds are rejected, never silently rendered with a different meaning.

Add a TypeScript runner over the same shared cases before claiming cross-language compatibility. Publication/history and relationship-display assertions require their later implementation slices; they are not covered by the startup-only loader.
