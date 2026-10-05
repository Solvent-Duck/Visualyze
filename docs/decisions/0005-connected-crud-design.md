# ADR 0005: Render a connected notes CRUD design

**Status:** Implemented  
**Date:** 2026-09-29

## Scope

Exercise the existing contract with a small connected design: a notes frontend, one service, one SQLite database, session authentication, and owner-only note access. This is an architecture simulation; the work does not implement or deploy a notes web application.

## Decision

Add `public/examples/notes-crud.json` without changing the schema version or adding positioning fields. All claims are declared, synthetic, and explicitly partial. The single service owns two interfaces, implements authentication and authorization, and reads/writes the database. Policy coverage is expressed only through `applies_to`; it does not propagate along request/data links. Registration, recovery, deployment, and operational hardening are omitted.

Separate renderer-only composition from geometry and scene construction. `auto` retains the existing seven-object gallery for structural-only fixtures and assembles models containing other relation kinds. `--view gallery` and `--view architecture` override this choice. File, stdin, and embedded input continue through the same bounded parser, schema, and semantic graph checks before rendering.

Use stable kinds and IDs to order placement and routes. Lay out clients/services together, stores below their consumers, interfaces as attached ports, concerns above or beside their explicit targets, and scope frames around descendants. Keep containment, interface ownership, coverage, and communication separate. Each semantic entity has its own root and owned cuboid geometry; parent-child transforms encode containment and ownership. Relations also retain stable IDs and source/target/basis metadata. Structural relations have no connection geometry.

Route the other relations as orthogonal cuboid segments. Requests, reads, and writes have distinct colors and directional chevrons; reads point from reader to store, following the contract predicate. Policy coverage uses violet dashes; implementation uses muted dashes behind the components. Other logical relation kinds use labeled muted dashes. Inferred/unknown runtime claims are dashed and marked in the label. This is static architecture, not live traffic.

## Validation and limits

Tests exercise the new public fixture, file/stdin parity, view arguments, deterministic composition/routing, interface parenting, scope enclosure, explicit-only policy coverage, separate read/write routes, every contract relation kind, scene geometry ownership, and the existing gallery. The native connected smoke test projects and samples all eight objects and nine non-structural connectors and saves a screenshot for visual inspection. The gallery smoke remains separate.

This is a small-model layout, not a general graph-layout engine. It does not avoid every obstacle or label overlap, optimize large/nested scope arrangements, animate calls, inspect source evidence, expose selection, or refresh snapshots. Dense models and arbitrary orbit angles may overlap. Deployment remains a separate object with an explicit logical link rather than a spatial nesting claim. All coordinates remain renderer-owned. No production dependencies were added.

Verified on the native macOS host: 26 workspace tests (including 76 shared conformance cases), formatting, and all-target Clippy pass. Both GPU smoke checks pass and their images were inspected. One gallery run saved its screenshot and then stalled while Bevy dropped `RenderAppChannels`; the process was stopped and the same command passed on retry. This intermittent native shutdown behavior remains a limitation. The connected scene also exposed a focus-dependent resize bug; camera fitting now updates before focus gating, covered by a regression test. The existing transitive `block` future-Rust warning is unchanged.
