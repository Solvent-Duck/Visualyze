# Project Specification: Agent-Driven Spatial Codebase Visualizer

**Version:** 0.1  
**Date:** 2026-09-29  
**Product name:** Undecided  
**Status:** Consolidated product direction and initial implementation specification

> A local, read-only visual runtime that lets a developer's existing AI coding agent explain software through a fluent, consistent, interactive 3D language.

This document records the current conclusions. Earlier ideas that were superseded are explicitly excluded. Concrete interface names and implementation details below are proposed defaults, not an already implemented API.

---

## 1. Product definition

The product is **visual learning for codebases**: architectural understanding, explanations of flows, and understanding how a system evolves.

It is not a coding agent, repository-hosting service, observability platform, or general-purpose diagram editor. It does not modify the software being visualized.

The user brings their existing coding agent, initially Claude Code or Codex. That agent inspects the repository, interprets it, and emits a standardized, structured description. Our software turns that description into a spatial model and exposes tools through which the agent can teach with it.

The division of responsibility is:

**The user's agent owns repository understanding. Our runtime owns representation.**

The intended business is a monetizable developer side tool. It does not need venture-scale adoption to justify itself. Initial users include developers trying to understand agent-generated software, consultants entering unfamiliar systems, and engineers learning or explaining an existing architecture.

### The three product pillars

| Pillar | User question | Product behavior |
|---|---|---|
| Architectural understanding | “What is this system, and how do its parts fit together?” | Present a coherent, explorable assembly with meaningful containment, layering, interfaces, and cross-cutting structures. |
| Flow explanation | “How does auth work?” or “How does CSV input become ontology data?” | Isolate the relevant architectural slice, preserve its surrounding context, and guide the user through supported relationships. |
| Evolution over time | “How did this architecture change?” | Compare snapshots while preserving component identity and spatial continuity. |

**MVP priority:** prove architectural comprehension first. Support a minimal agent-guided explanation using those same architectural objects. Detailed transformations, runtime behavior, and a full history experience follow later.

### The premium proposition

Customers pay for three things: **functional visual quality, consistency across repositories, and human/agent co-legibility**. Predictable rendering and honest treatment of uncertainty support all three.

The quality target is an interactive explanation that feels deliberately composed by a strong technical animator—not an arbitrary graph made attractive by lighting.

---

## 2. Hard product boundaries

| Decision | Requirement |
|---|---|
| Read-only comprehension | No source editing, refactoring, patches, commits, pull requests, deployments, or “fix this” actions. Changing a view changes only the visualization. |
| User-provided intelligence | Use the user's coding-agent session. Do not operate a product-owned inference service or require a second model API subscription. |
| No repository intake | No ZIP upload, GitHub OAuth, Git credentials, or repository cloning by our product in the MVP. |
| Local-first runtime | Render and store architecture descriptions locally. A hosted rendering service is not required. |
| Public semantic input | The architecture format, types, validation contract, and agent-facing interfaces are public. |
| Proprietary execution quality | Keep the production renderer and detailed spatial-composition implementation proprietary. |
| Architecture-first scope | Begin with language-independent architectural concepts, not classes, functions, variables, or arbitrary source-level execution. |
| Actual 3D | Spatial composition must communicate information. This is not a flat workflow canvas tilted into perspective. |

“Read-only” applies to the repository. Saving local visualization snapshots, preferences, and view state is allowed. Store those outside the repository by default.

Our extension must not expose repository-write capabilities. The user's general-purpose coding agent may possess unrelated write tools; use the host's applicable read-only permissions where available rather than claiming that a prompt can revoke those tools.

---

## 3. Design philosophy

### 3.1 The core metaphor: inspect an assembly, not a city

The model should feel like **pulling apart a clock, exploring a fidget cube, or inspecting a compact precision assembly**.

It should not feel like looking down on a city, navigating a galaxy of nodes, or following a left-to-right conveyor belt. The clock analogy concerns interdependence and inspectability—not literal gears, screws, bolts, or mechanical decoration.

The object is compact, layered, and composed. Neighboring surfaces, nested volumes, shared panes, and connected chambers reveal structure. Exploding or unfolding it exposes relationships that were already present; it does not replace the architecture with another diagram.

### 3.2 Position does at least as much work as text

The intended hierarchy of communication is:

**Position and spatial relationships → form and construction → interaction and motion → color → text.**

A user who has learned the visual language should recognize peers, containment, shared capabilities, mediation, and dependency relationships before reading a paragraph.

Text remains available for exact identity, technical qualifications, evidence, ambiguity, search, and accessibility. “Text as a last resort” means not relying on labels to compensate for meaningless composition; it does not mean removing essential names or an accessible textual representation.

**Design test:** with most labels hidden, can a familiar user still understand the important structural relationships? Exact service names are not expected to be inferable from geometry.

### 3.3 Everything is constructed from cuboids

**Current prototype simplification:** Use one plain rectangular box per object, with color as the primary kind differentiator and labels retaining exact identity. Width, height, and depth may vary to support placement. Connected containment uses a rectangular outline so children remain visible. Geometry and routing consume the same renderer-owned bounds. Defer decorative constructions and internal subdivisions while developing the placement algorithm; the richer vocabulary below remains a future design direction.

