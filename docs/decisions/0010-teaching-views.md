# ADR 0010: Teaching views

**Status:** Accepted
**Date:** 2026-10-07

## Context

A walkthrough (ADR 0008) lights what it is about and dims or hides everything
else, but always inside the full system's layout. That works for a subsystem
that lives in one scope: the step opens the scope. It fails for a part whose
pieces live in different scopes, such as "walk me through our deep research
loop". The loop then reads as a jagged path across a mostly dark system. Its
pieces sit far apart, placed for the whole system rather than for the
explanation.

The layout engine needs no new capability for this: it lays out whatever
projection it is given. What is missing is a way for the agent to ask for a
smaller one, and a way for the viewer to show it without treating it as a model.

## Decision

1. **The agent chooses the part; Visualyze lays it out.** A walkthrough may
   declare `teaching_views`, each a list of existing entity IDs (`members`) with a
   title and a layout hint (`pipeline` or `peers`). A step joins a view with
   `"view": "<id>"`. Visualyze never chooses members, in keeping with ADR 0008's
   "the agent explains; Visualyze shows" and ADR 0007's rule that only supplied
   structure is ever shown.
2. **A teaching view cites; it never states.** Members must exist in the
   published revision. Nothing in a teaching view adds an entity or a relation.
   If the explanation needs an object the model lacks, the agent adds it to the
   model with evidence. What is shown is derived:
   - the members, plus the interfaces each exposes (an interface travels with its
     owner, so listing one is rejected);
   - every relation other than `contains` with both ends shown.

   A member may not sit inside another member. The view is flat, and showing a
   scope beside its own contents would show the contents twice.
3. **Flattening is a display placement, labelled as one.** The derived snapshot
   keeps the model's ID, revision, system, entities and relations. It drops all
   `contains` relations, then adds one from the system to each member that had a
   parent. That added relation's rationale says it is a teaching view placement
   and names the real parent. The derived snapshot passes the same graph
   validation as any model, so a derivation cannot produce a model the viewer
   would otherwise refuse.
4. **Steps stay inside their view.** A step in a view takes no `root`. Its focus,
   trace hops, and label targets must be shown in the view
   (`outside_teaching_view`). Every honest-trace rule from ADR 0008 still applies
   unchanged, because the IDs are the model's own.
5. **Temporary, not a second model.** The viewer builds one layout source per
   view when playback starts, and reuses it for every step in that view. Entering
   a view swaps the viewer's layout source and sets aside the published model's
   view state and composed constraints. The next step without a view, stopping
   playback, or a publish restores them. Nothing is saved while a teaching view
   shows, and the model file is never touched. `get_scene_state` reports the
   current teaching view.
6. **Say what is left out.** Flow relations with one end in the view are not
   drawn. Next to the existing "Explanatory order" badge, the viewer shows the
   view's title, how many objects it shows out of the model's total, and how many
   outside connections it leaves out.
7. **`set_view` takes an inline `teaching_view`**, so a quick follow-up can show
   one part on its own without a full walkthrough.

## First slice and later work

The first slice cuts between layouts: the full model, then the flat view. The
intended motion is to light the members in place, let everything else fall
away, then move the members to their flat positions. Members keep their
semantic IDs across both layouts, so this needs per-ID interpolation between two
accepted layouts. That is a narrow case of the retained reconciliation listed
in ADR 0008, and is the next step.

Later work:

- Keeping a listed container as a frame instead of rejecting nested members.
- Stubs at the view's edge for the outside connections, instead of only a count.

## Consequences

- An agent can explain a cross-cutting loop or path compactly, from the model's
  own facts.
- The walkthrough schema gains `teaching_views` and a step's `view`; both are
  optional, so existing walkthroughs are unchanged. The `never_opens` advice does
  not fire for a walkthrough that plays in a teaching view.
- While a teaching view shows, `Reload` is refused. The person stops the
  walkthrough first.
- The viewer's saved layout for a model can never contain a teaching view's
  layout.
