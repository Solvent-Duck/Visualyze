# ADR 0007: Supplied structure and a visual noise budget

Status: **Proposed** (2026-10-05). No code has changed for this decision.

## Context

Rating 116 layout pairs showed that past roughly 16 boxes in one layer, or
about 27 boxes in a view, a scene is hard to read whatever the layout
algorithm. Some 20-box views still worked well. The engine's current limits
were set for scale, not perception, and are hard cutoffs
(`private/layout/src/provider_projection.rs`, `projection.rs`):

| Limit | Current value | What it does |
| --- | --- | --- |
| `peer_ceiling` | 64 | Immediate peers before automatic grouping or paging |
| `group_target` | 32 | Maximum automatic groups per assembly |
| `max_children` | 32 | Children per page |
| `visible_budget` | 250 | Total visible representations |

When a level is too wide, the renderer invents suggested visual groups. These
have no architectural meaning, and their labels show it (`Suggested:
bevy_render | ...`).

The product goal is closer to a system-design explainer: a small number of
true subsystems at the top, and the ability to go into any box to see what it
contains. Truthful representation of the architecture matters more than
saving space.

## Decision

### 1. The producer supplies the structure

The model producer (the user's coding agent, and later the MCP skill) states
the architecture's real hierarchy using the existing contract: `scope` with
`contains`, plus the bundle profiles' typed ownership (datastore → table,
repository → package → file, and so on). No new kinds or relations are needed.

- The LLM **reflects** the repository. A scope says "these parts belong to
  this subsystem"; it never says "put these near each other." Grouping claims
  carry a basis. A grouping the LLM deduced, rather than one stated in the
  repository, is `inferred` with a rationale.
- The top level is the highest truthful abstraction. It can be three boxes
  (frontend, application, data) when that is what the system is.
- Updates are incremental: the producer receives its previous model and edits
  it, keeping stable IDs. It does not regenerate from scratch. This already
  matches the snapshot rule that a refresh retains content it does not mean to
  replace.
- Cross-cutting parts (shared auth, a database used by several domains) keep
  one home. Other subsystems reach them through relations or concerns. They
  are not duplicated to make the tree tidy.

The renderer stops being the primary source of grouping. Its automatic
suggested groups remain only as a fallback for oversized input that has no
supplied structure. Existing manual visual groups and pins are unchanged.

### 2. Validation has two tiers

| Tier | Examples | Result |
| --- | --- | --- |
| Structural | Dangling IDs, containment cycles, two owners, orphaned objects, wrong ownership kinds | **Reject** (already enforced by the shared Rust validator) |
| Readability | A scope whose interior is noisy | **Advise**, never reject |

