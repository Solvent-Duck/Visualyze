# Contributing

## Before changing behavior

1. Read the product specification and relevant architecture decisions.
2. Keep public semantic artifacts independent of the private renderer.
3. Preserve the read-only, local-first, and no-source-upload boundaries.
4. Keep implementation slices small and testable without a GPU window where possible.

## Development expectations

- Pin dependencies and commit lockfiles when the first implementation slice selects them.
- Add behavioral tests with every behavioral change.
- Run focused validation first; add visual fixture checks when a change affects composition or interaction.
- Do not introduce a repository scanner, product-owned inference, network listener, or repository-write capability without an explicit product decision.
- Keep generated visualization snapshots, preferences, logs, and other machine-local state outside the repository.

## Public/private boundary

Public artifacts define the architecture model and compatibility contract. The private runtime may consume that contract but must not make proprietary engine types part of it. See [`docs/decisions/0001-repository-boundaries.md`](./docs/decisions/0001-repository-boundaries.md).

## First implementation milestone

Phase 0 proves the spatial language with hand-authored synthetic models. It does not include agent integration, repository ingestion, or production deployment.
