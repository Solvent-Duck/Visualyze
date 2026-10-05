# Development environment

## First supported desktop

The initial visual proof targets **macOS on Apple Silicon**, using Bevy's Metal backend. Other desktop platforms have not been validated; the current feature selection is deliberately limited to this alpha target.

Use Rustup and Xcode command-line tools. The repository pins Rust **1.96.0**, Bevy **0.18.1**, Serde **1.0.228**, and serde_json **1.0.149**. `Cargo.lock` locks the full dependency graph. See [ADR 0003](decisions/0003-single-database-proof.md) for the compatibility result.

## Native workflow

There is no container workflow: this slice needs the host's native window system and GPU.

```sh
cargo run --locked
```

For validation, run the narrow model tests first, then the complete workspace:

```sh
cargo test -p visualyze-model --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

The model, camera math, and scene-construction tests do not open a window. A separate visual smoke check exercises the real renderer:

```sh
cargo run --locked --example render_smoke -- --model public/examples/objects.json
cargo run --locked --example connected_smoke -- --model public/examples/notes-crud.json
```

This ingests the explicit JSON file, opens a window, captures after a short shader warmup, checks a separate projected region for each of the seven semantic objects, saves `visualyze-objects.png` to the OS temporary directory, and exits. A person still needs to inspect the image for visual quality. A timeout, clipped specimen, or missing-object result fails the command.

The connected smoke check loads the notes design through the same file ingestion path, verifies all eight objects and nine non-structural connectors, checks framing and sampled GPU pixels, and saves `visualyze-notes-crud.png`. Inspect API attachment, separate read/write channels, policy targets, and label readability.

For manual gallery checks: confirm seven distinct labeled objects and no connection geometry, drag and arrow-key orbit, scroll and keyboard zoom, reset with `R`, and resize the window. Home framing scales with scene bounds and window aspect ratio. Labels follow their objects; zoom limits keep the camera outside the scene's bounding sphere.

## Deferred tooling and local data

The TypeScript/Node.js bridge has no manifest yet; its runtime, package manager, SDK, and IPC choice will be pinned when that slice starts. It is not required to render the database.

The runtime embeds its default synthetic object fixture and schema at compile time. `--model <path>` reads an explicitly supplied model file; `--model -` reads standard input. Add `--validate` for a headless input check. File and stdin payloads share the same 2 MiB bound, strict JSON parsing, JSON Schema validation, and graph checks as the embedded fixture.

`jsonschema` 0.58.2 is pinned with default features disabled; HTTP and filesystem reference resolution are not enabled. See [ADR 0004](decisions/0004-contract-ingestion-and-object-gallery.md). Schema/graph and CLI conformance run headlessly, including 76 shared cases in `public/conformance/architecture-cases.json`.

The runtime does not load a visualized repository, open evidence locations, or expose network access. The experimental placement view persists private accepted layout/view state under `~/Library/Application Support/Visualyze/layouts/`; gallery and legacy connected views do not. Developer smoke examples disable this persistence and write only temporary screenshots. See [ADR 0006](decisions/0006-placement-engine.md) and [placement validation](placement-validation.md) for the new headless checks, bundle benchmarks, and native placement smoke.
