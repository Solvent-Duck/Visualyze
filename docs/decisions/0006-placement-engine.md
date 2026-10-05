# ADR 0006: Headless placement, bounded projections, and local semantic bundles

Status: experimental implementation; default-engine promotion is not approved by the release gates.

## Decision

Keep the existing gallery/connected renderer as the default. Add explicit `--layout floorplan`, `layered`, `constrained`, and `treemap` comparison modes. The new native viewer uses the same headless private Rust core as the comparison harness. The public snapshot contract is unchanged; richer supplied semantics use the additive public `visualyze.bundle.v1` profiles.

The private `visualyze-layout` crate separates projection, placement, routing, acceptance, and persistence. No Bevy, Java, JavaScript, Python, external optimizer, or new third-party package is required by the solver. Serde and serde_json reuse the workspace's existing pinned versions for private local view persistence. The public provider also uses existing dependencies.

## Implemented engine

Coordinates are quantized to tenths of a scene unit. Stable ID ordering, deterministic seed permutations, and bounded candidate counts make results independent of input array order and worker timing. Non-solid expanded containers and solid collapsed proxies retain explicit semantic ID membership. Interface pads are attached to their actual owner faces. Every other final connector has real surface endpoints and independently verified orthogonal segments.

The floorplanner decodes sequence pairs through separation DAG longest paths within responsibility bands. For larger bands it also evaluates physical shelves, coordinating their full subtree height with neighboring bands. Graph-aware ordering puts peers with upper-layer connections near the top, and reserved passages include solids on physical shelves within endpoint bands. It coordinates neighboring bands, tries permitted footprint proportions, reserves single-purpose skip-band passages, and evaluates promising candidates with the actual shared router. It retains a completely validated result. Layered rows, constrained barycentric adjustment with collision checks, and nested grid packing provide executable native baselines. These are lightweight comparison implementations, not embedded ELK/libcola implementations or a full squarified treemap.

The router first tests direct and short Manhattan paths, then searches a lazy orthogonal channel graph with incoming direction and congestion costs. Coordinate planes, visited states, visible objects, and relationships have hard limits. Exhausting a search budget is a diagnostic, not a proof of infeasibility. Parallel/reverse relationships cannot silently collapse into an identical connector. Routes contain ordered boundary ports through their common containing assembly; the independent validator recomputes every required crossing, rejects reentry and keeps paths inside their common ancestor. Collinear boundary ports do not count as extra bends. The verifier also rejects omitted routes, changed membership, non-orthogonal segments, off-surface attachments, and unrelated obstacle penetration.

Quality profile 2 adds bounded feedback from actual detours and routing failures. The solver can move eligible obstacle subtrees horizontally to open a shorter passage, reroute, and retain a fully validated improvement. Pins, bands and unaffected regions remain constraints. Four additional feedback candidates are allowed by default, with zero available for controlled comparisons. This is an adaptive second pass, not a complete negotiated multi-commodity router. The quality version participates in native cache and saved-state identity.

Hard validation precedes scoring: bounds, supplied/display identity, containment, allowed dimensions, band ordering, pins, complete routing, and clearance. Quality profile 4 includes actual routed length, bends, occupied bounding volume, proportions, a fixed-view projected-overlap proxy, size distortion, and final geometric congestion. Congestion measures additional route occupancy on exactly shared tracks and distinct perpendicular junctions; explicit aggregate membership does not multiply it. It does not measure traffic capacity, line thickness, near misses or camera-projected crossings. Independent proposal guards reject congestion increases above 10% plus 0.1; that tolerance is provisional. Preferred dimensions fix the normalization scale for a projection. Continuity is measured separately from structural cost.

When ordinary candidates and routing feedback fail, a bounded dimension fallback tries at most 28 additional placements, further limited by the requested work budget. Dimensions stay within declared ranges; compatible accepted leaf sizes and unrelated repaired geometry remain fixed. The fallback prefers the least dimension change before the original structural score and still requires full validation. All 28 ordinary profile-4 comparison results remain unchanged apart from timing, so that fallback itself did not change quality version 4. The later stronger Layered initializer is version 5. This extends feasible placement search, not the scope of optimality claims.

