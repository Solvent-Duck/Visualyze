# Explaining a codebase with Visualyze

Visualyze is a 3D visual teaching aid. **You do the explaining.** Visualyze shows
the architecture, moves the camera, lights what you are talking about, and
animates the paths you trace. It never writes prose. Your chat answer carries the
explanation; the viewer illustrates it.

Use this when someone asks things like "walk me through our auth", "how does our
research agent loop work", or "show me this architecture".

## 1. Load or update the model

The architecture model lives in the repository at `.visualyze/model.json`, so the
whole team shares it.

- **If it exists**, read it. Compare `snapshot.source_revision` with the current
  commit (`git rev-parse HEAD`). If files that matter to the question changed
  since then, edit the model rather than regenerating it. Keep every existing ID,
  add or change only what changed, raise `revision` by one, and set
  `source_revision` to the current commit.
- **If it is missing**, build it from manifests, routes, configuration, and code
  structure. Follow the architecture format (resource
  `visualyze://format/architecture`). Never include source code or secrets.
- Write one record per line, sorted by `id`, so teammates' edits merge cleanly.

The model states facts. Mark anything you deduced as `"basis": "inferred"` with a
`rationale`. Give each real subsystem one home through `contains`. Do not invent
groupings to make a view smaller; readability advice tells you where a real
subsystem might be missing.

## 2. Make sure the model can answer the question

A walkthrough can only show what the model contains. Before explaining "our auth",
check that the model has the pieces auth involves: the client, the login
interface, the services that check credentials and issue sessions, the stores they
read and write, and the `requests`, `reads`, and `writes` between them. Add what
is missing, with evidence, then call `validate_model` and `publish_model`.

## 3. Write the walkthrough

A walkthrough is a short sequence of scenes over model IDs. Shape it like a good
explainer video:

1. **Big picture.** Start with `"camera": "overview"` on the system or subsystem.
2. **Zoom in.** Focus on the few objects that matter (`"camera": "frame_focus"`).
3. **Trace the main path.** Use `trace` with ordered hops and
   `"camera": "follow_trace"`. Add `"return": true` hops for responses.
4. **One important branch or failure path.** Only if it teaches something.
5. **Recap.** Return to an overview with `"reveal": "cumulative"` so everything
   you covered stays lit.

Guidelines:

- Aim for 5 to 10 steps. Each step should make one point.
- Step titles are on-screen captions. Keep them to about 6 words, as a headline
  rather than a sentence.
- Use at most 3 `labels` per step: short callouts such as "bcrypt hashes" or
  "JWT, 15 min".
- Use `"repeat_trace": true` for genuine loops, such as an agent's plan, act,
  observe cycle. The trace must end where it started.
- A trace must be contiguous: each hop leaves from where the previous hop arrived
  (or from the service that owns the interface it reached). If a hop is missing,
  add the relation to the model. Never skip it.
- Trace order is explanatory, not a recorded execution, and the viewer says so on
  screen.

Call `present_walkthrough`. Repair any diagnostics it returns, then call it again.

## 4. Narrate in chat

After presenting, write your explanation in chat with one section per step. Use
headings that match the step titles exactly, numbered in order:

```
**1 · The notes system**
...
**2 · The browser asks for a session**
...
```

Tell the person to press `.` in the Visualyze window to go to the next step and `,`
to go back, or `?` to autoplay. Keep each section to a few sentences. The picture
is doing half the work.

## 5. Follow-ups

- "What's this?" or "explain the selected thing": call `get_scene_state` to see
  what the person selected and which step they are on, then answer about that.
- "Show me the refresh path": use `set_view` for a single scene, or present a
  short new walkthrough.
- If the person moved the camera, playback pauses (`"state": "interrupted"`).
  That is expected.
