# Visualyze

Visualyze is a local, read-only visual runtime for explaining software architecture through an interactive 3D assembly. A developer's existing coding agent supplies a public, structured architecture description; the local runtime renders and explores it.

The native simulator ingests the draft architecture contract and renders an object gallery with orbit, zoom, and reset controls. The default specimen contains one of each kind: scope, client, service, datastore, interface, concern, and deployment. Models with non-structural relationships automatically use a connected architecture view.

## Run the object simulator

On an Apple Silicon Mac with Rustup and Xcode command-line tools installed:

```sh
cargo run --locked
```

The pinned toolchain is selected automatically. The first build downloads and compiles Bevy and can take several minutes; later builds reuse those dependencies.

Drag with the left mouse button or use the arrow keys to orbit. Scroll or use `+` / `−` to zoom. Press `R` or `Home` to restore the initial view. Close the native window to quit.

The default scene uses an embedded, hand-authored [seven-object model](public/examples/objects.json). Each object is a plain rectangular box, differentiated primarily by kind color, with a label showing its kind, name, evidence basis, and boundary. Width, height, and depth are renderer-owned and adjustable; placement, connection routing, and rendered geometry share the same dimensions. Positioning is entirely programmatic, with no connection lines. The five structural records in this fixture establish logical containment and interface ownership for validation; they are not drawn.

Load a model file or pipe JSON from an LLM through the same ingestion path:

```sh
cargo run --locked -- --model public/examples/objects.json
cat public/examples/objects.json | cargo run --locked -- --model -
```

Validate a payload without opening a window or initializing the GPU:

```sh
cargo run --locked -- --validate --model public/examples/objects.json
```

Input passes through bounded JSON decoding, the embedded JSON Schema, graph validation, typed model construction, and scene creation. Invalid input exits with diagnostics before opening a window. The runtime reads only the explicitly supplied model; it does not inspect source references, scan a repository, connect to services, or start a listener. The exception is `--serve`: it accepts models and walkthroughs from local agents over a per-user Unix socket ([ADR 0008](docs/decisions/0008-agent-walkthroughs.md)).

## Run a connected CRUD design

```sh
cargo run --locked -- --model public/examples/notes-crud.json
```

The [personal notes model](public/examples/notes-crud.json) is a declared, synthetic design with a browser frontend, one application service, SQLite, two API interfaces, authentication, and per-user authorization. The one service handles both sessions and note operations; it is not a running notes web application.

Its eight objects and sixteen relationships use the existing contract unchanged. Five containment links form the system frame, two ownership links attach the APIs to the service, and nine connectors represent requests, reads, writes, policy coverage, and implementation. Authentication applies to the sign-in UI and Notes API; ownership checks apply to the Notes API. Neither concern claims coverage of the database.

The renderer computes all placement and orthogonal cuboid routes. Objects use the same plain boxes as the gallery; containment uses rectangular outlines to keep children visible, and interfaces attach to their owner's box face. Runtime arrows follow the predicate direction: `service reads datastore` points from the reader to the store. Dashed policy and implementation links are distinct from requests. There is no live traffic animation. Layout is an initial deterministic composition for small models, without general obstacle avoidance or label collision resolution.

A supplied `--model` uses the nested `composed` placement engine by default; pass `--layout legacy` for the original composition. The embedded specimen and `--view gallery` keep the legacy engine.

Use `--view gallery` to inspect disconnected specimens from any model, or `--view architecture` to force assembly even when a model only contains structural relationships. View preferences are CLI options, never payload coordinates.

## Try the experimental placement engine

```sh
cargo run --locked -- --model public/examples/sketch-crud.json --layout floorplan
cargo run --locked -- --bundle public/examples/data-messaging.bundle.jsonl
cargo run --locked -- --bundle public/examples/data-messaging.bundle.jsonl --layout composed
```

The new engine packs rectangular objects in coordinated layers, avoids solid obstacles, and validates every connector before showing the refined assembly. `--layout layered`, `constrained`, and `treemap` select comparison candidates. The existing renderer remains the default for snapshots while the new engine's release gates are incomplete.