An unchanged compatible arrangement is rerouted in place. Local repairs rebuild affected assemblies at their previous origins, freeze unrelated subtrees, and reject growth that cannot fit. Pins apply in hierarchy order, and pinned frames retain centered envelopes. An unrelated model, future revision, incompatible layout version, or invalid old geometry is not a valid repair seed. Successful revision reloads schedule one background global alternative; users may also request one explicitly. Applying an alternative always requires an explicit action.

## Native inspection and state

Initial indexing/projection/refinement runs on a cancellable worker. The accepted scene remains until a complete replacement passes validation. Cancellation or failure restores the accepted view's focus, breadcrumbs, and constraints. Transactional explicit-file reload validates the replacement model identity, increasing revision and geometry before atomically publishing its provider and scene. Reorganization after publication is optional background work: navigation can cancel it immediately without rolling back the accepted revision.

The source adapter also keeps an exact projection memo shared only by clones of that source: at most 16 entries and a 32 MiB estimated retained-memory charge. Its bounded exact request key includes all projection-affecting view settings, including offscreen retained groups in their original order; oversized keys/results bypass the memo. Reload creates a fresh source and memo. A bundle hit still checks the provider revision/file fingerprint and refreshes loaded-versus-unsupplied detail counts. Returned projections are independent copies; solver-cache checks and final geometry validation remain mandatory. Group-retention changes can legitimately miss this memo, so a first return is not guaranteed to be a hit. Existing locked Serde/serde_json dependencies encode private request keys; public model formats are unchanged.

If a focused suggested group no longer has two members under its supplied parent, reload recovers to a surviving supplied parent or known ancestor before projection. It preserves the disappearing group's pins in their original frame and restores the destination frame's pins. The replacement must still pass the ordinary revision and geometry checks; a corrupt or incompatible replacement retains the accepted scene. Successful publication clears prior-revision undo. Failed expansion retains valid selection, explains that the prior arrangement is unchanged, and offers separate inspection while retaining the technical diagnostic.

The experimental viewer supports selection, enter/back, explicit expansion, layer separation, connected-neighbor isolation, layout policy selection, pins, layer preferences, and bounded view undo. Reorganization search has a larger deterministic budget, proposes only a 15% structural-cost improvement with readability guards, and supports preview/apply/dismiss. Reorganization requires an idle accepted view; proposal identity is checked before preview/application. Applying and undoing share bounded cache insertion. Pins are scoped to the focused coordinate frame and survive restarting inside a subsystem and returning to its parent.

Layer separation monotonically opens existing gaps without changing accepted geometry. Connector axes and solid-box sizes stay unchanged. Picking intersects visible box geometry, including preview geometry. Each focused region retains its scene origin; expansion does not recenter neighboring objects or reset the camera. Projected labels prioritize selection and reserve their full bounded caption area. Exact IDs, original relation kinds, evidence IDs, aspects, and rationales remain available in Details. F1 opens its bounded scroll panel; its initial collapsed header leaves the assembly visible. Reference boxes and inferred groups are named as such.

Search runs on a worker against indexed IDs/labels. Child and relationship pages are independent. Reference-port entry resolves to its supplied semantic endpoint. Selected-region zoom uses hysteresis and explicit choices take precedence. Local group controls preserve supplied parents and support collapse, expansion, entry, removal and undo. J/K selects individual supplied relationships; I isolates their displayed path without changing accepted geometry. A selected aggregate member retains its own predicate/evidence and the shared path's full membership. The bounded layout cache shares cached projections through `Arc`; starting a new worker no longer clones every cached claim map on the render thread. Grouping suggestions and native selection during active work have focused tests; broader comprehension and frame-time evidence are still needed.

Accepted view/layout state is saved under `~/Library/Application Support/Visualyze/layouts/` on macOS. Filenames are derived from model identity, never source paths. Saved state is private, versioned, limited to 8 MiB, and atomically replaced with restrictive file permissions. Unsupported, corrupt, other-model, or newer-revision files are preserved. Startup reconstructs the current projection, validates saved geometry, and recomputes measurements before accepting a saved scene. A bounded set of known prior quality filenames is checked when the current file is absent. Only the quality identity may migrate. If membership changes invalidate exact restoration, compatible saved bounds and sizes seed the normal local-repair solver; model, root, effective lens and layout version must establish the same coordinate frame. Pins and overrides remain authoritative. An unverified frame or failed repair preserves the old file and reports failure. Native smoke persistence is disabled by default; the optional `--persist` mode uses an isolated temporary directory. Saving current bounded view state is synchronous; timings retain the latest 64 samples and lifetime totals, including state preparation and file synchronization. The dated persistence-enabled runs include a 4.5-second first acceptance and a 718 ms CPU-update outlier, which the measured save durations alone do not explain. Broader measurements must establish latency before choosing an asynchronous save design. Large model indexing and solving are background work.

