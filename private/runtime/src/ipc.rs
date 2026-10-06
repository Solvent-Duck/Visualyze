//! Serve mode: local agents publish models and stage explanations over the
//! per-user socket from `visualyze_mcp::protocol`. Connection threads only frame
//! and authenticate; every request is validated and applied on the main thread.
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::Duration,
};

use bevy::prelude::*;
use serde_json::{Value, json};
use visualyze_mcp::protocol::{self, Request, Response};
use visualyze_model::{architecture::Architecture, walkthrough::Walkthrough};

use crate::{
    layout_scene::{LayoutStatus, Published, ViewerAction, ViewerActions},
    playback::{Control, Playback},
    report,
};

/// Requests waiting for the main thread. A full queue answers `busy`.
const INBOX: usize = 64;
/// Large models can take a while to lay out; the bridge reports a timeout clearly.
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);

struct Envelope {
    request: Request,
    reply: mpsc::SyncSender<Response>,
}

#[derive(Resource)]
pub(crate) struct IpcInbox(Mutex<mpsc::Receiver<Envelope>>);

/// Removes the socket and token when the viewer exits.
#[derive(Resource)]
pub(crate) struct ServeGuard {
    socket: PathBuf,
    token: PathBuf,
}

impl Drop for ServeGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.token);
    }
}

/// Bind before the window opens, so a second viewer fails fast.
pub(crate) fn bind() -> io::Result<(IpcInbox, ServeGuard)> {
    let dir = protocol::run_dir()?;
    protocol::prepare_dir(&dir)?;
    let socket = protocol::socket_path(&dir);
    protocol::check_socket_path(&socket)?;
    if socket.exists() {
        if UnixStream::connect(&socket).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "another Visualyze viewer is already serving agents",
            ));
        }
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    let token = protocol::issue_token(&dir)?;
    let (send, receive) = mpsc::sync_channel(INBOX);
    std::thread::Builder::new()
        .name("visualyze-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let send = send.clone();
                let token = token.clone();
                let _ = std::thread::Builder::new()
                    .name("visualyze-ipc-connection".into())
                    .spawn(move || {
                        let _ = connection(stream, &token, &send);
                    });
            }
        })?;
    Ok((
        IpcInbox(Mutex::new(receive)),
        ServeGuard {
            socket,
            token: protocol::token_path(&dir),
        },
    ))
}

fn read_line(reader: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    let read = reader
        .take(protocol::MAX_LINE as u64 + 1)
        .read_until(b'\n', &mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > protocol::MAX_LINE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request too large",
        ));
    }
    Ok(Some(line))
}

fn write(stream: &mut UnixStream, response: &Response) -> io::Result<()> {
    let mut line = serde_json::to_vec(response)?;
    line.push(b'\n');
    stream.write_all(&line)
}

fn connection(
    stream: UnixStream,
    token: &str,
    inbox: &mpsc::SyncSender<Envelope>,
) -> io::Result<()> {
    let mut out = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let Some(line) = read_line(&mut reader)? else {
        return Ok(());
    };
    let hello: Request = match serde_json::from_slice(&line) {
        Ok(request) => request,
        Err(_) => {
            return write(
                &mut out,
                &Response::err(0, "unauthorized", "hello required"),
            );
        }
    };
    let supplied = hello.params["token"].as_str().unwrap_or_default();
    if hello.method != "hello" || !protocol::token_matches(token, supplied) {
        return write(
            &mut out,
            &Response::err(hello.id, "unauthorized", "invalid session token"),
        );
    }
    if hello.params["protocol"] != protocol::PROTOCOL {
        return write(
            &mut out,
            &Response::err(
                hello.id,
                "protocol_mismatch",
                format!("viewer speaks {}", protocol::PROTOCOL),
            ),
        );
    }
    write(
        &mut out,
        &Response::ok(
            hello.id,
            json!({"protocol": protocol::PROTOCOL, "viewer": env!("CARGO_PKG_VERSION")}),
        ),
    )?;
    while let Some(line) = read_line(&mut reader)? {
        let request: Request = match serde_json::from_slice(&line) {
            Ok(request) => request,
            Err(error) => {
                write(
                    &mut out,
                    &Response::err(0, "invalid_request", error.to_string()),
                )?;
                continue;
            }
        };
        let id = request.id;
        let (reply, answer) = mpsc::sync_channel(1);
        let response = match inbox.try_send(Envelope { request, reply }) {
            Err(mpsc::TrySendError::Full(_)) => {
                Response::err(id, "busy", "the viewer has too many requests waiting")
            }
            Err(mpsc::TrySendError::Disconnected(_)) => {
                Response::err(id, "viewer_closed", "the viewer is shutting down")
            }
            Ok(()) => answer.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| {
                Response::err(id, "timeout", "the viewer did not finish in time")
            }),
        };
        write(&mut out, &response)?;
    }
    Ok(())
}

