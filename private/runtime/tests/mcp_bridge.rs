//! `visualyze mcp` over real stdio, without a viewer (docs/walkthrough-test-battery.md, tier 2).
//!
//! Only calls that finish before the bridge would start a viewer are made here:
//! `publish_model`, `present_walkthrough`, `set_view`, `get_scene_state`, and
//! `control_playback` with valid input all launch one, so those belong to tier 3.
#![cfg(unix)]
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, channel},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

const EXAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../public/examples");
const PUBLIC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../public");

struct Scratch(PathBuf);
impl Scratch {
    fn new(label: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("vz-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    next: u64,
}

impl Session {
    /// `root` is the directory the bridge may read; `run` is where a viewer socket would go.
    fn start(root: &Path, run: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_visualyze"))
            .arg("mcp")
            .arg("--root")
            .arg(root)
            .env("VISUALYZE_RUN_DIR", run)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (send, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        let mut session = Self {
            child,
            stdin,
            lines,
            next: 0,
        };
        let init = session.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "battery", "version": "0"}}),
        );
        assert!(init["result"]["serverInfo"].is_object(), "{init}");
        session.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        session
    }

    fn send(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("no reply to {method}"));
            // Anything on stdout that is not a JSON-RPC message would corrupt the protocol.
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line {line:?}: {e}"));
            assert_eq!(message["jsonrpc"], "2.0", "{line}");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// A tool call: the parsed JSON the bridge returned, and whether it flagged an error.
    fn tool(&mut self, name: &str, arguments: Value) -> (Value, bool) {
        let reply = self.request("tools/call", json!({"name": name, "arguments": arguments}));
        let result = &reply["result"];
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: {reply}"));
        let body = serde_json::from_str(text).unwrap_or_else(|_| text.into());
        (body, result["isError"] == true)
    }

    fn finish(mut self) {
        drop(self.stdin.take());
        let started = std::time::Instant::now();
        while self.child.try_wait().unwrap().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the bridge ignored EOF"
            );
            thread::sleep(Duration::from_millis(50));
        }
        assert!(self.child.wait().unwrap().success());
    }
}

fn listing(dir: &Path) -> Vec<(String, u64)> {
    let mut out: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                e.metadata().unwrap().len(),
            )
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_handshake_lists_the_tools_resources_and_prompt_an_agent_needs() {
    let (root, run) = (Scratch::new("hs-root"), Scratch::new("hs-run"));
    let mut s = Session::start(&root.0, &run.0);

    let tools = s.request("tools/list", json!({}))["result"]["tools"].clone();
    let names: Vec<_> = tools
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
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
    for tool in tools.as_array().unwrap() {
        assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
        assert!(
            !tool["description"].as_str().unwrap().is_empty(),
            "{}",
            tool["name"]
        );
        // Only tools that change nothing may claim to be read-only.
        let read_only = tool["annotations"]["readOnlyHint"] == true;
        let expected = ["get_capabilities", "validate_model", "get_scene_state"]
            .contains(&tool["name"].as_str().unwrap());
        assert_eq!(read_only, expected, "{}", tool["name"]);
    }
    let by_name = |n: &str| {
        tools
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == n)
            .unwrap()
            .clone()
    };
    assert!(by_name("present_walkthrough")["inputSchema"]["$defs"]["step"].is_object());
    // A single scene has no ID of its own; the bridge names it.
    assert!(
        by_name("set_view")["inputSchema"]["properties"]["scene"]["properties"]["id"].is_null()
    );

    let resources = s.request("resources/list", json!({}))["result"]["resources"].clone();
    let uris: Vec<_> = resources
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["uri"].as_str().unwrap())
        .collect();
    assert_eq!(uris.len(), 5);
    for uri in &uris {
        let text =
            s.request("resources/read", json!({"uri": uri}))["result"]["contents"][0]["text"]
                .as_str()
                .unwrap_or_else(|| panic!("{uri} is empty"))
                .to_owned();
        assert!(text.len() > 200, "{uri}");
        if uri.contains("schema") {
            serde_json::from_str::<Value>(&text).unwrap_or_else(|e| panic!("{uri}: {e}"));
        }
        // What an agent reads must be the shipped files, not a stale copy.
        let file = match *uri {
            "visualyze://guide/walkthroughs" => "skills/shared/visualyze-walkthrough.md",
            "visualyze://format/architecture" => "schema/architecture-format.md",
            "visualyze://schema/architecture" => "schema/architecture.schema.json",
            "visualyze://format/walkthrough" => "schema/walkthrough-format.md",
            "visualyze://schema/walkthrough" => "schema/walkthrough.schema.json",
            other => panic!("unexpected resource {other}"),
        };
        assert_eq!(
            text,
            std::fs::read_to_string(format!("{PUBLIC}/{file}")).unwrap(),
            "{uri}"
        );
    }
    assert!(s.request("resources/read", json!({"uri": "visualyze://nope"}))["error"].is_object());

    let prompt = s.request(
        "prompts/get",
        json!({"name": "walkthrough", "arguments": {"topic": "our billing retries"}}),
    );
    let text = prompt["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap();
    assert!(text.contains("our billing retries") && text.contains("present_walkthrough"));
    assert!(s.request("prompts/get", json!({"name": "nope"}))["error"].is_object());

    assert!(
        listing(&run.0).is_empty(),
        "the bridge started a viewer or wrote to the run directory"
    );
    s.finish();
}