## Public data and scale

See [the bundle contract](../../public/schema/model-bundle.md). It defines independent versioned architecture, data, code, messaging, and instance profiles. Table ownership, asynchronous predicates, code ownership, and deployment instances are explicit records. They are not encoded as invented services or requests.

A local regular JSON-lines file is indexed with bounded records, count/byte limits, revision-bound queries, explicit pagination, cancellation, source-change detection, and a bounded detail cache. Geometry, pins, visual groups, and camera state never enter public semantic records. The viewer never opens source locators, scans repositories, or uploads architecture. A separate authoring CLI can parse an explicitly supplied immutable Rust checkout into a validated bundle without executing source, Cargo or build scripts. Its pinned Bevy inventory and coverage limits are recorded separately.

The producer's opt-in `syntax-v2` identities use lexical item and import anchors, keeping tested body edits, comments and reordering from changing IDs. Evidence still names the current revision and lines, and ambiguous duplicate anchors fail before publication. Legacy generation remains the default and preserves captured fixture bytes. Renames, moves and changes to conditional or self/trait syntax can change identities. Changing schemes requires a distinct model identity; existing saved arrangements and pins are not migrated between schemes. The pinned syntax-v2 corpus passes a 21-action native proof and an all-pages audit of 232 page combinations across 165 sampled regions. Those dated reports certify their recorded implementation checkpoints, not subsequent provider changes or every declaration and expansion.

The provider projection loads bounded local regions and references. Explicit expansion supports mixed branches (database/tables/columns beside collapsed services and code), retaining supplied parents and accounting for reference boxes within the visible budget. Expanding a grouped member opens its containing visual group without expanding its siblings' contents or changing the group's membership. Lowest-layer assemblies grow downward from their former top face; upper-layer assemblies retain their bottom face. The default bundle regression expands the database while every unrelated box retains its exact world bounds. Conflicting growth preserves the last valid view and reports the conflict. Partial pages and unloaded/unsupplied detail are explicit.

Indexed descendant queries map relations to the nearest visible ancestor while retaining original IDs, kinds, direction, evidence and aspects. A reference budget stops before consuming an unrepresentable claim and returns a cursor for that exact deferred record. Fresh queries use local ownership-interval endpoint indexes, visiting at most 8,192 endpoints per call rather than scanning unrelated relationships. An internal edge consumes up to two visits but contributes one original claim/count, including across disjoint group roots. Existing saved global cursors retain their original traversal. Preorder plus canonical incident lists build the index in linear time with exactly reserved base capacity; binary-depth jumps bound external ancestry work and check cancellation. Queries accept at most 8,448 representative/member IDs; visible scene geometry remains capped at 250 objects. Internal counts are distinct from bounded resident membership samples. Virtual-group entry queries the union of its member subtrees, with outside endpoints kept as references. Manual and suggested groups remain view state, never supplied architectural facts.

## Release gates and remaining work

The implementation establishes executable foundations across all five stages; it does not close all five stages. [Validation results and the human evaluation protocol](../placement-validation.md) track the distinction.

Logical/code/deployment lenses and transactional file reload are implemented with scoped tests. Outstanding production work includes automatic-group usability and continuity across arbitrary topology changes, dense-region readability, and performance coverage beyond the sampled workloads. The syntax-v2 all-pages audit includes individual solves above the one-second local-detail target. Repeated cold-start and persistence-enabled measurements, GPU presentation evidence, frozen training/held-out quality calibration and human comprehension sessions remain gates. A diagnostic quantitative control now captures the original renderer unchanged, with invalid geometry reported separately. A render-extraction shutdown repair passes the recorded native production and million-input runs as well as a channel-backed ownership/destruction regression. Scoped tests do not replace graphical verification after changes.

