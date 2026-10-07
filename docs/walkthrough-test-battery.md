# Walkthrough test battery

How to show that the walkthrough path works: the agent writes a model and a script, the bridge relays them, the viewer plays them. The walkthrough is only as good as its weakest link, so the battery has one tier per link. Tiers 1 and 2 are implemented and run in CI today. Tiers 0, 3, 4 and 5 are specified here and need the real repositories, a display, or an agent.

| Tier | Link under test | Needs | Status |
| --- | --- | --- | --- |
| 0 | The model matches the repository | repo checkouts | specified |
| 1 | The validator, on real models | nothing | **implemented**, 15 tests |
| 2 | The MCP bridge over real stdio | the `visualyze` binary | **implemented**, 5 tests |
| 3 | The viewer's IPC and revision rules | a display, isolated run dir | specified |
| 4 | An agent, end to end, on each repo | repos, a headless agent, MCP wired in | specified |
| 5 | The playback itself | a display | specified |

```
cargo test -p visualyze-model --locked --test walkthrough_real_systems   # tier 1
cargo test -p visualyze --locked --test mcp_bridge                       # tier 2
```

## The repositories

The committed models were built from three open-source systems at pinned commits:

| Slot | Model | Pinned `source_revision` | Objects / relations |
| --- | --- | --- | --- |
| 1 | `public/examples/otel-demo.json` (OpenTelemetry Astronomy Shop) | `7ea09b86…` | 42 / 126 |
| 2 | `public/examples/posthog.json` (docker-compose hobby deployment) | `87633d96…` | 52 / 170 |
| 3 | `public/examples/sentry.json` (self-hosted, feature-complete) | `7b07b5eb…` | 75 / 246 |
| 4 | **not found** | | |

The checkouts were not on disk when this was drafted (not in Downloads, Projects, `/tmp`, or the shell history; `/private/tmp/visualyze-reference-sources` exists and is empty). No walkthrough exists for any of these three models; only `notes-crud` and the messaging example have one. Tiers 0, 4 and 5 therefore need the clones at the pinned commits, and a fourth model. To add a system to tier 1, add it to `SYSTEMS` in `public/types/rust/tests/walkthrough_real_systems.rs`.

## Tier 1 — validator on real models (implemented)

Walkthroughs are generated from each model's own relations and compared with an independent oracle written in the test, so a rule that only breaks at scale fails here. Each test was mutation-checked: breaking the contiguity rule, allowing `contains` as travel, and allowing a response leg on `writes` each fail exactly one test.

| Test | Claim |
| --- | --- |
| every stated flow relation… | Every `requests/reads/writes/depends_on/publishes/subscribes` relation traces, with the documented direction (a subscription travels channel → subscriber; `return` goes target → source) |
| a request and its response… | `repeat_trace` accepts request+response and rejects a one-way loop (`open_loop`) |
| consecutive hops… | Over up to 6,500 sampled hop pairs per system, the validator says `trace_gap` exactly when the oracle does, at `/steps/0/trace/1` |
| a trace may have sixteen hops… | A real 16-hop chain validates; 17 is a `schema_violation` at `/steps/0/trace` |
| structure and coverage… | `contains/exposes/applies_to/implemented_by/deployed_in` → `invalid_trace_relation` |
| only requests and reads… | A response leg on anything else → `invalid_return` |
| references are checked by kind | Unknown ID → `missing_reference`; relation used as entity or the reverse → `invalid_reference_kind` |
| focus must sit inside… | With an explicit root, accepted exactly when the oracle says the focus is inside it |
| the default root… | The default root strictly owns every focus object and no child of it does |
| a topic delivers only to… | On real brokers, publish → topic → its own subscriber travels; publishing to one topic and delivering from another is `trace_gap`; no response leg on publish |
| readability limits… | 12/13 focus, 12/13/24/25 steps, 64/65 focus: advice (`busy_step`, `long_walkthrough`) never rejects; the hard caps do |
| bound to one model and revision | `stale_revision`, `model_mismatch`, and a walkthrough for sentry rejected by posthog |
| deterministic and round-trips | Same input gives the same resolved script; the script survives serialization |
| hostile input | Empty, truncated, 100k-deep nesting, invalid UTF-8, BOM, trailing comma, huge/negative/float revision, NUL in title, > 2 MiB: errors, no panic |

## Tier 2 — bridge over real stdio (implemented)

Spawns `visualyze mcp` and speaks JSON-RPC. It makes only calls that finish before the bridge would start a viewer, and asserts that no viewer socket or log appeared.

| Test | Claim |
| --- | --- |
| handshake | Seven tools, five resources, one prompt; only the three read-only tools say so; resource text equals the shipped files byte for byte |
| advertised limits | `get_capabilities` limits and enums equal the walkthrough schema; every kind it names exists in the architecture schema |
| validate real systems | `validate_model` reports all three real models plus `notes-crud` with the right object counts; a rejected walkthrough returns `{code, path, message}` diagnostics including `stale_revision` |
| path jail | `../`, symlink out, absolute path, `dir/../..`, directory, missing, > 2 MiB are refused by every tool that takes a path; the project and its parent are byte-for-byte unchanged |
| malformed requests | Missing params, both/neither of `walkthrough` and `path`, bad JSON file, unknown tool: clean tool errors |

## Tier 0 — the model matches the repository (specified)

Everything downstream stands on the model, so check it first. Deterministic, per repo, at the pinned commit:

| Check | Pass |
| --- | --- |
| `git rev-parse HEAD` equals `snapshot.source_revision` | equal |
| Every `evidence[].locator.path` exists in the checkout | 0 dangling |
| Every compose service or deployable is a model object or is named in `omissions` | ≥ 90% accounted for |
| Every entity and relation has evidence, or `basis: inferred` with a `rationale` | 100% |
| Every `technology` name resolves to an icon (informational; unknown draws nothing) | report the misses |
| No source code or secrets: private-key blocks, `password=`, long hex/base64, values copied from `.env` files | 0 |

