# Private runtime workspace

This area is reserved for the proprietary local runtime and its support artifacts.

Planned contents:

- `runtime/` — native application, semantic-to-spatial composition, interaction, camera, IPC, and local persistence
- `assets/` — owned visual assets
- `visual-fixtures/` — render and interaction comparison fixtures
- `packaging/` — local desktop packaging and installation support

`runtime/` now contains contract ingestion, an object gallery, and a connected architecture view. Objects are rounded boxes, differentiated primarily by kind color (datastores are stacked platters, clients carry a title bar), with glass containers, labels, lighting, and camera controls. Kind and relationship colors render exactly (no tonemapping); only over-bright highlights bloom. Run it with `cargo run --locked` from the repository root. Use `-- --model <path|->` to supply JSON, or add `--validate` to check input without a window.

The runtime consumes only the public validated architecture wrapper; geometry, materials, placement, and camera behavior stay private. Every input object retains its ID on one semantic scene root. A single box uses the layout's width, height, and depth directly; connected containment uses an outlined box so children remain visible. Default launch uses an embedded fixture and works independently of the working directory. Explicit input reads only the requested model file or standard input, never evidence paths or source repositories. The gallery hides relationships; models with non-structural relationships automatically use the connected view.

`layout/` now contains the headless experimental placement engine and comparison harness, separate from Bevy. The native viewer can opt into it with `--layout floorplan` or consume a typed local `--bundle`. Read [ADR 0006](../docs/decisions/0006-placement-engine.md) for private state locations and limits, and [validation status](../docs/placement-validation.md) before promoting it to the default.