Use rectangular solids as the geometric vocabulary: blocks, plates, panes, pillars, frames, chambers, and rectangular channels.

Do not use circular nodes, spheres, cylinders, database barrels, discs, rings, or rounded transport tubes. Databases are rectangular assemblies too. Small edge bevels may improve readability without changing the rectangular silhouette.

Different concepts are distinguished by construction, proportion, internal subdivision, surface treatment, attachment, and placement—not by borrowing a different primitive shape for each type.

### 3.4 Restrained, not maximalist

The visual direction combines a calm productivity-tool interface with subtle holographic depth. Use dark neutral surfaces, clear silhouettes, modest translucency, and controlled blue/teal/violet accents. Focus and active explanations receive emphasis; everything should not glow simultaneously.

Avoid decorative hardware details, excessive grids, bloom, particles, rainbow coding, dense labels, and dashboard clutter. The previous mechanical mockups established the assembly direction but were too maximalist to be final targets.

The desired result is **mechanical clarity**, not cyberpunk spectacle.

---

## 4. Spatial grammar: architectural role determines representation

### 4.1 Do not freeze universal meanings onto all three axes

We considered front-to-back “book” depth and top-down “archaeology” depth. Neither becomes a mandatory global layout for every repository.

The renderer selects a coherent composition based on the architecture. A frontend may be an upper or outer face. A cross-cutting access-control concern may be a pane or pillar spanning several layers. Peer services may occupy neighboring chambers.

Consistency means **the same relationship is expressed through the same visual grammar**, not that every component with the same name occupies the same coordinate in every system.

Within an individual model, maintain a stable home orientation and preserve relative positions during exploration. Do not rotate the meaning of a view silently.

### 4.2 Initial role-to-form vocabulary

| Architectural role | Candidate spatial representation | Intended reading |
|---|---|---|
| Independently meaningful service | Bounded cuboid or chamber | A distinct runtime responsibility. |
| Peer responsibilities | Adjacent blocks within a common assembly | Comparable architectural level or shared domain. |
| Logical subsystem | Rectangular grouping frame containing components | These responsibilities belong together. |
| Cross-cutting concern | Spanning pane, pillar, wall, or spine | This concern applies to multiple explicitly identified components or layers. |
| Shared substrate | Supporting slab or attached foundation | Several components rely on this capability. |
| Gateway or mediator | Interface plate, bridge, or passage between relevant regions | Communication is mediated here. |
| Datastore | Cuboid with consistent internal slab or cell construction | Persistent state, with subtype expressed by internal structure. |
| Interface/API | Addressable port or face on the component that exposes it | A boundary through which interaction occurs. |
| External dependency | Attached but clearly separated cuboid outside the modeled application boundary | This dependency is external to the defined system. |
| Queue, when supported | Rectangular buffer with ordered internal cells | Buffered asynchronous exchange, not an ordinary synchronous call. |

These are grammar rules to refine through tests, not permission to invent facts. A generic block is preferable to a falsely expressive shape when the input is ambiguous.

### 4.3 Auth is the reference test for cross-cutting representation

Auth must not automatically become “another box on the same floor.” When the supplied model shows an access-control concern spanning the frontend, backend, data access, and billing entitlements, a pane or spine can communicate that span better than many long edges.

The renderer must show **where it applies and where it does not**. Only affected surfaces should visibly engage with the concern.

Distinguish authentication, authorization, service identity, database access control, and billing entitlements in the semantic model. Do not infer that a single auth service controls all of them just because their names sound related.

A logical concern and its implementing service are not necessarily the same object. A spanning concern may be implemented by several components. Conversely, a centralized auth service remains an actual service even when its broader concern is displayed as a pane.

### 4.4 Relationship semantics must remain distinct

Containment, deployment, policy coverage, and runtime communication must not collapse into one generic “connected” relation.

A service inside a logical subsystem is not necessarily inside the same operating-system process or Docker container. A component covered by an access-control pane is not necessarily making a network call to that pane. A database positioned beneath a service is not proof that the service owns it.

Use explicitly typed relationships and distinguishable construction treatments. Adjacency alone must not falsely imply a call. Decorative gaps and spacing carry no additional architectural claim.

### 4.5 Connections and motion

Route connections through restrained orthogonal channels with rectangular cross-sections and identifiable ports. Prefer local attachments and structured routes to loose wires through space.

Show only the relationships required by the current abstraction and question. A bundled channel must disclose that it represents multiple relationships and expand on inspection.

Animation explains an existing relationship: an interface lights up, a pane lifts, an internal layer separates, or a small cuboid follows a supported route. Animation must not fabricate timing, throughput, execution order, or successful security enforcement.

### 4.6 Stability and semantic zoom

The same component retains its identity across overview, focused view, expanded view, and later snapshots. Camera changes and exploded-view offsets are presentation changes, not changes to architectural facts.

