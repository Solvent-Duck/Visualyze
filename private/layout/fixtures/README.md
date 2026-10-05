# Placement development fixtures

These fixtures were declared after the initial notes/sketch prototype comparison.
They were subsequently used during repairs and are now historical development
evidence, not a fresh held-out cohort. The [evaluation manifest](../evaluation/README.md)
records exact hashes, roles, views and profile settings.
They are synthetic architectural examples, not reconstructions of any company.
They use the existing public snapshot contract and preserve its relation meanings.
No asynchronous messaging, table, or deployment-instance semantics are inferred
from its existing service nodes.

| Fixture | Cases exercised |
| --- | --- |
| `shared-saas.json` | Three tenant APIs, shared identity/storage, parallel reads/writes, concern coverage, and directed feedback cycles. |
| `nested-cross-scope.json` | Three sibling architectural scopes, nested checkout, internal relationships preserved under collapse, cross-scope references, and a request cycle. |
| `fanout-peers.json` | A coordinator and 24 workers, shared storage, directed ring dependencies, high degree, and a disconnected diagnostic service. |

Run the unchanged comparison profile with 32 deterministic candidate evaluations:

```sh
cargo run -p visualyze-layout --example compare --locked -- private/layout/fixtures/shared-saas.json private/layout/fixtures/nested-cross-scope.json private/layout/fixtures/fanout-peers.json
```

This command uses the default overview and CRUD band policy for every algorithm.
It measures the same projected objects and relationship set for each comparison.
The nested routing regression additionally expands all four supplied scopes and
checks all four algorithms against the same geometry and semantic validator:

```sh
cargo test -p visualyze-layout --lib heldout_nested_scope --locked -- --nocapture
```

Quality weights are not tuned against these fixtures. Route length, volume, bends,
occlusion proxy and elapsed time should be reported separately; a lower combined
cost does not imply that every individual measurement improved. These fixtures do
not substitute for comprehension sessions or million-entity provider benchmarks.

## Typed distributed AI fixture

`distributed-ai.bundle.jsonl` is a separate, explicitly declared synthetic model
using architecture, data, messaging and instance profiles. Its default 346 objects
and 564 relationships cover seven logical regions, twelve inference workers,
shared identity/storage, pub/sub fan-out, a directed worker dependency cycle,
80 tables with two columns each and a table-reference cycle, plus three regional
deployments with 51 instances. Deployment location is an independent forest;
instances do not invent logical ownership. An isolated diagnostic service tests
disconnected components. Auth coverage is asserted only on explicit targets.

All facts have `declared` basis and the manifest is `partial`. It does not model
OpenAI/ChatGPT or assert capacity, runtime traffic, complete security coverage,
failover behavior or any real deployment. Logical dependencies are not copied
onto deployment instances as invented network flows.

Regenerate to a **new** output path and validate using the public provider:

```sh
cargo run -p visualyze-layout --example generate_ai_fixture --locked -- /tmp/ai-evaluation.bundle.jsonl
cargo run --locked -- --bundle /tmp/ai-evaluation.bundle.jsonl --validate
cargo test -p visualyze-layout --test distributed_system --locked
```

Optional positional parameters after the path are regions (2–12), workers
(2–256), and tables (2–10,000). Defaults are `3 12 80`. Larger variants test
hierarchy and visibility budgets; they do not imply a real system's size or
performance. Separate deployment-lens traversal and native inspection tests are
required in addition to validating the records and routed logical regions.

`distributed-ai-many-tables.bundle.jsonl` uses the same declared generator with
`3 12 160`, producing 586 entities and 884 relationships. Its 160 tables require
a second bounded child page after automatic grouping; the 80-table default fits
one such page. The larger fixture exists to exercise actual page navigation,
not to label an empty or nonexistent page as a successful comparison.
