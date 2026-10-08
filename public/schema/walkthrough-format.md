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
| `steps` | 1 to 24 steps. More than 12 produces the `long_walkthrough` advice warning. |

## 2. Steps

| Field | Default | Meaning |
| --- | --- | --- |
| `id` | | Unique within the walkthrough |
| `title` | | On-screen caption, 60 characters at most, one line of plain text |
| `root` | closest common owner of `focus` | The entity whose interior the scene shows. Every focus entity must be inside it. |
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

## 5. Diagnostics

Diagnostics use the architecture format's `{code, path, message}` shape, where
`path` is a JSON Pointer into the walkthrough. Readability problems are warnings
and never reject a walkthrough:

| Warning | When |
| --- | --- |
| `busy_step` | A step has more than 12 focus objects |
| `long_walkthrough` | More than 12 steps |
| `numbered_title` | A step title starts with its own number ("1. ", "Step 2:"); the viewer shows progress |
| `never_opens` | 5 or more steps, and every step's root is the whole system while a scope has at least 2 objects inside it |

## 6. Publication and revisions

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