Expansion reveals the interior of the existing object. Collapse restores it. Filtering dims or hides context without automatically repacking the whole scene. Explicit layout changes remain possible, but they should be understandable and reversible.

---

## 5. MVP scope and semantic vocabulary

The smallest useful product is **services, a couple of datastore variants, interfaces, boundaries, relationships, and one excellent cross-cutting-concern representation**.

### Included in the first vertical slice

| Semantic concept | MVP treatment |
|---|---|
| Client/frontend | A recognizable outer-facing architectural component. |
| Service | One generic bounded compute component; language is optional metadata. |
| Datastore | Relational and graph-store examples using rectangular constructions. Unknown subtype falls back to a generic datastore. |
| API/interface | A port on its owner. An independently deployed API gateway remains a service with a gateway role. |
| Scope/boundary | Explicit logical grouping; deployment and external boundaries use separate metadata and treatments. |
| Cross-cutting concern | A logical entity with explicit coverage and optional implementation references. Access control is the first reference case. |
| Relationships | Requests/calls, reads, writes, coverage, and dependency. Containment is represented separately. |

Queues, caches, workers, scheduled jobs, object stores, and event buses are natural next additions. They should not become a prerequisite for proving the renderer. Do not infer a cache merely from a Redis dependency or a deployed service merely from a folder name.

### Excluded from the MVP

No class/function graphs, inheritance modeling, variable tracing, object lifetimes, program execution, detailed CSV row transformations, profiler integrations, live production traffic, automated bottleneck diagnosis, full Git-history reconstruction, collaboration platform, or arbitrary asset/shader scripting.

Architectural bottlenecks or concentration points may be subjects for an agent's explanation. Actual performance claims require runtime evidence and are not inferred from visual size or connection count.

Language-independent vocabulary is a goal; universal automatic understanding of every repository is not a launch promise.

---

## 6. End-to-end system architecture

```text
USER'S EXISTING CODING ENVIRONMENT

  Private repository
         |
         | Read by the user's coding agent
         v
  Claude Code / Codex
  + public extraction and teaching skill
         |
         | Architecture facts + explanation intent
         | No raw source required in the product payload
         v
  Public TypeScript MCP bridge
  - schema/capability discovery
  - input validation
  - model publication
  - view commands and scene-state queries
         |
         | Local IPC
         v
  Proprietary Rust desktop runtime
  - validates and stores semantic model
  - compiles semantics into spatial composition
  - renders the assembly
  - handles selection, expansion, focus, and playback
         |
         v
  Human views and explores the same model
         |
         +---- selection/view state back to the agent via MCP

OPTIONAL, SEPARATE COMMERCIAL PLANE
  Licensing / application updates
  No repository or architecture upload required for rendering
```

The coding client and renderer run in the user's environment. This does **not** imply that the user's model inference runs locally. Whatever repository context the coding agent sends to its provider remains governed by that provider and the user's settings.

The renderer must work with a saved valid model even when the coding agent is disconnected. New repository understanding requires the agent; rendering an existing model does not.

---

## 7. Stack decision

**Working implementation direction:** a native **Rust + Bevy** renderer, with a **TypeScript/Node.js MCP bridge**, and **JSON Schema** as the public contract.

This is the selected baseline to validate, not a commitment to write a graphics engine from scratch.

| Layer | Technology/direction | Responsibility |
|---|---|---|
| Native visualization runtime | Rust + Bevy | Scene entities, geometry, input, materials, cameras, animation, and application lifecycle. |
| Graphics backend | Bevy's renderer and underlying GPU abstraction | Use the engine's rendering infrastructure; do not begin with raw `wgpu` or custom pipelines. |
| Desktop UI | Bevy UI initially | Restrained toolbar, inspector, search, model status, and view controls. Validate text/input quality early. |
| Agent integration | TypeScript + Node.js + official MCP SDK | Thin local tool bridge and client-specific installation packages. |
| Public contract | Versioned JSON Schema | Architecture model, view intents, commands, responses, and validation rules. |
| Rust serialization | `serde` / `serde_json` | Typed deserialization; pair with schema and semantic validation. |
| TypeScript validation | Schema-backed validation | Avoid independently maintained contracts that drift from the Rust side. |
| Model storage | Local JSON snapshots and view-state files | No database server or graph database required. |
| Graph operations | In-memory typed indexes | Parent/child lookup, neighbor queries, filtering, and explicit relation traversal. |
| Testing | Rust tests/Clippy/formatting; TypeScript tests; render fixtures | Contract correctness, stable layout, visual quality, IPC behavior, and privacy boundaries. |

Bevy is an open-source Rust engine with 2D/3D and entity-component-system capabilities. Its own documentation warns about API churn; pin a compatible toolchain and dependencies rather than chasing every release.[^bevy]

The official MCP TypeScript SDK supports tools and local stdio serving.[^mcp-sdk] Codex documents local-process MCP support,[^codex-mcp] and Claude Code likewise documents local stdio servers.[^claude-mcp] Packaged skills provide the instructions for combining those tools into the extraction and teaching workflow.[^codex-skills][^claude-skills]

