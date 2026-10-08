# visualyze-mcp

The MCP bridge between a coding agent and the local Visualyze viewer. It ships
inside the `visualyze` binary as `visualyze mcp`. See
[ADR 0008](../../docs/decisions/0008-agent-walkthroughs.md).

## Connect an agent

Claude Code, from the project you want explained:

```sh
claude mcp add visualyze -- /path/to/visualyze mcp
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.visualyze]
command = "/path/to/visualyze"
args = ["mcp"]
```

Then ask "walk me through our auth". The agent writes `.visualyze/model.json`
and publishes it. It then presents a walkthrough and narrates in chat. The
viewer starts automatically. Press `.` and `,` to step through, and `?` to
autoplay.

## Tools

| Tool | Purpose |
| --- | --- |
| `get_capabilities` | Schema versions, kinds, limits, camera intents, and whether the viewer is running |
| `validate_model` | Diagnostics, warnings, and readability advice for a model file, and optionally a walkthrough |
| `publish_model` | Validate and show a model file |
| `present_walkthrough` | Play a walkthrough, given inline or as a file |
| `set_view` | Show one scene |
| `get_scene_state` | Model, location, selection, and playback state |
| `control_playback` | Play, pause, next, previous, goto, or stop |

The `walkthrough` prompt shows up as a slash command in hosts that support
prompts. The resources `visualyze://guide/walkthroughs`,
`visualyze://format/{architecture,walkthrough}`, and
`visualyze://schema/{architecture,walkthrough}` serve the guide, formats, and
schemas.

## Boundaries

- The bridge reads model and walkthrough files only under its start directory
  (or `--root`), each at most 2 MiB. It never writes.
- The bridge launches only its own executable, as `--serve`. Viewer output goes
  to `viewer.log` in the run directory.
- The protocol (`src/protocol.rs`) is public. The viewer re-validates
  everything it receives.