`--layout composed` tries the nested placement mode: each expanded region has its own interior, fitted into a bounded parent box. Its saved arrangements are separate from the other engines. This mode remains experimental; see the [current validation and limitations](docs/placement-validation.md) before relying on it for saved overrides across every navigation pattern.

New interiors use their actual connections to neighboring regions to refine placement. The bounded search keeps layers, sizes and scale fixed, reroutes the full assembly, and accepts only validated improvements. If a connection is initially blocked, it can move eligible blockers sideways to open a passage. Navigation preserves accepted interiors; explicit reorganization can search them again. Unresolved routes retain the accepted scene and report the affected connection. Enlarging a region's routing space remains future work.

Each box's interior has its own readability budget: boxes inside an expanded box never count toward the view around it. The focused view opens its supplied subsystems one level down on its own, each one whose interior stays readable; the status line shows the focused view's visual noise (`noise 4.2`, or `noise 31 (dense)` when it is over the threshold you asked for) and, when an expanded interior is dense, the densest one (`| densest interior 31 (dense)`). Your choices always win: E expands or collapses any box, an automatically opened box collapses and stays collapsed, and nothing is refused for being noisy. Click a box to select and isolate it (an expanded box isolates with its contents; click empty space to clear), double-click to enter it, or use Tab to select and Enter to enter a region; dragging only orbits, Backspace clears isolation and then returns, E to expand/collapse, and Space to separate layers. J/K selects a supplied relationship; I isolates its path (or neighboring links when no path is selected), and Shift+I clears path selection. A line that stands for several relationships (parallel relations of one kind and aspect between the same displayed boxes, including those hidden inside collapsed regions) is drawn thicker, and Details lists them; with a path selected, B/N page through its member relations. X expands both ends of the selected path (leaf boxes and references to outside objects are skipped, with a message), so its members become separate lines; the set of relationships never changes, only how many lines show them. P toggles a pin; Shift+P opens the region where it was created; brackets adjust the selected object's layer; L cycles layout policies. O searches for a reorganization, V previews a proposal, A applies it, D dismisses it, and U restores the previous view. T/F/R select top/side/home views. Esc cancels indexing/refinement while retaining the accepted scene. Exact identities and original predicates appear in the text inspector.

Press `/` to search supplied names or IDs; choose a result with arrows and Enter. PageUp/PageDown move through child pages, and Shift+PageUp/PageDown move through relationship pages. Z toggles selected-region semantic zoom. C switches Auto, Logical, Code, and Deployment views; F5 reloads an explicitly supplied file only when its model identity matches and its revision increases. Failed or cancelled reloads preserve the accepted scene. Mark siblings with M, then G creates a visual group; Shift+G removes the selected group, Shift+M clears marks, and U undoes corrections. These visual groups preserve the supplied ownership and original relationship records.

Details starts collapsed; press F1 to open or close it. While it is open, hover over it to scroll with the wheel or PageUp/PageDown; Home/End reaches the beginning/end without moving the camera or changing model pages. Initial bundle views show a bounded overview; Details discloses partial regions and available versus unsupplied detail, with full names, relationship evidence, and status.

In composed mode, supplied-object pins follow their region through parent and child views. A pin created inside a database fixes its local position there while the database can still move or scale within its parent. Inherited pins can be edited in their originating region; select one and press Shift+P to open it. Collapsed interiors retain their pins for reopening. Pins also retain enclosing manual or suggested visual groups, including after entering a child or restarting with empty caches. Reference pins stay with the region where they were created and remain dormant on other relationship pages; they never pin the real target or another region’s reference. Changing a protected group or its supplied ownership reports a conflict and preserves the accepted scene. Unpin in its originating region before changing that group.

In the bundle example, select the database and press E to reveal its tables while neighboring services stay fixed. If an expansion cannot fit around existing constraints, the accepted view stays visible with a diagnostic; entering the selected region provides a separate inspection frame.