### Version and complexity policy

Pin the Rust toolchain, Bevy, SDK, and supporting packages during the first working slice. Commit lockfiles. Keep repository semantics independent from Bevy types so engine changes do not change the public format.

Avoid custom shaders until ordinary materials and geometry demonstrably fail a visual requirement. Avoid a new layout framework, plugin scripting engine, or elaborate agent framework before the core assembly works.

The earlier React/Three.js/Tauri recommendation is **not the default architecture**. It remains a contingency if the native implementation proves uneconomical. Browser rendering and vendor-supplied inference are separate decisions; neither determines the other, but the chosen product direction is native and local.

AI-assisted implementation should proceed in small, tested units. No schedule or feasibility commitment should depend on an unverified claim about a particular model's Rust benchmark performance.

---

## 8. Separate semantic truth from presentation

Use three distinct representations:

| Representation | Contains | Owner |
|---|---|---|
| Architecture model | Entities, roles, typed relations, boundaries, evidence, source revision, and coverage. | Public format; produced by the user's agent or another compatible producer. |
| View/explanation state | Focus, selection, hidden context, expansion, camera intent, and walkthrough steps. | Public interface; controlled by the user or agent. |
| Render scene | Geometry, spatial arrangement, attachment points, labels, materials, animation state, and visibility. | Proprietary runtime implementation. |

The LLM does not emit meshes, shaders, arbitrary coordinates, or scene-building code. It provides architectural meaning and requests an explanatory action. The runtime chooses the representation.

Every meaningful rendered object maps back to stable semantic IDs. Multiple visual fragments of one concern retain the same logical identity; they must not appear as separate services in machine-readable state.

### Determinism and trust

For the same model, renderer version, layout state, and view inputs, produce predictable structure and behavior. Preserve prior layout anchors when updating an existing model.

Deterministic rendering is not proof that the agent's interpretation is correct. Show the basis and uncertainty of the supplied model instead of polishing uncertainty away.

---

## 9. Public architecture contract

**Detailed draft:** [Architecture format, `0.1.0-draft.1`](public/schema/architecture-format.md) now provides a concrete schema and graph rules. It uses typed `contains` and `exposes` relations instead of the `parent_id` illustration below, and separates deployment contexts from logical scopes. The payload below remains an earlier illustration; it is not a valid instance of the detailed draft. The native simulator validates full snapshots and previews all seven kinds as standalone specimens; relationship rendering and assembled layout remain later slices.

### Required concepts

The model needs a schema version; model and snapshot identity; stable entity and relation IDs; entity kinds and optional roles; containment; explicitly typed relationships; scope/coverage information; and a basis for important claims.

Support these evidence states: **supported**, **inferred**, **declared**, and **unknown**. “Supported” should reference supplied evidence. “Declared” means an explicitly authored model, not verified source extraction. Avoid invented numerical confidence scores.

Evidence may use opaque references or optional source locations. Source contents are not required. More detailed visualization does not inherently require transmitting implementation text.

### Illustrative payload

This is a synthetic architecture fixture, not a claim about a real codebase. The compact example describes a declared model; a repository-derived model would add evidence references and actual coverage metadata.

```json
{
  "schema_version": "0.1.0",
  "model_id": "example-platform",
  "revision": 1,
  "snapshot": {
    "id": "example-001",
    "source_revision": null,
    "coverage": "partial",
    "scope_description": "Client, API, user storage, and access-control concern"
  },
  "entities": [
    {
      "id": "app",
      "kind": "scope",
      "label": "Application",
      "scope_kind": "logical"
    },
    {
      "id": "web",
      "kind": "client",
      "label": "Web application",
      "parent_id": "app"
    },
    {
      "id": "api",
      "kind": "service",
      "label": "Application service",
      "parent_id": "app"
    },
    {
      "id": "users",
      "kind": "datastore",
      "label": "User store",
      "store_kind": "relational",
      "parent_id": "app"
    },
    {
      "id": "access",
      "kind": "concern",
      "label": "Access control",
      "role": "access_control",
      "parent_id": "app"
    }
  ],
  "relations": [
    {
      "id": "web-api",
      "kind": "requests",
      "source": "web",
      "target": "api",
      "basis": "declared"
    },
    {
      "id": "api-users",
      "kind": "reads",
      "source": "api",
      "target": "users",
      "basis": "declared"
    },
    {
      "id": "access-web",
      "kind": "applies_to",
      "source": "access",
      "target": "web",
      "aspect": "sign_in_interface",
      "basis": "declared"
    },
    {
      "id": "access-api",
      "kind": "applies_to",
      "source": "access",
      "target": "api",
      "aspect": "server_authorization",
      "basis": "declared"
    }
  ]
}
```

This model permits an access-control pane spanning the client and application service. It does **not** permit assuming that the datastore independently enforces the same policy: no such relationship was supplied.

### Validation requirements

Reject duplicate IDs, missing references, invalid containment, unsupported required versions, and malformed commands. Containment must be acyclic; runtime dependency cycles may be legitimate and should be represented rather than automatically rejected.

