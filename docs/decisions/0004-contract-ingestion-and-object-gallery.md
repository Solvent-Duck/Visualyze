# ADR 0004: Ingest the draft contract and preview every object kind

**Status:** Implemented  
**Date:** 2026-09-29

## Scope

The next requested slice tests the architecture contract by instantiating one object of every kind without rendering connections. It supersedes the single-database-only implementation scope of ADR 0003, while preserving its native rendering stack and public/private boundary.

## Decision

Add a public, immutable validated architecture wrapper. Its ingestion path is bounded bytes → strict JSON decoding → the embedded draft JSON Schema → typed decoding → semantic graph validation. Reject duplicate JSON keys, oversized or excessively nested input, unsupported semantics, unresolved or duplicate IDs, invalid endpoints, missing interface owners, invalid source locators, and containment/deployment cycles. Runtime dependency cycles remain legal. Bound diagnostic counts, and return paths/codes for correction.

Use the existing JSON Schema as the structural authority instead of maintaining independent handwritten field validation. Pin `jsonschema` 0.58.2 with default features disabled and update `Cargo.lock`. This is the only new direct production dependency. The schema and local references are embedded; HTTP/file reference resolution is disabled. Source/evidence data is never dereferenced. Serde types remain engine-independent.

Provide explicit model-file and stdin entry points plus headless `--validate`. The default specimen is embedded, so ordinary launch remains independent of the working directory. Input failure occurs before window creation. No network listener, repository scanner, watcher, IPC bridge, MCP tool, persistent store, or live replacement protocol is introduced.

The specimen contains exactly seven entities. Four `contains` records connect internal logical entities to its system root, and one `exposes` record gives the interface its required owner. These are necessary contract facts, not drawn connections. There are no runtime interaction, policy-coverage, or deployment-placement links in the specimen.

Render an explicitly labeled **object gallery**, not a proposed architecture assembly. A deterministic grid based on kind and ID allows inspection of each construction; JSON array order cannot affect placement. Scopes use open frames, clients faces with a base, services bounded compute blocks, datastores layered blocks, interfaces protruding plates, concerns thin panes, and deployments open chambers. All geometry is cuboid-based and renderer-owned. Each entity has one semantic root; geometry is attached as child fragments. The interface is displayed as a detached specimen for this slice, not as a claim about an ownerless interface.

Fit the camera from scene bounds and window aspect ratio. Display object labels, basis, and boundary; use an explicit note that relationships are hidden. All supplied relations are validated and retained even though this renderer slice does not draw them.

## Validation and limits

- Shared JSON mutation cases exercise schema and graph acceptance/rejection in Rust; additional tests cover bounded reads, malformed JSON, duplicate keys, numeric compatibility, and diagnostics.
- Headless CLI tests exercise both file and stdin ingestion.
- Headless scene tests run the same decoder and assert seven roots, seven kinds, all cuboid fragments owned, labels, camera, and order-independent placement.
- The native smoke example loads the specimen from a file and checks visible geometry in a separate projected region for every object, then saves a temporary screenshot for inspection.

The contract remains a draft. This implements validation of individual snapshots, not publication history, expected-revision synchronization, or cross-language conformance. Object forms are initial generic constructions; semantic assembly, relationships, selection, richer subtype treatment, and agent integration remain later slices. Rendering declared synthetic objects does not verify claims about a real repository.

Verified on the native macOS development host: all 16 workspace tests pass (including the 76 shared contract cases), formatting and all-target Clippy pass, and the file-driven GPU smoke exits successfully. The captured image was inspected for all seven objects, readable labels, distinct constructions, and absence of connection geometry. The previously documented transitive `block` future-Rust warning remains unchanged.
