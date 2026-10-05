# Holdout v1: selection recorded before scoring

Selected on 2026-10-04. This cohort is independently authored synthetic input,
not a description of any real organization's architecture and not an extraction
from a repository. It has not been selected from generated alternatives. No
layout, metric, comparison report, or solver result was inspected during its
construction. Existing format documentation and parser/evaluator source were
consulted only to satisfy semantic and request schemas.

The cohort has eight cases. Their topology and requested views are fixed by the
following coverage choices, without using layout outcomes:

1. **Clinic appointments:** two clients, six services, four stores, two concerns,
   and three interfaces beneath one scope. CRUD policy, root view. Authentication
   covers only three named services; observability covers all six. Includes a
   bulk import path that bypasses the gateway, and several data access paths.
2. **Research cooperative:** three supplied regions (field, catalog, shared),
   two clients, eight services, four stores, and one access concern. CRUD policy;
   expand all three regions. Several services use shared identity, membership,
   and audit resources without relocating their ownership.
3. **Parcel events:** eight services, four channels, four message types, two
   stores. Pipeline policy, root view. Four-way submitted-event fan-out and a
   repair-to-intake feedback cycle. Subscriber relations retain their declared
   subscriber-to-channel direction.
4. **Museum collections:** one client, three services, two stores, twelve tables,
   and four columns. CRUD policy; expand both stores, retaining table detail
   independently of column detail. Cross-store references and a two-table
   reference cycle are intentional.
5. **Compiler workspace:** repository, four packages, two files per package,
   one type per package, and one function per file (26 supplied objects including
   the model root). Code lens and policy; focus the repository and expand its
   packages, files, and types. Cross-package shared IR references and one cycle
   are source-dependency declarations, not runtime call claims.
6. **Regional ledger:** four logical services, two stores, one client, a fleet
   containing two regions with two zones each, and twelve explicit deployment
   instances. Deployment lens and policy; focus the fleet and expand the regions
   and zones. The logical service graph is separately declared. Instance traffic
   is not fabricated from prototype dependencies.
7. **Islands and claims:** two clients, eight services, two stores, one concern,
   and two interfaces. Peers policy, root view. Two disconnected runtime islands,
   three isolated services, an uncalled standby interface, a two-service cycle,
   and two separately labelled request claims on the same endpoint pair.
8. **Water control:** three scopes, three sensor clients, four services, two
   channels, two message types, two stores, four tables, and one concern. Pipeline
   policy; expand all scopes and the history store. Combines a telemetry fan-in,
   analysis fan-out, shared data, and an operations feedback loop across scopes.

All records are `declared`; coverage is `partial`. Omissions explicitly include
failure modes, traffic volumes, deployment facts where absent, and the fact that
these examples do not establish a real architecture. Semantic containment is
authored for each scenario. No coordinates, dimensions, pins, visual groups,
color assignments, or desired solver result occur in semantic records.

`generate.py` contains explicit lists and fixed integer loops. There is no random
seed because it uses no randomness. Every entity/relation identity is determined
by scenario and role. Records are sorted by ID; bundle completion counts and
SHA-256 hashes are derived from the emitted bytes. The generator refuses to
overwrite existing files. Changing a scenario after its first scored evaluation
requires a new cohort/version, not replacement of this holdout.

`cases.json` is a cases-only fragment for an evaluator manifest. Every role is
`held_out`. The root task must add the already-frozen quality profile and freeze
the combined manifest and source identities **before** its first scored run.
Layout failure is a result to report, not a reason to remove or simplify a case.
Schema or semantic validation corrections made before scoring must be recorded
in `validation.json`, with the corrected final bytes identified by SHA-256.

This cohort tests these declared scenarios only. It does not substitute for
production ingestion, million-entity scaling, incremental stability tests, or
human comprehension sessions.
