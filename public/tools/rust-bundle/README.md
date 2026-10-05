# Explicit Rust source bundle producer

`visualyze-rust-bundle` is an offline producer for a **supplied, immutable Rust
checkout**. It emits the existing `visualyze.bundle.v1` contract and validates the
entire result through the same model provider used by the viewer. It does not run
Cargo, build scripts, procedural macros, source code, or commands found in source
files. The viewer still consumes models; it does not discover repositories.

The producer reads Cargo manifests with `toml_edit` and Rust syntax with `syn`;
`quote` exposes parsed token structure for the optional syntax identity scheme.
These development-tool dependencies are already pinned in the workspace lockfile;
they are not dependencies of the shipped native viewer or layout engine.

## Reproduce the Bevy source model

The verified source is the official Bevy **v0.18.1** commit
[`f667c282dad2c1419afb5836ded22a3ec263970e`](https://github.com/bevyengine/bevy/tree/f667c282dad2c1419afb5836ded22a3ec263970e).
Use a new clone destination and a new output filename. The producer never
overwrites an existing bundle.

```sh
git clone --depth 1 --branch v0.18.1 https://github.com/bevyengine/bevy.git /private/tmp/visualyze-bevy-production
git -C /private/tmp/visualyze-bevy-production rev-parse HEAD
git -C /private/tmp/visualyze-bevy-production status --porcelain
mkdir -p visualyze-local/production
cargo run -p visualyze-rust-bundle --locked -- \
  --root /private/tmp/visualyze-bevy-production \
  --output visualyze-local/production/bevy-0.18.1.bundle.jsonl \
  --revision f667c282dad2c1419afb5836ded22a3ec263970e \
  --repository https://github.com/bevyengine/bevy \
  --name 'Bevy 0.18.1' \
  > visualyze-local/production/bevy-0.18.1.report.json
cargo run --locked -- --bundle visualyze-local/production/bevy-0.18.1.bundle.jsonl --validate
cargo run --locked -- --bundle visualyze-local/production/bevy-0.18.1.bundle.jsonl
```

Check the full commit matches the pinned value and that Git status is empty
before producing the model. **The CLI receives provenance from the caller; it
does not independently prove the checkout matches that revision.** Evidence
links include the full supplied revision and actual source path/line. An immutable
verified checkout is therefore a precondition for publishing those links.

The generated JSONL and report live in ignored local data. No upstream source or
generated bulk model is added to the repository.

## What is represented

| Input fact | Bundle representation |
|---|---|
| Supplied inventory boundary | `scope` containing one `code_repository` |
| Each Cargo package manifest | `code_package`, owned by the repository |
| Each `.rs` file | `code_file`, owned by its nearest manifest directory |
| Struct, enum, union, trait, type alias | `code_type` with source-line evidence |
| Function and method declarations | `code_function`; same-file methods belong to an unambiguous local type |
| Manifest path dependency on another supplied package | Supported `code_references`, with normal/dev/build/target category in the label |
| A syntactic `use` declaration | Inferred `code_references` to a supplied dependency package or conventional source module file; original import in the label |

Inline modules are represented in qualified labels. A method's owner falls back
to its file when the local type is ambiguous or declared elsewhere. Every
original source relationship retains its evidence and basis; inferred imports
are distinguishable from supported manifest declarations. No request, runtime
call, traffic, or service deployment claim is generated from syntax.

IDs are deterministic hashes of source identity keys, with collision rejection.
Package and file IDs depend on their relative paths, independently of publication
identity, revision, or identity scheme. The default `legacy-v1` item keys include
source line and column; its import claim keys include source line. In this scheme,
inserting lines before an item changes its identity across commits even when the
model identity remains stable. Opt-in `syntax-v2` addresses ordinary source edits,
with explicit migration and ambiguity limits described below. Evidence, entities, and relationships are
serialized in canonical ID order. Semantic records are accumulated under a
bounded producer budget and streamed to the bundle writer; the viewer then uses
its indexed, demand-loaded model provider.

## Publish increasing model revisions

By default, the model ID includes the supplied source commit and the numeric model
revision is `1`. Keeping the default `legacy-v1` scheme preserves the original
bundle bytes and IDs. To publish
successive source commits as revisions of one model, supply both optional flags:

```text
--model-id code:bevy --model-revision 2
```

Keep that model ID unchanged and increase the positive numeric model revision for
each publication. Continue supplying `--revision FULL_GIT_SHA` for the independently
verified source checkout: it controls source evidence, not the publication counter.
The JSON report records both `source_revision` and `model_revision`. Model IDs must
contain 1–128 lowercase ASCII letters, digits, `.`, `_`, `:`, or `-`, starting with
a letter. Both flags are required together; zero revisions are rejected.

The producer is stateless and cannot prove the counter increases over an earlier
publication. The publisher manages that sequence and uses a new output filename;
the producer never replaces a live bundle. Viewer revision checks remain responsible
for rejecting stale updates. Package/file paths remain stable across publications.
Declaration and import stability depends on the selected scheme.

## Opt in to stable syntax identities

For a **new** model intended to receive source updates, use:

```text
--identity-scheme syntax-v2 --model-id code:bevy-syntax-v2 --model-revision 1
```

Keep both the model ID and identity scheme unchanged for subsequent publications,
and increment the numeric revision. The report records the explicit scheme version
as `identity_scheme: "legacy-v1"` or `"syntax-v2"`. A syntax-v2 bundle also records
its scheme and limitations in manifest omissions. Without a supplied model ID,
syntax-v2 includes the scheme in the generated model key, so it cannot collide
with the corresponding default legacy model identity.

**Switching schemes requires a new model identity.** Declaration IDs and import
relationship IDs change at this boundary, along with ownership relationships whose
endpoints change. Package/file IDs stay compatible. The producer does not migrate
saved layouts, pins, selections, or grouping IDs. It is stateless and cannot verify
that an explicitly supplied model ID was not previously published with another
scheme; the publisher must choose a new ID when switching. The existing measured
Bevy bundle and its navigation IDs below remain legacy-v1 and are unchanged.

Syntax-v2 declaration anchors use the relative source path, declaration kind,
qualified module/name, lexical `cfg`/`cfg_attr` ancestry, and, for methods, the
full written self type and implemented trait path. Source positions, ordinary
documentation/attributes, declaration order, signatures and bodies are excluded.
Comments, whitespace, ordinary unrelated declaration insertions/reordering, and
body changes therefore preserve IDs. Import anchors use the inline module,
conditional ancestry, written import path, leading `::`, and rename alias;
reordering a grouped `use` preserves each claim's identity. Different aliases
and conditional declarations stay distinct. Evidence links and descriptions
still record the current source lines and supplied commit.

This is lexical identity, not compiler symbol resolution. Renaming or moving a
declaration, moving a file, changing conditional ancestry, or changing an impl's
self/trait syntax (including generic parameter spelling) can change IDs. Changing
only a declaration signature intentionally retains identity. Equivalent conditional
expressions and literal spellings are not normalized semantically. A conditional
type with several same-named declarations can remain an ambiguous method owner;
the existing conservative fallback to file ownership still applies.

Two declarations/imports with identical anchors are rejected with their file and
line locations **before an output bundle is created**, even if their bodies differ
or an import is unresolved. This includes duplicate same-name methods whose only
distinction is an omitted where-clause condition. There is no position fallback,
body hash, arbitrary ordinal, or silent merge. Such source needs a more expressive
future producer identity scheme or explicit producer-supplied identities; syntax-v2
does not claim to distinguish every legal Rust program. Syntax failures retain the
separate partial-inventory behavior described below.

## Coverage and safety limits

The manifest is deliberately `partial`. It includes test/example code and
conditional source declarations without feature or target evaluation. It is a
source inventory, not the set of compiler-reachable items for a particular build.

External dependencies, unresolved imports, Cargo self-dependencies, unowned source
files, and syntax failures are reported explicitly. A parse failure retains the
file record but creates no declarations for it. Imports are not compiler-resolved:
reexports, `#[path]`, generated `include!`, renamed library targets, and ambiguous
module paths can prevent exact resolution. Block-local items, constants, static
variables, fields, macro expansions, generated source, runtime behavior, and
deployment details are outside this profile.

The producer reads only the explicitly supplied source tree. It skips `.git`,
`target`, `node_modules`, and local viewer data. Source-file symlinks are rejected;
non-source symlinks are recorded and never followed. It rejects a symlink root,
escaping paths, changed files detected during reads, files larger than 8 MiB,
more than 100,000 source/manifest files, more than 1 GiB of source, nesting beyond
64 directories, and more than 2,000,000 generated identities. Use a quiescent
checkout: these checks are not a filesystem sandbox against an adversarial
process concurrently replacing ancestor directories. Output creation is
exclusive and the full resulting model must pass provider validation.

## Measured production inventory

The pinned Bevy run ingested all **1,622 tracked Rust files**, with **zero syntax
failures**, across **81 packages**. It produced **25,412 declarations**, **27,117
entities**, **44,019 relationships**, and **31,319 evidence records** in a
**40,169,426-byte** bundle. Relationships comprise 27,116 ownership/containment
records, 596 manifest dependency references, and 16,307 inferred import references.

The producer read **18,439,636 source/manifest bytes**. A second independent
production run generated a byte-identical bundle with SHA-256
`113c806186aacd4e5d67b24f6f1c9979b45d7ea4aae0f8eea5933cb07c767c36`.

The report also records 8,407 unresolved imports, 464 omitted external dependency
declarations, one omitted Cargo self-dependency, and one skipped assets symlink.
These are declared coverage limits, not invented replacement edges.

Representative navigation IDs for this **legacy-v1** producer run and source revision:

| Region | Semantic ID |
|---|---|
| Root inventory scope | `scope:05317a01f80959f2` |
| Bevy repository / package overview | `repository:05317a01f80959f2` |
| `bevy_ecs` package | `package:d1c2531f3a0a9b57` |
| `crates/bevy_ecs/src/world/mod.rs` | `file:b6ae9420da8c77aa` |
| `World` | `item:86c82b85a551c18a` |
| `World::spawn` | `item:a6ee753440d7d417` |

Producer tests cover a nested, real Rust workspace; ownership and typed reference
meaning; inferred import basis; deterministic output; non-overwrite behavior;
malformed source; source and assets symlinks; self-dependency reporting; input
size limits; pinned-provenance validation; paired publication flags; and distinct
source provenance with increasing model revisions and stable package/file IDs.
Syntax-v2 tests additionally cover edited-source identity continuity, current-line
evidence, impl/trait/conditional distinctions, import aliases, ambiguous-anchor
rejection, scheme migration boundaries, CLI reporting, and a pre-change legacy
bundle byte fingerprint:

```sh
cargo test -p visualyze-rust-bundle --locked
cargo clippy -p visualyze-rust-bundle --all-targets --locked -- -D warnings
```

Rendering and layout measurements are tracked separately in
[`docs/placement-validation.md`](../../../docs/placement-validation.md). Ingestion
success alone does not establish display quality, performance, or human
comprehension.
