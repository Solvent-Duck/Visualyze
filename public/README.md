# Public contract workspace

This area will contain the versioned, compatibility-oriented semantic interface for architecture models and agent control.

Planned contents:

- `schema/` — architecture, view-state, and command JSON Schemas
- `types/` — Rust and TypeScript bindings that mirror the public contract
- `validation/` — schema and semantic validators
- `mcp-server/` — local stdio bridge
- `skills/` — shared and host-specific agent instructions
- `examples/` — synthetic valid models and command examples
- `conformance/` — shared acceptance/rejection fixtures

The initial implementation includes `types/rust` (`visualyze-model`) and `examples/database.json`. They describe and validate one declared datastore using renderer-independent data. Unknown kinds, unknown fields, malformed JSON, and blank identity/label values are rejected.

These are experimental Phase 0 fixture types and remain separate from the proposed full architecture contract. No geometry or Bevy types belong here.

## Draft architecture contract

The proposed `0.1.0-draft.1` format is a typed graph with seven entity kinds and eleven relation kinds. The agent supplies semantics; the renderer owns construction and positioning.

- [Format and graph rules](schema/architecture-format.md)
- [JSON Schema](schema/architecture.schema.json)
- [Minimal model](examples/architecture-minimal.json)
- [Worked architecture model](examples/architecture.json)
- [Conformance checklist](conformance/architecture-contract.md)

`visualyze-model::architecture::Architecture` now loads this envelope through a bounded reader, the embedded JSON Schema, and semantic graph validation. The resulting model is immutable and preserves the original snapshot input boundary. The validator performs no source-file or network access.

[objects.json](examples/objects.json) supplies one of each kind for the simulator. It contains only the containment and interface-ownership records required to make that model valid; the gallery draws no relationships. [Shared conformance cases](conformance/architecture-cases.json) run as part of `cargo test -p visualyze-model --locked`.

The walkthrough format ([walkthrough-format.md](schema/walkthrough-format.md)), the MCP bridge ([mcp-server](mcp-server/README.md)), and the agent skills are implemented (ADR 0008). The bridge is Rust and reuses the Rust validator, so TypeScript bindings are no longer on the bridge's path. The original database-only type and fixture remain historical examples; they are not accepted as full snapshots by the new loader.

## Versioned local bundles

The additive [bundle contract](schema/model-bundle.md) supports explicit data, code, messaging, and deployment-instance profiles with revision-aware bounded queries. [The mixed example](examples/data-messaging.bundle.jsonl) preserves table ownership, asynchronous predicates, and instance identity. Existing snapshot fixtures and limits remain unchanged. Geometry and private view state are excluded from both formats.