Group membership is deterministic and retained as local state; automatic membership continuity for every topology change is not yet established. The exact development oracle proves minimum area only over its bounded fixed-size sequence-pair problem, not optimal 3D software layout. The million-entity headless proof samples six bounded projections; a separate native proof traverses 29 regions/46 actions, stays below the 2 GiB memory target, and verifies exact returns after cache pressure. The final local-index million-input run exits normally at 1,071,906,816 bytes peak RSS; it records 7.24 seconds initial acceptance including indexing, at most 238 ms fresh navigation and 37.3 ms sampled resident returns. That historical startup result combined stages. Current profile-5 measurements separate import from layout: the million-input run takes 6.67 seconds initial CPU acceptance, with 6.21 seconds indexing, 49.54 ms first projection and 83.71 ms first solve, at 1,021,837,312 bytes RSS. Repeated process-start and GPU presentation measurements remain open. The current local-index production audit preserves exact reference coverage across 223 page combinations in 165 selected regions, and its 21-action native proof passes with isolated persistence. These sparse synthetic samples do not establish arbitrary dense-graph performance or human comprehension.

Keep the engine opt-in until hard geometry, performance, and comparative comprehension gates pass. Never describe provider memory measurements or a low weighted cost as proof of an enjoyable million-entity product.


## Profile 5, continuity and saved dismissals

Layered now performs nested-endpoint crossing reduction and rectangular depth
packing within the same semantic bands and geometry/router constraints. All
non-Layered comparison metrics remain unchanged. The stronger control reduces
Floorplan's median structural-cost advantage on the seven historical overviews
to 8.65%, with two losses; old one-row comparisons are historical evidence.
Named coefficients and a hash-checked evaluation manifest make profile/corpus
changes explicit. The 19-view training/development checkpoint has 76 valid layouts;
computational freezing is not weight calibration or a fresh held-out study.

Metrics include diagnostic nearest-three surviving-sibling retention with
explicit exclusion counts and no-comparison states. It does not silently add a
new scoring weight. Bounded dismissed-proposal identities persist across restart,
scoped by model/root/lens and including geometry plus original claim semantics.
Rerouting alone intentionally does not reoffer the same arrangement. Old saved
files remain readable; malformed new metadata preserves the existing file.

The runtime records the last 64 job timing records with actual cache outcomes,
action/job/revision identities, exclusive import/projection/solve/restore stages,
and publication/scene/save costs. Cancelled retired jobs retain their own timing
snapshot. Scene timing measures command construction, not deferred command
application or GPU presentation. The latest production and 10k/100k/million
native records include this instrumentation and exit normally. The latest
direct-action proof examples discard desktop keyboard/mouse input before runtime
controls; normal viewer input is unchanged. A failed intervening 100k run is
preserved, with its unrecorded physical-input provenance explicitly unresolved.

The largest implementation gap is still bounded parent presentation envelopes:
the existing expanded parent takes its size from descendants. Completing this
requires independently cached canonical interiors and exact boundary route joins,
with shared presentation transforms for rendering, picking, pins and navigation.
A renderer-only shrink does not satisfy that requirement. See the full
[acceptance ledger](../placement-requirements.md) and current
[measurements](../placement-validation.md).

## Canonical composition

The private headless core now combines frame extraction, independent placement,
exact presentation and fixed-boundary routing in `composed::solve_composed`.
The native viewer opts into this representation with `--layout composed`.
`NativeLayout` retains the authoritative flat or composed solution; shared
`SceneGeometry` derives display coordinates without feeding rounded values back
to the solver. Existing flat arrangements and their saved format remain separate.
This is an experimental integration, not a default promotion.

`assembly_plan::plan_frames` extracts a private kernel projection per expanded
assembly. A child interior becomes a bounded solid proxy in its parent's kernel,
so external routes reserve its whole envelope. Allowed and preferred dimensions
remain fixed inputs while legal proportion search stays available. Real solid
owners and their interface pads remain together. A focused solid root and
parentless outside references share a canonical frame without fabricated
semantic containment. Original relationship directions and memberships remain
on explicit lowest-common-frame routing plans; boundary terminals are geometry,
not new semantic objects. Exact sorted descriptors cover frame size domains;
full result caches must also account for constraints, solver settings and view
context.