/// A model this viewer has accepted from an agent, kept so walkthroughs can be
/// checked against exactly what is shown.
pub(crate) struct ServedModel {
    pub model: Arc<Architecture>,
    content: Vec<u8>,
}

struct PendingPublish {
    ticket: u64,
    id: u64,
    reply: mpsc::SyncSender<Response>,
    model_id: String,
    revision: u64,
    served: ServedModel,
    summary: Value,
}

#[derive(Resource, Default)]
pub(crate) struct Served {
    models: HashMap<String, ServedModel>,
    pending: Vec<PendingPublish>,
}

impl Served {
    pub(crate) fn with(model: Architecture, content: Vec<u8>) -> Self {
        let mut served = Self::default();
        served.models.insert(
            model.snapshot().model_id.clone(),
            ServedModel {
                model: Arc::new(model),
                content,
            },
        );
        served
    }

    /// The model the viewer currently shows, if an agent or the command line supplied it.
    pub(crate) fn current(&self, status: &LayoutStatus) -> Option<&Arc<Architecture>> {
        let served = self.models.get(status.model_id.as_deref()?)?;
        (Some(served.model.snapshot().revision) == status.revision).then_some(&served.model)
    }
}

pub(crate) fn serve_requests(
    inbox: Res<IpcInbox>,
    mut served: ResMut<Served>,
    mut actions: ResMut<ViewerActions>,
    status: Res<LayoutStatus>,
    mut playback: ResMut<Playback>,
) {
    let mut finished = Vec::new();
    for (i, pending) in served.pending.iter().enumerate() {
        if status.action_finished(pending.ticket) {
            finished.push(i);
        }
    }
    for i in finished.into_iter().rev() {
        let pending = served.pending.remove(i);
        let shown = status.model_id.as_deref() == Some(pending.model_id.as_str())
            && status.revision == Some(pending.revision);
        let response = if shown {
            served.models.insert(pending.model_id, pending.served);
            Response::ok(pending.id, pending.summary)
        } else {
            Response::err(
                pending.id,
                "layout_failed",
                status
                    .action_error
                    .clone()
                    .or_else(|| status.error.clone())
                    .unwrap_or_else(|| "the viewer kept its previous model".into()),
            )
        };
        let _ = pending.reply.send(response);
    }

    let Ok(inbox) = inbox.0.lock() else {
        return;
    };
    // A few per frame keeps the window responsive under a burst.
    for _ in 0..8 {
        let Ok(Envelope { request, reply }) = inbox.try_recv() else {
            break;
        };
        let id = request.id;
        let outcome = match request.method.as_str() {
            "publish" => {
                publish(id, &request.params, &mut served, &mut actions, &status).map(|pending| {
                    if let Some(mut pending) = pending {
                        playback.stop();
                        pending.reply = reply.clone();
                        served.pending.push(pending);
                        None
                    } else {
                        Some(Response::ok(id, json!({"ok": true, "unchanged": true})))
                    }
                })
            }
            "state" => Ok(Some(Response::ok(id, state(&served, &status, &playback)))),
            "present" | "view" => present(
                request.method == "view",
                &request.params,
                &served,
                &status,
                &mut playback,
            )
            .map(|result| Some(Response::ok(id, result))),
            "control" => control(&request.params, &mut playback)
                .map(|()| Some(Response::ok(id, playback.state()))),
            other => Err(Response::err(
                id,
                "unknown_method",
                format!("unknown method {other}"),
            )),
        };
        match outcome {
            Ok(Some(response)) => {
                let _ = reply.send(response);
            }
            Ok(None) => {}
            Err(mut response) => {
                response.id = id;
                let _ = reply.send(response);
            }
        }
    }
}

fn invalid(code: &str, error: &visualyze_model::architecture::ValidationError) -> Response {
    Response::err_with(0, code, error.to_string(), report::error(error))
}