Enforce bounded payload sizes, nesting, labels, entity counts, and annotation lengths. Do not accept executable code, arbitrary shaders, or automatically fetched asset URLs.

Schema validation is necessary but insufficient. Semantic validation must detect contradictions such as a flow step referencing an absent relationship. Unknown information remains unknown.

Store projection scope separately from the canonical model. Publishing an auth-only view must not delete the rest of an already loaded architecture. In the MVP, publish explicit snapshots and keep scoped views as separate view state rather than implementing ambiguous partial merges.

---

## 10. MCP and agent integration

### Public agent skill

Package an extraction/teaching skill for each supported host, alongside a shared schema and examples. The skill instructs the agent to inspect only the requested scope, identify architectural responsibilities, distinguish facts from inference, omit source bodies and secrets, preserve stable IDs, validate its output, and use the renderer's tools.

The skill should prefer evidence in deployment descriptions, manifests, routes, interfaces, configuration examples, documentation, and relevant source locations. It must not treat a dependency declaration as proof of a live deployment or invent a service because a diagram looks more balanced with one.

Do not build our own mandatory parser, embedding index, RAG pipeline, or repository scanner. Third-party analyzers can later produce the same public model.

### Minimal proposed tools

| Tool | Purpose |
|---|---|
| `get_capabilities` | Return supported schema versions, semantic kinds, limits, and commands. |
| `validate_model` | Return structural and semantic diagnostics without changing the active view. |
| `publish_model` | Load an explicit model snapshot atomically into the local viewer. |
| `get_scene_state` | Return active model/revision, selection, visible and collapsed IDs, focus, and a compact semantic description of the displayed representation. |
| `set_view` | Focus, isolate, restore context, expand, and request a semantic camera framing. |
| `present_walkthrough` | Play bounded explanation steps referencing existing entities and relations, with optional annotations. |

The public MCP vocabulary expresses intent, not low-level rendering operations. A later `get_subgraph` can query the loaded model, but it must not imply that the bridge can discover missing relationships from source code.

Example `set_view` arguments:

```json
{
  "model_id": "example-platform",
  "expected_revision": 1,
  "focus_ids": ["access", "web", "api"],
  "context": "dim",
  "reveal_ids": ["access"],
  "camera_intent": "frame_selection",
  "presentation": "exploded"
}
```

### Bidirectional legibility

A click identifies a semantic entity, relation, port, or concern facet. The agent reads that selection rather than guessing from a screenshot. It can query relevant source through its existing tools and then highlight the same object the human is inspecting.

Return a concise state summary first; expand details on request. Do not send the entire architecture back into the agent's context for every camera movement.

**MVP interaction:** the user asks questions in their existing coding-agent interface. Selecting something in the viewer makes its context available on the agent's next tool call. A native-viewer chat box that initiates model turns requires a separately verified host adapter; MCP integration alone is not a promise of that behavior. A copyable selection reference is an acceptable initial bridge.

### Tool safety and synchronization

Use request IDs, explicit model revisions, bounded queues, cancellation, and acknowledgements of applied state. Reject stale commands that would target the wrong model. Validate before replacing the active scene, and retain the last valid state on failure.

A user's manual camera input interrupts an automated camera move. An explanation must not seize control of the app indefinitely.

---

## 11. Local runtime and process boundaries

The TypeScript bridge serves MCP over stdio and communicates with the native runtime through local IPC. The bridge handles protocol integration; the renderer owns the active scene and authoritative view state.

**Proposed IPC default:** a per-user Unix-domain socket on macOS/Linux and a named pipe on Windows, with session pairing and restricted access. The exact wire framing can be selected during implementation. Do not expose an unauthenticated network listener simply because it is bound to localhost.

The bridge can start the installed renderer executable. It must not execute an arbitrary binary path supplied by the model. Keep diagnostics out of MCP stdout, separate sessions/models, and handle renderer restarts cleanly.

No microservices, hosted backend, graph database, Kubernetes deployment, or application web server is necessary for the core product.

### Internal pipeline

```text
Validated architecture model
    -> semantic indexes and role resolution
    -> spatial composition plan
    -> Bevy entities and cuboid geometry
    -> connection routing and label placement
    -> interaction/explanation state
    -> rendered frame + machine-readable scene state
```

Keep model validation, graph operations, and as much layout calculation as possible testable without opening a GPU window.

---

## 12. Privacy and disclosure model

There are separate boundaries: the user's coding-agent provider, our local application, and any optional vendor service. Do not conflate them.

| Data | Default handling |
|---|---|
| Repository/source code | Accessed through the user's existing agent; not requested by our bridge or renderer. |
| Architecture descriptions | Passed to and retained by the local runtime. Not uploaded to us by default. |
| Source references | Optional, local metadata; omit or replace with opaque IDs when appropriate. |
| Agent credentials | Remain with the host. Never collect or proxy the user's subscription credentials. |
| License/update traffic | Separate from scene data and not dependent on transmitting architecture contents. |
| Analytics/crash reports | No source, model payloads, internal names, or paths by default; diagnostic sharing requires explicit review/consent. |