`assembly` retains integer canonical layouts and maps them with positive uniform
dyadic transforms. The fit helper chooses the smallest fitting exponent and
returns padded canonical bounds that map exactly to the parent envelope. The
caller must use that domain for routing and quality measurements. A mere smaller
interior fitting inside a larger box is insufficient: a selected parent boundary
port must inverse-map exactly onto the child's boundary. Coordinate arithmetic,
depth, denominators, geometry counts and ownership are checked before presenting
boxes, route vertices and ports through the same transform. Floating conversion
is explicit and does not promise pixel visibility at arbitrary depth.

An anchor pin records its canonical center and every intervening transform,
canonical domain and parent envelope. A pin made from an ancestor view locks those
links; a region-local pin permits movement above its anchor frame. The caller
must retain the same model, root, effective lens and policy context. Native pins
capture these anchors in the active view; independent canonical pins can also be
restored from composed state. Apply, undo, cancellation and failure restore the
corresponding accepted constraints. Flat pins are never implicitly reinterpreted.

`routing::route_fragments` and `validate_fragments` route and verify actual box
endpoints and exact boundary terminals, including explicitly selected inward
normals, domain bounds, obstacle clearance, membership, cancellation and distinct
complete paths. They reuse the sparse channel search; they do not introduce
phantom endpoint boxes. The independent `validate_composed` reconstructs every
original relationship's frame path and terminal identities, verifies exact joins
and direction, and validates all fragments in each frame as one batch. An external
connection to an assembly ends at its real boundary; it does not acquire a fake
interior segment. Missing, extra, redirected and duplicate paths fail validation.

At a boundary edge or corner, the signed normal identifies the incident face.
Rejecting every corner would be inconsistent with valid parent box attachments;
the child must accept the parent's chosen attachment without shifting it. Legal
solid-owner interface protrusion also remains bounded by every enclosing open
assembly, not merely by the immediate solid owner and the outermost frame.

Placement runs from interiors toward the root under one deterministic candidate
budget. The additive `solve_budgeted` counts initializations, rejected feedback
trials, accepted-geometry trials and size fallback; the existing `solve` retains
its profile-5 behavior. Each unvisited frame retains an allowance; unused work
returns to the common pool. Geometry and relationship-copy storage are bounded
before costly placement, and routed vertices are bounded while accumulating.

The compositor then fits domains and routes from parent frames toward interiors,
propagating real parent attachments as exact child boundary terminals. Padding is
distributed around existing interior centers to avoid corner drift; explicit
canonical root and leaf pins remain checked after fitting. The straight nested
auth regression passes across three frames. Growing the declared database
interior leaves its parent proxies unchanged.

After a complete valid composition, a bounded cross-frame refinement pass ranks
boundary fragments by routed detour and length. It proposes horizontal alignment
of an interior terminal toward its actual boundary attachment. A solid carries
its attached subtree; a child-frame proxy carries its exact instance translation.
Box sizes, canonical domains, vertical bands and dyadic scales stay unchanged.
Pins and fixed host subtrees are excluded. Stable ordering, quantized destinations
and at most 64 retained proposals bound deterministic search.

Up to `feedback_candidates` trials (default four, maximum sixteen) are reserved
inside the same total work budget before allocating frame initializers. Every
attempt is charged before construction/validation, including rejected geometry
and routing failures. All frame requests and routes are rebuilt top-down, with
exact matching routing batches eligible for reuse. Independent final validation
is required; acceptance requires lower total structural cost and non-increasing
canonical routed length. Cancellation publishes nothing, and unsuccessful trials
retain the prior valid candidate. Existing and reused frames cannot participate
during normal navigation/repair; explicit reorganization may search free frames.
The private placement key records this refinement revision.

Initially unroutable compositions now use the same reserved allowance to open
observed blocked corridors. The fragment router reports a bounded witness only
after a real routing failure: its actual lane-aware terminals, endpoint normals
and domain remain legal while up to sixteen named inflated obstacles block a
short orthogonal corridor. Invalid inputs and cancellation produce no witness;
evidence is advisory, never an accepted path or an infeasibility proof.

