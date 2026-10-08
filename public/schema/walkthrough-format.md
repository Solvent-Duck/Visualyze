# Walkthrough format — draft 0.1

A walkthrough is an explanatory sequence of scenes over one revision of an
architecture model ([architecture-format.md](architecture-format.md)). The
user's agent writes it and narrates it in chat. The viewer frames, lights, and
traces what each step references, and shows the agent's short step titles and
labels. Visualyze generates no explanation of its own.

The JSON Schema is [`walkthrough.schema.json`](walkthrough.schema.json). The Rust
validator is `visualyze_model::walkthrough::Walkthrough::from_json`, and both the
MCP bridge and the viewer use it. Conformance cases are in
[`../conformance/walkthrough-cases.json`](../conformance/walkthrough-cases.json).

## 1. Envelope

| Field | Meaning |
| --- | --- |
| `schema_version` | `walkthrough.0.1.0-draft.1` |
| `id` | Stable ID for this walkthrough, using the model ID syntax |
| `model_id`, `expected_revision` | The model and the exact revision the IDs refer to. A mismatch is rejected (`model_mismatch`, `stale_revision`). |
| `title` | On-screen title, 60 characters at most |
| `teaching_views` | Optional, up to 4. Temporary flattened views over a subset of the model; see §5. |
| `steps` | 1 to 24 steps. More than 12 produces the `long_walkthrough` advice warning. |

## 2. Steps

| Field | Default | Meaning |
| --- | --- | --- |
| `id` | | Unique within the walkthrough |
| `title` | | On-screen caption, 60 characters at most, one line of plain text |
| `root` | closest common owner of `focus` | The entity whose interior the scene shows. Every focus entity must be inside it. |
| `view` | | A `teaching_views` ID. The step plays in that view (§5). |
| `focus` | | 1 to 64 entities the step is about. More than 12 produces the `busy_step` advice warning. |
| `context` | `hide` with a `trace`, else `dim` | Everything else is `dim`med, `hide`den, or `show`n normally |
| `reveal` | `step` | `cumulative` keeps earlier steps' focus lit, for build-ups and recaps |
| `trace` | `[]` | Ordered hops over existing relations, animated in that order |
| `repeat_trace` | `false` | Loop the trace. Allowed only when it ends where it starts. |
| `labels` | `[]` | Up to 6 callouts, each `{target, text}`. The target is an entity or relation; the text is plain and 80 characters at most. |
| `camera` | `frame_focus` | `overview`, `frame_focus`, `follow_trace`, `close_up`, or `hold` |
| `presentation` | `assembled` | `exploded` separates layers |

These are intents. The viewer chooses the actual coordinates, timing, colours,
and motion. A walkthrough never contains coordinates, durations, or styling.

## 3. Ownership

"Inside" follows the model's `contains` and `exposes` relations. An interface
belongs to the component that exposes it. External objects and deployments sit
at the system level.

## 4. Traces must be honest

A trace animates travel along relations the model already states (spec §4.5):

- A hop must name a `requests`, `reads`, `writes`, or `depends_on` relation.
  Structure (`contains`, `exposes`) and coverage (`applies_to`,
  `implemented_by`, `deployed_in`) are not travel (`invalid_trace_relation`).
- A hop travels from source to target. `"return": true` travels the response leg
  back from target to source. It is allowed only on `requests` and `reads`,
  which imply a response (`invalid_return`), and only after the same trace has
  travelled that relation forward (`unpaired_return`). A response answers a
  request the viewer has shown; it is never a way to draw a relation backwards.
- Hops must be contiguous. Each hop leaves from where the previous one arrived.
  An interface and the component that exposes it count as the same place. If no
  stated relation connects two hops, the walkthrough is rejected (`trace_gap`).
  The fix is to add the missing relation to the model, with evidence, or to split
  the trace. Never jump.
- `repeat_trace` claims a loop. The last hop must arrive where the first left
  (`open_loop`).
- Hop order is explanatory. While a trace plays, the viewer shows "Explanatory
  order, not a live trace". A walkthrough makes no claim about timing,
  frequency, success, or concurrency.

## 5. Teaching views

A teaching view lifts part of the model out and lays it out on its own, for steps
that explain that part. The agent chooses the members; the viewer derives the
rest from the model and adds no facts.

| Field | Default | Meaning |
| --- | --- | --- |
| `id` | | Unique within the walkthrough |
| `title` | | On-screen name, 60 characters at most |
| `members` | | 2 to 32 existing entities. More than 16 produces `busy_teaching_view`. |
| `layout` | `pipeline` | `pipeline` arranges along the flow; `peers` arranges as equals |

- **Shown:** the members, directly inside the system, plus the interfaces each
  member exposes, and every relation other than `contains` between them.
- **Members:** each must be an entity (`missing_reference`), not the system
  (`teaching_member_system`), not an interface, which comes with its owner
  (`teaching_member_interface`), and not inside another member
  (`nested_teaching_member`).
- **Steps in a view** (`"view": "<id>"`) stand at the view's root and take no
  `root` (`root_in_teaching_view`). Their focus, trace hops, and label targets
  must be shown in the view (`outside_teaching_view`). All trace rules in §4
  still apply.
- **Leaving out:** flow relations with one end in the view are not drawn. The
  viewer says how many, beside the view's name and how many objects it shows.
- **Temporary:** the derived layout keeps the model's ID and revision, lives only
  in the viewer while steps play in it, and is never saved. The viewer returns to
  the published model at the next step without a `view`, and when playback stops.
  The flattening is recorded in the derived layout only as `contains` relations
  from the system whose rationale says they are a teaching view placement.
- **Motion:** entering a view, the viewer lights the members where they stand
  while the rest falls away, then lifts them out; leaving the view puts them
  back. Show the part in place in the step before, so the person sees where it
  lives. The motion is the
  viewer's choice; a walkthrough never specifies it.

## 6. Diagnostics

Diagnostics use the architecture format's `{code, path, message}` shape, where
`path` is a JSON Pointer into the walkthrough. Readability problems are warnings
and never reject a walkthrough:

| Warning | When |
| --- | --- |
| `busy_step` | A step has more than 12 focus objects |
| `long_walkthrough` | More than 12 steps |
| `numbered_title` | A step title starts with its own number ("1. ", "Step 2:"); the viewer shows progress |
| `never_opens` | 5 or more steps, every step's root is the whole system, no step plays in a teaching view, and a scope has at least 2 objects inside it |
| `busy_teaching_view` | A teaching view has more than 16 members |
| `unused_teaching_view` | No step plays in a teaching view |

## 7. Publication and revisions

The MCP bridge reads models from the repository, normally
`.visualyze/model.json`, which is shared through git. The viewer accepts a
published model when:

- `expected_revision`, if given, equals the revision the viewer last accepted
  for that model, and
- the new revision is strictly greater than the accepted one. A git pull may
  jump from 7 to 10. This relaxes architecture-format §5, which requires exactly
  prior + 1.
- Republishing the same revision with identical content changes nothing. With
  different content it is rejected (`revision_reused`).

See [ADR 0008](../../docs/decisions/0008-agent-walkthroughs.md).
