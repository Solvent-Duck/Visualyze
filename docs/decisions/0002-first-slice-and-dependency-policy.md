# ADR 0002: Begin with a Phase 0 compatibility and visual-language slice

**Status:** Accepted  
**Date:** 2026-09-29

## Context

The product direction selects Rust + Bevy, TypeScript/Node.js, and JSON Schema, while exact engine, plugin, SDK, IPC, package-manager, and toolchain versions remain unresolved. The repository must be ready for development without prematurely turning provisional choices into production code.

## Decision

The first implementation work is a narrowly scoped Phase 0 proof using hand-authored synthetic models. It must establish the spatial grammar for services, relational and graph datastores, peer grouping, and the access-control cross-cutting concern.

Dependency manifests, toolchain pins, and lockfiles are deferred until that slice begins. They must be added together with a documented compatibility result. No renderer source, schema implementation, MCP server, repository scanner, hosted component, or integration code is created during setup.

## Consequences

- The repository is structurally ready but does not pretend an unverified dependency selection is settled.
- The first contributor has an explicit compatibility decision to record before writing runtime behavior.
- Agent integration remains a later Phase 1 concern, preventing it from distorting the visual-language proof.