The user controls both **abstraction depth** and **which facts enter the visualization**. Later disclosure profiles may include anonymized system-level, named architecture, component-level, and detailed procedural descriptions.

Depth is not a guarantee of harmlessness: even a high-level topology can be sensitive. Conversely, a detailed semantic description can still omit raw source. Do not advertise a simplistic “zoom level equals source-code permission” rule.

Any future sharing/upload feature must provide a payload preview, explicit consent, and redaction. Local rendering must not depend on accepting that disclosure.

Treat repository text and agent-supplied labels as untrusted data. They cannot expand the renderer's tool permissions or cause code execution. Secret filtering is defense in depth, not a substitute for a no-upload default.

---

## 13. Frontend behavior

Use a restrained desktop shell: dominant 3D viewport, compact view controls, optional hierarchy/search panel, and contextual inspector. Keep the scene—not a collection of metrics cards—as the primary experience.

The inspector provides identity, role, relationships, evidence status, and concise agent-authored explanation when available. It does not become a code editor.

### Core interactions

Orbit, pan, zoom, select, focus, expand/collapse, isolate with context, restore, and move continuously between assembled and exploded presentations. Include a home view, breadcrumbs into nested assemblies, keyboard navigation, and reduced-motion behavior.

Opening a selected subsystem should feel like opening that part of the same machine. Preserve recognizable landmarks and avoid replacing the view with a detached diagram.

### Three eventual modes

**Explore:** inspect the architecture manually.  
**Explain:** the agent focuses and reveals a relevant slice.  
**History:** compare architectural snapshots while preserving identity.

Avoid a separate pipeline canvas as the default explanation mode. A sequential path may be highlighted where it is genuinely sequential, but it stays embedded in the assembly.

### Honest status labels

Show model revision, coverage, extraction basis, and refresh status. Do not display “Healthy,” live latency, throughput, deployed replica counts, or success indicators unless a supported data source actually supplied that information.

Educational playback is labeled as an explanation, not live execution. A static architecture description is not a running digital twin.

---

## 14. Evolution and deeper visualization

### Near-term: snapshot comparison

Save architecture snapshots locally with stable IDs, source revision when available, extraction scope, and renderer/layout version. Compare added/removed entities, changed relations, concern coverage, and boundaries. Preserve established placement wherever practical.

The user's agent may inspect Git history and produce these snapshots later. Our renderer does not need direct Git access.

Distinguish a software change from a change in extraction completeness or agent interpretation. Do not claim that a newly discovered edge was necessarily introduced by a new commit. Rename/move identity may require explicit mapping.

### Later: downward expansion

Progress from system architecture to component architecture, then program structure, then procedural/data-transformation explanations. Reuse the same vocabulary and interaction principles rather than inventing a disconnected viewer for each level.

The CSV-to-ontology example is a later target: show parsing, validation, normalization, mapping, and persistence in their actual architectural homes, with illustrative transformations when evidence supports them. Do not turn this into a permanent horizontal strip of boxes.

Runtime telemetry is a separate possible integration, not a dependency of these educational features.

---

## 15. Open/proprietary boundary

The distribution philosophy is an **open input ecosystem with a paid, proprietary visual runtime**, using the Wallpaper Engine analogy as a business inspiration rather than a dependency.

| Public/open-source | Proprietary |
|---|---|
| Architecture schema and file format | Production spatial-composition engine |
| Entity/relationship semantics | Detailed role-to-form selection and layout heuristics |
| Types, validators, migrations, and conformance fixtures | Geometry construction, materials, shaders, and visual assets |
| MCP server and command contract | Occlusion management, label placement, and semantic zoom implementation |
| Agent skills, prompts, examples, and adapters | Camera choreography, transitions, and motion tuning |
| A simple correct reference viewer, if useful | Polished desktop UX and complete rendering experience |

The semantic contract must be complete enough for another implementation to be genuinely compatible. Do not hide required meaning behind an undocumented proprietary field.

A competing implementation may reproduce much of the appearance. The commercial hypothesis is that the remaining composition quality, interaction, readability, and consistent taste are worth paying for. This is a hypothesis to test, not a claim of an uncopyable moat.

A reference viewer should be basic but correct—not intentionally broken. Community analyzers can improve the ecosystem without requiring us to own repository intelligence. Exact licenses, pricing, and update entitlements remain undecided.

---

## 16. Implementation sequence

### Phase 0 — Prove the visual language

Use hand-authored, synthetic JSON fixtures. Build a native window, typed model loading, services/datastores, a restrained assembly, selection, and an assembled/exploded transition.

The essential fixture contains peer services and one cross-cutting access-control concern. Show why the concern becomes a pane or spine rather than another disconnected box. Test a counterexample where auth is only a standalone service.

**Exit criterion:** the model is useful and spatially meaningful before any agent integration exists.

### Phase 1 — Prove the complete agent loop

Implement the schema, validator, MCP bridge, local IPC, and one host's skill. The agent publishes an architectural model, queries selection, and focuses an explanation in the native viewer. Add the second host using the same semantic contract.

