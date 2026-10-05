# Placement implementation validation

The new engine is experimental. Correctness checks and comparative results below do not establish the full product release gate. Human comprehension sessions have not been run.

The latest verified workspace checkpoint passed **484 tests**, with zero failures and two intentionally ignored production-bundle regressions; both passed explicitly. The native production helper passed **15 tests**, and the evaluation helper passed **five tests**. Formatting, workspace/all-target Clippy and native builds pass. [Current validation and source identities](measurements/presentation-pins-validation-2026-10-04.json) cover durable visual-group/reference pins and the native cache-order regression found and fixed during this work. The [450-test blocked-route recovery checkpoint](measurements/blocked-route-recovery-validation-2026-10-02.json) and earlier dated results remain historical evidence. Production ingestion/display is proven for the declared source inventory; renderer stalls, broader scale/performance, quality calibration and comprehension remain open. The full [approved-plan acceptance ledger](placement-requirements.md) remains authoritative.

## Reproduce

```sh
cargo test -p visualyze-layout --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p visualyze-layout --example compare --locked -- public/examples/notes-crud.json public/examples/sketch-crud.json private/layout/fixtures/shared-saas.json private/layout/fixtures/nested-cross-scope.json private/layout/fixtures/fanout-peers.json
cargo run -p visualyze --locked -- --model private/layout/fixtures/online-boutique.json --validate
cargo run -p visualyze --locked -- --model private/layout/fixtures/deathstar-hotel.json --validate
cargo run -p visualyze-layout --example compare --locked -- private/layout/fixtures/online-boutique.json private/layout/fixtures/deathstar-hotel.json
cargo run -p visualyze-layout --example bundle_layout --locked -- public/examples/data-messaging.bundle.jsonl datastore:orders
cargo run --locked --example render_smoke -- --model public/examples/objects.json
cargo run --locked --example connected_smoke -- --model public/examples/notes-crud.json
cargo run --locked --example layout_smoke -- --model public/examples/sketch-crud.json
cargo run --locked --example layout_smoke -- --bundle public/examples/data-messaging.bundle.jsonl
VISUALYZE_SMOKE_EXPAND=1 cargo run --locked --example layout_smoke -- --bundle public/examples/data-messaging.bundle.jsonl
```

The GPU examples require the native macOS graphical desktop and save screenshots to the OS temporary directory. The new smoke checks scene/route membership and visible colored pixels; manual image inspection is still required. Existing gallery/connected proofs retain their per-object/per-route sampling. Smoke tests disable private view persistence by default; `production_smoke --persist` instead uses a newly created, isolated OS-temporary state directory.

The expansion proof drives the native keyboard path: it requests reorganization during initial loading, selects the supplied database, expands it into tables, and asserts that neighboring rendered positions and the established camera remain exactly unchanged. It passed on the reference machine, and its screenshot was inspected. The standalone sketch proof also passed. One earlier native run captured successfully but hung in Bevy render teardown; that process was stopped and is not counted as a successful exit. The later repair and successful graphical runs are recorded below.

## Held-out cohort v1, October 5

The first fresh held-out cohort ([selection](../private/layout/evaluation/holdout-v1/SELECTION.md))
has eight independently authored synthetic cases: CRUD clinic, cross-region
research catalog, parcel event fan-out, museum tables, compiler code lens,
regional deployment lens, disconnected islands with parallel claims, and a
water-control pipeline. Before scoring, both snapshot cases failed contract
validation because the generator omitted the root scope's `system` subtype. The
generator was corrected without changing topology, views or bundle bytes; the
correction, one intentional duplicate-claim warning and final hashes are in
[validation.json](../private/layout/evaluation/holdout-v1/validation.json).

[The combined manifest](../private/layout/evaluation/quality5-holdout-v1.json)
adds these cases to the unchanged v3 training/development cases under the
unchanged quality5 profile. A new [training run](measurements/quality5-holdout-v1-training-2026-10-05.json)
and [freeze](../private/layout/evaluation/quality5-holdout-v1.lock.json) were
committed before any scored run. The [development](measurements/quality5-holdout-v1-development-2026-10-05.json)
and [held-out](measurements/quality5-holdout-v1-held-out-2026-10-05.json) passes
are all valid: 64 development and 32 held-out layouts, with no failures.

Lowest held-out structural cost (lower is better): Floorplan in six cases
(research, parcel, museum, regional ledger, islands, water control) and Layered
in two (clinic, compiler workspace). Constrained and Treemap won none. On
development views the split is Layered 9, Floorplan 7. Held-out median headless
solve time was 38–40 ms for Layered/Floorplan and 19–20 ms for
Constrained/Treemap, with a maximum of 79 ms.

These scores use the provisional, uncalibrated coefficients. They show that the
engine produces valid layouts on unseen declared inputs and how candidates rank
under the current proxy. They do not establish readability, comprehension or
calibrated quality. Any engine change made after inspecting these results turns
this cohort into development evidence; later held-out claims need a new cohort.

## Durable group and reference pins, October 4

Composed state version six records display ancestry separately from supplied
ownership. Pins inside manual and suggested visual groups now survive collapse,
entry/return, suggestion-cache eviction and cold restart. This includes pins
created after entering a supplied child whose enclosing group is offscreen.
Exact canonical coordinates and anchor links remain unchanged; inherited pins
stay owned by their original region and Shift+P opens that region for editing.

Reference pins retain both their original region and supplied target. Paging
away makes them dormant. Another region's same-ID reference and the actual
supplied target remain unpinned. Supplied target/origin removal or reparenting
preserves the accepted scene, registry and state file with a diagnostic.
Protected group corrections follow the same transactional rule. Cancellation
and undo retain exact accepted state.

The projector supplies a normalized group catalog, including offscreen
ancestors. New pin capture follows current manual precedence, dismissals,
ID collisions and surviving supplied membership instead of trusting stale
recent suggestions. Acceptance refreshes this membership **without changing
cache recency**. The first native attempt exposed a mistake here: treating every
catalog entry as recently visited evicted familiar package groups and failed the
exact cached-position assertion. The fix has an explicit recency regression;
the identical production walkthrough then passed. The failed run and its source
identities remain in the [validation record](measurements/presentation-pins-validation-2026-10-04.json).

The registry bounds 64 protected groups, 128 members per group and complete
supplied/display paths inside the existing 8 MiB state allowance. Versions 1–5
retain their original load semantics and reject presentation fields; current
model/geometry validation and repair precede a version-six save. Existing
protected membership changes conservatively require unpinning in the source
region. Legacy display pins need provenance captured there before inheritance.
A reference whose last relationship disappears may remain dormant while its
origin and target still exist; its pin never asserts a relationship's existence.

The new explicit production regression uses the real Bevy syntax-v2 bundle. It
enters a grouped source file, pins a supplied declaration, restarts with an empty
suggestion cache, expands the file inside its parent group and returns to the pin
origin. It checks exact anchors and original owned records. The separate native
production run rendered the same **27,117-entity / 44,019-relationship** model,
completed **38 actions and 33 saves**, and retained exact cached geometry. The
native supplied-interior pin walkthrough completed **22 actions and 23 saves**.
Both exited successfully; expanded production and restored nested-pin images
were inspected. Group/reference cases themselves are checked through the worker,
persistence and scene-construction tests; these native walkthroughs do not claim
physical keyboard/mouse verification of every new case.

This screenshot-enabled production sample recorded 1,138.89 ms initial CPU
acceptance, 122.95 ms repository collapse and 30.14 ms World collapse. The first
collapse still misses the 100 ms target. Maximum renderer join/extraction was
93.82 ms and the surface-acquisition bracket 30.68 ms; not reproducing the old
one-second pause does not establish a fix. The production scene remains dense
with overlapping routes/labels and the open Details panel obscuring its lower
portion. These are correctness and continuity results, not a readability,
physical-input latency or comprehension pass. Bevy 0.18.1 remains in the main
runtime; the isolated upgrade candidate requires rebasing before adoption.

```sh
cargo test -p visualyze --lib pin --offline --locked
cargo test -p visualyze --lib layout_scene::tests::presentation_pin_tests::production_grouped_descendant_pin_survives_restart_and_parent_navigation --offline --locked -- --ignored --exact
cargo test -p visualyze-layout --test inherited_constraints --test composed_persistence --offline --locked
```

## Projection reuse and native latency investigation

An exact source-scoped projection memo avoids rebuilding an unchanged visible
subset. It retains at most 16 entries under a 32 MiB estimated memory charge;
oversized requests bypass it. Keys include all projection dependencies, including
offscreen retained groups. Bundle hits still validate revision/file identity and
refresh detail-residency counts. Reload uses a fresh memo. Seven regressions cover
snapshot/bundle reuse, copy isolation, invalidation, mutation, cancellation and
bounds; layout-cache validation and publication checks remain unchanged.

The unchanged pre-optimization binary repeated the earlier production walkthrough
successfully: the [baseline](measurements/native-latency-baseline-2026-09-30.json)
recorded an 83.64 ms repository return, versus the earlier 1.22 s outlier. That
variability did not establish a cause. Three subsequent runs of the new build
each completed **38 actions and 33 successful saves**, with exact cached-position
checks. Each obtained seven actual projection memo hits. New retained groups
legitimately caused earlier misses; no post-retention alias was assumed.

| Run | Trace / screenshots | Final six repeated returns, CPU acceptance | Projection on those hits | Peak RSS |
| --- | --- | ---: | ---: | ---: |
| [A](measurements/native-projection-cache-2026-09-30-a.json) | On / on | 34.0–50.9 ms | 0.20–2.24 ms | 464,142,336 bytes |
| [B](measurements/native-projection-cache-2026-09-30-b.json) | On / off | 33.5–50.0 ms | 0.15–1.44 ms | 390,217,728 bytes |
| [C](measurements/native-projection-cache-2026-09-30-c.json) | Off / on | 34.3–51.6 ms | 0.21–2.01 ms | 463,732,736 bytes |

The native pin walkthrough also [passed all eight actions](measurements/native-projection-cache-pins-2026-09-30.json),
with nine successful saves and the exact pinned position preserved through
collapse, reopening and undo. Production expansion and restored-pin images were
inspected. The production overview remains visually dense, and its open Details
panel obscures part of the assembly; this is not a readability/comprehension pass.

Run B recorded a **1,058 ms renderer handoff/extraction span while unfocused**
without any screenshot capture. It overlaps the second package page's worker
solve and accounts for its final 247 ms publication wait. The span includes waiting
for the render world, executor work, extraction and dispatch; it does not identify
a GPU or OS cause by itself. Its main-loop gap was 1,062 ms. Cached collapse still
took 150–156 ms across the three runs. Keep these failures alongside the fast hits:
the responsiveness and universal cached-navigation gates remain open.

```sh
cargo run --offline --locked --example production_smoke -- \
  visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl \
  /tmp/visualyze-production-trace --persist --composed --trace-frames --repeat-navigation
# Repeat with a new output directory and --no-captures; omit --trace-frames for a control.
```

Tracing is opt-in and bounded to 8,192 spans. It preserves the existing schedule
and extraction callback, including shutdown, and never forces focus/update modes.
Common-clock job instants distinguish worker completion from observed publication.
Residual intervals are explicitly labeled, and screenshot CPU spans are nested,
not additive. Reports retain CPU phase summaries and the longest spans; full raw
traces remain at the recorded local paths with hashes. These warm-cache, scripted
native observations do not measure physical input or GPU presentation percentiles.

## Renderer surface investigation, October 2

