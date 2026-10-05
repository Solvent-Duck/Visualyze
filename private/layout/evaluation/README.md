# Reproducible placement comparisons

This development-only harness records the exact input bytes, view requests,
algorithms, deterministic work budget, projection limits, normalization equations,
and current scoring coefficients. The desktop runtime does not require Python.
The coefficients remain provisional; a successful computational comparison is
not human calibration or the product release gate.

`quality5-development-v3.json` declares three original training views and sixteen
development views. It includes the seven historical snapshot overviews, expanded
nested scopes, logical/code/deployment views, database expansion, inference and
messaging regions, and a second page from a declared 160-table database. Cases
used during development are explicitly **not held out**.

`quality5-holdout-v1.json` adds the first fresh held-out cohort
([`holdout-v1/`](holdout-v1/SELECTION.md)) to those unchanged cases. It was
validated, frozen as `quality5-holdout-v1.lock.json` after a new training run,
and scored once on 2026-10-05; reports are in `docs/measurements/`. Any engine
change after that run makes holdout-v1 development evidence.

Each comparison uses the same projection for Layered, Floorplan, Constrained and
Treemap. Actual bounds, route points, semantic membership, metrics, diagnostics,
and every failure are saved. Supplied-model coverage is separate from bounded
projection-query completeness. A missing requested expansion, nonexistent page,
or loaded relationship without representation fails the case.

From the repository root, use new output paths:

```sh
python3 -m unittest discover -s private/layout/evaluation -p 'test_*.py'
python3 private/layout/evaluation/run.py training private/layout/evaluation/quality5-development-v3.json /tmp/visualyze-training.json
python3 private/layout/evaluation/run.py freeze private/layout/evaluation/quality5-development-v3.json /tmp/visualyze-training.json /tmp/visualyze-profile.lock.json
python3 private/layout/evaluation/run.py development private/layout/evaluation/quality5-development-v3.json /tmp/visualyze-profile.lock.json /tmp/visualyze-development.json
```

The runner builds offline with the locked Rust dependencies, takes the executable
path from Cargo's artifact record, and checks the compiled profile. It checks
fixture hashes before and after evaluation, source and compiled-schema hashes,
the compiler artifact before and after running, and exact returned case/algorithm
coverage. A freeze requires successful matching training evidence, including its
actual rows rather than a trusted summary flag. Later runs reject changed source,
fixtures, profiles or executables. Reports use exclusive creation and are never
silently overwritten. The caller chooses fixture files; nothing is downloaded,
scanned from another repository, or uploaded.

`held_out` uses the same interface as `development`, but requires declared
held-out cases and a matching training freeze. The manifest rejects sharing the
same input bytes between a held-out role and training/development. Record a fresh
cohort and the selection/calibration procedure before inspecting its scores.
Do not reclassify a development fixture or change acceptance thresholds after
observing results. Any fixture used to fix the engine becomes development
evidence for subsequent engine changes.

## Recorded checkpoint

The final training and development reports in `docs/measurements/` contain all
76 valid layouts across 19 views. `quality5-verified.lock.json` records the exact
checkpoint. It will intentionally stop matching after future engine changes;
those require new training evidence and a new freeze, not editing an old lock.

The original `quality5-development.json` mistakenly requested a second page from
80 tables, which automatic grouping represented on one page. Its failed report
is preserved. Revision 2 uses a declared 160-table fixture so the requested page
exists. A second failed report exposed a harness reporting bug: serializing an
expansion set sorted the request while the evidence check expected the original
order. The harness now preserves the declared request order and converts to a
set only when projecting. No placement algorithm or scoring coefficient changed
in response to either failure.

Initial-placement cases deliberately have no predecessor. The neighborhood
continuity metric therefore reports `no_previous`; update behavior has separate
regressions and still needs a frozen update corpus. The original renderer now has
the diagnostic comparison below. Weight calibration, a fresh held-out cohort
and human comprehension remain open.

Revision 3 adds an explicit service-interface expansion case. Interface pads
remain attached to a solid owner; the harness validates those visible children
without incorrectly requiring the owner to become an open interior frame. This
is now covered by an example regression, including rejection of an unknown
requested expansion.

## Original renderer control

```sh
cargo test -p visualyze --example legacy_control --offline --locked
cargo run -p visualyze --example legacy_control --offline --locked -- public/examples/notes-crud.json public/examples/sketch-crud.json private/layout/fixtures/nested-cross-scope.json
```

This headless runtime example invokes the actual original `Architecture`
composition and connection code. It compares fully expanded snapshot views with
identical supplied objects, ownership, solid/frame treatment and directed
relationship membership. Focused views, bundles, grouped representations and
synthetic display containment are explicitly unsupported. Interfaces are
expanded too; comparing a collapsed service against its fully exposed legacy
ports would change the problem.

The capture preserves stored floating point centers, dimensions and connector
points without snapping, repairing attachments, resizing or rerouting. Every
adapter uses the same supplemental float audit and measurements. The new solver
also passes its authoritative exact geometry validator. The supplemental audit
covers semantic identity, size domains, containment/interface attachment, layers,
overlap, route endpoints, orthogonality, solid clearance, common enclosures and
ordered noncommon enclosure crossings. It is not a replacement for the exact
validator or proof of all legacy geometry invariants.

Length and enclosing volume (including routes) use scene units. Normalizers are
fixed preferred dimensions from the common projection. The three projected-area
overlap values are rectangular proxies normalized by fixed preferred area.
Pairwise track-sharing length and per-relationship-pair crossing counts include
shared attachments; these explicitly named diagnostics differ from the solver's
congestion score. Their quadratic calculation is capped at 4,096 segments and
reports `not_measured_segment_limit` rather than returning zero above that cap.

Records with failed audits remain useful diagnostics but are excluded from
valid-winner aggregates. The original renderer can draw endpoints a short
distance off object faces and does not satisfy all current dimension/layer
rules. A short invalid route cannot beat a valid solver on correctness. These
existing fixtures are development evidence, not newly held-out inputs. Record
the comparison adapter, original runtime modules, layout/model sources, fixture
hashes and executable identity alongside results; the layout-only evaluation
manifest does not cover this separate control.
