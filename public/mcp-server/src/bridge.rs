//! `visualyze mcp`: an MCP server on stdio that forwards an agent's intent to the
//! local viewer. It reads model and walkthrough files only under the directory it
//! was started in, never writes anything, and launches only its own executable.
use std::{
    ffi::OsString,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        GetPromptRequestParams, GetPromptResponse, GetPromptResult, Implementation,
        ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParams, Prompt,
        PromptArgument, PromptMessage, ReadResourceRequestParams, ReadResourceResponse,
        ReadResourceResult, Resource, ResourceContents, Role, ServerCapabilities, ServerConfig,
        Tool,
    },
    service::RequestContext,
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::Mutex,
};
use visualyze_model::architecture::MAX_BYTES;

use crate::protocol::{self, Request, Response};

const ARCHITECTURE_SCHEMA: &str = include_str!("../../schema/architecture.schema.json");
const ARCHITECTURE_FORMAT: &str = include_str!("../../schema/architecture-format.md");
const WALKTHROUGH_SCHEMA: &str = include_str!("../../schema/walkthrough.schema.json");
const WALKTHROUGH_FORMAT: &str = include_str!("../../schema/walkthrough-format.md");
const GUIDE: &str = include_str!("../../skills/shared/visualyze-walkthrough.md");
const LAUNCH_WAIT: Duration = Duration::from_secs(20);

const INSTRUCTIONS: &str = "Visualyze shows a 3D model of the user's architecture and plays walkthroughs you write. \
You do all the explaining in chat; Visualyze only shows things. To explain a system or a flow: keep the model in \
.visualyze/model.json, publish it with publish_model, present a walkthrough with present_walkthrough, then narrate with \
one heading per step title. Read the resource visualyze://guide/walkthroughs first.";

/// Run the bridge until the agent closes stdin. Arguments: `[--root <dir>]`.
pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = args.into_iter();
    let mut root = std::env::current_dir()?;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--root") => {
                root = PathBuf::from(args.next().ok_or("--root requires a directory")?);
            }
            _ => return Err(format!("unknown argument: {}", arg.to_string_lossy()).into()),
        }
    }
    let bridge = Bridge::new(root.canonicalize()?, std::env::current_exe()?);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let service = bridge.serve(rmcp::transport::stdio()).await?;
            service.waiting().await?;
            Ok(())
        })
}

struct Connection {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
}

#[derive(Clone)]
pub struct Bridge {
    root: PathBuf,
    exe: PathBuf,
    viewer: Arc<Mutex<Option<Connection>>>,
    next_id: Arc<AtomicU64>,
}

/// A failure the agent should see and act on, as a tool error.
struct Failure {
    code: String,
    message: String,
    details: Value,
}

impl Failure {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Value::Null,
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::new("io", error.to_string())
    }
}