A size limit that rejects would push the LLM to invent fake subgroups ("Services
A–M") to pass. Advice instead asks: *is there a real subsystem these belong to?*
If not, the wide, truthful level stands and the renderer pages or lays it out
wide.

The command-line loader and the future MCP bridge must call the same
validator, so a model is never rejected on one path and accepted on the other.

### 3. A visual noise score replaces the hard cutoffs

Every projected view gets a **visual noise** score from what it would show
before layout: boxes, boxes per layer, and drawn connectors. Each term is free
up to a soft knee and then grows quadratically, so a small overshoot is cheap
and a large one is expensive. There is no point at which a view is refused.

```text
h(x, knee) = 100 * (max(0, x - knee) / knee)^2

noise = h(B, 27)                 total visible boxes
      + sum over layers h(n_l, 12) boxes in each layer (Node.band)
      + h(C, 36)                 drawn connectors after aggregation
```

- **B** counts visible solid boxes, frames and external references. Interface
  pads attached to an owner count as part of the owner.
- **n_l** is boxes in layer `l`. Layers are the projection's bands, so this is
  known before solving.
- **C** counts drawn connectors. An aggregated line counts once, however many
  relations it carries. That is the point of aggregation.

Illustrative values with the initial knees:

| View | Noise |
| --- | --- |
| 27 boxes, no layer over 12, 36 connectors | 0 |
| 30 boxes, one layer of 14, 40 connectors | 1.2 + 2.8 + 1.2 = 5.2 |
| 35 boxes, one layer of 16, 48 connectors | 8.8 + 11.1 + 11.1 = 31 |
| 45 boxes, one layer of 20, 70 connectors | 44 + 44 + 89 = 178 |

Crossings and bends are not in this score. They depend on placement and stay
in the layout's structural cost, which the calibration found already matches
the rater's preferences. Visual noise decides **what to show**; structural cost
decides **where to put it**.

#### Where the score is used

- **Automatic expansion and paging.** The projector picks, among the views it
  would otherwise consider (which supplied groups to open, whether to page a
  wide level), the one that reveals the most supplied structure while keeping
  noise at or under a threshold, initially **10**. With these knees that lands
  views around 30 boxes, which is the intended average. A view over the
  threshold is still allowed when no lower-noise option exists without
  inventing structure.
- **Explicit user actions always win.** Expanding or entering a box is never
  refused on noise. The status line can show the noise level.
- **Fallback grouping** only runs when noise is far over the threshold and the
  model supplies no structure for that level.
- **`visible_budget` = 250 stays** as a performance guard. It protects frame
  rate and memory, not readability, and is the only remaining hard limit.

#### Advice to the producer

Validation reports, per supplied scope, the noise of its fully expanded
interior: its direct children, their layers, and connectors among them after
aggregation. Above the threshold it returns graded advice ("consider whether a
real subsystem groups some of these"). The knees and the general shape (12 per
layer, about 27 per view) are published as guidance. The exact formula stays in
the private engine, so tuning it does not change the public contract.

### 4. Aggregated connectors must be decomposable

Collapsing groups can turn hundreds of relations into one drawn line. Each
projected `Edge` and `Route` already lists the relations it represents
(`semantic_ids`), and the projection keeps every original record
(`relation_metadata`). What is missing is presentation:

- Show how many relations a line carries, by thickness or a count badge.
- Selecting a line lists its member relations with their original predicates,
  directions and evidence. Details already lists some of this.
- One action expands both endpoints so the member relations become visible
  lines.
- The visible count changes with expansion; the semantic membership never does.

### 5. Going deeper

Every box with supplied children can be entered or expanded, and each interior
is its own view with its own noise budget. A data store's interior shows tables;
a table's shows columns; a package's shows files. Nesting depth is limited only
by the supplied model and existing structural validation. Cached interiors and
fixed sibling positions on entry already exist (ADR 0006).

## Consequences

- The renderer's grouping logic shrinks to a fallback. Most layout work moves to
  small, truthful views, where the solver can search more thoroughly and where
  layered layouts did best in calibration.
- Changing automatic expansion changes default views. Projection keys change,
  so cached layouts are recomputed. Saved pins remain valid, but views that
  previously auto-expanded may open collapsed. Persistence needs a migration
  test.
- The current evaluation corpus is mostly 19–29 boxes per view, above the new
  knees. It is rebuilt under the new budget before further layout tuning.
- Source producers such as the Rust bundle producer still emit exact package and
  file inventories. A producer LLM adds a grouping layer above them (for
  example, Bevy's crates into rendering, ECS, assets and platform). It groups
  dozens of packages, not tens of thousands of objects.

## Initial values and how they will be checked

The knees (27, 12, 36), the quadratic shape, equal term weights and the
threshold of 10 are a starting point, not measurements. The calibration ratings
compared layouts of the same view, so they cannot show where views become too
dense. A follow-up rating study will compare the same system at several levels
of detail (about 8, 14, 20, 28 and 36 boxes) to check the knees and the
connector weight.

## Implementation order

1. **Score only.** Add a `visual_noise` function over `Projection` with unit
   tests, and report it in projection diagnostics and evaluation reports. No
   behavior change.
2. **Producer advice.** Add per-scope interior noise advice to validation output
   on both the CLI and the future MCP path.
3. **Aggregated connector presentation:** count or thickness, member listing,
   and expand-both-endpoints.
4. **Projection decisions.** Replace `peer_ceiling`, `group_target` and the
   automatic-grouping trigger with noise-driven choices, keeping
   `visible_budget`. Add a persistence migration test.
5. **Evaluation.** Rebuild the corpus within the budget and run the
   level-of-detail rating study.

## Open questions

1. Should frames (expanded groups drawn as outlines) count as a full box, or
   less than one?
2. Should very large aggregated lines (for example, more than 50 relations)
   count slightly more than one connector?
3. Should the advisory knees be exposed to producers as numbers, or only as
   prose guidance?
4. Where views exceed the threshold with no truthful grouping, is paging or a
   wide single view preferred?
