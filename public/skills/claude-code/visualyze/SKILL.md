---
name: visualyze
description: Explain a codebase's architecture or a flow (auth, request handling, an agent loop) with a 3D Visualyze walkthrough while you narrate in chat. Use when the user asks to be walked through, shown, or taught how part of their system works, and the visualyze MCP server is connected.
---

# Visualyze walkthroughs

Follow the shared guide in [visualyze-walkthrough.md](../../shared/visualyze-walkthrough.md).
The same text is also available from the MCP server as the resource
`visualyze://guide/walkthroughs`.

Claude Code specifics:

- Read and write `.visualyze/model.json` with your normal file tools. The
  Visualyze server only reads files under the project directory.
- Unless the team already commits `.visualyze/`, add `/.visualyze/` to the file
  `git rev-parse --git-path info/exclude` prints, so the model stays out of git.
- Run `git rev-parse HEAD` to compare against `snapshot.source_revision`.
- The tools are `get_capabilities`, `validate_model`, `publish_model`,
  `present_walkthrough`, `set_view`, `get_scene_state`, and `control_playback`.
- You narrate. Put the explanation in your reply, with one heading per step title.