/// Revision rules (ADR 0008): a supplied `expected_revision` must match what this
/// viewer last accepted for the model; revisions only move forward; the same
/// revision is accepted again only with identical content.
fn publish(
    id: u64,
    params: &Value,
    served: &mut Served,
    actions: &mut ViewerActions,
    status: &LayoutStatus,
) -> Result<Option<PendingPublish>, Response> {
    let content = params["content"]
        .as_str()
        .ok_or_else(|| Response::err(0, "invalid_request", "content is required"))?;
    let path = params["path"].as_str().map(PathBuf::from);
    let expected = params["expected_revision"].as_u64();
    let model = Architecture::from_json(content.as_bytes())
        .map_err(|error| invalid("invalid_model", &error))?;
    let snapshot = model.snapshot();
    let (model_id, revision) = (snapshot.model_id.clone(), snapshot.revision);
    let known = served.models.get(&model_id);
    let known_revision = known.map(|m| m.model.snapshot().revision);
    if let (Some(expected), Some(known)) = (expected, known_revision)
        && expected != known
    {
        return Err(Response::err(
            0,
            "stale_revision",
            format!(
                "expected revision {expected}, but the viewer has revision {known} of {model_id}; re-read the model file"
            ),
        ));
    }
    if let Some(known) = known
        && revision == known.model.snapshot().revision
    {
        if known.content != content.as_bytes() {
            return Err(Response::err(
                0,
                "revision_reused",
                format!(
                    "revision {revision} of {model_id} was already published with different content; increase the revision"
                ),
            ));
        }
        if status.model_id.as_deref() == Some(model_id.as_str())
            && status.revision == Some(revision)
        {
            return Ok(None);
        }
    } else if let Some(known) = known_revision
        && revision < known
    {
        return Err(Response::err(
            0,
            "stale_revision",
            format!("the viewer already has revision {known} of {model_id}"),
        ));
    }
    let summary = report::model(&model);
    let shared = Arc::new(model);
    // The layout input owns its own copy; both come from the same validated bytes.
    let layout_model = Architecture::from_json(content.as_bytes())
        .map_err(|error| invalid("invalid_model", &error))?;
    let ticket = actions
        .submit(ViewerAction::Publish(Published::new(layout_model, path)))
        .map_err(|error| Response::err(0, "busy", error))?;
    let (reply, _) = mpsc::sync_channel(1);
    Ok(Some(PendingPublish {
        ticket,
        id,
        reply,
        model_id,
        revision,
        served: ServedModel {
            model: shared,
            content: content.as_bytes().to_vec(),
        },
        summary,
    }))
}

fn present(
    single_view: bool,
    params: &Value,
    served: &Served,
    status: &LayoutStatus,
    playback: &mut Playback,
) -> Result<Value, Response> {
    let model = served.current(status).cloned().ok_or_else(|| {
        Response::err(
            0,
            "no_model",
            "the viewer is not showing an agent-supplied model yet; publish one first",
        )
    })?;
    let script = if single_view {
        let snapshot = model.snapshot();
        let mut step = params["scene"].clone();
        if !step.is_object() {
            return Err(Response::err(0, "invalid_request", "scene is required"));
        }
        step["id"] = "view".into();
        json!({
            "schema_version": visualyze_model::walkthrough::SCHEMA_VERSION,
            "id": "view",
            "model_id": params.get("model_id").cloned().unwrap_or_else(|| snapshot.model_id.clone().into()),
            "expected_revision": params.get("expected_revision").cloned().unwrap_or_else(|| snapshot.revision.into()),
            "title": step["title"].clone(),
            "steps": [step],
        })
    } else {
        params["walkthrough"].clone()
    };
    let walkthrough = Walkthrough::from_json(script.to_string().as_bytes(), &model)
        .map_err(|error| invalid("invalid_walkthrough", &error))?;
    let result = report::walkthrough(&walkthrough);
    let start = params["start_step"].as_u64().unwrap_or(0) as usize;
    let autoplay = params["autoplay"].as_bool().unwrap_or(false);
    playback.start(walkthrough, model, start, autoplay, !single_view);
    Ok(result)
}

fn control(params: &Value, playback: &mut Playback) -> Result<(), Response> {
    let control = match params["action"].as_str() {
        Some("play") => Control::Play,
        Some("pause") => Control::Pause,
        Some("next") => Control::Next,
        Some("previous") => Control::Previous,
        Some("stop") => Control::Stop,
        Some("goto") => Control::Goto(
            params["step"]
                .as_u64()
                .ok_or_else(|| Response::err(0, "invalid_request", "goto needs a step index"))?
                as usize,
        ),
        _ => {
            return Err(Response::err(
                0,
                "invalid_request",
                "action must be play, pause, next, previous, goto, or stop",
            ));
        }
    };
    playback
        .control(control)
        .map_err(|e| Response::err(0, "no_walkthrough", e))
}

fn state(served: &Served, status: &LayoutStatus, playback: &Playback) -> Value {
    let model = served.current(status);
    let label = |id: &str| {
        model.and_then(|m| {
            m.snapshot()
                .entities
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.label.clone())
        })
    };
    json!({
        "ready": status.ready,
        "loading": status.loading,
        "model_id": status.model_id,
        "revision": status.revision,
        "agent_supplied": model.is_some(),
        "root": status.root,
        "breadcrumbs": status.breadcrumbs,
        "selected": status.selected.as_ref().map(|id| json!({"id": id, "label": label(id)})),
        "selected_relation": status.selected_relation,
        "visible": {"objects": status.object_count, "connectors": status.relation_count},
        "error": status.action_error.as_ref().or(status.error.as_ref()),
        "playback": playback.state(),
    })
}
