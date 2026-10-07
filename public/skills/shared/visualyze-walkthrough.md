# Explaining a codebase with Visualyze

Visualyze is a 3D visual teaching aid. **You do the explaining.** Visualyze shows
the architecture, moves the camera, lights what you are talking about, and
animates the paths you trace. It never writes prose. Your chat answer carries the
explanation; the viewer illustrates it.

Use this when someone asks things like "walk me through our auth", "how does our
research agent loop work", or "show me this architecture".

## 1. Load or update the model

The architecture model lives in the repository at `.visualyze/model.json`.

- **If it exists**, read it. Compare `snapshot.source_revision` with the current
  commit (`git rev-parse HEAD`). If files that matter to the question changed
  since then, edit the model rather than regenerating it. Keep every existing ID,
  add or change only what changed, raise `revision` by one, and set
  `source_revision` to the current commit.
- **If it is missing**, build it from manifests, routes, configuration, and code
  structure. Follow the architecture format (resource
  `visualyze://format/architecture`). Never include source code or secrets.
- Write one record per line, sorted by `id`, so edits merge cleanly.

**Keep the model out of git unless the team shares it.** When you create
`.visualyze/`, check `git ls-files .visualyze`. If nothing is tracked and the
person has not asked to share the model, append the line `/.visualyze/` to the
file that `git rev-parse --git-path info/exclude` prints, unless it is already
there. That file is local and never committed, so the repository's own files stay
untouched. Tell the person once that the model is excluded, and that removing
the line and committing `.visualyze/` shares it with the team.

The model states facts. Mark anything you deduced as `"basis": "inferred"` with a
`rationale`. Give each real subsystem one home through `contains`. Each scope
should be something the repository itself names: a directory, a deployable, a
compose profile, or a product area in its docs. Cite that as evidence. If you
group by function instead (for example "Background work"), mark the scope and
its `contains` relations inferred. Do not invent groupings to make a view
smaller; readability advice tells you where a real subsystem might be missing.

Every box should take part in something. If an object has no `requests`,
`reads`, `writes`, `depends_on`, `publishes`, or `subscribes`, connect it with
evidence, or leave it out and list it in `omissions` (`unconnected_entity`).

Model a message broker (Kafka, RabbitMQ, SQS) as a `service` with `subtype: "broker"`, never as a datastore. Each topic or queue you can name is an `interface` it exposes (`subtype: "topic"`), and producers and consumers use `publishes` and `subscribes`. Relate to the broker itself when you have not verified the topic.

Name real products in `technology` (for example `PostgreSQL`, `Redis`, `Clerk`,
`Next.js`) and set `subtype` where it fits. The viewer draws a small icon on the
top of the box from them. Use the product's common name only for technology the
code actually uses; an unrecognised name draws nothing.

## 2. Make sure the model can answer the question

A walkthrough can only show what the model contains. Before explaining "our auth",
check that the model has the pieces auth involves: the client, the login
interface, the services that check credentials and issue sessions, the stores they
read and write, and the `requests`, `reads`, and `writes` between them. Add what
is missing, with evidence, then call `validate_model` and `publish_model`.

**Match the depth to the request.**

- "Onboard me" or "show me this architecture" means every major subsystem the
  repository has, not only the main request path. Leave a subsystem out only when
  it does not matter for the question, list it in `omissions`, and offer it in
  chat as a follow-up.
- Where a subsystem has real internals (for example an ingestion pipeline's
  capture service, topics, consumers, and workers), model them inside its scope,
  so the walkthrough can open the scope and show them.
- A focused question ("how does login work") needs its path in full detail and
  little else.
- If the repository is large and the person's intent is unclear, build the
  overview and close by naming the areas you can open next.

## 3. Write the walkthrough

A walkthrough is a short sequence of scenes over model IDs. Shape it like a good
explainer video:

1. **Big picture.** Start with `"camera": "overview"` on the system or subsystem.
2. **Zoom in.** Focus on the few objects that matter (`"camera": "frame_focus"`).
   To open a subsystem, focus only on objects inside one scope; the scene's root
   follows the focus, so the viewer shows that scope's interior. You can also set
   `root` explicitly, and use `"camera": "close_up"` for a single object. A focus
   that spans scopes shows the whole system with everything else dimmed, which is
   right for the overview and recap, not for the middle of a tour. Open at least
   the subsystem the question is about (`never_opens` warns when no step does).
3. **Trace the main path.** Use `trace` with ordered hops and
   `"camera": "follow_trace"`. Add `"return": true` hops for responses.
4. **One important branch or failure path.** Only if it teaches something.
5. **Recap.** Return to an overview with `"reveal": "cumulative"` so everything
   you covered stays lit.

Guidelines:

- Aim for 5 to 10 steps, or up to 12 for an onboarding tour of many subsystems.
  Each step should make one point.
- Step titles are on-screen captions. Keep them to about 6 words, as a headline
  rather than a sentence. Do not number them; the viewer shows progress and your
  chat headings carry the numbers (`numbered_title`).
- Use at most 3 `labels` per step: short callouts such as "bcrypt hashes" or
  "JWT, 15 min". Keep the picture sparse; explanation belongs in chat.
- Use `"repeat_trace": true` for genuine loops, such as an agent's plan, act,
  observe cycle. The trace must end where it started.
- A trace must be contiguous: each hop leaves from where the previous hop arrived
  (or from the service that owns the interface it reached). If a hop is missing,
  add the relation to the model. Never skip it.
- A `"return": true` hop is the response to a request this trace already showed,
  so it must come after that relation's forward hop. Never use `return` to draw a
  relation backwards. If data really moves the other way, the model is missing a
  relation in that direction.
- Trace order is explanatory, not a recorded execution, and the viewer says so on
  screen.

Call `present_walkthrough`. Repair any diagnostics it
returns, then call it again. Warnings never reject a walkthrough, but read them:
each names something the viewer will show badly. To repair a `trace_gap`, add the missing relation
with evidence, or split the step into two. Do not delete a hop that your
narration describes, and do not change a hop's meaning to make the trace fit.

## 4. Narrate in chat

After presenting, write the whole explanation in one reply. The person moves
through the viewer at their own pace with `.` (next), `,` (back), and `?`
(autoplay); tell them so.

1. Open with one or two sentences on what the model covers, what it leaves out,
   and how many of its relations are inferred.
2. Write one section per step, with a heading that matches the step title
   exactly, numbered in order:

   ```
   **1 · The notes system**
   ...
   **2 · The browser asks for a session**
   ...
   ```

   Keep each section to a few sentences: what this part does, why it is built
   this way, and where in the code to look. If a section describes a relation
   marked `inferred`, say so there ("likely; not traced to a line"). The picture
   is doing half the work.

## 5. Follow-ups

- "What's this?" or "explain the selected thing": call `get_scene_state` to see
  what the person selected and which step they are on, then answer about that.
- "Show me the refresh path": use `set_view` for a single scene, or present a
  short new walkthrough.
- If the person moved the camera, playback pauses (`"state": "interrupted"`).
  That is expected.
