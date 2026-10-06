//! End-to-end walkthrough proof: serve agents, then act as one over the local
//! socket. Publish a model, present a walkthrough, and save one frame per step
//! once its camera move and trace have had time to play.
//!
//! cargo run --example walkthrough_smoke -- [model.json walkthrough.json]
use bevy::{
    app::AppExit,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};
use visualyze_mcp::protocol::{self, Request, Response};

/// Seconds to let a step's camera move and first trace pass play out.
const SETTLE: f32 = 3.2;

enum Ask {
    Capture(String, mpsc::Sender<()>),
    Exit(bool),
}

#[derive(Resource)]
struct Asks(Mutex<mpsc::Receiver<Ask>>);

fn main() -> AppExit {
    let mut args = std::env::args().skip(1);
    let model = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "public/examples/notes-crud.json".into()),
    );
    let walkthrough = PathBuf::from(
        args.next()
            .unwrap_or_else(|| "public/examples/notes-crud.walkthrough.json".into()),
    );
    if std::env::var_os("VISUALYZE_RUN_DIR").is_none() {
        // SAFETY: set before any other thread starts.
        unsafe {
            std::env::set_var(
                "VISUALYZE_RUN_DIR",
                std::env::temp_dir().join(format!("vz-smoke-{}", std::process::id())),
            );
        }
    }
    let out = std::env::temp_dir().join("visualyze-walkthrough");
    std::fs::create_dir_all(&out).expect("output directory");
    let (send, receive) = mpsc::channel();
    let mut app =
        visualyze::app_serving(None, visualyze::LayoutEngine::Composed).expect("serve agents");
    app.insert_resource(Asks(Mutex::new(receive)))
        .add_systems(Update, answer);
    std::thread::spawn(move || {
        let ok = match agent(&model, &walkthrough, &out, &send) {
            Ok(()) => true,
            Err(error) => {
                eprintln!("walkthrough smoke failed: {error}");
                false
            }
        };
        let _ = send.send(Ask::Exit(ok));
    });
    app.run()
}

fn answer(asks: Res<Asks>, mut commands: Commands, mut exit: MessageWriter<AppExit>) {
    let Ok(asks) = asks.0.lock() else {
        return;
    };
    while let Ok(ask) = asks.try_recv() {
        match ask {
            Ask::Capture(path, done) => {
                commands.spawn(Screenshot::primary_window()).observe(
                    move |capture: On<ScreenshotCaptured>| {
                        let image = capture
                            .image
                            .clone()
                            .try_into_dynamic()
                            .expect("GPU image")
                            .to_rgb8();
                        image.save(&path).expect("save frame");
                        let _ = done.send(());
                    },
                );
            }
            Ask::Exit(ok) => {
                exit.write(if ok {
                    AppExit::Success
                } else {
                    AppExit::error()
                });
            }
        }
    }
}

struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    next: u64,
}

impl Client {
    fn connect() -> Result<Self, String> {
        let dir = protocol::run_dir().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let stream = loop {
            if let Ok(stream) = UnixStream::connect(protocol::socket_path(&dir)) {
                break stream;
            }
            if Instant::now() > deadline {
                return Err("viewer socket never appeared".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let mut client = Self {
            reader: BufReader::new(stream.try_clone().map_err(|e| e.to_string())?),
            writer: stream,
            next: 0,
        };
        let token = protocol::read_token(&dir).map_err(|e| e.to_string())?;
        client.call(
            "hello",
            json!({"token": token, "protocol": protocol::PROTOCOL}),
        )?;
        Ok(client)
    }

    fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next += 1;
        let request = Request {
            id: self.next,
            method: method.into(),
            params,
        };
        let mut line = serde_json::to_vec(&request).unwrap();
        line.push(b'\n');
        self.writer.write_all(&line).map_err(|e| e.to_string())?;
        let mut reply = String::new();
        self.reader
            .read_line(&mut reply)
            .map_err(|e| e.to_string())?;
        let response: Response = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        match response.error {
            Some(error) => Err(format!("{}: {}", error.code, error.message)),
            None => Ok(response.result.unwrap_or(Value::Null)),
        }
    }
}

fn agent(
    model: &PathBuf,
    walkthrough: &PathBuf,
    out: &std::path::Path,
    ask: &mpsc::Sender<Ask>,
) -> Result<(), String> {
    let mut client = Client::connect()?;
    let content = std::fs::read_to_string(model).map_err(|e| e.to_string())?;
    let published = client.call("publish", json!({"content": content}))?;
    println!("published {}", published["model"]["model_id"]);
    let script: Value =
        serde_json::from_slice(&std::fs::read(walkthrough).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let presented = client.call("present", json!({"walkthrough": script}))?;
    let total = presented["steps"].as_array().map_or(0, Vec::len);
    for step in 0..total {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let state = client.call("state", json!({}))?;
            let playback = &state["playback"];
            if playback["step"] == step && playback["state"] != "staging" {
                if let Some(error) = playback["error"].as_str() {
                    return Err(format!("step {step}: {error}"));
                }
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("step {step} never finished staging: {state}"));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_secs_f32(SETTLE));
        let path = out.join(format!("step-{step}.png"));
        let (done, wait) = mpsc::channel();
        ask.send(Ask::Capture(path.to_string_lossy().into_owned(), done))
            .map_err(|e| e.to_string())?;
        wait.recv_timeout(Duration::from_secs(10))
            .map_err(|_| "screenshot timed out".to_string())?;
        println!(
            "step {step}: {} -> {}",
            presented["steps"][step]["title"],
            path.display()
        );
        if step + 1 < total {
            client.call("control", json!({"action": "next"}))?;
        }
    }
    Ok(())
}