#[test]
fn advertised_limits_match_the_schema_the_viewer_enforces() {
    let (root, run) = (Scratch::new("cap-root"), Scratch::new("cap-run"));
    let mut s = Session::start(&root.0, &run.0);
    let (caps, is_error) = s.tool("get_capabilities", json!({}));
    assert!(!is_error);
    assert_eq!(caps["viewer_running"], false);
    assert_eq!(caps["walkthrough_schema"], "walkthrough.0.1.0-draft.1");

    let schema: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{PUBLIC}/schema/walkthrough.schema.json")).unwrap(),
    )
    .unwrap();
    let step = &schema["$defs"]["step"]["properties"];
    let limits = &caps["limits"];
    assert_eq!(limits["steps"], schema["properties"]["steps"]["maxItems"]);
    assert_eq!(limits["hops_per_trace"], step["trace"]["maxItems"]);
    assert_eq!(limits["labels_per_step"], step["labels"]["maxItems"]);
    assert_eq!(
        limits["label_chars"],
        schema["$defs"]["label"]["properties"]["text"]["maxLength"]
    );
    assert_eq!(limits["title_chars"], schema["$defs"]["title"]["maxLength"]);
    let enum_of = |v: &Value| v["enum"].clone();
    assert_eq!(caps["camera_intents"], enum_of(&step["camera"]));

    // Every kind an agent is told about is one the architecture schema accepts.
    let architecture =
        std::fs::read_to_string(format!("{PUBLIC}/schema/architecture.schema.json")).unwrap();
    for kind in caps["entity_kinds"]
        .as_array()
        .unwrap()
        .iter()
        .chain(caps["relation_kinds"].as_array().unwrap())
    {
        assert!(
            architecture.contains(&format!("\"{}\"", kind.as_str().unwrap())),
            "{kind}"
        );
    }
    s.finish();
}

#[test]
fn validate_model_reports_real_systems_and_repair_ready_errors() {
    let (root, run) = (Scratch::new("val-root"), Scratch::new("val-run"));
    for file in ["otel-demo", "posthog", "sentry", "notes-crud"] {
        std::fs::copy(
            format!("{EXAMPLES}/{file}.json"),
            root.0.join(format!("{file}.json")),
        )
        .unwrap();
    }
    std::fs::copy(
        format!("{EXAMPLES}/notes-crud.walkthrough.json"),
        root.0.join("notes-crud.walkthrough.json"),
    )
    .unwrap();
    let mut stale: Value = serde_json::from_str(
        &std::fs::read_to_string(root.0.join("notes-crud.walkthrough.json")).unwrap(),
    )
    .unwrap();
    stale["expected_revision"] = 9.into();
    stale["steps"][0]["focus"] = json!(["nope:missing"]);
    std::fs::write(root.0.join("stale.walkthrough.json"), stale.to_string()).unwrap();
    std::fs::write(root.0.join("garbage.json"), "{not json").unwrap();

    let mut s = Session::start(&root.0, &run.0);
    for (file, objects) in [
        ("otel-demo", 42),
        ("posthog", 52),
        ("sentry", 75),
        ("notes-crud", 8),
    ] {
        let (report, is_error) = s.tool("validate_model", json!({"path": format!("{file}.json")}));
        assert!(!is_error, "{file}: {report}");
        assert_eq!(report["ok"], true, "{file}");
        assert_eq!(report["model"]["objects"], objects, "{file}");
        assert!(
            report["advice"].is_array() && report["warnings"].is_array(),
            "{file}"
        );
    }

    let (report, is_error) = s.tool(
        "validate_model",
        json!({"path": "notes-crud.json", "walkthrough_path": "notes-crud.walkthrough.json"}),
    );
    assert!(!is_error && report["walkthrough"]["ok"] == true, "{report}");
    assert!(report["walkthrough"]["steps"].as_array().unwrap().len() >= 4);

    // A rejected walkthrough tells the agent which pointer to repair.
    let (report, is_error) = s.tool(
        "validate_model",
        json!({"path": "notes-crud.json", "walkthrough_path": "stale.walkthrough.json"}),
    );
    assert!(is_error, "{report}");
    let diagnostics = report["walkthrough"]["errors"].as_array().unwrap();
    assert!(
        diagnostics
            .iter()
            .all(|d| d["code"].is_string() && d["path"].is_string() && d["message"].is_string()),
        "{report}"
    );
    assert!(
        diagnostics.iter().any(|d| d["code"] == "stale_revision"),
        "{report}"
    );

    let (report, is_error) = s.tool("validate_model", json!({"path": "garbage.json"}));
    assert!(is_error && report["ok"] == false, "{report}");
    s.finish();
}

