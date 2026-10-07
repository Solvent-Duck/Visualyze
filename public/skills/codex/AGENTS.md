# Visualyze walkthroughs (Codex)

When the user asks to be walked through or shown how part of their system works,
and the `visualyze` MCP server is configured, follow
[`../shared/visualyze-walkthrough.md`](../shared/visualyze-walkthrough.md). It is
also served as the MCP resource `visualyze://guide/walkthroughs`.

You narrate in chat, with one heading per step title. Visualyze only shows the model, the camera
moves, the traces, and your short labels. Unless the team already commits
`.visualyze/`, add `/.visualyze/` to `git rev-parse --git-path info/exclude`.