## Tier 3 — viewer IPC (specified)

Start `visualyze --serve` with `VISUALYZE_RUN_DIR` set to a temp directory and kill it at teardown. Drive the socket directly (newline-delimited JSON, `hello` first) so the bridge is not in the way.

| # | Case | Expected |
| --- | --- | --- |
| 1 | No `hello`, or a wrong token | refused; socket is `0600`, directory `0700`, token file `0600` |
| 2 | `present` or `state` before any publish | `no_model` / `agent_supplied: false` |
| 3 | Publish rev 1; republish identical | ok; `unchanged: true` |
| 4 | Same revision, different bytes | `revision_reused` |
| 5 | Lower revision; `expected_revision` that is not current | `stale_revision` |
| 6 | Revision jump 1 → 5 | accepted (a git pull) |
| 7 | Publish an invalid model | `invalid_model`; `state.model_id` and `revision` unchanged |
| 8 | Publish otel-demo → posthog → sentry → otel-demo | each reaches `ready`; `visible.objects > 0`; no error |
| 9 | Present a valid walkthrough; one for the wrong revision | step list; `invalid_walkthrough` with diagnostics |
| 10 | `control`: next past the last step, previous at 0, goto out of range, stop, play, pause; with nothing presented | defined behavior each time; `no_walkthrough` when empty |
| 11 | 200 concurrent requests | every one answers `ok` or `busy`; the viewer stays responsive |
| 12 | Kill the viewer, then call a tool | the bridge relaunches and reconnects once |
| 13 | Bridge exits | the viewer stays up (observed today; decide whether that is intended) |

## Tier 4 — agent end to end (specified)

This is the test of the guide, which has never run on a real system. Run a headless agent with the Visualyze MCP server configured and the checkout as its working directory. Run each cell three times, because the agent is not deterministic, and score the artifacts, not the prose.

Questions per repo:

| # | Prompt | What it exercises |
| --- | --- | --- |
| Q1 | "Show me this architecture." | Overview, model creation from scratch |
| Q2 | The main ingest or request path ("how does an event/order/trace get in") | Contiguous traces, `follow_trace` |
| Q3 | "Where is X stored and who reads and writes it?" | `reads`/`writes`, data stores |
| Q4 | "What goes through Kafka, and who consumes it?" | Broker-as-service, topics, `publishes`/`subscribes` |
| Q5 | "What happens when the datastore is down?" | Honest traces: no claim about timing or success |
| Q6 | Select a service in the viewer, then "explain this" | `get_scene_state` |
| Q7 | Run Q1 at an older commit, check out a newer one, ask again | Edit, do not regenerate; ID stability |
| Q8 | Ask about something listed in `omissions` | Extends the model with evidence instead of inventing |

Scoring, automated from the model file, the walkthrough, and the transcript:

| # | Check | Pass |
| --- | --- | --- |
| S1 | Walkthrough validates within two `present_walkthrough` calls | yes |
| S2 | 5–10 steps; step 1 is an `overview`; the last is an `overview` with `reveal: cumulative` | yes |
| S3 | Titles ≤ 6 words; ≤ 3 labels per step | yes |
| S4 | Required entities for the question appear in focus or trace; none of the forbidden ones do | recall ≥ 0.8 |
| S5 | Every traced relation has evidence or `basis: declared` | 100% |
| S6 | Narration has one numbered heading per step, equal to the step title | exact |
| S7 | Model is one record per line, sorted by `id` | yes |
| S8 | No source code or secrets in the model (tier 0 scan) | 0 |
| S9 | Q7: ≥ 95% of unchanged IDs kept; `revision` rose by exactly 1; `source_revision` equals `HEAD` | yes |
| S10 | Anything deduced is `basis: inferred` with a `rationale` | 100% |
| S11 | Q5: no step claims order, latency, success, or frequency in titles or labels | 0 |
| S12 | Human sample: no scope was invented to make a view smaller | reviewed |

Each question needs a golden file of required and forbidden IDs. Derive them from the model, then have a person who knows the repo sign them off; do not let the agent under test write its own answer key.

## Tier 5 — playback (specified)

Needs a window. For each repo's golden walkthrough, publish, present, and step through with `control_playback`, calling `get_scene_state` after each step.

| Check | Pass |
| --- | --- |
| `playback.step` advances by one per `next`; `error` stays null; `visible.objects` is at least the step's focus count | every step |
| A per-step screenshot is not blank and differs from the previous step | every step |
| A trace step shows motion (two frames a second apart differ) | every trace step |
| The honesty badge text appears while a trace plays | yes |
| Moving the camera sets `state: interrupted` and pauses autoplay | yes |
| `.` goes forward, `,` back, `?` toggles autoplay (real key events) | yes |
| p95 frame time while transitioning the 75-object sentry model | < 33 ms |
| Recap step on each real model is not `busy_step` | yes |

Screenshots go to a human for the visual judgement; the automated checks only catch blank, frozen, and crashed.

## Findings so far

1. `Script` serializes an absent `root` as `null`, which its own schema rejects, so a serialized script does not validate. Nothing serializes one today (`report::walkthrough` builds its JSON by hand), so it is latent. Tier 1 strips the null and says why.
2. A bridge call that needs the viewer starts `visualyze --serve` even when the answer will be `no_model`. The viewer outlives the bridge. A probe left an orphan window, which was killed. Tier 3 must isolate the run directory and kill the viewer.
3. No real model has ever had a walkthrough validated against it. Tier 1 now does this for every relation in three of them.