#[test]
fn the_bridge_reads_only_under_its_root_and_never_writes() {
    let (parent, run) = (Scratch::new("jail"), Scratch::new("jail-run"));
    let root = parent.0.join("project");
    std::fs::create_dir_all(root.join("dir")).unwrap();
    std::fs::copy(
        format!("{EXAMPLES}/notes-crud.json"),
        root.join("model.json"),
    )
    .unwrap();
    std::fs::write(parent.0.join("secret.json"), "{}").unwrap();
    std::os::unix::fs::symlink(parent.0.join("secret.json"), root.join("link.json")).unwrap();
    std::fs::write(root.join("big.json"), vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    let (before, beside) = (listing(&root), listing(&parent.0));

    let mut s = Session::start(&root, &run.0);
    for (path, code) in [
        ("../secret.json", "outside_root"),
        ("link.json", "outside_root"),
        ("/etc/hosts", "outside_root"),
        ("dir/../../secret.json", "outside_root"),
        ("missing.json", "not_found"),
        ("dir", "not_a_file"),
        ("big.json", "payload_too_large"),
    ] {
        for tool in ["validate_model", "publish_model"] {
            let (body, is_error) = s.tool(tool, json!({"path": path}));
            assert!(is_error && body["code"] == code, "{tool} {path}: {body}");
        }
        let (body, is_error) = s.tool("present_walkthrough", json!({"path": path}));
        assert!(
            is_error && body["code"] == code,
            "present_walkthrough {path}: {body}"
        );
        let (body, is_error) = s.tool(
            "validate_model",
            json!({"path": "model.json", "walkthrough_path": path}),
        );
        assert!(
            is_error && body["code"] == code,
            "walkthrough_path {path}: {body}"
        );
    }
    assert_eq!(
        listing(&parent.0),
        beside,
        "the bridge created or removed something beside the project"
    );
    assert_eq!(listing(&root), before, "the bridge changed the project");
    assert!(
        listing(&run.0).is_empty(),
        "a refused path still started a viewer"
    );
    s.finish();
}

#[test]
fn malformed_requests_fail_cleanly_before_any_viewer_is_involved() {
    let (root, run) = (Scratch::new("bad-root"), Scratch::new("bad-run"));
    std::fs::write(root.0.join("broken.json"), "{oops").unwrap();
    let mut s = Session::start(&root.0, &run.0);

    for (tool, args, code) in [
        ("validate_model", json!({}), "invalid_params"),
        ("publish_model", json!({}), "invalid_params"),
        ("present_walkthrough", json!({}), "invalid_params"),
        (
            "present_walkthrough",
            json!({"walkthrough": {}, "path": "x.json"}),
            "invalid_params",
        ),
        (
            "present_walkthrough",
            json!({"path": "broken.json"}),
            "invalid_json",
        ),
        ("no_such_tool", json!({}), "unknown_tool"),
    ] {
        let (body, is_error) = s.tool(tool, args.clone());
        assert!(is_error && body["code"] == code, "{tool} {args}: {body}");
        assert!(
            body["message"].as_str().is_some_and(|m| !m.is_empty()),
            "{tool}: {body}"
        );
    }
    assert!(
        listing(&run.0).is_empty(),
        "a malformed request started a viewer"
    );
    s.finish();
}
