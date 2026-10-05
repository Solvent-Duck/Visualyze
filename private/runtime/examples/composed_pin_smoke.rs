//! Native pin/collapse/reopen/undo proof with fresh, isolated private state.
//! Usage: composed_pin_smoke DATA_MESSAGING_BUNDLE NEW_OUTPUT_DIRECTORY
#[path = "support/scripted_input.rs"]
mod scripted_input;
use bevy::{
    app::AppExit,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};
use visualyze::{
    LayoutEngine, LayoutStatus, LayoutStorage, PlacedObject, ViewerAction, ViewerActions,
};

#[derive(Resource)]
struct Proof {
    output: PathBuf,
    storage: PathBuf,
    steps: Vec<ViewerAction>,
    index: usize,
    ticket: Option<u64>,
    settled: usize,
    awaiting: bool,
    pin_position: Option<Vec3>,
    pin_local: Option<visualyze_layout::Point>,
    child_anchor: Option<visualyze_layout::assembly::AnchorPin>,
    parent_position: Option<Vec3>,
    started: Instant,
    records: Vec<serde_json::Value>,
}
#[derive(Resource, Default)]
struct Captures(usize);
fn main() -> AppExit {
    let mut args = std::env::args_os().skip(1);
    let bundle = PathBuf::from(args.next().expect("data messaging bundle"));
    let output = PathBuf::from(args.next().expect("new output directory"));
    assert!(args.next().is_none());
    std::fs::create_dir(&output).expect("output directory must be new");
    let storage = output.join("state");
    std::fs::create_dir(&storage).unwrap();
    let steps = vec![
        ViewerAction::Expand("datastore:orders".into()),
        ViewerAction::Expand("table:orders".into()),
        ViewerAction::Pin("column:customer-id".into()),
        ViewerAction::Collapse("datastore:orders".into()),
        ViewerAction::Expand("datastore:orders".into()),
        ViewerAction::Expand("table:orders".into()),
        ViewerAction::Unpin("column:customer-id".into()),
        ViewerAction::Undo,
        ViewerAction::Enter("datastore:orders".into()),
        ViewerAction::Enter("table:orders".into()),
        ViewerAction::Back,
        ViewerAction::Back,
        ViewerAction::Unpin("column:customer-id".into()),
        ViewerAction::Enter("datastore:orders".into()),
        ViewerAction::Pin("column:customer-id".into()),
        ViewerAction::Back,
        ViewerAction::Collapse("datastore:orders".into()),
        ViewerAction::Expand("datastore:orders".into()),
        ViewerAction::OpenPinOrigin("column:customer-id".into()),
        ViewerAction::Unpin("column:customer-id".into()),
        ViewerAction::Undo,
        ViewerAction::Back,
    ];
    let mut app = visualyze::app_with_bundle(bundle, LayoutEngine::Composed);
    scripted_input::install(&mut app);
    app.insert_resource(LayoutStorage(Some(storage.clone())))
        .insert_resource(Proof {
            output,
            storage,
            steps,
            index: 0,
            ticket: None,
            settled: 0,
            awaiting: false,
            pin_position: None,
            pin_local: None,
            child_anchor: None,
            parent_position: None,
            started: Instant::now(),
            records: vec![],
        })
        .init_resource::<Captures>()
        .add_systems(Update, drive)
        .run()
}
#[allow(clippy::too_many_arguments)]
fn drive(
    mut proof: ResMut<Proof>,
    captures: Res<Captures>,
    status: Res<LayoutStatus>,
    objects: Query<(&PlacedObject, &Transform)>,
    mut actions: ResMut<ViewerActions>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    assert!(
        proof.started.elapsed().as_secs() < 60,
        "native pin proof timed out"
    );
    assert!(status.error.is_none(), "layout error: {:?}", status.error);
    assert!(
        status.action_error.is_none(),
        "action error: {:?}",
        status.action_error
    );
    if !status.ready || status.loading {
        return;
    }
    if proof.awaiting {
        let expected = proof
            .records
            .iter()
            .filter(|r| r["screenshot"].as_bool() == Some(true))
            .count();
        if captures.0 < expected {
            return;
        }
        proof.awaiting = false;
        proof.ticket = None;
        proof.index += 1;
        proof.settled = 0;
    }
    if proof.index == proof.steps.len() {
        assert_eq!(status.save_attempts, proof.steps.len() as u64 + 1);
        assert_eq!(status.save_failures, 0);
        let report = serde_json::json!({"actions":proof.records,"input_mode":scripted_input::INPUT_MODE,
            "save_attempts":status.save_attempts,"save_failures":status.save_failures,"save_max_ms":status.save_max_ms,
            "scope":"Actual native composed display and isolated v6 persistence. Exact rendered pinned position checked across collapse/reopen/unpin/undo and parent return; exact canonical point and truncated anchors checked through database/table descent. Child-origin pins preserve their original anchors when displayed in parents, including collapse/reopen, open-origin navigation and undo. Inheritance never copies owned registry entries. Restart and eviction have separate worker/core regressions; this process does not simulate hardware input or establish latency percentiles."});
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(proof.output.join("pin-report.json"))
            .unwrap();
        serde_json::to_writer_pretty(file, &report).unwrap();
        proof.index += 1;
        exit.write(AppExit::Success);
        return;
    }
    if proof.index > proof.steps.len() {
        return;
    }
    if proof.ticket.is_none() {
        proof.ticket = Some(actions.submit(proof.steps[proof.index].clone()).unwrap());
        return;
    }
    if !status.action_finished(proof.ticket.unwrap()) {
        return;
    }
    proof.settled += 1;
    if proof.settled < 3 {
        return;
    }
    let index = proof.index;
    let position = objects
        .iter()
        .find(|(o, _)| o.id == "column:customer-id")
        .map(|(_, t)| t.translation);
    let saved_file = std::fs::read_dir(&proof.storage)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .expect("accepted private state");
    let saved = visualyze_layout::composed::persistence::load(&saved_file, &AtomicBool::new(false))
        .unwrap();
    let retained = saved
        .constraint_sets
        .iter()
        .flat_map(|s| s.pins.values())
        .any(|p| p.contains_key("column:customer-id"));
    if index == 2 {
        assert!(position.is_some());
        proof.pin_position = position;
        proof.pin_local = Some(saved.canonical_pins["table:orders"]["column:customer-id"]);
        assert!(retained);
    }
    if index == 3 {
        assert!(position.is_none());
        assert!(retained, "collapse erased durable pin");
        assert!(
            saved.canonical_pins.is_empty(),
            "hidden pin passed to visible solve"
        );
    }
    if index == 5 || index == 7 || index == 11 {
        assert_eq!(
            position, proof.pin_position,
            "rendered pinned position changed"
        );
        assert!(retained);
        assert!(status.composition_frames >= 3);
    }
    if index == 6 {
        assert!(!retained, "unpin did not remove override");
    }
    let local = saved
        .canonical_pins
        .get("table:orders")
        .and_then(|pins| pins.get("column:customer-id"))
        .copied();
    if (8..=10).contains(&index) {
        assert!(position.is_some());
        assert_eq!(local, proof.pin_local, "inherited canonical pin moved");
        assert_eq!(
            saved.constraint_sets.len(),
            1,
            "inherited pin duplicated into registry"
        );
        assert_eq!(saved.constraint_sets[0].context.root, "scope:orders");
        let anchor = &saved.anchor_pins[0];
        assert_eq!(anchor.anchor(), saved.context.root);
        anchor.validate(&saved.accepted.assembly).unwrap();
    }
    if index == 12 || index == 19 {
        assert!(!retained, "origin unpin did not remove override");
    }
    if index == 14 {
        proof.pin_local = local;
        proof.child_anchor = Some(saved.anchor_pins[0].clone());
        assert_eq!(saved.anchor_pins[0].anchor(), "datastore:orders");
    }
    if index == 15 {
        assert!(position.is_some());
        proof.parent_position = position;
    }
    if index == 16 {
        assert!(position.is_none());
        assert!(retained);
        assert!(saved.canonical_pins.is_empty());
        assert!(saved.anchor_pins.is_empty());
    }
    if matches!(index, 14 | 15 | 17 | 18 | 20 | 21) {
        assert_eq!(local, proof.pin_local, "child-origin canonical pin moved");
        assert_eq!(saved.anchor_pins[0], *proof.child_anchor.as_ref().unwrap());
        saved.anchor_pins[0]
            .validate(&saved.accepted.assembly)
            .unwrap();
        assert_eq!(saved.constraint_sets.len(), 1);
        assert_eq!(saved.constraint_sets[0].context.root, "datastore:orders");
    }
    if index == 17 || index == 21 {
        assert_eq!(
            position, proof.parent_position,
            "parent return moved child-origin pin"
        );
    }
    let capture = matches!(index, 2 | 3 | 7 | 9 | 11 | 14 | 17 | 21);
    let action = format!("{:?}", proof.steps[index]);
    proof.records.push(serde_json::json!({"step":index,"action":action,
        "objects":status.object_count,"routes":status.relation_count,"frames":status.composition_frames,
        "pin_position":position.map(|p|p.to_array()),"canonical_pin":local,
        "focus":saved.context.root,"owned_contexts":saved.constraint_sets.len(),
        "durable_pin_present":retained,"screenshot":capture}));
    if capture {
        proof.awaiting = true;
        let path = proof.output.join(format!("step-{index}.png"));
        commands.spawn(Screenshot::primary_window()).observe(
            move |capture: On<ScreenshotCaptured>, mut captures: ResMut<Captures>| {
                capture
                    .image
                    .clone()
                    .try_into_dynamic()
                    .unwrap()
                    .save(&path)
                    .unwrap();
                captures.0 += 1;
            },
        );
    } else {
        proof.ticket = None;
        proof.index += 1;
        proof.settled = 0;
    }
}