**Exit criterion:** “Show me this architecture” and “Explain the selected auth component” work without product-owned inference, source uploads, or repository changes.

### Phase 2 — Make a reliable MVP

Harden ambiguity handling, partial coverage, nested grouping, view continuity, persistence, and the installation experience. Add minimal walkthrough playback, evidence inspection, and conformance tests.

**Exit criterion:** the same learned visual vocabulary remains useful across multiple unrelated codebases and both supported agent hosts.

### Phase 3 — Evolution and additional primitives

Add snapshot comparison, more architectural kinds, saved teaching sequences, and carefully selected deeper representations. Expand based on comprehension value, not on how many node types can fit in a feature list.

---

## 17. Acceptance criteria and tests

| Area | Acceptance criterion |
|---|---|
| Spatial comprehension | After learning the vocabulary, a user can identify peer groups, containment, shared dependencies, and cross-cutting coverage with most labels hidden. |
| Architectural honesty | A pane's visible coverage matches explicit semantic relations. Unsupported enforcement or runtime behavior is not implied. |
| Visual restraint | No cityscape, arbitrary force-directed layout, cylindrical nodes, or mandatory linear pipeline view. |
| Identity and continuity | Focus, explosion, collapse, and snapshot transitions preserve semantic identity and recognizable placement. |
| Agent control | An agent can publish, inspect actual view state, focus, and guide a short explanation using public tools. |
| Read-only behavior | End-to-end tests confirm no product-originated source reads or repository writes; agent-originated repository access stays separate. |
| Privacy | Core rendering generates no vendor-bound source or architecture traffic. Saved models work without an active agent connection. |
| Failure handling | Malformed input and stale commands return useful diagnostics and do not replace the last valid model. |
| Contract compatibility | Rust and TypeScript accept/reject the same conformance fixtures. |
| Accessibility | Selection, exploration, and explanations remain usable without relying exclusively on color, motion, or hidden labels. |

**Initial performance targets, to validate rather than advertise:** responsive interaction and approximately 60 FPS for a modest scene of tens of visible architectural entities on the chosen development machine. Test larger models through grouping and level-of-detail reduction instead of rendering every relationship at once. Establish measured budgets before promising platform-wide performance.

Use unit tests for semantics and graph operations, deterministic layout fixtures, tolerant image comparisons, and manual inspection of animation and occlusion. Compiler success alone does not establish visual quality.

A useful product experiment is to compare comprehension tasks against a conventional diagram and an agent's prose explanation. The product succeeds when users understand and revisit their system more effectively—not merely when screenshots look impressive.

---

## 18. Suggested project organization

This is a logical module layout. Public and proprietary components should be separated into appropriate repositories or release boundaries before publication.

```text
public/
  schema/
    architecture.schema.json
    view.schema.json
    commands.schema.json
  types/
    rust/
    typescript/
  validation/
  mcp-server/
  skills/
    shared/
    claude-code/
    codex/
  examples/
  conformance/

private/
  runtime/
    src/
      model/
      composition/
      primitives/
      connections/
      interaction/
      camera/
      explanation/
      ui/
      ipc/
      persistence/
  assets/
  visual-fixtures/
  packaging/
```

Keep proprietary engine types out of the public model. Keep provider-specific installation details out of the renderer. Do not let licensing concerns become dependencies of schema validation or model portability.

---

## 19. Unresolved decisions

The product direction is settled; these implementation choices still need a short proof or explicit selection:

| Decision | Current direction |
|---|---|
| First shipping desktop OS | Pick one for the initial alpha; preserve the Rust/Bevy portability boundary. |
| Final desktop UI implementation | Start with Bevy UI; validate inspection, text entry, keyboard access, and polish before locking it in. |
| Exact dependencies and IPC framing | Pin a compatible set during Phase 0/1. |
| First datastore subtypes | Relational and graph storage are the proposed reference pair. |
| Conflicting spatial-role rules | Develop a small, deterministic precedence system using fixtures; do not let the model invent layouts ad hoc. |
| Viewer-initiated agent conversations | Host-specific follow-up; ordinary MCP commands from the coding agent are the MVP. |
| Pricing, licenses, name, distribution channel | Undecided; do not inherit earlier illustrative prices or mockup brand names as commitments. |

---

## 20. Final design mandate

> **Render architectural roles and relationships, not merely the existence of components.**
>
> **Make the system feel like an inspectable assembly, not a city, graph cloud, or conveyor belt.**
>
> **Let position and construction explain as much as possible before text is needed.**
>
> **Keep the language consistent across repositories and directly addressable by the user's agent.**
>
> **The user brings the intelligence. We supply the fluent visual language.**

---

## Implementation references

Product requirements above come from the design discussion. These official references support the third-party integration and stack choices; checked 2026-09-29. Version-specific installation instructions should be rechecked when dependencies are pinned.

[^bevy]: Bevy, “Introduction” and “Getting Started.” Rust engine, ECS, 2D/3D scope, and stability warning. `https://bevy.org/learn/quick-start/introduction/` and `https://bevy.org/learn/quick-start/getting-started/`.