Recovery translates eligible solid-owner subtrees completely beyond the observed
segment on X or Z. A domain limit rejects the move instead of clamping it into a
still-blocked corridor. Dimensions, domains, bands, scales, pins, fixed hosts and
cached/previous interiors retain the same protections. Every attempt is charged
before construction, including overlap failures. An incomplete seed may advance
only when routing reaches a later frame/relationship or the same failed corridor
has fewer distinct blockers. It never reaches publication. The first complete,
independently validated result satisfies the feasibility gate; any remaining
allowance can improve its quality. Exhaustion retains the original named
frame/relationship diagnostic and leaves the accepted cache and revision watermark
unchanged. Search is bounded and greedy, so failure does not prove impossibility.

The existing feedback counters include recovery attempts and the first complete
recovery as an accepted improvement. The private placement key advances its
algorithm revision; state format 5, composition and quality versions stay unchanged.
Routing-space enlargement and broader congestion-driven placement remain open.
The `composed_layout --feedback 0` probe disables both flat-kernel and composed
feedback and changes shared allocation; it is not an isolated measurement of
either pass. Dense view readability and calibrated comparisons still require
separate acceptance evidence.

`ComposedSolution::present` deduplicates proxy/root representations and supplies
one exact geometry set for rendering, picking, labels and cameras.
Canonical quality sums actual fragment lengths and frame costs using fixed
preferred-size reference scales, including padded routing domains. Presented
length and scale are separate diagnostics and never reduce structural cost.
The geometry validator does not certify stored metric values or replay work
counters; these are freshly computed/reported by the solver.

`CanonicalPlacementCache` stores bounded local placements and exact routing batches.
Exact keys bind semantic membership, dimension domains, edges, constraints,
algorithm, quality settings, typed lens/policy and the total work budget.
After rebuilding boundary requests from the current frame topology, routing may
reuse a complete batch only when the model/frame identity, all boxes, routing
domain, clearance and ordered requests (including original memberships, boundary
positions and normals) match exactly. A placement-key hit alone is insufficient.
Whole batches preserve the router's deterministic lane and congestion ordering;
individual routes are never spliced from different routing passes. Every final
route, join, obstacle clearance and attachment is still independently validated,
and quality is measured again. Compatible previous
frames preserve positions; a changed frame uses local repair. Entry limits and a
conservative byte estimate include both placement and routing data under the
same 128-entry / 32 MiB default limit. Immutable routing batches are shared when
staging cache updates. Revision watermarks
reject older requests. A cancelled or invalid result cannot publish a cache.
New diagnostic routing counters default to zero when reading older saved work
records; semantic, geometry and saved-state versions are unchanged.

Explicit reorganization searches free frames while retaining canonical pins and
anchor-chain host geometry. It leaves accepted caches unchanged. Apply and undo
validate and publish the chosen canonical interiors, then invalidate dependent
whole-view entries. A candidate cannot meet the readability gate by increasing
the compression of a retained frame.

The composed private-state format stores checked exact geometry, view state,
canonical pins, anchors and dismissed proposals. Version 2 also stores bounded
constraint records for hidden interiors and inactive view contexts. Exact anchor
records carry their semantic members, owner-to-anchor domains, host bounds and
transforms; cache eviction no longer removes the data needed to enforce a pin.
Only visible constraints reach the current solve. A hidden pin requires a
supplied collapsed ancestor and a matching supplied ownership path; deletion or
reparenting fails with the accepted scene and previous save retained. Independent
canonical pins and anchor locks remain strictly validated. Decimal integer numerators
avoid loss through JSON floating point. Loading reconstructs checked assemblies;
the worker then rebuilds the supplied projection, validates claims/geometry and
recomputes metrics. Exact restoration warms the canonical cache. Same-context
revision changes may repair saved geometry; corrupt, future or incompatible
state is preserved. Its stable model-specific filename is separate from flat
state, and format/layout/quality headers are checked rather than silently ignored.
Versions one through five retain their original format semantics while
loading without being rewritten; the viewer validates and repairs their accepted
geometry under current constraints before saving version six. Version three
used downward-only inheritance; older versions constrained their current view.
Contradictory active registry records are rejected during load. The registry
permits at most 64 typed contexts and 16,384 total pins/anchors within its storage
budget; adding an override past those bounds is rejected before publication.