The source and executables were rechecked against the cache checkpoint before
profiling. An OS stack sample of only the temporary verification process captured
waits along `prepare_windows → get_current_texture → acquire_texture →
CAMetalLayer nextDrawable → semaphore_timedwait`. Its [record](measurements/renderer-sampling-2026-10-02.json)
retains raw-report hashes and local paths. This is an aggregate call graph without
per-sample timestamps; sampler overhead means it is diagnostic evidence, not an
acceptance latency run.

The opt-in proof tracer now brackets Bevy’s existing `prepare_windows` system.
It shares bounded diagnostic records with the render world, using the same clock
as layout jobs and main phases. These intervals include marker scheduling and
concurrent render work; they are not exclusive function/GPU timings. The runtime
viewer and rendering settings are unchanged.

A [new run without the stack sampler or screenshots](measurements/native-surface-trace-2026-10-02.json)
completed 38 actions and 33 successful saves. It recorded 298 surface brackets
with no dropped samples. A **1,068.0 ms acquisition bracket** overlapped
**1,066.4 ms of a 1,070.9 ms renderer handoff**, while the window was unfocused.
Together with the stack sample, this identifies the surface-acquisition path as
the next investigation target. It does not prove that occlusion caused the wait,
that a specific GPU/OS defect exists, or that responsiveness is fixed.

[Validation](measurements/renderer-trace-validation-2026-10-02.json) includes five
example tests, scoped Clippy, workspace formatting and the native build. At that
checkpoint the only source change since the 401-test workspace run was the
optional proof tracer. Its shared ring has a separate 8,192-span cap.
Source/executable hashes were checked before and after the native run.

The subsequent [occlusion trial](measurements/native-occlusion-trace-2026-10-02.json)
adds independent primary-window visibility notifications to that tracer, covered
by a sixth example test. Notifications record when PreUpdate observes them, not
when the OS generated them; unknown initial visibility remains null. The trial
completed all 38 actions and 33 saves but did **not** reproduce the one-second
surface bracket. Its longest renderer handoff happened while the last observed
occlusion was false. This does not justify changing rendering based on focus or
assuming that occlusion caused the earlier pause. The underlying renderer/GPU
responsiveness gate remains open.

## Presentation and occlusion experiments, October 2

The [experiment record](measurements/presentation-probes-2026-10-02.json) adds
example-only presentation controls, per-span requested-mode reporting and an
optional covered-window guard. **No normal viewer setting, production dependency,
placement algorithm or cache version changed.** The workspace rerun passed 450
tests (one intentionally ignored); the production proof helper has 15 passing
tests after review, including an installed render-schedule ordering regression.
Workspace/all-target Clippy, formatting and the example build pass.

