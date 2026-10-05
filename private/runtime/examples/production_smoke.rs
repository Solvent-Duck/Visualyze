//! Native end-to-end proof against the pinned, source-derived Bevy bundle.
//! Usage: production_smoke BUNDLE NEW_OUTPUT_DIRECTORY [--persist] [--composed] [--trace-frames] [--no-captures] [--repeat-navigation] [--present-immediate | --present-background-immediate] [--skip-occluded] [--await-occlusion-cycle]
#[path = "support/frame_trace.rs"]
mod frame_trace;
#[path = "support/occlusion_probe.rs"]
mod occlusion_probe;
#[path = "support/presentation_probe.rs"]
mod presentation_probe;
#[path = "support/scripted_input.rs"]
mod scripted_input;

use bevy::{
    app::AppExit,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use std::{collections::BTreeMap, path::PathBuf, time::Instant};
use visualyze::{
    LayoutEngine, LayoutStatus, LayoutStorage, LayoutTiming, PlacedObject, PlacedRelation,
    ViewerAction, ViewerActions,
};
use visualyze_model::provider::{EntityKind, Limits, LocalBundle, ModelProvider, PageRequest};

const REPOSITORY: &str = "repository:05317a01f80959f2";
const PACKAGE: &str = "package:d1c2531f3a0a9b57";
const FILE: &str = "file:b6ae9420da8c77aa";

struct Step {
    name: &'static str,
    action: ViewerAction,
    root: String,
    page: usize,
    relation_page: usize,
    screenshot: bool,
    cached: bool,
    view_key: &'static str,
}
#[derive(Resource)]
struct Proof {
    type_id: String,
    engine: LayoutEngine,
    function_id: String,
    output: PathBuf,
    persistence_directory: Option<PathBuf>,
    counts: visualyze_model::provider::Counts,
    started: Instant,
    steps: Vec<Step>,
    index: usize,
    ticket: Option<u64>,
    action_started: Instant,
    accepted_ms: Option<f64>,
    settled_frames: usize,
    awaiting_capture: bool,
    records: Vec<serde_json::Value>,
    positions: BTreeMap<String, BTreeMap<String, Vec3>>,
    frame_deltas: Vec<f64>,
    worker_frame_deltas: Vec<f64>,
    idle_frame_deltas: Vec<f64>,
    was_loading: bool,
    was_capturing: bool,
    previous_frame: Instant,
    first_acceptance_ms: Option<f64>,
    selected_route: Option<(String, Vec<String>, Vec<Vec3>)>,
}
#[derive(Resource, Default)]
struct Captures {
    completed: usize,
}

// Resolve declarations by supplied ownership and kind, so both explicitly
// versioned producer identity schemes exercise the same real source records.
fn child_named(provider: &mut LocalBundle, parent: &str, label: &str, kind: EntityKind) -> String {
    let revision = provider.manifest().revision;
    let mut after = None;
    let mut found = Vec::new();
    loop {
        let page = provider
            .children(revision, parent, PageRequest { limit: 128, after })
            .unwrap();
        found.extend(
            page.items
                .into_iter()
                .filter(|e| e.label == label && e.kind == kind)
                .map(|e| e.id),
        );
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(
        found.len(),
        1,
        "expected one {label} under supplied owner {parent}"
    );
    found.pop().unwrap()
}

fn main() -> AppExit {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().expect("bundle path"));
    let output = PathBuf::from(args.next().expect("new output directory"));
    let mut persist = false;
    let mut trace_frames = false;
    let mut no_captures = false;
    let mut repeat_navigation = false;
    let mut skip_occluded = false;
    let mut await_occlusion_cycle = false;
    let mut presentation = presentation_probe::PresentationProbe::default();
    let mut engine = LayoutEngine::Floorplan;
    for arg in args {
        match arg.to_str() {
            Some("--persist") if !persist => persist = true,
            Some("--trace-frames") if !trace_frames => trace_frames = true,
            Some("--no-captures") if !no_captures => no_captures = true,
            Some("--repeat-navigation") if !repeat_navigation => repeat_navigation = true,
            Some("--skip-occluded") if !skip_occluded => skip_occluded = true,
            Some("--await-occlusion-cycle") if !await_occlusion_cycle => {
                await_occlusion_cycle = true
            }
            Some("--present-immediate") | Some("--present-background-immediate") => {
                presentation = presentation.with_argument(arg.to_str().unwrap());
            }
            Some("--composed") if engine == LayoutEngine::Floorplan => {
                engine = LayoutEngine::Composed
            }
            _ => panic!(
                "optional arguments are --persist, --composed, --trace-frames, --no-captures, --repeat-navigation, --skip-occluded, --await-occlusion-cycle and one presentation probe, each once"
            ),
        }
    }
    assert!(
        !skip_occluded || no_captures,
        "--skip-occluded requires --no-captures; suspended screenshot handling is not implemented"
    );
    std::fs::create_dir(&output).expect("output directory must be new");
    let persistence_directory = persist.then(|| {
        let path = std::env::temp_dir().join(format!(
            "visualyze-persistence-smoke-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).expect("isolated persistence directory must be new");
        path
    });
    let mut provider =
        LocalBundle::open(&path, Limits::default()).expect("validated production model");
    let counts = provider.stats().counts;
    assert!(
        counts.entities >= 20_000 && counts.relations >= 40_000 && counts.evidence >= 30_000,
        "this proof requires the complete pinned production-source model"
    );
    let revision = provider.manifest().revision;
    for (id, label) in [(PACKAGE, "bevy_ecs"), (FILE, "src/world/mod.rs")] {
        assert_eq!(
            provider.summary(revision, id).unwrap().unwrap().label,
            label
        );
    }
    let type_id = child_named(&mut provider, FILE, "World", EntityKind::CodeType);
    let function_id = child_named(
        &mut provider,
        &type_id,
        "World::spawn",
        EntityKind::CodeFunction,
    );
    drop(provider);
    let step = |name, action, root: &str, page, screenshot, cached| Step {
        name,
        action,
        root: root.into(),
        page,
        relation_page: 0,
        screenshot,
        cached,
        view_key: "collapsed",
    };
    let mut steps = vec![
        step(
            "repository",
            ViewerAction::Jump(REPOSITORY.into()),
            REPOSITORY,
            0,
            true,
            false,
        ),
        step(
            "search",
            ViewerAction::Search("bevy_ecs".into()),
            REPOSITORY,
            0,
            false,
            false,
        ),
        step(
            "package",
            ViewerAction::Jump(PACKAGE.into()),
            PACKAGE,
            0,
            true,
            false,
        ),
        step(
            "package-page-2",
            ViewerAction::NextPage,
            PACKAGE,
            1,
            true,
            false,
        ),
        step(
            "package-page-1-return",
            ViewerAction::PreviousPage,
            PACKAGE,
            0,
            false,
            true,
        ),
        step(
            "source-file",
            ViewerAction::Jump(FILE.into()),
            FILE,
            0,
            true,
            false,
        ),
        Step {
            relation_page: 1,
            ..step(
                "source-file-relationships-2",
                ViewerAction::NextRelations,
                FILE,
                0,
                true,
                false,
            )
        },
        step(
            "source-file-relationships-1-return",
            ViewerAction::PreviousRelations,
            FILE,
            0,
            false,
            true,
        ),
        step(
            "world-type",
            ViewerAction::Jump(type_id.clone()),
            &type_id,
            0,
            true,
            false,
        ),
        step(
            "world-methods-page-2",
            ViewerAction::NextPage,
            &type_id,
            1,
            true,
            false,
        ),
        step(
            "world-methods-page-1-return",
            ViewerAction::PreviousPage,
            &type_id,
            0,
            false,
            true,
        ),
        step(
            "world-spawn",
            ViewerAction::Jump(function_id.clone()),
            &function_id,
            0,
            true,
            false,
        ),
        step(
            "world-type-return",
            ViewerAction::Back,
            &type_id,
            0,
            false,
            true,
        ),
        step(
            "source-file-return",
            ViewerAction::Back,
            FILE,
            0,
            false,
            true,
        ),
        step(
            "package-return",
            ViewerAction::Back,
            PACKAGE,
            0,
            false,
            true,
        ),
        step(
            "repository-return",
            ViewerAction::Back,
            REPOSITORY,
            0,
            false,
            true,
        ),
        step(
            "select-relationship",
            ViewerAction::SelectRelation(String::new()),
            REPOSITORY,
            0,
            false,
            true,
        ),
        step(
            "show-details",
            ViewerAction::ToggleDetails,
            REPOSITORY,
            0,
            false,
            true,
        ),
        step(
            "isolated-relationship",
            ViewerAction::ToggleIsolate,
            REPOSITORY,
            0,
            true,
            false,
        ),
        step(
            "restore-relationships",
            ViewerAction::ToggleIsolate,
            REPOSITORY,
            0,
            false,
            true,
        ),
        step(
            "clear-relationship",
            ViewerAction::ClearRelationSelection,
            REPOSITORY,
            0,
            false,
            true,
        ),
    ];
    if engine == LayoutEngine::Composed {
        steps.extend([
            Step {
                view_key: "ecs-expanded",
                ..step(
                    "repository-ecs-expanded",
                    ViewerAction::Expand(PACKAGE.into()),
                    REPOSITORY,
                    0,
                    true,
                    false,
                )
            },
            step(
                "repository-ecs-collapsed",
                ViewerAction::Collapse(PACKAGE.into()),
                REPOSITORY,
                0,
                false,
                true,
            ),
            step(
                "world-file-nested-start",
                ViewerAction::Jump(FILE.into()),
                FILE,
                0,
                false,
                true,
            ),
            Step {
                view_key: "world-expanded",
                ..step(
                    "world-type-expanded",
                    ViewerAction::Expand(type_id.clone()),
                    FILE,
                    0,
                    true,
                    false,
                )
            },
            step(
                "world-type-collapsed",
                ViewerAction::Collapse(type_id.clone()),
                FILE,
                0,
                false,
                true,
            ),
        ]);
    }
    if repeat_navigation {
        for _ in 0..4 {
            steps.extend([
                step(
                    "repeat-package",
                    ViewerAction::Jump(PACKAGE.into()),
                    PACKAGE,
                    0,
                    false,
                    true,
                ),
                step(
                    "repeat-source-file",
                    ViewerAction::Jump(FILE.into()),
                    FILE,
                    0,
                    false,
                    true,
                ),
                step(
                    "repeat-repository",
                    ViewerAction::Jump(REPOSITORY.into()),
                    REPOSITORY,
                    0,
                    false,
                    true,
                ),
            ]);
        }
    }
    if no_captures {
        for step in &mut steps {
            step.screenshot = false;
        }
    }
    let now = Instant::now();
    let mut app = visualyze::app_with_bundle(path, engine);
    presentation_probe::install(&mut app, presentation, await_occlusion_cycle, now);
    if skip_occluded {
        occlusion_probe::install(&mut app);
    }
    scripted_input::install(&mut app);
    if trace_frames {
        frame_trace::install(&mut app, now);
    }
    app.insert_resource(LayoutStorage(persistence_directory.clone()))
        .insert_resource(Captures::default())
        .insert_resource(Proof {
            type_id,
            engine,
            function_id,
            output,
            persistence_directory,
            counts,
            started: now,
            steps,
            index: 0,
            ticket: None,
            action_started: now,
            accepted_ms: None,
            settled_frames: 0,
            awaiting_capture: false,
            records: vec![],
            positions: BTreeMap::new(),
            frame_deltas: vec![],
            worker_frame_deltas: vec![],
            idle_frame_deltas: vec![],
            was_loading: false,
            was_capturing: false,
            previous_frame: now,
            first_acceptance_ms: None,
            selected_route: None,
        })
        .add_systems(Startup, |mut windows: Query<&mut Window>| {
            for mut window in &mut windows {
                window.title = "Visualyze - production verification".into();
            }
        })
        .add_systems(Update, drive)
        .run()
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut proof: ResMut<Proof>,
    mut trace: Option<ResMut<frame_trace::FrameTrace>>,
    captures: Res<Captures>,
    status: Res<LayoutStatus>,
    mut actions: ResMut<ViewerActions>,
    objects: Query<(&PlacedObject, &Transform)>,
    relations: Query<&PlacedRelation>,
    mut exit: MessageWriter<AppExit>,
    presentation: Res<presentation_probe::PresentationProbe>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    occlusion: Option<Res<occlusion_probe::OcclusionProbeReport>>,
    cycle: Res<presentation_probe::OcclusionCycle>,
) {
    let now = Instant::now();
    let delta = now.duration_since(proof.previous_frame).as_secs_f64() * 1000.0;
    proof.previous_frame = now;
    if proof.first_acceptance_ms.is_some() && proof.frame_deltas.len() < 30_000 {
        proof.frame_deltas.push(delta);
        // Exclude intervals touching capture at either end, including the frame
        // that scheduled it after the state sample below. Keep all intervals.
        if !proof.was_capturing && !proof.awaiting_capture {
            if proof.was_loading || status.loading {
                proof.worker_frame_deltas.push(delta);
            } else {
                proof.idle_frame_deltas.push(delta);
            }
        }
    }
    proof.was_loading = status.loading;
    proof.was_capturing = proof.awaiting_capture;
    assert!(
        proof.started.elapsed().as_secs() < 180,
        "production viewer timed out at step {}",
        proof.index
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
    if proof.first_acceptance_ms.is_none() {
        proof.first_acceptance_ms = Some(proof.started.elapsed().as_secs_f64() * 1000.0);
    }
    if !cycle.complete() {
        return;
    }
    if proof.awaiting_capture {
        let expected = proof
            .records
            .iter()
            .filter(|r| r["screenshot"].as_bool() == Some(true))
            .count();
        if captures.completed < expected {
            return;
        }
        proof.awaiting_capture = false;
        if let Some(trace) = trace.as_mut() {
            trace.capture_pending(false);
        }
        proof.ticket = None;
        proof.index += 1;
        proof.settled_frames = 0;
    }
    if proof.index == proof.steps.len() {
        if proof.persistence_directory.is_some() {
            assert!(
                status.save_attempts > 0,
                "persistence mode did not save accepted state"
            );
            assert_eq!(status.save_failures, 0, "persistence-mode save failed");
        } else {
            assert_eq!(status.save_attempts, 0);
        }
        let report = serde_json::json!({"source_counts":proof.counts,"initial_acceptance_ms":proof.first_acceptance_ms,
            "input_mode":scripted_input::INPUT_MODE,"engine":format!("{:?}", proof.engine),
            "frame_trace":trace.as_ref().map(|trace| trace.report()),
            "presentation_probe":presentation.report(windows.single().expect("primary window")),
            "occlusion_probe":occlusion.as_ref().map(|probe| probe.report()),
            "occlusion_cycle":cycle.report(),
            "presentation_limitations":"Window mode records are requests, not GPU observations. With --skip-occluded, accepted architecture actions and the three settlement updates can complete while the window is suspended: those counts do not prove the corresponding scenes were presented. This probe prohibits screenshots and is never enabled by the normal viewer. --await-occlusion-cycle records notifications; desktop UI actions must be independently verified to claim a controlled minimize/restore or physical input test.",
            "layout_version":visualyze_layout::LAYOUT_VERSION,"quality_version":visualyze_layout::QUALITY_VERSION,
            "model_id":status.model_id,"revision":status.revision,"completed_steps":proof.records,
            "layout_job_timings":status.layout_timings.iter().map(|timing| timing_record(timing, proof.started)).collect::<Vec<_>>(),
            "initial_layout_jobs":status.layout_timings.iter().filter(|timing| timing.action_id.is_none() && timing.outcome == "accepted").map(|timing| timing_record(timing, proof.started)).collect::<Vec<_>>(),
            "all_update_intervals_ms":interval_summary(&proof.frame_deltas),
            "worker_update_intervals_ms_excluding_capture":interval_summary(&proof.worker_frame_deltas),
            "idle_update_intervals_ms_excluding_capture":interval_summary(&proof.idle_frame_deltas),
            "persistence":{"enabled":proof.persistence_directory.is_some(),"isolated_directory":proof.persistence_directory,
                "attempts":status.save_attempts,"failures":status.save_failures,"total_ms":status.save_total_ms,"max_ms":status.save_max_ms,
                "last_64_attempts_ms":status.save_samples_ms,"scope":"Synchronous main-thread state preparation and atomic save, including existing-state validation, serialization, file/directory fsync. Acceptance and frame intervals include this cost when enabled. The fresh temp directory is retained for inspection; no user state is read or written."},
            "notes":"Real native renderer. Original source records validated before app startup; OS bundle cache is warm. Acceptance is the first update observing the action ticket complete and loading false, before the three render-settlement frames. It includes action dispatch and worker publication, not screenshot work. Frame intervals are CPU update intervals, not GPU presentation latency. The bounded layout_job_timings records actual hit/miss/saved_exact/saved_repair decisions and matching action/revision IDs. Empty per-action layout_jobs means the action did not submit a layout job. Worker indexing, source lock wait, projection, restoration, solve and preparation residual are nonoverlapping; worker_total includes those stages. Publication wait, CPU acceptance, scene construction and save follow worker completion. These are CPU measurements and exclude deferred command application/GPU presentation. The cached step field is only an expectation; actual cache decisions are in layout_jobs. Cached geometry is compared exactly; all scenes are bounded to 250 representations."});
        let path = proof.output.join("native-report.json");
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        serde_json::to_writer_pretty(file, &report).unwrap();
        println!("Production native proof: {}", path.display());
        proof.index += 1;
        exit.write(AppExit::Success);
        return;
    }
    if proof.index > proof.steps.len() {
        return;
    }
    let index = proof.index;
    if proof.ticket.is_none() {
        proof.action_started = Instant::now();
        proof.accepted_ms = None;
        let action = if proof.steps[index].name == "select-relationship" {
            let route = relations
                .iter()
                .min_by_key(|route| &route.id)
                .expect("connected repository overview");
            proof.selected_route = Some((
                route.id.clone(),
                route.semantic_ids.clone(),
                route.points.clone(),
            ));
            ViewerAction::SelectRelation(route.semantic_ids[0].clone())
        } else {
            proof.steps[index].action.clone()
        };
        proof.ticket = Some(actions.submit(action).unwrap());
        return;
    }
    if !status.action_finished(proof.ticket.unwrap()) {
        return;
    }
    if proof.accepted_ms.is_none() {
        proof.accepted_ms = Some(proof.action_started.elapsed().as_secs_f64() * 1000.0);
    }
    // The render command queue must finish before comparing visible entities.
    proof.settled_frames += 1;
    if proof.settled_frames < 3 {
        return;
    }
    let step = &proof.steps[index];
    assert_eq!(
        status.root.as_deref(),
        Some(step.root.as_str()),
        "wrong inspection root at {}",
        step.name
    );
    assert_eq!(status.child_page, step.page, "wrong page at {}", step.name);
    assert_eq!(
        status.relation_page, step.relation_page,
        "wrong relationship page at {}",
        step.name
    );
    let isolated = step.name == "isolated-relationship";
    if isolated {
        assert_eq!(
            relations.iter().count(),
            1,
            "only the selected path is displayed"
        );
        let (id, members, points) = proof.selected_route.as_ref().unwrap();
        let route = relations.iter().next().unwrap();
        assert_eq!(&route.id, id);
        assert_eq!(&route.semantic_ids, members);
        assert_eq!(
            &route.points, points,
            "inspection must not reroute the accepted path"
        );
        assert_eq!(
            status.selected_relation.as_deref(),
            Some(members[0].as_str())
        );
        assert_eq!(&status.selected_relation_members, members);
        assert!(
            status.inspector_scroll_max > 0.0,
            "long original metadata must remain reachable through the bounded inspector"
        );
    } else {
        assert_eq!(objects.iter().count(), status.object_count);
        assert_eq!(relations.iter().count(), status.relation_count);
    }
    assert!(status.object_count <= 250);
    assert!(objects.iter().all(|(o, _)| !o.semantic_ids.is_empty()));
    assert!(
        relations
            .iter()
            .all(|r| r.points.len() >= 2 && !r.semantic_ids.is_empty())
    );
    if step.name == "repository" {
        // Suggested visual groups may summarize supplied packages. Require an
        // actual connected overview, not a fixed count of ungrouped meshes.
        assert!(status.object_count >= 3 && status.relation_count >= 1);
        assert!(
            !status.inspector_expanded,
            "initial view must leave the assembly unobscured"
        );
    }
    if step.name == "show-details" {
        assert!(status.inspector_expanded);
    }
    if step.name == "source-file" {
        assert!(
            status.has_next_relations,
            "external references require a resumable page"
        );
    }
    if step.name == "world-spawn" {
        assert!(objects.iter().any(
            |(o, _)| o.id == proof.function_id && o.semantic_ids == [proof.function_id.clone()]
        ));
    }
    if step.name == "search" {
        assert!(status.search_results.iter().any(|hit| hit.id == PACKAGE));
    }
    if step.root == proof.type_id {
        assert_eq!(status.total_children, 150);
    }
    if step.page > 0 {
        assert!(status.has_previous_page);
    }
    if step.view_key != "collapsed" {
        assert!(
            status.composition_frames >= 2,
            "expansion did not create canonical interior frames"
        );
        assert!(
            status.maximum_scale_shift > 0,
            "nested production view did not exercise fractional presentation"
        );
    }
    let key = format!(
        "{}:{}:{}:{}",
        step.root, step.page, step.relation_page, step.view_key
    );
    let current: BTreeMap<_, _> = objects
        .iter()
        .map(|(o, t)| (o.id.clone(), t.translation))
        .collect();
    if step.cached {
        assert_eq!(
            proof.positions.get(&key),
            Some(&current),
            "cached navigation moved geometry"
        );
    }
    let screenshot = step.screenshot;
    let name = step.name;
    let elapsed = proof.action_started.elapsed().as_secs_f64() * 1000.0;
    let record = serde_json::json!({"step":name,"step_index":index,"focus":status.root,"child_page":status.child_page,
        "relation_page":status.relation_page,
        "objects":objects.iter().count(),"routes":relations.iter().count(),
        "accepted_objects":status.object_count,"accepted_routes":status.relation_count,
        "composition_frames":status.composition_frames,"maximum_scale_shift":status.maximum_scale_shift,
        "selected_relation":status.selected_relation,"selected_relation_members":status.selected_relation_members,
        "inspector_scroll_max":status.inspector_scroll_max,
        "inspector_expanded":status.inspector_expanded,
        "save_attempts":status.save_attempts,"save_total_ms":status.save_total_ms,
        "original_routed_relations":relations.iter().map(|r|r.semantic_ids.len()).sum::<usize>(),
        "has_more_relation_pages":status.has_next_relations,"total_children":status.total_children,
        "acceptance_ms":proof.accepted_ms,
        "action_started_ms":proof.action_started.duration_since(proof.started).as_secs_f64()*1000.0,
        "layout_jobs":status.layout_timings.iter().filter(|timing| timing.action_id == proof.ticket && timing.revision == status.revision).map(|timing| timing_record(timing, proof.started)).collect::<Vec<_>>(),
        "elapsed_ms_including_3_settle_frames":elapsed,"cached":step.cached,"screenshot":screenshot});
    println!(
        "Native {}: {} objects, {} routes, {:.1} ms",
        name,
        objects.iter().count(),
        relations.iter().count(),
        elapsed
    );
    proof.records.push(record);
    if isolated {
        let baseline = &proof.positions[&key];
        assert!(
            current
                .iter()
                .all(|(id, position)| baseline.get(id) == Some(position)),
            "isolation moved an endpoint or containing frame"
        );
    } else {
        proof.positions.insert(key, current);
    }
    if screenshot {
        let path = proof.output.join(format!("{name}.png"));
        commands.spawn(Screenshot::primary_window()).observe(
            move |capture: On<ScreenshotCaptured>,
                  mut captures: ResMut<Captures>,
                  mut trace: Option<ResMut<frame_trace::FrameTrace>>| {
                let started = Instant::now();
                let image = capture.image.clone().try_into_dynamic().unwrap();
                let colored = image
                    .to_rgb8()
                    .pixels()
                    .filter(|p| {
                        let c = p.0;
                        *c.iter().max().unwrap() > 60
                            && c.iter().max().unwrap() - c.iter().min().unwrap() > 20
                    })
                    .count();
                assert!(
                    colored > 1500,
                    "production scene contains too little visible geometry"
                );
                image.save(&path).unwrap();
                captures.completed += 1;
                if let Some(trace) = trace.as_mut() {
                    trace.capture_cpu(started);
                }
            },
        );
        proof.awaiting_capture = true;
        if let Some(trace) = trace.as_mut() {
            trace.capture_pending(true);
        }
    } else {
        proof.ticket = None;
        proof.index += 1;
        proof.settled_frames = 0;
    }
}

fn interval_summary(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        sorted
            .get(((sorted.len().saturating_sub(1)) as f64 * p).round() as usize)
            .copied()
    };
    serde_json::json!({"samples":sorted.len(), "median":percentile(0.5),
        "p95":percentile(0.95),"p99":percentile(0.99),"max":sorted.last()})
}

// Values come from the matching runtime job, not inferred navigation history.
fn timing_record(timing: &LayoutTiming, origin: Instant) -> serde_json::Value {
    let offset = |time: Option<Instant>| {
        time.and_then(|time| time.checked_duration_since(origin))
            .map(|d| d.as_secs_f64() * 1000.0)
    };
    serde_json::json!({
        "job_id":timing.job_id,
        "action_id":timing.action_id,
        "revision":timing.revision,
        "outcome":timing.outcome,
        "cache_outcome":timing.cache_outcome,
        "indexing_ms":timing.indexing_ms,
        "snapshot_read_ms":timing.snapshot_read_ms,
        "source_lock_wait_ms":timing.source_lock_wait_ms,
        "projection_ms":timing.projection_ms,
        "projection_calls":timing.projection_calls,
        "projection_cache_hits":timing.projection_cache_hits,
        "queued_at_ms":offset(timing.queued_at),
        "worker_finished_at_ms":offset(timing.worker_finished_at),
        "observed_at_ms":offset(timing.observed_at),
        "restoration_ms":timing.restoration_ms,
        "solve_ms":timing.solve_ms,
        "routing_batches_reused":timing.routing_batches_reused,
        "routing_batches_computed":timing.routing_batches_computed,
        "preparation_ms":timing.preparation_ms,
        "worker_queue_ms":timing.worker_queue_ms,
        "worker_total_ms":timing.worker_total_ms,
        "publication_wait_ms":timing.publication_wait_ms,
        "acceptance_ms":timing.acceptance_ms,
        "scene_build_ms":timing.scene_build_ms,
        "save_ms":timing.save_ms,
        "total_ms":timing.total_ms,
        "partial_phase":timing.partial_phase,
        "partial_phase_ms":timing.partial_phase_ms,
    })
}