Version four separates source-owned records from effective pins across related
supplied regions. A pin created inside a child also constrains its interior when
shown inside an ancestor. The resolver requires explicit supplied ownership, the
same model/effective lens/policy, and a canonical owner at or below both source
and target roots. Descending truncates the exact owner-to-anchor chain at the
entered root. Ascending retains the original anchor without extending its locks
into the parent. The source region can therefore move or scale within its parent
while the pin keeps its local coordinates and captured internal frame links.
A pin fixes position and these links; it does not freeze every dimension or route.

Current ancestry of both the target and source regions is checked on the worker
before filtering stored ownership paths. Removed/reparented supplied objects,
conflicting coordinates, memberships, policies or shared frame locks preserve
the accepted scene and save. Same-context previous frames remain repair seeds;
the core revalidates each against new constraints independently, preserving
unaffected parent objects. Cross-scope routes are rebuilt and geometrically
validated. Collapsing a supplied interior keeps its constraints dormant. A pin
on its canonical root never becomes a pin on the parent representation.

Inherited pins are read-only outside their origin. Shift+P opens the originating
region to change or remove them; the inspector discloses that source. Origin
selection matches the actual effective owner, coordinates and supplied path,
so an unrelated retained same-ID group override cannot hijack navigation.
Unpinning invalidates reuse through exact effective-constraint comparison.
Undo snapshots the bounded owned registry and geometry; cancellation restores
the accepted registry. Navigation never creates new owned copies. Version-four
files use the existing wire fields but require the new version to prevent older
readers from interpreting previously inactive child pins under wrong semantics.
Version six extends that provenance to protected presentation frames as described
below; arbitrary frame-topology changes remain separate work.

Version five adds separate composed feedback evaluation/improvement counters.
Absent counters default to zero for older files; improvements cannot exceed
attempts, and checked frame-plus-feedback sums must fit the shared work budget.
Version four and five use the same bidirectional pin semantics. This format change
does not change the semantic contract, composition version or quality formula.

Version six adds explicit presentation ancestry alongside supplied ownership.
Each pin records only the relevant visual-group definitions and complete supplied
paths for their parents/members. The projector exposes its normalized group
catalog even when an enclosing group is outside the focused graph; capture uses
that catalog rather than raw recent suggestions. Group corrections, deleted or
reparented members, dismissed suggestions and supplied-ID collisions therefore
follow the same projection rules before a new pin is created. Accepted membership
refresh keeps offscreen groups in their original cache-recency positions; only
displayed groups become recent. Existing protected
definitions are checked strictly, not silently pruned or replaced.

Protected definitions are reseeded before projection and survive the bounded
recent-suggestion cache. Descending and ascending through a group use the same
exact anchor rules as supplied frames. A reference has separate target and origin
evidence; its pin is effective only in its originating region, even when another
region emits the same display ID. A missing page representation stays dormant
after supplied evidence is rechecked. A vanished incident claim can leave a
reference pin dormant; that record does not assert that the relationship still
exists. Removing or reparenting its supplied target/origin is a conflict.

The registry shares its 8 MiB bound with at most 64 protected group definitions,
128 members per group, 65 IDs per supplied path and 130 per display path. Old
versions reject presentation fields rather than interpreting them with legacy
semantics. All current evidence is rechecked on the worker before publication;
failed correction/reload, cancellation and undo preserve owned records and saved
state. Protected-group membership changes conservatively require unpinning in
the originating region; automatic topology repair remains future work.

Display conversion subtracts a stable origin before conversion to GPU precision.
Unrepresentable boxes or segments reject a new scene; inspection transformations
use the same coordinate map for bounds and paths. Connector widths follow each
fragment's scale, arrowheads remain bounded and include depth-only directions,
and long dashed segments have a bounded number of pieces.

The headless `composed_layout` probe remains separate evidence from native GPU
and interaction checks. See the dated [validation record](../placement-validation.md).
Inactive-view restart, collapsed supplied interiors and prior-cache eviction now
have durable pin recovery and scoped regressions. Pins remain typed by
model/root/lens/policy: navigation retains ownership in the original context
and derives compatible constraints for related supplied and protected group frames.
Reference proxies stay in their original regions, with dormant state across pages.
Unsupported topology changes still fail rather than discarding pins. Broader recovery
and routing-space enlargement, general topology continuity, calibrated comparison, performance
and comprehension gates remain open.