[^mcp-sdk]: Model Context Protocol, official TypeScript SDK documentation. Tools, server interfaces, and local stdio transport. `https://ts.sdk.modelcontextprotocol.io/v2/`.

[^codex-mcp]: OpenAI, “Model Context Protocol.” Codex local-process MCP integration. `https://developers.openai.com/codex/mcp`.

[^claude-mcp]: Anthropic, “Connect Claude Code to tools via MCP.” Local stdio server integration. `https://code.claude.com/docs/en/mcp`.

[^codex-skills]: OpenAI, “Skills.” Skills as repeatable workflow instructions around tools. `https://developers.openai.com/plugins/concepts/skills`.

[^claude-skills]: Anthropic, “Extend Claude with skills.” Skill packaging and invocation. `https://code.claude.com/docs/en/skills`.


---

## Appendix A — Open-source reference candidates

**Loose references, not dependency commitments.** These are candidates for reuse, adaptation, or interaction-design study. They do not change the architecture or implementation sequence above. No combined build or dependency audit is implied.

| Reference | Potentially useful material | Boundary / intended treatment |
|---|---|---|
| [LikeC4](https://github.com/likec4/likec4) · [MCP tools](https://github.com/likec4/likec4/blob/main/packages/mcp/README.md) | Architecture-model traversal, nested elements, incoming/outgoing relationship queries, bounded path discovery, and agent-facing query patterns. | Study or adapt selected semantic/query machinery; consider an input adapter. Keep our JSON contract independent. Do not inherit its layout or equate a graph path with an execution trace. |
| [Bevy mesh-picking example](https://github.com/bevyengine/bevy/blob/main/examples/picking/mesh_picking.rs) | Mesh selection, hover, pointer interactions, and connecting visual entities to semantic IDs. | Prefer engine facilities over a custom picking implementation. Resolve selections to our model IDs rather than exposing engine entities as the public contract. |
| [bevy_panorbit_camera](https://github.com/Plonq/bevy_panorbit_camera) | Orbit, pan, zoom, projection options, and basic programmatic camera control. | Candidate utility beneath our own framing, orientation, and teaching choreography. Camera mechanics are not the proprietary visual grammar. |
| [bevy_tweening](https://github.com/djeedai/bevy_tweening) | Transform/material interpolation and coordinated animation sequences. | Candidate utility for assembled/exploded transitions, pane reveals, and context dimming. We still define what moves, why, and how that motion communicates. |
| [bevy-inspector-egui](https://github.com/jakobhellermann/bevy-inspector-egui) | Developer-facing inspection and interactive parameter tuning. | Consider an internal design/debug panel for spacing, pane thickness, explosion distance, opacity, and transition duration. Not a commitment to use this UI in the shipped product. |
| [Official MCP TypeScript SDK](https://github.com/modelcontextprotocol/typescript-sdk) | Tool definitions, structured inputs/outputs, and local stdio transport. | Implementation reference for the already selected thin MCP bridge. Keep provider-specific integration outside the renderer and avoid implementing the protocol ourselves. |
| [Codebase Visualizer](https://github.com/1avishek/codebase-visualizer) | Agent skill structure, read-only analysis instructions, evidence handling, uncertainty, and focused architectural questions. | Adapt instructions and evidence rules to our IR and local publication workflow. Do not make its scanner mandatory or inherit its HTML/Mermaid presentation. Save our artifacts outside the repository by default. |
| [CodeCharta](https://github.com/MaibornWolff/codecharta) · [Explorer controls](https://codecharta.com/docs/visualization/user-controls/explorer/) | Interaction references for de-emphasizing versus excluding objects, preserving context, filtering, and comparing snapshots. | Borrow behavior and test ideas rather than its city metaphor or complete viewer. Preserve our compact assembly and spatial continuity. |
| [arch-viewer](https://github.com/axumquant/arch-viewer) | Comparative reference for MCP-driven architecture inspection and component/data-flow queries. | Study selectively, not as an application foundation. Do not inherit a mandatory scanner, hosted inference, graph/vector database stack, or editor. |

### Useful files to inspect first

For agent instructions, start with Codebase Visualizer's [SKILL.md](https://github.com/1avishek/codebase-visualizer/blob/main/SKILL.md), [analysis rules](https://github.com/1avishek/codebase-visualizer/blob/main/references/analysis-rules.md), and [output format](https://github.com/1avishek/codebase-visualizer/blob/main/references/output-format.md). For semantic tool design, start with LikeC4's MCP documentation linked above.

### Reuse guardrails

Evaluate the exact revisions, licenses, attribution requirements, transitive dependencies, and asset licenses before incorporating code. Pin a mutually compatible Bevy/plugin set rather than choosing each package's latest release independently. Reuse should preserve the open semantic contract and the proprietary renderer boundary.

**Working principle:** reuse camera mechanics, picking, interpolation, protocol plumbing, and useful query patterns. Own the role-to-form grammar, spatial composition, concern coverage, contextual unfolding, and explanatory choreography.
