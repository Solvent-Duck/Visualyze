# ADR 0003: Start Phase 0 with one database cuboid

**Status:** Accepted and verified  
**Date:** 2026-09-29

## Scope

The first implementation request narrows Phase 0 to one successfully rendered database rectangle. Build a native window, typed synthetic fixture loading, exactly one rectangular solid, lighting, and basic orbit/zoom/reset controls. The wider Phase 0 grammar in ADR 0002 remains future work.

Do not add services, relationships, database internals, floors, grids, selection, repository ingestion, persistence, MCP, or a hosted service in this slice. The small text overlay identifies the declared example and explains the controls.

## Dependency decision

| Dependency | Exact selection | Reason |
| --- | --- | --- |
| Rust | 1.96.0 | Installed stable toolchain; compatible with the selected engine |
| Bevy | 0.18.1 | Stable patch release selected for this bounded compatibility proof |
| Serde | 1.0.228 | Typed semantic deserialization |
| serde_json | 1.0.149 | Hand-authored JSON fixture loading |

Use the pinned Bevy version rather than automatically following the latest engine release. Enable native window/input, 3D rendering, text/UI, and PNG capture features explicitly. Omit audio, gamepads, asset importers, external camera plugins, and network asset support. The workspace lockfile records transitive versions.

Primary references: [Bevy 0.18.1 feature flags](https://docs.rs/crate/bevy/0.18.1/features) and [Bevy's official examples](https://github.com/bevyengine/bevy/tree/v0.18.1/examples). The implementation uses built-in cuboids and ordinary materials; no custom shader or third-party camera code is incorporated.

## Boundaries

`public/types/rust` contains a deliberately experimental database fixture type. It carries identity, label, kind, and declared evidence basis; it imports no engine types and accepts no geometry. This is not a replacement for the full versioned architecture contract.

`private/runtime` owns scene composition, shape, material, camera, and native UI. Each meaningful rendered object retains its semantic ID. The fixture is embedded during compilation, so the running viewer does not read the repository or require external assets. It has no listener, source scanner, or snapshot persistence.

Initial platform: Apple Silicon macOS, with the Metal graphics backend. Node.js/MCP choices are deferred until agent integration is actually implemented.

## Compatibility result

Verified on macOS 26.6.2, Apple M5 (Metal), with the pinned Rust 1.96.0 toolchain:

- All five workspace tests pass: fixture acceptance/rejection, camera orientation and limits, and scene construction with exactly one database mesh and one camera.
- Formatting and Clippy checks pass for all workspace targets.
- The native GPU smoke example exits successfully and its screenshot shows one centered cuboid with three legible shaded faces on a dark background.

The sandboxed process could not discover the GPU; the same smoke command passed with native desktop/GPU access. This is an execution-environment requirement, not a fallback renderer.

Cargo reports a future-Rust compatibility warning in the transitive `block 0.1.6` dependency. It does not fail this pinned toolchain; revisit it when upgrading the engine/toolchain. No other desktop platform or performance budget is claimed by this proof.
