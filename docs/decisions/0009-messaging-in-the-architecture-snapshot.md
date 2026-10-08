# ADR 0009: Messaging in the architecture snapshot

**Status:** Accepted
**Date:** 2026-10-07

## Context

Modeling three production systems (OpenTelemetry Demo, PostHog, Sentry
self-hosted) showed that the snapshot format could not express an asynchronous
broker. Kafka became a `datastore` with the role `queue`. That was wrong in four
ways: a datastore cannot be the source of a relation, so Kafka's own telemetry
edge was dropped; ClickHouse consuming Kafka had to be written as `depends_on`;
`document` was an inaccurate subtype; and producers and consumers read as
database reads and writes.

The bundle contract already has a messaging profile (`message_channel`,
`publishes`, `subscribes`, `carries`). The snapshot, whose seven entity kinds are
a closed vocabulary, had nothing equivalent.

## Decision

1. **A broker is a `service` with subtype `broker`.** It is not a datastore.
2. **A topic or queue is an `interface` the broker exposes**, with subtype
   `topic` or `queue` and a protocol such as `kafka`. Interfaces already have an
   owner, a boundary, and a place in the layout, so this adds no entity kind and
   no new shape to draw.
3. **Two relation kinds: `publishes` and `subscribes`.** They use the bundle's
   names and its predicate-direction rule: both point from the acting component
   to the channel, as `reads` points from reader to store.
   - `publishes`: client or service to a service or to an interface a service
     exposes.
   - `subscribes`: any component to the same targets. A datastore may
     subscribe, which expresses pull-based ingestion such as ClickHouse
     Kafka-engine tables, and nothing else may be written as `depends_on` to
     stand in for it.
   - Both accept `condition` and reject `aspect` and the dependency qualifiers.
   - A relation to the broker itself states no topic.
4. **New descriptive vocabulary.** Datastore subtypes `columnar`, `time_series`,
   `search`, `log`; service subtype `broker`; interface subtypes `topic`,
   `queue`. The `queue` role is not added: the broker subtype and topic
   interfaces replace it.
5. **Walkthrough traces may travel both relations.** The payload of a
   subscription travels from the channel to the subscriber, so the forward leg of
   a `subscribes` hop leaves from the channel. Neither relation has a response
   leg. A trace that is delivered from a different topic interface than it
   published to is a `trace_gap`; the broker standing for all of its topics
   remains acceptable.
6. **Evidence honesty.** Evidence that backs a `supported` claim but has no
   source line range is accepted with a `coarse_evidence` warning.

## Rendering

Both relations draw as runtime arrows in one new legend colour, following the
predicate direction like every other relation. Per-topic interfaces attach to the
broker and roll up into aggregated lines when the view is collapsed, under the
existing rule that hidden relations map to visible ancestors.

## Alternatives considered

- **A `message_channel` entity kind, as in bundles.** It would unify the two
  formats, but it adds an eighth kind with its own appearance and ownership
  rules. Interfaces already carry what a channel needs. Bundle messaging still
  draws as the generic dependency style; unifying it with the snapshot
  appearance is future work.
- **Keeping Kafka a datastore and relaxing `reads`/`writes`.** It misstates what
  a broker is and keeps the subtype and direction problems.
- **A replica or instance count for identical consumers.** Not changed. The
  hierarchy already expresses them, and a count invites padding.

## Consequences

- Existing snapshots stay valid. No relation or entity changes meaning.
- Producers must remodel any broker they had written as a datastore.
- Per-topic detail is optional: a producer may publish or subscribe to the
  broker where it has not verified a topic.
- Open: deployment layers (compose profiles) remain logical scopes in the three
  example models and should move to `deployment` contexts.