impl Bridge {
    pub fn new(root: PathBuf, exe: PathBuf) -> Self {
        Self {
            root,
            exe,
            viewer: Arc::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Resolve a file the agent named, refusing anything outside the root.
    fn resolve(&self, path: &str) -> Result<PathBuf, Failure> {
        let joined = self.root.join(path);
        let resolved = joined
            .canonicalize()
            .map_err(|e| Failure::new("not_found", format!("cannot read {path}: {e}")))?;
        if !resolved.starts_with(&self.root) {
            return Err(Failure::new(
                "outside_root",
                format!("{path} is outside {}", self.root.display()),
            ));
        }
        let meta = std::fs::metadata(&resolved)?;
        if !meta.is_file() {
            return Err(Failure::new("not_a_file", format!("{path} is not a file")));
        }
        if meta.len() > MAX_BYTES as u64 {
            return Err(Failure::new(
                "payload_too_large",
                format!("{path} exceeds 2 MiB"),
            ));
        }
        Ok(resolved)
    }

    async fn open(&self, launch: bool) -> Result<Connection, Failure> {
        let dir = protocol::run_dir()?;
        let socket = protocol::socket_path(&dir);
        let stream = match UnixStream::connect(&socket).await {
            Ok(stream) => stream,
            Err(_) if launch => {
                // Only this executable, never a path from a model or the agent.
                // Its output goes to a log beside the socket, so a failed start
                // can be reported without a pipe that could fill and block it.
                let log_path = dir.join("viewer.log");
                let log = std::fs::File::create(&log_path)?;
                let mut child = std::process::Command::new(&self.exe)
                    .arg("--serve")
                    .stdin(std::process::Stdio::null())
                    .stdout(log.try_clone()?)
                    .stderr(log)
                    .spawn()
                    .map_err(|e| {
                        Failure::new("launch_failed", format!("could not start the viewer: {e}"))
                    })?;
                let deadline = tokio::time::Instant::now() + LAUNCH_WAIT;
                loop {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    if let Ok(stream) = UnixStream::connect(&socket).await {
                        break stream;
                    }
                    if let Ok(Some(exit)) = child.try_wait() {
                        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
                        let tail: Vec<_> = log.lines().rev().take(8).collect();
                        return Err(Failure::new(
                            "launch_failed",
                            format!(
                                "the viewer exited ({exit}): {}",
                                tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
                            ),
                        ));
                    }
                    if tokio::time::Instant::now() > deadline {
                        return Err(Failure::new(
                            "viewer_unavailable",
                            "the Visualyze viewer did not start in time; run `visualyze --serve` and try again",
                        ));
                    }
                }
            }
            Err(_) => {
                return Err(Failure::new(
                    "viewer_unavailable",
                    "the viewer is not running",
                ));
            }
        };
        let (read, writer) = stream.into_split();
        let mut connection = Connection {
            reader: BufReader::new(read),
            writer,
        };
        let token = protocol::read_token(&dir)?;
        exchange(
            &mut connection,
            &Request {
                id: 0,
                method: "hello".into(),
                params: json!({"token": token, "protocol": protocol::PROTOCOL}),
            },
        )
        .await?;
        Ok(connection)
    }

    /// One request to the viewer, starting it if needed. A dropped connection is
    /// reopened once, for a viewer that restarted.
    async fn call(&self, method: &str, params: Value) -> Result<Value, Failure> {
        let mut viewer = self.viewer.lock().await;
        for attempt in 0..2 {
            if viewer.is_none() {
                *viewer = Some(self.open(true).await?);
            }
            let request = Request {
                id: self.next_id.fetch_add(1, Ordering::Relaxed),
                method: method.into(),
                params: params.clone(),
            };
            match exchange(viewer.as_mut().unwrap(), &request).await {
                Err(failure) if failure.code == "io" && attempt == 0 => *viewer = None,
                other => return other,
            }
        }
        unreachable!()
    }

    async fn running(&self) -> bool {
        let viewer = self.viewer.lock().await;
        viewer.is_some() || self.open(false).await.is_ok()
    }

    async fn tool(&self, name: &str, args: &Map<String, Value>) -> Result<Value, Failure> {
        let text = |key: &str| args.get(key).and_then(Value::as_str);
        match name {
            "get_capabilities" => Ok(capabilities(self.running().await)),
            "validate_model" => {
                let path = self.resolve(text("path").ok_or(Failure::new("invalid_params", "path is required"))?)?;
                let mut command = tokio::process::Command::new(&self.exe);
                command.arg("--validate").arg("--json").arg("--model").arg(&path);
                if let Some(walkthrough) = text("walkthrough_path") {
                    command.arg("--walkthrough").arg(self.resolve(walkthrough)?);
                }
                let output = command.output().await?;
                serde_json::from_slice(&output.stdout).map_err(|_| {
                    Failure::new(
                        "validator_failed",
                        String::from_utf8_lossy(&output.stderr).into_owned(),
                    )
                })
            }
            "publish_model" => {
                let path = self.resolve(text("path").ok_or(Failure::new("invalid_params", "path is required"))?)?;
                let content = std::fs::read_to_string(&path)?;
                self.call(
                    "publish",
                    json!({
                        "path": path,
                        "content": content,
                        "expected_revision": args.get("expected_revision").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await
            }
            "present_walkthrough" => {
                let walkthrough = match (args.get("walkthrough"), text("path")) {
                    (Some(walkthrough), None) => walkthrough.clone(),
                    (None, Some(path)) => {
                        let bytes = std::fs::read(self.resolve(path)?)?;
                        serde_json::from_slice(&bytes).map_err(|e| {
                            Failure::new("invalid_json", format!("{path}: {e}"))
                        })?
                    }
                    _ => {
                        return Err(Failure::new(
                            "invalid_params",
                            "give exactly one of walkthrough or path",
                        ));
                    }
                };
                self.call(
                    "present",
                    json!({
                        "walkthrough": walkthrough,
                        "start_step": args.get("start_step").cloned().unwrap_or(Value::Null),
                        "autoplay": args.get("autoplay").cloned().unwrap_or(false.into()),
                    }),
                )
                .await
            }
            "set_view" => {
                let mut params = json!({"scene": args.get("scene").cloned().unwrap_or(Value::Null)});
                if let Some(revision) = args.get("expected_revision") {
                    params["expected_revision"] = revision.clone();
                }
                self.call("view", params).await
            }
            "get_scene_state" => self.call("state", json!({})).await,
            "control_playback" => {
                self.call(
                    "control",
                    json!({"action": args.get("action").cloned().unwrap_or(Value::Null), "step": args.get("step").cloned().unwrap_or(Value::Null)}),
                )
                .await
            }
            other => Err(Failure::new("unknown_tool", format!("unknown tool {other}"))),
        }
    }
}

async fn exchange(connection: &mut Connection, request: &Request) -> Result<Value, Failure> {
    let mut line = serde_json::to_vec(request).map_err(|e| Failure::new("io", e.to_string()))?;
    line.push(b'\n');
    connection.writer.write_all(&line).await?;
    let mut reply = String::new();
    if connection.reader.read_line(&mut reply).await? == 0 {
        return Err(Failure::new("io", "the viewer closed the connection"));
    }
    let response: Response =
        serde_json::from_str(&reply).map_err(|e| Failure::new("io", e.to_string()))?;
    match (response.result, response.error) {
        (_, Some(error)) => Err(Failure {
            code: error.code,
            message: error.message,
            details: error.details,
        }),
        (Some(result), None) => Ok(result),
        (None, None) => Ok(Value::Null),
    }
}

fn capabilities(running: bool) -> Value {
    json!({
        "viewer_running": running,
        "model_schema": "0.1.0-draft.1",
        "walkthrough_schema": visualyze_model::walkthrough::SCHEMA_VERSION,
        "model_file": ".visualyze/model.json",
        "entity_kinds": ["scope", "client", "service", "datastore", "interface", "concern", "deployment"],
        "relation_kinds": ["contains", "exposes", "requests", "reads", "writes", "depends_on", "applies_to", "implemented_by", "deployed_in"],
        "trace_relation_kinds": ["requests", "reads", "writes", "depends_on"],
        "camera_intents": ["overview", "frame_focus", "follow_trace", "close_up", "hold"],
        "limits": {
            "steps": 24,
            "advised_steps": visualyze_model::walkthrough::ADVISED_STEPS,
            "advised_focus_per_step": visualyze_model::walkthrough::ADVISED_FOCUS,
            "labels_per_step": 6,
            "title_chars": 60,
            "label_chars": 80,
            "hops_per_trace": 16,
            "model_bytes": MAX_BYTES,
        },
        "viewer_keys": {"next": ".", "previous": ",", "autoplay": "?"},
    })
}

fn object(value: Value) -> Arc<Map<String, Value>> {
    match value {
        Value::Object(map) => Arc::new(map),
        _ => unreachable!("tool schemas are objects"),
    }
}

/// Tool input schemas reuse the walkthrough schema's definitions, so the agent
/// sees exactly the contract the viewer enforces.
fn tools() -> Vec<Tool> {
    let schema: Value = serde_json::from_str(WALKTHROUGH_SCHEMA).expect("embedded schema");
    let defs = schema["$defs"].clone();
    let mut walkthrough = schema.clone();
    for key in ["$schema", "$id", "$defs"] {
        walkthrough.as_object_mut().unwrap().remove(key);
    }
    let mut scene = defs["step"].clone();
    scene["properties"].as_object_mut().unwrap().remove("id");
    scene["required"] = json!(["title", "focus"]);
    let path = json!({"type": "string", "description": "Path relative to the project directory."});
    let read_only = |tool: Tool| {
        let mut tool = tool;
        tool.annotations = Some(rmcp::model::ToolAnnotations::new().read_only(true));
        tool
    };
    vec![
        read_only(Tool::new(
            "get_capabilities",
            "What this Visualyze supports: schema versions, kinds, limits, camera intents, and whether the viewer is running.",
            object(json!({"type": "object", "properties": {}})),
        )),
        read_only(Tool::new(
            "validate_model",
            "Check a model file (and optionally a walkthrough against it) without showing it. Returns diagnostics with JSON pointers, warnings, and readability advice.",
            object(json!({
                "type": "object",
                "properties": {"path": path, "walkthrough_path": path},
                "required": ["path"],
            })),
        )),
        Tool::new(
            "publish_model",
            "Validate a model file (normally .visualyze/model.json) and show it in the viewer, starting the viewer if needed. Pass the revision the viewer last showed as expected_revision to avoid overwriting newer work.",
            object(json!({
                "type": "object",
                "properties": {
                    "path": path,
                    "expected_revision": {"type": ["integer", "null"], "minimum": 1},
                },
                "required": ["path"],
            })),
        ),
        Tool::new(
            "present_walkthrough",
            "Play a walkthrough over the published model: scenes that frame, light, and trace model IDs, with your short step titles and labels. You narrate in chat; use headings that match the step titles. Returns the step list.",
            object(json!({
                "type": "object",
                "properties": {
                    "walkthrough": walkthrough,
                    "path": path,
                    "start_step": {"type": "integer", "minimum": 0},
                    "autoplay": {"type": "boolean", "default": false},
                },
                "$defs": defs,
            })),
        ),
        Tool::new(
            "set_view",
            "Show one scene over the published model, for a quick follow-up such as 'show me the refresh path'.",
            object(json!({
                "type": "object",
                "properties": {
                    "scene": scene,
                    "expected_revision": {"type": "integer", "minimum": 1},
                },
                "required": ["scene"],
                "$defs": defs,
            })),
        ),
        read_only(Tool::new(
            "get_scene_state",
            "What the viewer shows now: model and revision, where the person is, what they selected, and walkthrough progress.",
            object(json!({"type": "object", "properties": {}})),
        )),
        Tool::new(
            "control_playback",
            "Play, pause, step, jump, or stop the current walkthrough.",
            object(json!({
                "type": "object",
                "properties": {
                    "action": {"enum": ["play", "pause", "next", "previous", "goto", "stop"]},
                    "step": {"type": "integer", "minimum": 0},
                },
                "required": ["action"],
            })),
        ),
    ]
}

const RESOURCES: &[(&str, &str, &str, &str)] = &[
    (
        "visualyze://guide/walkthroughs",
        "walkthrough-guide",
        "How to explain a system with Visualyze",
        "text/markdown",
    ),
    (
        "visualyze://format/architecture",
        "architecture-format",
        "Architecture model format and rules",
        "text/markdown",
    ),
    (
        "visualyze://schema/architecture",
        "architecture-schema",
        "Architecture model JSON Schema",
        "application/schema+json",
    ),
    (
        "visualyze://format/walkthrough",
        "walkthrough-format",
        "Walkthrough format and rules",
        "text/markdown",
    ),
    (
        "visualyze://schema/walkthrough",
        "walkthrough-schema",
        "Walkthrough JSON Schema",
        "application/schema+json",
    ),
];

fn resource_text(uri: &str) -> Option<&'static str> {
    Some(match uri {
        "visualyze://guide/walkthroughs" => GUIDE,
        "visualyze://format/architecture" => ARCHITECTURE_FORMAT,
        "visualyze://schema/architecture" => ARCHITECTURE_SCHEMA,
        "visualyze://format/walkthrough" => WALKTHROUGH_FORMAT,
        "visualyze://schema/walkthrough" => WALKTHROUGH_SCHEMA,
        _ => return None,
    })
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default()
}

impl ServerHandler for Bridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("visualyze", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let args = request.arguments.clone().unwrap_or_default();
        Ok(match self.tool(&request.name, &args).await {
            Ok(value) => {
                let failed = value.get("ok") == Some(&Value::Bool(false));
                let content = vec![ContentBlock::text(pretty(&value))];
                if failed {
                    CallToolResult::error(content)
                } else {
                    CallToolResult::success(content)
                }
            }
            Err(failure) => CallToolResult::error(vec![ContentBlock::text(pretty(&json!({
                "ok": false,
                "code": failure.code,
                "message": failure.message,
                "details": failure.details,
            })))]),
        }
        .into())
    }

    async fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListPromptsResult, McpError> {
        Ok(ListPromptsResult::with_all_items(vec![Prompt::new(
            "walkthrough",
            Some(
                "Explain part of this codebase with a 3D Visualyze walkthrough while narrating in chat",
            ),
            Some(vec![
                PromptArgument::new("topic")
                    .with_description(
                        "What to explain, e.g. 'our auth' or 'the research agent loop'",
                    )
                    .with_required(true),
            ]),
        )]))
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetPromptResponse, McpError> {
        if request.name != "walkthrough" {
            return Err(McpError::invalid_params("unknown prompt", None));
        }
        let topic = request
            .arguments
            .as_ref()
            .and_then(|a| a.get("topic"))
            .and_then(Value::as_str)
            .unwrap_or("this codebase's architecture");
        Ok(GetPromptResult::new(vec![PromptMessage::new_text(
            Role::User,
            format!(
                "Walk me through {topic}, using Visualyze to show it while you explain.\n\n{GUIDE}"
            ),
        )])
        .with_description(format!("Visualyze walkthrough: {topic}"))
        .into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        Ok(ListResourcesResult::with_all_items(
            RESOURCES
                .iter()
                .map(|(uri, name, description, mime)| {
                    let mut resource = Resource::new(*uri, *name).with_description(*description);
                    resource.mime_type = Some((*mime).into());
                    resource
                })
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, McpError> {
        let text = resource_text(&request.uri)
            .ok_or_else(|| McpError::resource_not_found(request.uri.clone(), None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, request.uri)]).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schemas_carry_the_walkthrough_definitions() {
        let tools = tools();
        let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "get_capabilities",
                "validate_model",
                "publish_model",
                "present_walkthrough",
                "set_view",
                "get_scene_state",
                "control_playback"
            ]
        );
        let present = &tools[3].input_schema;
        assert!(present["$defs"]["step"].is_object());
        assert_eq!(
            present["properties"]["walkthrough"]["properties"]["steps"]["items"]["$ref"],
            "#/$defs/step"
        );
        let view = &tools[4].input_schema;
        assert!(
            view["properties"]["scene"]["properties"]
                .get("id")
                .is_none()
        );
        // The tool schemas are themselves valid JSON Schema.
        for tool in &tools {
            jsonschema_ok(&Value::Object((*tool.input_schema).clone()));
        }
    }

    fn jsonschema_ok(schema: &Value) {
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn paths_outside_the_root_are_refused() {
        let root = std::env::temp_dir()
            .join(format!("visualyze-bridge-{}", std::process::id()))
            .canonicalize()
            .unwrap_or_else(|_| {
                let dir =
                    std::env::temp_dir().join(format!("visualyze-bridge-{}", std::process::id()));
                std::fs::create_dir_all(dir.join("inner")).unwrap();
                dir.canonicalize().unwrap()
            });
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::write(root.join("inner/model.json"), "{}").unwrap();
        let bridge = Bridge::new(root.join("inner"), PathBuf::from("/bin/false"));
        assert!(bridge.resolve("model.json").is_ok());
        std::fs::write(root.join("secret.json"), "{}").unwrap();
        assert_eq!(
            bridge.resolve("../secret.json").unwrap_err().code,
            "outside_root"
        );
        assert_eq!(
            bridge.resolve("/etc/hosts").unwrap_err().code,
            "outside_root"
        );
        assert_eq!(
            bridge.resolve("missing.json").unwrap_err().code,
            "not_found"
        );
        assert_eq!(bridge.resolve(".").unwrap_err().code, "not_a_file");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn every_resource_resolves() {
        for (uri, ..) in RESOURCES {
            assert!(resource_text(uri).is_some_and(|t| !t.is_empty()), "{uri}");
        }
    }
}
