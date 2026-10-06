# ADR 0008: Agent-narrated walkthroughs over MCP

**Status:** Accepted
**Date:** 2026-10-06

## Context

The product goal is this: a person asks their coding agent "walk me through our
auth" or "how does our research agent loop work", and gets an explainer-quality
3D walkthrough while the agent explains. Spec §10 and §11 sketched an MCP bridge,
local IPC, and `present_walkthrough`. Spec §16 Phase 1 set the exit test: "Show me
this architecture" and "Explain the selected auth component". No agent layer
existed before this decision.

## Decision

1. **The agent explains; Visualyze shows.** Visualyze generates no prose. A
   walkthrough carries only intent: model IDs to frame, light, and trace, plus
   the agent's short step titles and labels. The agent narrates in chat, with
   headings that match the step titles. Captions only; no voice and no video
   export for now.
2. **The agent sends the whole script; the viewer sets the pace.** The agent
   sends the complete walkthrough in one call, so pacing never waits on tool
   round trips. The person advances with `.` and `,`, or autoplays with `?`. The
   agent can also drive playback with `control_playback`.
3. **The bridge is Rust, as `visualyze mcp`.** It departs from the TypeScript
   choice in spec §7. The bridge (`public/mcp-server`, crate `visualyze-mcp`, on
   `rmcp` 3.5.1) calls the public `visualyze-model` validator directly, so
   validation cannot drift between languages. Everything ships as one binary.
   The private runtime links the bridge only to dispatch the subcommand.
4. **The model lives in the repository.** The agent writes
   `.visualyze/model.json` with its own tools, and the team shares it through
   git. The bridge only reads files, and only under the directory it was
   started in. Neither the bridge nor the viewer writes to the repository.
5. **Local IPC uses a per-user Unix socket.**
   - Location: `~/Library/Application Support/Visualyze/run/viewer.sock`
     (directory 0700, socket 0600), or `VISUALYZE_RUN_DIR`.
   - Framing: newline-delimited JSON (`visualyze.ipc.v1`).
   - Authentication: every connection opens with `hello` carrying a random
     session token from a 0600 file.
   - The bridge starts only its own executable, with `--serve`. It never starts
     a path taken from a model or from the agent.

   This is the explicit product decision that CONTRIBUTING requires for a
   listener. It is not a network listener.
6. **Honest traces.** The walkthrough validator (`visualyze_model::walkthrough`)
   rejects the following:
   - containment or coverage relations used as travel
   - response legs on relations that imply no response
   - gaps between hops
   - loops that do not close

   While a trace plays, the viewer shows "Explanatory order, not a live trace".
   Readability problems produce advice and never reject a walkthrough.
7. **Revisions move forward.** A published model is accepted when its
   `expected_revision` (if given) matches what the viewer last accepted, and its
   revision is strictly greater. Republishing identical content does nothing.
   This relaxes the spec's "prior + 1" rule so that a git pull can jump several
   revisions.
8. **Validation advice stays prose.** `visualyze --validate --json` and the
   publish result report advice as `{scope_id, level, message}`. The noise score
   stays private (ADR 0007).

## Runtime shape

- `ipc.rs`: connection threads authenticate and frame requests. Requests pass
  through a bounded queue (64) and are applied on the main thread. A full queue
  answers `busy`.
- `ViewerAction::Publish`: reuses the transactional reload path and may switch
  to a different model.
- `ViewerAction::Stage`: sets root, expansion, explosion, and a `Spotlight`. The
  spotlight lights the focus, trace endpoints, and the containers that frame
  them, and dims or hides everything else.
- `camera.rs`: the orbit camera gains a movable target, framed shots, and eased
  tweens (`CameraDirector`). Any manual camera input ends the tween and marks the
  explanation interrupted, which pauses autoplay (spec §10).
- `playback.rs`: a per-step state machine (stage, wait for acceptance, present,
  hold). It also draws the trace token (request legs cool, response legs warm,
  loops repeat), the lower-third caption, progress, the honesty badge, and
  world-anchored callouts. The exploration HUD hides while a walkthrough is
  presented.

## Consequences

- Spec Phase 1 exit criteria are met for Claude Code. Codex uses the same server
  with `public/skills/codex/AGENTS.md`.
- Scene changes still rebuild the scene. Objects therefore do not glide between
  layouts; the camera and the token carry the motion. Retained reconciliation
  (diffing the scene by semantic ID) is the next step for build-up and enter
  transitions.
- Only snapshot models (`.json`) can be published. Bundles (`.jsonl`) for
  code-level agent loops are later work.
- macOS only, because of the socket location and Unix sockets. A Windows named
  pipe is later work.
