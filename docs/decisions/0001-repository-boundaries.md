# ADR 0001: Preserve the public semantic contract and private runtime boundary

**Status:** Accepted  
**Date:** 2026-09-29

## Context

The product's value depends on a durable, compatible architecture-model contract and a polished proprietary spatial runtime. The specification explicitly separates public semantics from proprietary execution quality.

## Decision

Use a single development repository with two top-level areas:

- `public/` holds the future versioned schema, language bindings, validation, MCP bridge, agent skills, examples, and conformance fixtures.
- `private/` holds the future renderer, visual composition, geometry, interaction, assets, fixtures, and packaging.

Public semantic types must not import, serialize, or expose Bevy entities, renderer handles, meshes, material settings, coordinates, or arbitrary scene instructions. The runtime consumes validated semantic models and owns render-scene construction.

## Consequences

- Contract compatibility can be tested independently of graphics work.
- Public artifacts can be extracted into a separate distribution boundary later without rewriting their meaning.
- Runtime changes do not redefine the model format.
- This directory structure does not grant a license or publish any future content; licensing and distribution remain open decisions.