Typed bundles add tables/columns, code units, asynchronous messaging, and deployment instances without changing the existing snapshot contract. Bundles are indexed in the background and viewed in bounded regions. Descendant relationships map to visible ancestors with inspectable original membership; bounded query pages and partial detail are disclosed. Models stay local, and referenced source paths are never opened by the viewer.

For a real production-sized source inventory, the separate [Rust source producer](public/tools/rust-bundle/README.md) ingests an explicitly supplied checkout without executing it. The pinned Bevy example produced 27,117 objects and 44,019 relationships and has been displayed in the native viewer. The [declared distributed AI fixture](private/layout/fixtures/distributed-ai.bundle.jsonl) exercises messaging, shared services, tables and regional instances; it is not a reconstruction of any company's stack.

Accepted layouts, pins, and view preferences are stored privately under `~/Library/Application Support/Visualyze/layouts/` on macOS. Saved layouts are validated against the supplied model before display. Dismissed reorganizations remain dismissed across restart and harmless reload; changed arrangements or relationship meanings can produce a new proposal. Read [implementation details](docs/decisions/0006-placement-engine.md), [measured results and remaining gates](docs/placement-validation.md), [the approved-plan acceptance ledger](docs/placement-requirements.md), and [the bundle contract](public/schema/model-bundle.md).

## Validate

```sh
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run --locked --example render_smoke -- --model public/examples/objects.json
cargo run --locked --example connected_smoke -- --model public/examples/notes-crud.json
```

The last two commands need a graphical desktop. The gallery check verifies a separate screen region for every object and saves `visualyze-objects.png`. The connected check verifies eight objects and nine routed relationships, samples their rendered geometry, and saves `visualyze-notes-crud.png`. Both write to the OS temporary directory and exit. Inspect the images for shape, labels, and routing; these are not a full visual regression suite.

## Product constraints

- The product must not read from or write to the visualized repository.
- Architecture models and view state stay local by default.
- The public semantic contract remains separate from the proprietary rendering runtime.
- Agent integrations express semantic intent rather than meshes, shaders, or arbitrary scene instructions.

## Repository map

| Location | Purpose | Current state |
| --- | --- | --- |
| [`spatial-codebase-visualizer-spec(1).md`](./spatial-codebase-visualizer-spec(1).md) | Consolidated product and implementation specification | Source of truth for the initial direction |
| [`docs/`](./docs/) | Architecture decisions and development guidance | Initialized |
| [`public/`](./public/) | Renderer-independent semantic types and synthetic fixtures | Draft schema, Rust snapshot validator, shared conformance cases, and object fixtures |
| [`private/layout/`](./private/layout/) | Proprietary headless placement | Projection, deterministic candidates, geometry/routing validation, repairs, and local state |
| [`private/runtime/`](./private/runtime/) | Proprietary native runtime | File/stdin ingestion, object gallery, connected architecture composition, and camera controls |

## Beginning development

Read [`docs/development-environment.md`](./docs/development-environment.md) and [`docs/decisions/0001-repository-boundaries.md`](./docs/decisions/0001-repository-boundaries.md) before changing the implementation. [`ADR 0003`](./docs/decisions/0003-single-database-proof.md) records this slice and its dependency choices.

See [ADR 0004](docs/decisions/0004-contract-ingestion-and-object-gallery.md) for the ingestion and object-gallery slice. [ADR 0005](docs/decisions/0005-connected-crud-design.md) adds the first connected assembly. [ADR 0006](docs/decisions/0006-placement-engine.md) adds the experimental headless placement engine, bounded bundle views, inspection, and private persistence. [ADR 0008](docs/decisions/0008-agent-walkthroughs.md) adds the MCP bridge (`visualyze mcp`), serve mode, and agent-narrated walkthroughs. Production comprehension gates and packaging remain incomplete.

The draft agent-to-renderer format is documented in [the architecture contract](public/schema/architecture-format.md). Single-snapshot schema and graph validation now run in the native ingestion path; walkthroughs have their own [format](public/schema/walkthrough-format.md), and serve mode enforces forward-only revisions.