The pinned Metal implementation calls `nextDrawable` without a visibility guard.
The upstream [macOS issue](https://github.com/gfx-rs/wgpu/issues/8309) describes a
similar pause; its [merged fix](https://github.com/gfx-rs/wgpu/pull/9141) checks
visibility during acquisition and adds an error result. A
[follow-up](https://github.com/gfx-rs/wgpu/pull/10302) discusses initial-frame
starvation. These are relevant leads, not proof that the same fix is compatible
with this pinned Bevy runtime.

Fifteen native runs used the pinned Bevy source inventory, composed placement,
isolated persistence, frame tracing and no screenshots. Every run completed the
same 38 architecture actions and 33 saves without failure; the action-level
semantic counts match and existing cached-geometry assertions pass.

| Series / requested behavior | Runs | Runs with surface bracket >250 ms | Longest bracket in each run (ms) |
| --- | ---: | ---: | --- |
| Presentation / existing Fifo | 3 | 1 | 22.8, 23.2, 1,068.8 |
| Presentation / Immediate | 3 | 2 | 1,039.1, 21.4, 1,048.6 |
| Presentation / Immediate only when unfocused | 3 | 3 | 1,048.3, 1,019.0, 1,035.7 |
| Occlusion / existing renderer | 3 | 3 | 1,059.1, 1,067.2, 1,068.1 |
| Occlusion / park known-covered windows | 3 | 3 | 1,061.3, 1,055.6, 1,065.5 |

Desktop focus and covering were not standardized. These counts describe the
observed runs; they do not rank modes or estimate failure probabilities. Both
Immediate variants demonstrably fail to eliminate the pause.

The guard parks only windows reported as covered, permits initial presentation,
and restores the same extracted records after rendering. Resuming forces surface
reconfiguration, including dimensions changed while hidden. Its three runs parked
224, 227 and 12 window frames; the last resumed once. All three still paused
**before the corresponding occlusion notification was observed**. This makes an
event-only workaround insufficient. Unknown visibility and visible-but-unfocused
windows continue rendering. Screenshots are prohibited with this experimental
option because their suspension/resumption path has not been implemented.

The trace is a CPU scheduling bracket around `prepare_windows`, including its
acquisition and internal Outdated recovery. Ordinary `create_surfaces`
configuration may occur before the bracket or overlap it; it is not separately
attributed. These intervals are not exclusive function or GPU presentation times.
In guarded runs, architecture acceptance and three settlement updates can occur
while rendering is suspended. They do not prove those scenes were displayed.

An additional sampled run completed normally but did not reproduce the stall.
A desktop-controlled hide/restore attempt did not produce reliable UI-action or
exit verification and is excluded from the table. Its notification-only report
also dropped early main-frame spans. Both limitations and raw artifact identities
are retained in the record. The optional `--await-occlusion-cycle` waits for
notifications; it is not itself a physical-input test.

Reproduce a trial with a new output directory; append `--present-immediate`,
`--present-background-immediate` or `--skip-occluded` for the corresponding probe:

```sh
cargo build -p visualyze --example production_smoke --offline --locked
./target/debug/examples/production_smoke visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl /tmp/visualyze-new-presentation-trial --persist --composed --trace-frames --repeat-navigation --no-captures
cargo test -p visualyze --example production_smoke --offline --locked
```

The presentation experiments used an earlier helper binary; the occlusion series
has its own source snapshot. Later edits clarify report limitations and add a
completion counter/order test. The record distinguishes their hashes from the
final helper instead of claiming all timings came from the final executable.
The next renderer step is a compatible acquisition-level fix, verified for first
draw, rapid hide/restore, resize, screenshots and teardown. Production settings
remain unchanged; responsiveness and the full placement plan remain open.

## Isolated Bevy 0.19.1 compatibility candidate, October 2

The [compatibility record](measurements/bevy-0.19.1-compatibility-2026-10-02.json)
and [reproducible patch](measurements/bevy-0.19.1-compatibility-2026-10-02.patch)
preserve an isolated upgrade to Bevy **0.19.1 / wgpu 29.0.4**. The normal viewer,
its dependency pins, placement algorithms and saved-state versions are unchanged.
The candidate lives at `/private/tmp/visualyze-bevy19-compat`; applying the patch
to a fresh copy of the recorded source reproduces its eight changed files. Patch
application and reverse application were checked without modifying either tree.

The migration preserves pixel font sizes, uses the updated text constructors and
mutable asset guards, adds the mouse-wheel phase to synthetic inputs, and moves
diagnostic ordering to the renamed render set. The custom shutdown drain remains
necessary. Two headless scene fixtures also require Bevy's new skinned-mesh asset
resource; their geometry assertions remain intact. The full candidate rerun
passes **450 workspace tests**, the explicitly invoked production-continuity
test, **15 native-proof helper tests**, formatting and all-target Clippy. The
viewer and every native example build successfully. Initial failures and their
resolutions are preserved in the record.

Two native processes used the same 27,117-object **Bevy 0.18.1 source inventory**
with the newer renderer. Each completed 38 scripted actions and 33 isolated
saves, passed the existing cached-geometry assertions, and exited successfully.
Action-level semantic counts and selections match between runs. The Mac was
locked and screenshots were disabled, so this is functional compatibility
evidence, **not a new display or responsiveness proof**.

The first process recorded a 4,499 ms renderer join near startup; the immediate
repeat's maximum was 121 ms. This suggests a warm-up effect but does not establish
its cause. Acquisition brackets were at most 1.62 and 1.18 ms, respectively. The
new acquisition guard can return without a drawable, so short brackets do not
prove that frames were presented. Collapse acceptance was 116.44 and 99.32 ms;
these observations do not close the navigation target.

Do not adopt this candidate yet. First verify visible startup, gallery/CRUD text
and geometry, resize, actual hide/restore, captures and exit during active work
on the unlocked desktop. Then repeat controlled presentation trials. The record
lists the exact pending commands and inspected upstream source identities. No
performance, comprehension or default-promotion gate has been closed.

## Indexed read improvement, October 2

An independent headless projection benchmark isolated repeated reads as one
contributor to navigation cost. On Unix the bundle reader now uses absolute
positional reads, eliminating the separate seek for each indexed record. It
still reads fresh bytes, checks each checksum, and retains revision/source-change
checks and all paging limits. The other-platform fallback remains seek/read;
there is no added dependency, record cache or resident-memory allowance.

[Three alternating before/after trials](measurements/positional-projection-reads-2026-10-02.json)
used the same pinned production model, 30 projections per case per trial, and no
sampler during measurement. Pooled median projection time changed from **33.78
to 31.50 ms** for fresh grouping requests and **32.61 to 31.38 ms** for retained
groups (6.7% and 3.8%). The benchmark compares full repeated results within each
case outside the timed interval. These are provider projection measurements,
not end-to-end cached navigation or GPU latency.

[Validation](measurements/positional-reader-validation-2026-10-02.json) records
402 workspace tests, six native-proof helper tests, formatting, all-target
Clippy, and the native production flow. A new provider regression reads complete
entity, detail, relationship and evidence records in alternating/reversed file
order with the detail cache disabled. The native check completed 38 actions and
33 saves; the final six repeated visits took 34–52 ms with exact geometry
preserved. Repository collapse remained **142.5 ms**, above the 100 ms target.
The surface pause did not recur in this one trial; that is not a renderer fix.

## Exact routing reuse, October 2

Canonical caches now retain complete routing batches alongside placements under
the same 128-entry / 32 MiB estimated-memory limit. Reuse requires exact model and
frame identities, every box, the domain, clearance, and all rebuilt requests,
including original relation memberships and signed boundary normals. Rebuilding
requests first prevents changed parent attachment lanes from reusing stale
interior routes. Whole batches preserve lane ordering and congestion history.
Final geometry, memberships, boundary joins and metrics are checked again before
publication. Cancellation and rejected reorganization cannot publish cache changes.

[Three alternating before/after pairs](measurements/native-routing-cache-2026-10-02.json)
ran the same pinned production input with fresh isolated persistence, phase
tracing, no screenshots and no OS sampler. Each of the six processes completed
**38 actions and 33 successful saves**, then exited normally. Existing user
previews stayed open; window focus and update settings were unchanged.

| Repository collapse | Before | After |
| --- | ---: | ---: |
| Trial 1, CPU acceptance | 140.24 ms | 121.19 ms |
| Trial 2, CPU acceptance | 143.16 ms | 102.85 ms |
| Trial 3, CPU acceptance | 148.21 ms | 105.45 ms |
| Median CPU acceptance | 143.16 ms | 105.45 ms |
| Median solve phase | 41.03 ms | 8.42 ms |

Every new-build collapse reused one complete routing batch and computed none.
Median acceptance improved **26.3%** and the solve phase **79.5%**. All three
collapse samples still exceed 100 ms. These are a small set of scripted CPU
observations, not physical-input/GPU presentation percentiles or a renderer-stall
fix. The full cached-navigation gate remains open.

For each pair, the final saved repository composition is exactly identical after
excluding work counters: frame geometry/transforms, all route fragments and
memberships, and identity/version fields. Intermediate cached positions are
checked by the native proof. The separate eight-action pin walkthrough completed
nine saves, preserving the exact rendered pin through collapse, reopening and
undo; its restored-pin image was inspected. Tiny interiors and label occlusion
remain visible limitations, not a comprehension pass.

[Validation and provenance](measurements/routing-cache-validation-2026-10-02.json)
record the 12 cache tests, full 406-test workspace, explicit production continuity
test, six native helper tests, formatting, all-target Clippy and native builds.
Regressions cover changed memberships, clearance, model identity and boundary
terminals; the changed-terminal result equals an uncached solve. Existing checks
cover cancellation, deterministic eviction, repair and anchor constraints.
Older saved work counters deserialize with zero routing counts. Source and
executable hashes still match those captured before the paired trials. Geometry,
quality and saved-state versions are unchanged.

## Inherited pins across descendant navigation, October 2

Pins created in a parent composed view now constrain supplied descendant views.
Entering a database or table preserves the exact canonical point and truncates
the locked anchor chain at the entered frame. A shorter local anchor remains
unchanged. Derived constraints never become newly owned registry entries through
navigation. Inherited pins are read-only there; the inspector directs users to
the originating view to change or remove them. Undo restores the full bounded
owned registry, and cancellation restores its last accepted state.

The worker verifies current supplied ancestry before filtering stored paths.
This prevents a stale path from silently omitting a pin when an object moves
into the entered region. Regression tests cover both live reload and cold
restore: a conflict preserves the accepted scene, registry and saved file.
Other tests cover exact fractional transforms, local anchors, deterministic
merging, ownership/frame conflicts, cancellation and bounded storage.

Private composed state is now version 3. Versions 1 and 2 load under their
original semantics and must validate/repair before a version-3 save. This keeps
older readers from treating inherited active pins as newly owned pins. Public
model contracts, dependencies, composition and quality versions are unchanged.

[Validation and provenance](measurements/inherited-pins-validation-2026-10-02.json)
record the full **418-test** workspace and checks above. The final native pin
walkthrough completed **12 actions and 13 saves**, including database/table
entry and return. It retained the canonical point `(18, 6, 10)`, restored the
exact rendered position on parent return, and kept exactly one owned context.
Table and parent-return images were inspected; small interiors and occlusion
remain, so this is not a comprehension pass.

The same final build completed the production source-inventory walkthrough:
**38 actions, 33 saves, zero failures**, and normal exit. Initial CPU acceptance
was 813 ms. The final six repeated visits took 34–52 ms, while repository collapse
took **118.4 ms**, still above the 100 ms target. Peak RSS was 419,004,416 bytes.
These are single warm-cache, scripted native regression runs with isolated
persistence; they do not prove physical input latency, GPU presentation or a
renderer-stall fix. Source and executable hashes match the pre-run capture.

This checkpoint supports parent-to-descendant supplied-object inheritance.
Child-origin pins propagating upward and inheritance through visual groups or
references remain open requirements.

## Region-relative pins in parent views, October 2

The next checkpoint closes the supplied child-origin gap above. A pin created
inside a database now constrains that interior when the database is displayed
within a parent. Its exact local point and original anchor chain remain unchanged;
links above its source region stay free, allowing the parent to fit the region.
Entering a descendant still truncates the anchor at that inspection frame.
Neither direction copies derived pins into the owned registry.

Current ancestry is checked for both source and target contexts before using
stored paths. Reparenting a pinned source into the viewed region cannot bypass
its constraints. Tests cover live reload and cold restore failures, preserving
the accepted scene and original save. A pin on an interior's canonical root
becomes dormant when it is collapsed; it never constrains the same-ID parent
proxy. Compatible previous frames remain individual repair seeds, with a
regression confirming exact positions for unrelated parent objects.

Select an inherited pin and press **Shift+P** to open its originating region.
The inspector names its origin, and editing stays local to that source. Review
caught a retained same-ID group pin incorrectly winning the shortcut's lookup;
the corrected lookup matches the actual effective owner, point, context and
supplied ancestry. A regression follows the shortcut to the correct origin.

Private composed state is version 4. Version 3 is validated using its original
downward-only semantics before repair; versions 1 and 2 retain their previous
interpretation. A valid older parent view with inactive child-owned pins can
therefore upgrade without being mistaken for corruption. No public model,
dependency, composition or quality version changed.

[The final validation record](measurements/upward-pins-validation-2026-10-02.json)
includes **428 workspace tests**, the explicit production-continuity test, six
proof-helper tests, all-target Clippy, formatting and native builds. The native
pin proof completed **22 actions and 23 saves**, including pins in both
directions, collapse/reopen, origin navigation, unpin and undo. Original anchors,
canonical point `(18, 6, 10)`, exact parent-return rendered position, and a single
owned context are asserted. Database and final parent images were inspected;
small interiors and occluded labels remain. Native scripted actions do not prove
physical key delivery; cold restore/cache eviction have separate worker tests.

The final production walkthrough again completed **38 actions and 33 saves**
with no failures and normal exit. Initial CPU acceptance was 707 ms; the final
six repeated visits took 34–50 ms. Cached repository collapse was **109.1 ms**,
still above the 100 ms target. Peak RSS was 420,184,064 bytes. Source and binary
hashes match their pre-run capture. This single warm-cache regression run does
not establish a latency improvement, GPU presentation percentile or renderer
fix. Group/reference inheritance, arbitrary topology continuity, calibrated
quality comparison and human comprehension remain open.

## Boundary-driven placement refinement, October 2

Complete cross-frame routes now guide bounded horizontal alignment of newly
placed interiors. A terminal moves toward its real boundary attachment while
keeping box sizes, frame domains, bands and dyadic scales unchanged. Attached
interfaces move with their owner; nested proxies carry an exact translation of
their existing interior. Pins and fixed hosts remain hard constraints.

The solver reserves up to four default feedback trials inside the same total
work budget. Every attempt, including invalid or unroutable candidates, is
charged. It rebuilds complete boundary requests and routes, validates all joins
and geometry, then accepts only lower structural cost with non-increasing total
canonical routed length. Failed trials retain the best validated composition.
Cancellation cannot publish a partial layout or cache. Normal navigation and
repair freeze existing/reused interiors; explicit reorganization can search
free frames without modifying the accepted cache.

A controlled fixture improves routed length **569 → 256** and bends **2 → 0**,
with two accepted trials. Tests assert unchanged sizes, domains, bands and
scales, input-order determinism, pins, tiny budgets, cancellation and cache
isolation. A three-frame case moves a proxy while preserving its exact interior
and revalidates the complete original relationship through all boundary joins.
Private state version **5** adds feedback counters with checked shared-budget
accounting; field-absent version-four files remain readable. Pin semantics remain
version-specific during migration. No public contract, dependency, quality or
composition version changes.

[The validation record](measurements/cross-frame-feedback-validation-2026-10-02.json)
includes 437 workspace tests, the explicit retention test, six native-helper
tests, Clippy, formatting, builds, source hashes and every following probe.
Independent read-only review found no actionable integration defect.

| Expanded region | Route length, feedback 0 → 4 | Bends, 0 → 4 | Structural cost, 0 → 4 | Composition ms, 0 → 4 | Accepted composed trials / attempts |
| --- | ---: | ---: | ---: | ---: | ---: |
| Data/messaging database and table | 346 → 324 | 9 → 3 | 39.14 → 37.24 | 9.54 → 11.28 | 1 / 4 |
| Distributed AI data region | 2,562 → 2,526 | 71 → 69 | 112.98 → 110.85 | 76.40 → 111.25 | 1 / 4 |
| Bevy ECS within repository | 37,008 → 35,725 | 880 → 835 | 1,131.83 → 1,084.92 | 414.18 → 701.88 | 2 / 4 |
| Bevy World within source file | 392 → 392 | 0 → 0 | 27.11 → 27.11 | 118.73 → 118.71 | 0 / 0 |

All eight probes passed geometry and original-membership validation. The zero
control disables both local kernel feedback and composed refinement and changes
shared work allocation. These are single sequential policy-control development
comparisons, not isolated causal timings, held-out evaluation or a release gate.
The complete profiles can trade individual metrics: congestion increased in
some frames despite better total quality; dense-view occlusion remains high.
Additional rerouting adds solve time. World had no applicable boundary work.

```sh
cargo run -p visualyze-layout --example composed_layout --offline --locked -- \
  public/examples/data-messaging.bundle.jsonl - crud datastore:orders table:orders --feedback 4
cargo run -p visualyze-layout --example composed_layout --offline --locked -- \
  visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl \
  repository:05317a01f80959f2 code package:d1c2531f3a0a9b57 --feedback 4
# Repeat with --feedback 0 for the policy control; all four cases are in the record.
```

The final native pin proof completed **22 actions and 23 saves**; its parent-view
image was inspected. Small interiors and obscured labels remain. The production
proof completed **38 actions and 33 saves**, with all exact-return checks passing
and normal exit. Initial CPU acceptance was **839 ms**, final six repeated visits
**37–54 ms**, and cached repository collapse **116.9 ms**, still above target.
Peak production RSS was **391,266,304 bytes**. All 106 source and four binary
identities matched the pre-run capture.

The no-screenshot production run reproduced a **1,058.7 ms render handoff**
overlapping a **1,057.8 ms surface-acquisition bracket**. Last observed occlusion
was false; the window was unfocused. The second package page took **1,676 ms**,
including a 757 ms publication wait. This is another unresolved responsiveness
failure, not proof of an occlusion cause or GPU remedy. Native scripted actions
do not measure physical-input delivery or presentation percentiles.

At this checkpoint the placement pass started from an already valid routed
composition. The following checkpoint adds bounded failure recovery; routing-space
enlargement, broader
congestion feedback, group/reference overrides, calibrated quality and human
comprehension remain open.

## Initially blocked composed connections, October 2

The next checkpoint adds recovery before a complete routed assembly exists. On
an actual fragment-routing failure, the router derives a short orthogonal
corridor using the same lane-aware terminals. Its endpoint surfaces, boundary
normals and domain stay legal; up to sixteen unrelated inflated obstacles are
identified. This witness guides search and is never rendered as a connector.
Invalid input, cancellation and failures without a corridor produce no invented
evidence.

Recovery can move an eligible obstacle and its attached subtree horizontally
beyond that corridor. It preserves box sizes, frame domains, layers, scales,
pins, fixed hosts and accepted/reused interiors. A move that cannot fully clear
the corridor inside the domain is rejected. Every attempt—including overlap
rejections—uses the existing shared feedback budget. A still-blocked seed can
advance only after reaching a later failed frame/relationship or reducing the
same failure's distinct blockers. It is not published. The first complete result
is independently validated; remaining trials may improve it. Exhaustion returns
the original named failure and leaves the accepted scene/cache unchanged.

Thirteen new tests cover router witnesses, bounded move generation, composed
recovery and public solver/cache behavior. One public fixture seals a target
inside its child frame: independent frame placement succeeds, boundary routing
fails without feedback, and feedback opens a complete route while all pinned
components remain fixed. Each control performs exactly one initializer per
frame; the recovery run explicitly adds its trials to the total allowance.
A subsequent revision pins the blocker shut and fails while preserving the old
cache and revision watermark; the old view still restores identical geometry
with no evaluations.

A separate exact-geometry fixture blocks the child's fixed entry at
`(50, 100, 50)`. A twelve-unit horizontal blocker move opens the same entry and
normal. Two touching blockers require two moves: a one-trial allowance returns
failure, and two trials produce a complete validated route. A pinned wall causes
an overlapping first proposal to be rejected and charged before the second
proposal succeeds. Tests check full relationship membership, boundary joins,
fixed geometry, input order, cancellation and pinned/frozen failure behavior.

[The complete validation record](measurements/blocked-route-recovery-validation-2026-10-02.json)
contains **450 workspace tests**, the explicit retention test, six proof-helper
tests, formatting, all-target Clippy, native/probe builds and final source hashes.
Independent read-only review found no actionable integration defect. The eight
representative probe reports from the preceding checkpoint remain identical in
every field except elapsed time; these already-valid cases provide regression
evidence, not new recovery or held-out quality coverage.

```sh
cargo test -p visualyze-layout --test composed_recovery --offline --locked
cargo test -p visualyze-layout composed::solver::recovery_tests --offline --locked
```

Native regression checks again completed **22 pin actions / 23 saves** and
**38 production actions / 33 saves**, with zero failures and normal exit. The
final parent-view image was inspected; small nested content and obscured labels
remain. Initial production CPU acceptance was **671 ms**, final six repeated
visits **33–51 ms**, and cached repository collapse **117.3 ms**. Peak RSS was
**390,266,880 bytes**. All 108 source and four executable identities match the
pre-run capture. Native checks do not display the sealed headless fixture or
prove physical-input delivery and comprehension.

The no-screenshot run again recorded a **1,067.8 ms render handoff** overlapping
a **1,065.7 ms surface-acquisition bracket**, while unfocused and with last-observed
occlusion false. The renderer pause and cached-navigation target remain open;
this checkpoint makes no latency improvement claim.

Recovery is bounded and greedy. Search exhaustion does not prove the constraints
impossible. Enlarging routing space, jointly changing parent attachment choices,
broader congestion negotiation, group/reference overrides, quality calibration,
scale/performance gates and actual human comprehension remain incomplete. The
private placement algorithm key advances to revision 2; state stays version 5,
composition version 1, quality version 5 and the production default is unchanged.

## Quality profile 5 checkpoint

The Layered initializer now lifts nested endpoints to their sibling representatives,
performs deterministic crossing reduction, and packs variable rectangular boxes
in depth within policy bands. Its five new regressions cover lifted crossings,
variable dimensions, direct auth alignment, pins/cancellation, shared endpoints,
cycles, parallel claims and disconnected peers. All 28 historical comparison
layouts validate, and the 21 non-Layered metric rows are unchanged apart from
version and timing. The old one-row Layered control understated its quality.

Against the strengthened control, Floorplan's per-fixture median improvement is
**10.84% in routed length, 8.65% in structural cost, and 5.26% in enclosing volume**.
It wins four fixtures, ties one, and loses two on length/cost; median bend
improvement is zero. Layered is better on the two public-reference overviews.
This is a more credible development comparison, not a reason to promote the
engine before calibration and comprehension testing. Exact rows are in
[profile-5 comparison data](measurements/placement-quality5-2026-09-30.csv).

The [evaluation harness](../private/layout/evaluation/README.md) records fixture
SHA-256 hashes, exact view/page/expansion requests, algorithms, budgets,
coefficients, normalization equations, compiler artifact and source identities.
It rejects stale freezes and missing/failed cases. Its final three training and
sixteen development views produced **76 valid layouts**, including expanded
nesting, logical/code/deployment views, messaging, database expansion and a real
second page from a 160-table fixture. See the [training report](measurements/quality5-training-verified-2026-09-30.json),
[development report](measurements/quality5-development-verified-2026-09-30.json) and
[freeze](../private/layout/evaluation/quality5-verified.lock.json). Two earlier failed
reports are retained: the initial manifest requested a nonexistent page from the
80-table grouped view; the next run exposed request-order normalization in the
harness. No placement or scoring behavior was changed to improve those results.
These fixtures are development evidence. Weight calibration, a fresh held-out
cohort and an update corpus remain open.

Continuity now separately reports retention of each object's nearest three
surviving comparable siblings, with exact half-unit centers and stable ID ties.
Added/removed/reparented objects, changed representation membership and changed
ancestor frames have explicit exclusion counts. No predecessor or comparable
neighbors produces no ratio, rather than a perfect score. This diagnostic does
not change structural cost or movement weights; cohorts of four or fewer retain
all possible neighbors, and callers must supply the same logical coordinate
frame. Nine targeted checks cover these boundaries and backward serialization.

Reorganization dismissals now persist atomically, capped at 32 identities scoped
by model, root and effective lens. The versioned private fingerprint includes box
geometry/membership and original directed claim kinds/aspects. Rerouting the same
arrangement and claims remains dismissed intentionally; changed geometry or claim
semantics can produce a new proposal. Restart and harmless reload tests preserve
exact accepted boxes/routes. Old files default to no dismissals, while malformed,
duplicate, incompatible or oversized dismissal records preserve the original file.

## Canonical-frame foundation checks

The new headless composition APIs pass 38 focused checks: 18 exact-frame/pin
tests, 17 extraction tests, and three cross-module connector joins. Real expanded
database/table and solid-focus/interface/reference fixtures are extracted into
kernel frames, solved with all four algorithms and validated. These checks cover
bounded parent proxy dimensions, faithful LCA edge topology, exact padded domains,
input ordering, malformed ownership/coordinates, cancellation and anchor-frame
pins. The connector examples route the parent first, inverse-map its chosen
attachment, route the child from that exact terminal, and check the common
presentation point. They cover straight auth-to-middleware, an unrelated parent
obstacle detour and a valid corner attachment. All 26 router unit tests also pass, including fixed faces,
normal direction, tampering, bounds and self-return paths.

Independent review also added regressions for interface pads escaping an enclosing
open group and for an in-range one-link transform near opposite coordinate limits
failing only because of an unnecessary identity composition. Both geometry
validators now enforce every open ancestor while permitting solid-owner protrusion;
transform composition retains the valid explicit origin and pin locks.

The [final comparison](measurements/canonical-final-quality5-2026-09-30.csv)
has 28 valid rows. Every reported non-timing field matches the previous profile-5
comparison. This verifies the reported baseline measurements, not an exhaustive
proof of identical geometry on all inputs. Existing production solve/render
callers do not yet invoke these APIs; the saved flat solution format is unchanged.
The [validation record](measurements/canonical-foundations-validation-2026-09-30.json)
includes commands, counts and source hashes. Earlier foundation comparison records
remain historical checkpoints before the final review fixes.

## Original renderer quantitative control

The new `legacy_control` example invokes the real original Architecture
composition and connection functions without a GPU. Its shared floating geometry
adapter preserves all centers, dimensions and path points, including off-surface
attachments. It compares seven fully expanded development snapshots against
Layered, Floorplan, Constrained and Treemap under the same projected semantics,
32-candidate work budget, geometry audit and fixed measurement scales.

The [final development record](measurements/legacy-renderer-control-final-2026-09-30.json)
contains **35 rows**, with pre/post source and executable identities checked. Its non-timing rows exactly match the [earlier development run](measurements/legacy-renderer-control-2026-09-30.json). All **28 new-solver rows pass** both the exact validator and
the supplemental float audit; all **seven original-renderer rows have geometric
violations**. Those rows remain diagnostic measurements and do not enter
valid-winner aggregates. The original renderer's connectors often terminate
0.04–0.08 scene units off a box face; other failures include size domains, layer
ordering, enclosure exits and obstacle clearance. No repair is applied before
measurement. This is useful evidence about the actual old renderer, not proof of
quality or comprehension superiority.

| Fully expanded view | Original routed length | Floorplan routed length | Original / Floorplan volume including routes |
| --- | ---: | ---: | ---: |
| Notes CRUD | 66.61 | 26.40 | 499.21 / 414.72 |
| Sketch CRUD | 82.50 | 16.80 | 861.47 / 393.98 |
| Shared SaaS | 359.76 | 88.80 | 1,503.42 / 746.50 |
| Nested scopes | 145.15 | 49.60 | 2,409.13 / 1,651.68 |
| Fan-out peers | 3,068.82 | 455.90 | 4,647.00 / 1,524.10 |
| Online Boutique | 292.20 | 68.60 | 1,863.00 / 743.04 |
| DeathStar hotel | 713.95 | 164.60 | 2,057.85 / 1,382.40 |

Lengths are scene units; volumes are cubic scene units. The original numbers are
for **invalid geometry under current requirements**. Layered remains better than
Floorplan on routed length for the two public fixtures (63.00 and 124.20). All
views expose every supplied interface and region; these are not necessarily the
same projections as older default-view comparison tables. The control rejects
unsupported abstraction or ownership rather than silently comparing different
problems. Adapter tests cover capture fidelity, normalization, input reordering,
changed directions/membership/endpoints, clearance and scope reentry. See the
[evaluation instructions](../private/layout/evaluation/README.md#original-renderer-control)
for exact diagnostic definitions and measurement bounds.

## Complete headless canonical solve

The new `composed::solve_composed` combines the foundation APIs into independently
placed interiors with complete directed routes. Eight integration tests cover all
four placement initializers, exact deduplicated presentation, growing interiors
without moving parent proxies, canonical root/leaf pins, original surface
endpoints, cycles/parallel claims, ordering, cancellation and real snapshot/bundle
views. An unobstructed auth-to-middleware route remains vertical through three
scaled frames with zero bends.

Eight independent validator suites construct their own canonical geometry and
fragments, then reject missing/extra/reordered relationships, altered memberships,
gaps, wrong normals, obstacle penetration, duplicate complete paths, stale
identity, invalid work/pins and excessive storage. Capacity is checked before
placement when expanding relationship copies would exceed the bounded result.
Six strict-budget integration tests and one rejected-feedback unit regression
verify one total candidate allowance across initializer, repair, feedback and
size-fallback trials. These 23 new checks bring the workspace total to 342.
The [validation record](measurements/composed-validation-2026-09-30.json) retains
commands, counts and source hashes. The additive budget path leaves the existing
solver's [28 comparison rows](measurements/composed-legacy-quality5-2026-09-30.csv)
valid, with every reported non-timing field unchanged from the profile-5 baseline.

The [headless measurement record](measurements/composed-headless-2026-09-30.json)
contains exact commands, source/binary hashes, production provenance and full
results. Four sequential runs passed complete composed geometry validation and
resolved every displayed membership to supplied records:

| View | Frames | Objects | Relationships / fragments | Composition time, excluding indexing and projection |
| --- | ---: | ---: | ---: | ---: |
| Declared database expanded to table/columns | 3 | 12 | 8 / 10 | 12.68 ms |
| Bevy repository with ECS package expanded | 3 | 63 | 180 / 338 | 492.60 ms |
| ECS package with World file expanded | 3 | 100 | 32 / 32 | 94.70 ms |
| World file with its type expanded | 2 | 97 | 33 / 33 | 112.69 ms |

These are single warm-access development observations on the documented Apple
M5 machine, with existing native previews left running and no concurrent build.
The three Bevy views each disclose two remaining projection queries; their
visible relationships are complete, while the whole source inventory is not
instantiated. The input retains 27,117 source objects and 44,019 relationships.
This is neither native rendering evidence for the new composition path nor a
performance release gate.

Canonical quality counts each actual fragment once and includes padded routing
domains, using the fixed preferred-size scales of the same frame partition.
Presentation compression is reported separately. The independent validator
certifies geometry and internally consistent work records; metric values are
fresh solver measurements, not replay-certified evidence. This headless checkpoint predates independent interior caching, previous-composition
repair, native accepted-state integration and reversible persistence, which are
covered by the subsequent native checkpoint below. General cross-frame placement
feedback and full durable override continuity remain open.

```sh
cargo run -p visualyze-layout --example composed_layout --offline --locked -- public/examples/data-messaging.bundle.jsonl - crud datastore:orders table:orders
cargo test -p visualyze-layout --test composed --test composed_validation --test solver_budget --offline --locked
```

## Native proof after containment hardening

Before the new composed path was added, tightening the production geometry
validator was followed by a freshly built native
production proof completed all 21 actions and exited normally. Its 16 layout
jobs passed the phase/cache audit, all 16 isolated saves succeeded, and the package
screenshot was inspected. Initial CPU acceptance was 918 ms including 418 ms
indexing; the slowest measured navigation miss was 769 ms and cache hit 74.13 ms.
Peak process RSS was 357,662,720 bytes. The
[complete repeat](measurements/native-canonical-validator-production-2026-09-30.json)
records its input-isolated, recently accessed-bundle conditions. The synthetic
10k/100k/million observations below precede this containment hardening; they were
not rerun for a change limited to interface/open-ancestor geometry validation.

This historical native evidence exercises the flat accepted-layout path. The
subsequent composed native checkpoint adds runtime integration through caches,
navigation, pins, persistence and undo; general cross-frame feedback and full
override continuity are separate requirements. Neither checkpoint closes Stage 3.
The [ADR](decisions/0006-placement-engine.md#canonical-composition) records these
boundaries explicitly.

## Opt-in native composition checkpoint

`--layout composed` now retains the authoritative composed solution, with shared
fractional display geometry for rendering, picking, labels and layer separation.
Canonical placement caches are independently bounded; at this historical
checkpoint every route was recomputed and revalidated. The October 2 routing
reuse extension described above retains exact batches under the same cache budget while
preserving final validation. Reorganization searches do not mutate the accepted cache; Apply and
Undo publish validated interiors and invalidate dependent full-view entries.
Current-view pins, anchor chains, accepted geometry and dismissed proposals have
separate versioned persistence, with atomic writes and corrupt/future-file
preservation. Numeric rendering checks reject unsafe coordinate precision before
publication. These checks do not establish that tiny interiors are legible.

The [composed native production record](measurements/native-composed-production-2026-09-30.json)
includes the pinned syntax-v2 bundle, exact command, pre-run source/binary hashes,
all actions/jobs, screenshot hashes, measured memory and limitations. The native
process completed **26 actions, 21 accepted jobs and 21 successful saves**, with
zero save failures and exit status 0. It traversed Bevy's 27,117-object / 44,019-relation
source inventory and rendered both mixed-detail cases:

| Expanded view | Canonical frames | Visible objects / paths | Largest scale exponent | Action CPU acceptance |
| --- | ---: | ---: | ---: | ---: |
| Repository with ECS interior | 3 | 63 / 191 | 6 | 239.04 ms |
| World source file with type interior | 2 | 97 / 33 | 4 | 54.12 ms |

Expanded screenshots were inspected. The overview has dense crossing projections
and many occluded labels; the open Details panel also covers part of the assembly.
This is successful nested rendering, not a comprehension or readability pass.
The screenshot captures and bounded projection disclose additional unloaded pages.
Original relationship direction/membership, path isolation and exact returned
geometry are checked by the executable proof.

Initial CPU acceptance was **2,927.95 ms**, including **1,533.69 ms** indexing.
The slowest detail action took **2,598.61 ms**, including **2,457.96 ms** solving;
this exceeds the one-second target. The slowest whole-view cache hit was
**97.84 ms**, while collapsing the repository using canonical placement reuse
required **164.19 ms** end-to-end (136.99 ms attributed to its layout job), exceeding
the cached-transition target. Worker-active CPU update intervals reached
**1,028.32 ms**, with a 64.04 ms p95 across heterogeneous updates; no cause is
established by this observation. Peak RSS was **359,645,184 bytes** and measured
peak memory footprint **765,068,560 bytes**. No memory target failure was observed
for this bounded production workload.

These are single warm-file-cache observations on the Apple M5, with existing
preview processes preserved and no concurrent build. Actions were injected through
the viewer API; timestamps are CPU acceptance, not hardware-input or GPU-presentation
latency. Existing million-input records use the flat engine and cannot close the
new composed engine's large-model performance gate.

Three subsequent sequential [render checks](measurements/composed-render-checkpoint-2026-09-30.json)
also exited 0: the seven-object gallery, the eight-object/nine-path legacy Notes
assembly, and the nine-object/ten-path composed CRUD cube. All three images were
inspected. The new source record is a checkpoint before durable inactive/dormant
pin work: collapsing pinned interiors, inactive-view restart recovery and recovery
after prior-composition eviction still need their own implementation and tests.

```sh
cargo build -p visualyze --example production_smoke --offline --locked
./target/debug/examples/production_smoke visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl visualyze-local/production/<new-output-directory> --persist --composed
./target/debug/examples/layout_smoke --model public/examples/sketch-crud.json --layout composed
```

## Durable hidden/interior pin checkpoint

Pins now retain a checked exact anchor record independently of cached layout
history. A collapsed supplied interior keeps its full constraints in a bounded
registry while sending only the visible subset to the solver. Reopening validates
the supplied ownership and restores the original anchor even after both caches
are emptied. The registry also preserves inactive coordinate contexts across a
restart. Removed/reparented supplied objects, conflicting active records, invalid
exact transforms and capacity violations fail before publication or replacement
of the previous saved file. Version-one state loads without a write and migrates
to version two only through validated acceptance/save.

Four new core tests exercise durable records, empty-cache solving/reorganization,
constraint conflicts, determinism, cancellation and strict work budgets. Runtime
lifecycle tests cover collapse→restart→empty caches→reopen; enter→restart→return;
hidden reparenting; pre-publication registry limits; and existing unpin/undo/save
behavior. Persistence tests add version-one migration and corrupt registry
preservation. The final workspace and example checks are recorded above.

The [native pin walkthrough](measurements/native-durable-pins-2026-09-30.json)
completed **eight actions and nine successful isolated saves**, with exit 0. It
expanded a database and table, pinned a column, collapsed the database, reopened
it, unpinned and undid the change. The original and final rendered column positions
match exactly. The collapsed saved state retains its dormant pin; Unpin removes
it and Undo restores it. Three screenshots were inspected. This native process
proves rendering and saved state through the sequence; restart/cache-eviction
coverage comes from the separate runtime/core tests.

A [fresh composed production repeat](measurements/native-durable-production-2026-09-30.json)
completed **26 actions, 21 accepted jobs and 21 successful isolated saves**, with
exit 0. The mixed-detail screenshots were inspected. Initial CPU acceptance was
**1,143.40 ms** and the slowest detail action **909.79 ms**. However, the cached
repository return took **1,222.24 ms**, including **1,051.77 ms** between worker
completion and publication. The cached canonical collapse took **139.50 ms**.
Worker-active CPU update intervals reached **1,085.40 ms**. Peak RSS was
**386,760,704 bytes**, and peak memory footprint **780,207,424 bytes**. The previous
slower detail observation remains recorded; the faster repeat does not erase it
or establish latency percentiles. No cause is established for the main-loop delay.
The pinned Bevy inventory remains 27,117 supplied entities / 44,019 relations.

Both native runs were sequential after builds/checks, using recently accessed
files and preserved user previews. Sources and executables were hashed before and
after all runs and remained unchanged. These direct viewer-action proofs do not
verify hardware input delivery or GPU presentation latency. Native persistence
uses isolated directories, leaving the user's saved views untouched.

Pins are still scoped by model, root, lens and policy. Entering a child preserves
its parent's constraints for return; it does not reinterpret those parent anchors
inside the child's coordinate system. Generated groups and reference proxies lack
a supplied ownership chain, so hiding their pins still produces an explicit
conflict. General cross-frame placement feedback, dense/deep readability,
responsiveness, composed large-model performance, calibrated held-out evaluation
and human comprehension remain open.

```sh
cargo run -p visualyze --example composed_pin_smoke --offline --locked -- public/examples/data-messaging.bundle.jsonl visualyze-local/production/<new-output-directory>
```

## Instrumented native observations (profile 5)

### Input-isolated repeat

The latest four processes completed after the timing boundary correction and
example-only desktop-input isolation. They ran sequentially, with both user
previews open and no concurrent builds. Production used fresh isolated saved
state; synthetic proofs disabled persistence. Bundles were recently accessed;
the OS cache was uncontrolled. These are direct-action native checks, not
physical-keyboard/mouse or GPU presentation latency measurements.

| Input | Initial CPU acceptance | Indexing in first job | First projection / solve | Slowest fresh navigation | Slowest actual cache hit | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Bevy: 27,117 entities | 1,133 ms | 609 ms | 9.17 / 0.38 ms | 753 ms | 80.16 ms | 341,147,648 bytes |
| 10,000 synthetic entities | 519 ms | 61 ms | 2.82 / 138.46 ms | 402 ms | 34.80 ms | 290,340,864 bytes |
| 100,000 synthetic entities | 1,128 ms | 642 ms | 17.37 / 197.73 ms | 737 ms | 37.02 ms | 363,331,584 bytes |
| 1,000,000 synthetic entities | 6,710 ms | 6,295 ms | 54.62 / 83.63 ms | 352 ms | 36.52 ms | 1,113,128,960 bytes |

The production proof completed 21 actions; each synthetic proof completed 46.
An independent report audit checked all 151 layout jobs: contiguous unique IDs,
matching revisions, accepted outcomes, exclusive worker phase sums and no overlap
between outer phases. It allowed only floating-point rounding (64 ULPs), with no
wall-time overlap allowance. Warm navigation performed no indexing or snapshot
reads; all 49 actual cache hits performed no solving. The million-input first job
took 6,451 ms including indexing. Its sampled CPU worker-update interval p95 was
19.52 ms, maximum 61.70 ms. Fresh production-package and million-input region
screenshots were inspected; this is not a comprehension evaluation.

Full records: [production](measurements/native-quality5-isolated-2026-09-30.json),
[10k](measurements/native-10k-quality5-isolated-2026-09-30.json),
[100k](measurements/native-100k-quality5-isolated-2026-09-30.json),
[million](measurements/native-million-quality5-isolated-2026-09-30.json), and
[summary](measurements/native-quality5-isolated-summary-2026-09-30.json).

An intervening 100k repeat exited 101 with `No current reorganization proposal`;
the [failure record](measurements/native-quality5-repeat-failure-2026-09-30.json)
is retained. The script never requested Preview, but physical input was still
enabled. A regression reproduces the exact error with KeyV through real runtime
controls and verifies its absence with input isolation while a queued action
still succeeds. The original OS event was not recorded, so its provenance remains
unproven. The shared proof-only helper clears raw input, converted states, deltas
and hover before runtime controls. Normal viewer input and native window
activation behavior are unchanged. The remaining million run in the failed
sequence was never started. The new complete runs do not erase that failure.

### First instrumented pass

Fresh native processes ran sequentially after builds and headless checks on the
Apple M5 reference machine, with both user previews left open. Production uses
an isolated saved-state directory; scale proofs disable persistence. The bundles
had been accessed recently, so these are not disk-cold imports. Each job now
records exclusive indexing, snapshot reading, source-lock waiting, projection,
cache restoration, solving and preparation, plus worker queue/publication delay,
CPU acceptance, scene-command construction and saving. Every projection lock
acquisition is timed, including reprojection. Bounded histories carry action,
job and revision IDs; cancelled/stale workers cannot rewrite published records.

| Input | Initial CPU acceptance | Indexing in first job | First projection / solve | Slowest fresh navigation | Slowest actual cache hit | Peak RSS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Bevy: 27,117 entities | 1,212 ms | 429 ms | 7.90 / 0.46 ms | 755 ms | 78.55 ms | 342,327,296 bytes |
| 10,000 synthetic entities | 1,176 ms | 198 ms | 12.79 / 167.95 ms | 385 ms | 34.48 ms | 257,409,024 bytes |
| 100,000 synthetic entities | 1,708 ms | 791 ms | 25.05 / 198.89 ms | 754 ms | 36.19 ms | 330,366,976 bytes |
| 1,000,000 synthetic entities | 6,668 ms | 6,212 ms | 49.54 / 83.71 ms | 267 ms | 36.24 ms | 1,021,837,312 bytes |

Production completed 21 actions, with seven measured layout-cache hits and eight
navigation misses. Each scale proof completed 46 actions across 29 regions, with
fourteen actual hits and thirty navigation misses. Search/view-only actions do
not invent layout jobs. All warm navigation jobs recorded zero indexing; every
actual cache hit recorded zero solving. The first job is an additional miss.
Initial app acceptance begins before renderer setup, so it is longer than the
first layout job alone; those clocks must not be equated. In the million-input
run the first layout job took 6,356 ms, including 6,212 ms indexing. Its sampled
CPU worker-update interval p95 was 19.83 ms, with a 35.88 ms maximum. Screenshots
from the production package and a million-input local region were inspected.

These observations fit the selected CPU navigation and memory targets; they do
not prove latency distributions across repeated process starts, real OS input,
GPU presentation, arbitrary dense graphs or human comprehension. Initial million
import time is explicitly separate from layout. Complete records include the
[production run](measurements/native-quality5-phases-2026-09-30.json),
[10k run](measurements/native-10k-quality5-phases-2026-09-30.json),
[100k run](measurements/native-100k-quality5-phases-2026-09-30.json),
[million run](measurements/native-million-quality5-phases-2026-09-30.json) and
[compact summary](measurements/native-quality5-phases-summary-2026-09-30.json).

A separate headless index probe attributes the million-input scan to about
6.22 seconds opening/reading/decoding/validating records and incrementally indexing
metadata, then 0.26 seconds finalizing ownership/lens/endpoint indexes and provider
validation. It loaded no detail cache or scene. The instrumented wrapper exited
normally at 784,744,448 bytes RSS. An earlier sandboxed wrapper wrote its JSON but
failed to obtain macOS resource counters and is not counted as a successful wrapper
exit. [All phase samples and caveats](measurements/index-phases-2026-09-30.json)
are retained. This identifies import work as the main startup cost on this corpus;
it does not establish a disk-cold import percentile.

## Frozen initial comparison (historical profile 1)

Native Apple M5, 16 GiB RAM, macOS 26.6.2, Rust 1.96.0, optimized development profile, 32 candidate evaluations, up to 8 routed candidates, clearance 2 and band gap 12. Costs are provisional fixed coefficients, not human-calibrated weights. Notes and the declared sketch were development fixtures; the three synthetic distributed fixtures were added afterward without tuning those coefficients.

All 20 fixture/algorithm combinations below passed the common geometry and route validator. Length is in integer layout units (10 units per scene unit). Volume can worsen while proportions and routes improve; report those tradeoffs separately.

| Fixture | Layered length / bends | Floorplan length / bends | Floorplan cost | Layered / constrained / nested cost | Floorplan volume | Floorplan observed solve time |
| --- | ---: | ---: | ---: | --- | ---: | ---: |
| Notes | 268 / 8 | 132 / 3 | 11.254 | 20.649 / 13.781 / 15.267 | 373,248 | 6.3 ms |
| User sketch | 262 / 7 | 166 / 2 | 10.703 | 17.714 / 14.517 / 14.866 | 370,656 | 3.4 ms |
| Shared SaaS | 2,484 / 59 | 888 / 23 | 36.362 | 94.169 / 46.540 / 51.302 | 746,496 | 20.4 ms |
| Nested overview | 172 / 6 | 116 / 4 | 10.777 | 15.864 / 10.777 / 10.777 | 221,184 | 2.1 ms |
| Fan-out peers | 25,398 / 155 | 6,712 / 136 | 174.616 | 562.216 / 175.979 / 216.395 | 2,488,320 | 159.1 ms |

These timings are single-run observations with build time excluded; other runs were slower. They are not latency percentiles or a desktop responsiveness measurement. The fan-out occlusion proxy saturates at 1.0 and remains a readability concern. Floorplan volume exceeds the layered volume on all five fixtures. Median routed/combined cost improves, but uniform packing-volume improvement is not claimed.

The expanded nested-scope regression validates 12 routes with floorplan length 496 and 7 bends; the layered candidate uses length 1,000 and 18 bends. Single-target auth alignment and a straight waitlist passage through the intermediate band have dedicated tests. Shared auth/store identity, cycles, parallel/reverse edges, input reordering, conflicting pins, stale revisions, integer extremes, local updates, and cancellation are covered separately.

The exhaustive small packing oracle enumerates both orderings for up to six fixed rectangles (518,400 pairs at six). Fixed-port channel search is compared against independent bounded voxel Dijkstra on three obstacle courses. Optimality claims are limited to these reduced problems.

## Pinned public references

Two additional partial fixtures were derived from inspected primary sources at
verified Git commits. Every entity and relationship is `supported` by evidence
whose URL includes that commit. Both snapshots passed the full model validator
with zero warnings. These sources describe public examples, not any company's
private production architecture.

- **Online Boutique:** [GoogleCloudPlatform/microservices-demo at `38e7348eb289`](https://github.com/GoogleCloudPlatform/microservices-demo/tree/38e7348eb289eb5b87c0c6e8cb19ced0449dc389). The fixture contains 13 objects and 28 total relations, of which 16 are configured endpoint dependencies. Its [release manifest](https://github.com/GoogleCloudPlatform/microservices-demo/blob/38e7348eb289eb5b87c0c6e8cb19ced0449dc389/release/kubernetes-manifests.yaml) supports `depends_on` with unknown necessity; environment settings are not promoted to verified requests. The optional shopping assistant is explicitly omitted.
- **DeathStarBench Hotel Reservation:** [delimitrou/DeathStarBench at `6ecb09706140`](https://github.com/delimitrou/DeathStarBench/tree/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation). The fixture contains 24 objects and 54 total relations: 22 [Compose startup dependencies](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/docker-compose.yml), 9 requests verified in [frontend](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/services/frontend/server.go) and [search](https://github.com/delimitrou/DeathStarBench/blob/6ecb09706140f8730b5385c08f1386c654c3c526/hotelReservation/services/search/server.go) client bindings/call sites, and 23 containment relations. Startup dependencies do not assert runtime requests or storage operations.

Exact extraction rules, upstream license-file links, and omitted areas are
documented in [fixture provenance](../private/layout/fixtures/SOURCES.md). No
upstream implementation code was copied into the product workspace.

All eight additional fixture/algorithm combinations passed the same geometry and
route validator, using the unchanged coefficients and 32-evaluation budget.
The measurements below were collected in a separate single run on the same
reference machine; elapsed times exclude compilation.

| Reference | Layered length / bends | Floorplan length / bends | Floorplan cost | Layered / constrained / nested cost | Floorplan volume | Floorplan observed solve time |
| --- | ---: | ---: | ---: | --- | ---: | ---: |
| Online Boutique | 1,848 / 40 | 628 / 16 | 27.532 | 78.984 / 29.976 / 33.350 | 995,328 | 15.3 ms |
| DeathStarBench Hotel | 4,882 / 89 | 1,840 / 52 | 61.490 | 146.639 / 70.802 / 70.475 | 1,382,400 | 48.4 ms |

The combined score and routed length improve on both references, with material
space tradeoffs. Boutique's floorplan volume equals the constrained/nested
baselines and exceeds the layered baseline's 829,440. Hotel's floorplan volume is
more than twice every baseline's 663,552; its aspect ratio of 3.75 also exceeds
the constrained/nested baseline's 3.0, while the projected occlusion proxy improves
from 0.843 to 0.650. These observations support further evaluation of the hybrid
objective, not a claim that floorplanning improves every readability or packing
measure. They do not satisfy the outstanding comprehension gate.

## Large-model measurements

The updated public provider benchmark ran 10,000, 100,000, and 1,000,000 entities. After semantic-lens indexing was added, one million entities plus 999,999 ownership relations took 2.876 s for finalization/indexing/semantic validation, with 833,077,248 bytes peak RSS (about 794 MiB), and 72 µs to query 32 root summaries. The full index remains resident; details are demand-loaded. The earlier 710 MiB result predates the descendant index and indexed labels. See [methodology](../public/schema/model-bundle.md#synthetic-measurements) and [lens-index measured records](measurements/provider-lenses-2026-09-29.json).

The headless scale proof goes through generation, full indexing, bounded view projection, actual obstacle-aware routing, independent geometry validation, and reuse of an accepted arrangement. Its 32-way hierarchy adds a peer dependency for every eighth leaf and a shared dependency for every sixty-fourth leaf. Five regions at different depths plus a repeated root are solved with 48 evaluations and up to four feedback candidates. Every displayed object/route resolves to the supplied records. The test also exhausts the real provider detail cache and verifies that an evicted record reloads identically.

| Entities | Total relationships | Finalize + index + validation | Peak RSS | Projection range | Fresh solve range | Largest visible view |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 10,000 | 11,361 | 31.0 ms | 13,484,032 bytes | 0.19–2.05 ms | 2.6–165.9 ms | 34 objects |
| 100,000 | 113,622 | 274.3 ms | 83,771,392 bytes | 1.49–5.81 ms | 14.0–369.5 ms | 34 objects |
| 1,000,000 | 1,136,229 | 3,245.0 ms | 773,177,344 bytes | 13.24–57.88 ms | 12.2–362.4 ms | 34 objects |

Measured on the same Apple M5 reference machine in fresh processes, using recently written files (warm OS cache). The index column includes the writer's final flush and synchronization. Timing excludes compilation and reports individual observations, not latency percentiles. Only six bounded views are measured per model; their relation pages are not an exhaustive traversal. The repeated root is solved afresh; it is not reported as a native cache hit. These measurements use quality profile 3, before the enclosure-search repair and final congestion scoring. Reusing accepted geometry through the layout API took 0.03–19.88 ms across these samples. All models retain at most 256 detail records (about 1.1 MiB) after real cache eviction. Full machine-readable results are in [placement scale measurements](measurements/placement-scale-quality3-2026-09-29.json).

```sh
cargo build -p visualyze-layout --example scale_view_bench --locked
/usr/bin/time -l target/debug/examples/scale_view_bench 10000 /tmp/visualyze-10k-view.json
/usr/bin/time -l target/debug/examples/scale_view_bench 100000 /tmp/visualyze-100k-view.json
/usr/bin/time -l target/debug/examples/scale_view_bench 1000000 /tmp/visualyze-million-view.json
```

Report paths must be new. An optional final path preserves the generated bundle for viewing. These observations meet the memory target for these synthetic headless workloads. They do not establish renderer memory/frame performance for million-entity input, disk-cold import, arbitrary high-degree graphs, or native cached-navigation latency. No million-object scene is instantiated.

## Native scale checkpoint (quality profile 3)

The real native viewer completed 46 actions over 29 distinct focus regions for each of the three synthetic inputs, comparing exact rendered box/frame geometry, connector point bits, and semantic membership on returns. Fourteen returns follow resident-cache navigation; two return after pressure exceeds the actual 16-entry cache. Those labels are inferred from navigation history, not instrumented cache-hit events. Selection, paging and search remain bounded. Each run captured four rendered scenes.

| Entities | Initial acceptance / screenshot completion | Peak RSS | Largest fresh-detail acceptance | Largest resident return | Process exit |
| ---: | ---: | ---: | ---: | ---: | --- |
| 10,000 | 318 / 445 ms | 298,549,248 bytes | 268 ms | 17.7 ms | Normal |
| 100,000 | 741 ms / see report | 351,371,264 bytes | 520 ms | 18.3 ms | Normal |
| 1,000,000 | 4,052 / 4,172 ms | 1,066,631,168 bytes | 216 ms | 34.7 ms | Navigation finished; shutdown hung |

The million-input process saved its completed report and screenshots, then blocked while dropping Bevy's pipelined render channels. A captured native stack showed the main thread waiting for the render world while the render schedule waited on its executor. Only that test process was terminated. Its 1,542,113,152-byte peak physical footprint and 1,066,631,168-byte maximum RSS both remain below 2 GiB; this is useful memory evidence, not a successful complete process run. The later repair and successful fresh graphical runs are recorded below.

These are individual warm-file-cache runs on the reference machine, with the user's older preview still open. Initial acceptance includes GPU startup and indexing; screenshot completion is a separate observable upper bound, not a GPU presentation timestamp. Reported CPU update intervals exclude both edges of screenshot work and do not measure GPU frame latency. Sparse synthetic success does not cover dense production views or human comprehension. Full action distributions, scope and termination status are in [native scale measurements](measurements/native-scale-quality3-2026-09-29.json).

## Production source ingestion and native display

The official Bevy v0.18.1 source at commit `f667c282dad2c1419afb5836ded22a3ec263970e` was ingested with the separate [explicit Rust bundle producer](../public/tools/rust-bundle/README.md). It parsed all 1,622 tracked Rust files across 81 packages with zero syntax failures, yielding 27,117 entities, 44,019 relationships, and 31,319 evidence records. Two runs produced the same 40,169,426-byte bundle and SHA-256. Coverage remains explicitly partial: syntactic imports are inferred, unresolved/external references are disclosed, and no runtime call or deployment architecture is invented from source.

The first headless production audit solved and geometrically validated 163 regions: the root, repository, every package, and one deterministic file per package. These are bounded first pages, not every possible view. The first native proof completed twelve actions in the actual GPU viewer: repository overview, search, package entry, package paging, source-file entry, type entry, method paging, and cached return navigation. It captured six nonempty rendered scenes and verified exact positional stability on return. The `World` region exposes 150 supplied methods through pages. Initial refined-scene acceptance was observed at 1.424 s with the bundle's OS cache warm; this timestamp does not establish when the GPU first presented it.

The [initial native report](measurements/production-native-initial-2026-09-29.json) is historical evidence of production display. It predates reference-budget pagination repair, grouping controls, and quality profile 2. In that run an oversized external-reference set left some loaded claims unrepresented; the later provider regression traverses all 131 external claims across three pages without dropping any. The historical screenshots alone do not prove that repair. Package transitions took up to 1.61 s and cached returns up to 156 ms including three render-settlement frames. They do not pass the 1 s/100 ms gates. The current native proof records action acceptance separately from settlement and splits worker/idle update intervals while excluding capture. A fresh profile-3 run passed the former enclosure-routing failure but exposed changing suggested groups on returning from a deeply nested file. It failed the exact-geometry assertion and is not counted as a successful proof. The subsequent headless reproduction and portable partial-retention regression fixed group recomposition. A profile-4 native run then passed exact returns; see the current checkpoint below.

```sh
cargo run -p visualyze-layout --example production_audit --locked -- \
  visualyze-local/production/bevy-0.18.1.bundle.jsonl /tmp/bevy-layout-audit.json
cargo run -p visualyze-layout --example production_pages --locked -- \
  visualyze-local/production/bevy-0.18.1.bundle.jsonl /tmp/bevy-pages-audit.json \
  file:b6ae9420da8c77aa item:86c82b85a551c18a
cargo run --locked --example production_smoke -- \
  visualyze-local/production/bevy-0.18.1.bundle.jsonl /tmp/bevy-native-proof
```

A newer 165-region production audit passed with zero loaded claims unrepresented. Its densest package exposed 704 displayed routes and took 12.37 seconds in the headless solve; the corresponding native first acceptance took 14.24 seconds. These fail the complex-view latency target and produce a cluttered screenshot. Router-history indexing, final congestion scoring and caption collision repair were verified against this exact workload. A later bounded-peer projection checkpoint also passes all 165 regions with no loaded claim omitted; its largest solve was 662 ms. That improvement combines algorithmic work with a smaller default reference page, so it is not presented as a like-for-like layout-speed ratio. Deferred claims remain reachable through explicit relationship pages.

Both output paths must be new. The production model and screenshots remain local. This demonstrates ingestion and display of a substantial real Rust codebase; it does not claim a compiler-resolved runtime architecture or a reconstruction of ChatGPT.

The subsequent [all-pages production audit](measurements/production-pages-quality4-2026-09-29.json) validates 234 child/relationship-page combinations across the same 165 sampled regions, covering 175 child pages. ECS contributes 29 page combinations; `src/world/mod.rs` and `bevy_reflect` each contribute 11. Every page passes independent geometry validation, resolves displayed objects and route membership to supplied IDs, and leaves zero loaded claims unrepresented. Cursor checks reject loops and duplicate loaded claims across relationship pages.

This audit independently reads the bundle's raw ownership and code-reference records, then checks that loaded references plus exact internal counts equal the supplied relevant-reference total for every child page. Loaded references that become internal to suggested groups are counted once. The ECS region accounts for all 4,549 relevant supplied references, with 4,417 distinct originals routed across its pages; the others are represented by internal counts. Internal IDs may be bounded samples, as the viewer explicitly discloses. This expands evidence beyond first pages; it is still not an exhaustive traversal of all 25,412 declarations, every possible expansion, pins, or revision combinations. The 35.1-second audit ran alongside compilation, so its individual solve times are diagnostic observations, not performance-gate measurements.

The opt-in `syntax-v2` producer scheme keeps declaration and import IDs stable through tested whitespace, comments, body edits, unrelated declarations and source reordering, while evidence points to the current source revision and lines. It includes lexical scope, self/trait syntax, import aliases and conditional context; identical ambiguous anchors fail before publication. The legacy default remains byte-compatible with a captured fixture. Renames, file moves and conditional/self/trait syntax changes can change IDs. Switching schemes requires a distinct model identity; no saved-layout or pin migration between schemes is claimed.

The [syntax-v2 production record](measurements/syntax-v2-production-2026-09-30.json) ingests the same pinned Bevy source into 27,117 objects, 44,019 relations and 31,319 evidence records, with a distinct model ID and a 40,169,745-byte bundle. Its native proof completed 21 actions and exited normally. The [syntax-v2 all-pages audit](measurements/production-pages-syntax-v2-2026-09-30.json) passes all 165 sampled regions, 175 child pages and 232 child/relationship-page combinations, with every region's supplied reference total accounted for by routed records and exact internal counts. `src/world/mod.rs` accounts for all 454 relevant references across 11 combinations; the v2 `World` ID is `item:1a4125968da0ac7b`. The 50.7-second audit's largest individual solve was 1,714.6 ms for 425 displayed routes, and another took 1,221.4 ms. These diagnostic samples exceed the one-second detail target; the audit establishes correctness over these pages, not universal performance or exhaustive declaration/expansion coverage. These dated reports precede the subsequent provider traversal-index changes and do not certify those changes.

## Routing feedback and distributed coverage

Quality profile 2 adds up to four adaptive placement refinements after observing actual detours or routing failures. A blocked shorter passage identifies eligible horizontal obstacle-subtree moves; every new placement is rerouted and independently validated. Existing pins, layer ordering, unaffected regions and the best valid candidate remain protected. This is a bounded feedback pass, not a full negotiated multi-commodity router. `feedback_candidates: 0` permits a controlled comparison with the previous candidate loop.

Five focused regressions pass: a routed obstacle case improves length 100→90 with the same volume and pinned endpoints; a previously blocked attachment becomes routable after moving its unpinned obstacle; pinned blockers/unaffected regions remain unchanged; input reordering and cancellation remain deterministic; and the complete layered sketch solve improves 262→256 length and 7→6 bends with the same initial work budget. Extra feedback evaluations cost additional time. Individual metrics can trade off: the SaaS floorplan length increased 888→912 while bends fell 23→21 and combined structural cost fell slightly. Historical profile-1 tables above remain labelled as such.

The declared [distributed AI fixture](../private/layout/fixtures/distributed-ai.bundle.jsonl) contains 346 objects and 564 relationships, including three regional deployment forests, 51 component instances, twelve logical inference workers, shared auth/storage, asynchronous fan-out, a dependency cycle, and 80 tables with two columns each. It is synthetic and explicitly partial; no real company's architecture or runtime capacity is asserted. Tests validate original pub/sub direction, instance/location semantics, table ownership, and six routed logical regions. Logical, Code and Deployment lenses now select separate indexed ownership forests. Auto selects Code for pure code bundles and Logical otherwise. Deployment views preserve supplied locations and instance-of boundaries; no instance traffic is invented.

## Quality profile 4 and current native checks

The final quality score now includes geometric congestion from actual routed paths: extra occupancy along exactly shared tracks and unique perpendicular intersection locations. Scope-port splitting does not inflate it; multiple semantic members of an explicitly aggregated route count as one displayed path. Its normalization is fixed to the same projection. This metric does not estimate traffic capacity, connector thickness, near misses or projected crossings. Seven fixtures and four candidate algorithms produce 28 validated results in [the profile-4 comparison](measurements/placement-quality4-2026-09-29.csv). Across those seven development/reference fixtures, the median per-fixture routed-length reduction is 60.1% versus layered, 18.5% versus constrained, and 24.2% versus nested packing. Median combined-cost reductions are 54.3%, 18.3%, and 24.0%; median occupied-volume change is only 3.0% smaller versus layered and equal versus the other two. These are per-fixture ratios, not pooled total lengths, and do not imply every fixture or metric improves. Coefficients remain provisional, and neither congestion nor the other metrics replace comprehension testing.

A bounded dimension fallback now runs only when ordinary placement and routing feedback find no valid result. It tries at most `min(size_candidates, work_budget)` additional placements, with a default limit of 28, inside the original size bounds. Accepted, compatible leaf dimensions and unrelated repaired geometry remain fixed. It prefers smaller dimension changes before comparing the unchanged structural score, then validates the complete result. Seven focused regressions cover permitted shrink/growth, bands, pins, input-order determinism, cancellation, bounded work and impossible ranges, including exact reuse of accepted fallback geometry with further dimension search disabled. A fresh comparison matches all 28 historical profile-4 rows in every field except elapsed time; quality version 4 remains unchanged. This is a bounded feasibility fallback, not unrestricted dimension optimization.

Routing keeps an indexed history of collinear tracks and canonical occupied paths. The index preserves the previous search cost, including segment splits and tie breaks; an independent scan regression checks its equivalence. Candidate validation also reuses each edge's unchanged obstacle envelopes. Final completeness, attachment, enclosure and obstacle validation remain mandatory.

A recorded profile-4 native production run completed 21 actions and exited normally. In addition to repository/package/file/method traversal and exact return positions, it selected a supplied relationship, opened Details, isolated its shared displayed route, verified unchanged connector points and endpoint positions, checked aggregate membership, and restored the full scene. The package and selected-path screenshots were inspected. Captions reserve their entire bounded two-line area and avoid Details. Details starts collapsed so it does not cover the lower assembly; F1 opens its bounded scroll panel with complete metadata. Tests cover input routing, scroll preservation, explicit member expansion through fresh/retained/manual groups, group collapse, and exact undo. Each route reuses one cube mesh across its dashes and arrows while retaining its own highlight material; independent vertex comparisons and the legacy GPU connector check preserve geometry.

The [final production report](measurements/production-native-quality4-final-2026-09-29.json) records 502 ms initial acceptance, 354 ms maximum sampled detail acceptance, and 53.2 ms maximum cached-return acceptance. Peak RSS was 361,971,712 bytes. Initial acceptance follows a full preflight model validation that warms the file cache. Acceptance timestamps observe CPU publication; they are not GPU presentation timestamps. These sampled transitions meet the stated latency targets, but do not cover every production region, relationship page, or update.

The corrected shutdown handler wraps Bevy's render extraction boundary, pumps the main-thread executor until the final render world returns, and drops that world on its owning thread. A channel-backed test verifies a normal frame, exit, non-send destruction, and a further update without a second receive. The [earlier profile-4 checkpoint](measurements/native-scale-quality4-2026-09-29.json) exited normally but overlapped compilation/tests. The final runs below were sequential, after checks/builds ended, with warm files and the user's older preview still open. Each completed 46 actions across 29 regions, verified exact returns after cache pressure, captured four scenes and exited normally.

| Entities | Initial acceptance including indexing/GPU startup | Peak RSS | Largest fresh-detail acceptance | Largest resident return |
| ---: | ---: | ---: | ---: | ---: |
| 10,000 | 198 ms | 288,243,712 bytes | 200 ms | 36.6 ms |
| 100,000 | 487 ms | 371,146,752 bytes | 370 ms | 36.1 ms |
| 1,000,000 | 3,357 ms | 1,121,026,048 bytes | 154 ms | 52.4 ms |

Full records, separate screenshot completion times, CPU update intervals and measurement limits are in [final native scale results](measurements/native-scale-quality4-final-2026-09-29.json). The million-input RSS is about 1.04 GiB, below the 2 GiB target for this workload. These are single warm-cache observations across heterogeneous actions, not repeated-trial latency percentiles or GPU frame measurements. The index remains resident; only bounded detail and scene objects are instantiated. Dense arbitrary graphs and human comprehension remain separate gates.

## Reload recovery and persistence-enabled measurements

Reloading while focused on a suggested group now checks its members against the replacement revision. If fewer than two remain under its supplied parent, the worker returns to a surviving supplied parent or known ancestor; it does not invent a semantic group record. The bounded recovery saves pins in the old coordinate frame and restores the destination frame's pins. Tests cover a group shrinking to one member, deletion of its parent, corrupt replacement retention, restored ancestor pins, and clearing prior-revision undo after successful publication. Failed expansion also retains a valid selection, explains that the previous arrangement is unchanged, and offers separate inspection with Enter while keeping the technical diagnostic available.

The native persistence mode measures state preparation, serialization, existing-state validation, file writes, and file/directory synchronization on the main thread. Runtime instrumentation retains the latest 64 samples plus lifetime attempts, failures, total and maximum duration. The proof includes those costs in acceptance and CPU-update intervals, uses a fresh temporary directory, and never reads or writes the user's normal saved state.

```sh
cargo run --locked --example production_smoke -- \
  visualyze-local/production/bevy-0.18.1.bundle.jsonl /tmp/bevy-persistence-proof --persist
cargo run --locked --example production_smoke -- \
  visualyze-local/production/bevy-0.18.1-syntax-v2.bundle.jsonl /tmp/bevy-v2-proof --persist
```

Both runs completed 21 actions and exited normally, with 16 successful saves and no save failures:

| Run | Initial acceptance | Largest uncached action / cached return | Save total / maximum | Peak RSS |
| --- | ---: | ---: | ---: | ---: |
| [Legacy identity with persistence](measurements/native-persistence-2026-09-30.json) | 4,501 ms | 765 / 91.0 ms | 149.1 / 14.3 ms | 346,472,448 bytes |
| [Syntax-v2 with persistence](measurements/native-syntax-v2-2026-09-30.json) | 795 ms | 736 / 90.7 ms | 148.1 / 11.3 ms | 354,320,384 bytes |

These were sequential runs after builds/tests ended, with both user previews open and the desktop locked. Preflight warmed the bundle's file cache. The first run also recorded a 718.4 ms CPU-update interval during worker activity, outside screenshot edges; its worker p95 was 27.1 ms. The v2 run's corresponding maximum/p95 were 75.6/18.5 ms. The measured save durations alone do not explain the first run's startup delay or CPU outlier, and no cause is established. Keep the slower observation alongside the later one. These are single observations with indexing and native startup combined; they do not establish cold-start percentiles, GPU presentation latency, or uninterrupted responsiveness. Saves remain synchronous pending broader evidence, rather than attributing the outlier to persistence without measurement.

## Local relationship queries and upgrade continuity

New region queries use a sorted ownership-endpoint index rather than scanning every relationship in the model. Queries visit only requested subtree intervals; internal claims are emitted or counted once, including across disjoint visual-group members. Original direction, evidence and parallel relationships remain intact. Existing global-scan cursors retain their original traversal, including a nonzero saved cursor reopened in a fresh provider. The local index changes first-page membership/order; completeness still requires exhausting disclosed relationship pages. It is not evidence of lower routing cost for an identical projection.

Tests place 10,000 unrelated relationships before four local/boundary claims and verify that the selected region needs at most 11 endpoint visits. Binary-depth ancestor jumps replace linear external-parent walks; a 32,768-deep fixture compares results to an independent parent walk across intermediate forks and separate external trees, in both ownership and deployment views, with at most 256 checkpoints and deterministic cancellation. Memory-limit tests reject one byte below the completed index estimate and accept the exact budget.

The first endpoint-vector implementation exceeded the existing 1 GiB metadata budget on the million-input fixture and exited with an explicit index-limit error. The limit was kept unchanged. Exact capacity allocation removed growth slack; walking already validated preorder and incident lists constructs the sorted index in linear time. The intermediate exact-capacity/sorting run passed but took 7.91 seconds for initial acceptance; its [report](measurements/native-million-local-index-exact-2026-09-30.json) remains alongside the final results.

The [final all-pages audit](measurements/production-pages-local-index-final-2026-09-30.json) validates 223 page combinations across 175 child pages in 165 regions, with zero failures and exact reference coverage against the independent raw ownership oracle. It covers the repository, every package, one deterministic file per package, and explicitly selected World regions, not every declaration or expansion combination. Linear construction produces identical non-timing audit fields to the earlier local-index implementation. Audit timing overlaps verification work and is diagnostic, not a latency gate.

Startup also retains compatible saved geometry as a repair seed when changed page contents invalidate exact restoration. Model, revision, layout version, root and effective lens must match the coordinate frame; unknown legacy frames require identical projection identity. Regression tests preserve an unaffected unpinned neighbor's exact bounds and nondefault size after another region changes. Conflicting pins fail without accepting or writing a replacement scene; the prior state file remains byte-identical.

Final native runs were sequential, after builds/tests ended, with both user previews open on the Apple M5 reference machine and warm OS file caches. The production proof uses isolated synchronous persistence; the synthetic proof disables persistence and reads only bounded header/footer metadata before the viewer performs indexing. Both exit normally. Production package/path-isolation and synthetic region screenshots were inspected.

| Final run | Actions / regions | Initial CPU acceptance, including indexing | Largest fresh action / resident return | Peak RSS |
| --- | ---: | ---: | ---: | ---: |
| [Bevy syntax-v2](measurements/native-local-index-final-2026-09-30.json) | 21 actions | 601 ms | 724 / 74.1 ms | 355,352,576 bytes |
| [Million-entity synthetic](measurements/native-million-local-index-final-2026-09-30.json) | 46 actions / 29 regions | 7,238 ms | 238 / 37.3 ms | 1,071,906,816 bytes |

The production run recorded 16 successful saves, zero failures, 146.4 ms total save time and 14.3 ms maximum. The million-input worker CPU-update p95/max were 19.3/22.4 ms; 388 sampled updates retained the accepted scene during work. The 7.24-second startup remains slower than the earlier 3.36-second sparse run and combines native startup, indexing, projection and solving. These observations establish successful bounded operation for this workload, not the unmeasured after-indexing performance gate, disk-cold percentiles, GPU presentation latency, or arbitrary dense-graph performance. Stage timing and broader performance work remain required.

## Comprehension gate: not yet run

Recruit less-experienced and experienced developers. Counterbalance fixture and viewer order; compare the existing connected scene, a conventional layered control, and the compact candidate. Freeze the candidate and scoring profile before collecting held-out results.

Tasks: locate a component; trace a request; identify only explicitly supplied auth coverage; inspect a table; follow a cross-system dependency; identify a change after an update. Record completion time, correctness, wrong turns/backtracking, and confidence on the same scale. Separately record the ability to distinguish inferred groups, unloaded content, reference endpoints, and uncertain claims.

Pre-register sample size, exclusion criteria, task-level success thresholds, and acceptable regression margins with the product owner before recruiting. Do not select those thresholds after seeing results. Promote the engine only when every hard geometry test passes, measured platform targets pass, and comprehension improves without material accuracy regressions. No simulated participants or fabricated usability results are included here.

## Scope status

Implemented and tested at scoped checkpoints: private headless solver/router with ordered scope ports and bounded routing feedback; comparative harness and reduced exact oracles; CRUD/distributed fixtures; descendant boundary aggregation and child/relationship paging; provider visual-group traversal; native search, selected-region semantic zoom and group controls; stable repair/local persistence; typed bundle provider; explicit real Rust source producer; production native display; and headless scale measurements through one million entities.

Implemented in additional tests: automatic grouping from a bounded 128-peer window, distinct logical/code/deployment lenses, transactional explicit-file revision publication, recovery from disappearing suggested focus, accepted-layout migration across known quality profiles, bounded dimension fallback, and opt-in lexical source identities. Persistence-enabled production measurements now complement the earlier persistence-disabled proofs. Bounded parent presentation envelopes, independently cached canonical interiors and the quantitative original-renderer control are implemented at the newer checkpoints above. Still incomplete: group continuity across arbitrary topology changes, broader dense-region performance/readability coverage, broader topology and cross-context override semantics, broader cross-frame recovery and routing-space enlargement, repeated cold-start and normal-persistence latency gates, GPU presentation measurements, weight calibration (holdout-v1 is scored under provisional weights), and human comprehension gates. Direct phase/cache instrumentation, bounded durable dismissals and diagnostic neighborhood retention are now implemented as recorded above. Expanded Details intentionally overlays the lower view; collapse it with F1 to restore the full assembly. The default remains the previous renderer. See [ADR 0006](decisions/0006-placement-engine.md) for technical boundaries. The overall plan is not complete.
