//! Native GPU/navigation proof for generated synthetic scale-view bundles.
//! Usage: large_model_smoke BUNDLE NEW_OUTPUT_DIRECTORY
//! Run under `/usr/bin/time -l` to measure process peak RSS externally.
#[path = "support/scripted_input.rs"]
mod scripted_input;

use bevy::{
    app::AppExit,
    mesh::VertexAttributeValues,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::Instant,
};
use visualyze::{
    LayoutEngine, LayoutStatus, LayoutStorage, LayoutTiming, PlacedObject, PlacedRelation,
    ViewerAction, ViewerActions,
};
use visualyze_model::provider::{BundleRecord, Counts, Manifest, SemanticLens};

const FANOUT: usize = 32;
const CACHE_CAPACITY: usize = 16;
const SETTLE_FRAMES: usize = 3;
type Geometry = BTreeMap<String, serde_json::Value>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Visit {
    First,
    ResidentReturn,
    EvictionExpectedReturn,
    Search,
}
struct Step {
    name: String,
    action: ViewerAction,
    focus: usize,
    visit: Visit,
    screenshot: bool,
    search_hit: Option<usize>,
}
struct FrameSample {
    milliseconds: f64,
    worker: bool,
    capture: bool,
}
#[derive(Resource)]
struct Proof {
    output: PathBuf,
    source: PathBuf,
    manifest: Manifest,
    counts: Counts,
    preflight_ms: f64,
    started: Instant,
    previous_frame: Instant,
    previous_loading: bool,
    previous_capture: bool,
    samples: Vec<FrameSample>,
    initial_acceptance_ms: Option<f64>,
    initial_scene: Option<serde_json::Value>,
    initial_settled_frames: usize,
    awaiting_capture: Option<usize>,
    steps: Vec<Step>,
    index: usize,
    ticket: Option<u64>,
    action_started: Instant,
    acceptance_ms: Option<f64>,
    settled_frames: usize,
    records: Vec<serde_json::Value>,
    geometry: BTreeMap<usize, Geometry>,
    navigation: Vec<usize>,
    last_scene: Option<usize>,
    worker_preservation_samples: usize,
    routed_memberships: usize,
    finished: bool,
}
#[derive(Resource)]
struct Captures {
    started: Instant,
    records: Vec<serde_json::Value>,
}

fn id(index: usize) -> String {
    format!("entity:{index:08}")
}
fn first_leaf(count: usize) -> usize {
    (count - 2) / FANOUT + 1
}

/// Read only bounded header/footer records; the native worker performs the sole
/// complete index. The completion footer is independently checked by that index.
fn preflight(path: &Path) -> (Manifest, Counts) {
    let mut file = File::open(path).expect("open supplied synthetic bundle");
    let size = file.metadata().unwrap().len();
    assert!(size > 0);
    let mut header = Vec::new();
    (&mut file).take(65_536).read_to_end(&mut header).unwrap();
    let end = header
        .iter()
        .position(|byte| *byte == b'\n')
        .expect("bounded manifest line");
    let BundleRecord::Manifest(manifest) = serde_json::from_slice(&header[..end]).unwrap() else {
        panic!("first record must be a bundle manifest")
    };
    file.seek(SeekFrom::Start(size.saturating_sub(65_536)))
        .unwrap();
    let mut tail = Vec::new();
    file.read_to_end(&mut tail).unwrap();
    let last = tail
        .split(|byte| *byte == b'\n')
        .rfind(|line| !line.is_empty())
        .unwrap();
    let BundleRecord::Complete(counts) = serde_json::from_slice(last).unwrap() else {
        panic!("last record must declare complete source counts")
    };
    assert!([10_000, 100_000, 1_000_000].contains(&counts.entities));
    assert_eq!(
        manifest.model_id,
        format!("synthetic:scale-view:{}", counts.entities)
    );
    assert_eq!(manifest.root_id, id(0));
    assert_eq!(manifest.revision, 1);
    assert_eq!(counts.evidence, 0);
    let leaf = first_leaf(counts.entities);
    let peers = (counts.entities - leaf).div_ceil(8);
    let shared = counts.entities.saturating_sub(leaf + 64).div_ceil(64);
    assert_eq!(counts.relations, counts.entities - 1 + peers + shared);
    (manifest, counts)
}

fn steps(count: usize) -> Vec<Step> {
    let mut result = vec![];
    let mut add = |name: String, action, focus, visit, screenshot, search_hit| {
        result.push(Step {
            name,
            action,
            focus,
            visit,
            screenshot,
            search_hit,
        });
    };
    add(
        "search-level-1".into(),
        ViewerAction::Search("Synthetic object 1".into()),
        0,
        Visit::Search,
        false,
        Some(1),
    );
    add(
        "level-1".into(),
        ViewerAction::Enter(id(1)),
        1,
        Visit::First,
        true,
        None,
    );
    add(
        "level-2".into(),
        ViewerAction::Enter(id(33)),
        33,
        Visit::First,
        false,
        None,
    );
    add(
        "level-1-immediate-return".into(),
        ViewerAction::Back,
        1,
        Visit::ResidentReturn,
        false,
        None,
    );
    for focus in 2..=22 {
        add(
            format!("distinct-region-{focus:02}"),
            ViewerAction::Jump(id(focus)),
            focus,
            Visit::First,
            false,
            None,
        );
    }
    let leaf = first_leaf(count);
    for focus in leaf - 4..leaf {
        add(
            format!("deep-region-{focus}"),
            ViewerAction::Jump(id(focus)),
            focus,
            Visit::First,
            focus == leaf - 1,
            None,
        );
    }
    add(
        "root-after-cache-pressure".into(),
        ViewerAction::Jump(id(0)),
        0,
        Visit::EvictionExpectedReturn,
        false,
        None,
    );
    add(
        "level-1-after-cache-pressure".into(),
        ViewerAction::Jump(id(1)),
        1,
        Visit::EvictionExpectedReturn,
        false,
        None,
    );
    for round in 0..6 {
        add(
            format!("cached-deep-round-{round}"),
            ViewerAction::Jump(id(leaf - 1)),
            leaf - 1,
            Visit::ResidentReturn,
            false,
            None,
        );
        add(
            format!("cached-level-1-round-{round}"),
            ViewerAction::Back,
            1,
            Visit::ResidentReturn,
            false,
            None,
        );
    }
    add(
        "search-last-entity".into(),
        ViewerAction::Search(format!("Synthetic object {}", count - 1)),
        1,
        Visit::Search,
        false,
        Some(count - 1),
    );
    add(
        "last-leaf".into(),
        ViewerAction::Jump(id(count - 1)),
        count - 1,
        Visit::First,
        true,
        None,
    );
    add(
        "return-from-leaf".into(),
        ViewerAction::Back,
        1,
        Visit::ResidentReturn,
        false,
        None,
    );
    result
}

fn main() -> AppExit {
    let mut args = std::env::args_os().skip(1);
    let source = PathBuf::from(args.next().expect("bundle path"));
    let output = PathBuf::from(args.next().expect("new output directory"));
    assert!(
        args.next().is_none(),
        "usage: large_model_smoke BUNDLE NEW_OUTPUT_DIRECTORY"
    );
    std::fs::create_dir(&output).expect("output directory must be new");
    let preflight_start = Instant::now();
    let (manifest, counts) = preflight(&source);
    let preflight_ms = preflight_start.elapsed().as_secs_f64() * 1000.0;
    let started = Instant::now();
    let mut app = visualyze::app_with_bundle(source.clone(), LayoutEngine::Floorplan);
    scripted_input::install(&mut app);
    app.insert_resource(LayoutStorage(None))
        .insert_resource(Captures {
            started,
            records: vec![],
        })
        .insert_resource(Proof {
            output,
            source,
            manifest,
            counts,
            preflight_ms,
            started,
            previous_frame: started,
            previous_loading: false,
            previous_capture: false,
            samples: vec![],
            initial_acceptance_ms: None,
            initial_scene: None,
            initial_settled_frames: 0,
            awaiting_capture: None,
            steps: steps(counts.entities),
            index: 0,
            ticket: None,
            action_started: started,
            acceptance_ms: None,
            settled_frames: 0,
            records: vec![],
            geometry: BTreeMap::new(),
            navigation: vec![],
            last_scene: None,
            worker_preservation_samples: 0,
            routed_memberships: 0,
            finished: false,
        })
        .add_systems(Startup, |mut windows: Query<&mut Window>| {
            for mut window in &mut windows {
                window.title = "Visualyze - scale verification".into();
            }
        })
        .add_systems(PostUpdate, drive)
        .run()
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut proof: ResMut<Proof>,
    captures: Res<Captures>,
    status: Res<LayoutStatus>,
    mut actions: ResMut<ViewerActions>,
    objects: Query<(&PlacedObject, &Transform, &Children)>,
    parts: Query<(&Mesh3d, &Transform)>,
    meshes: Res<Assets<Mesh>>,
    relations: Query<&PlacedRelation>,
    mut exit: MessageWriter<AppExit>,
) {
    if proof.finished {
        return;
    }
    let now = Instant::now();
    let milliseconds = now.duration_since(proof.previous_frame).as_secs_f64() * 1000.0;
    proof.previous_frame = now;
    if proof.initial_acceptance_ms.is_some() {
        let worker = proof.previous_loading || status.loading;
        let capture = proof.previous_capture || proof.awaiting_capture.is_some();
        proof.samples.push(FrameSample {
            milliseconds,
            worker,
            capture,
        });
    }
    proof.previous_loading = status.loading;
    proof.previous_capture = proof.awaiting_capture.is_some();
    assert!(
        proof.started.elapsed().as_secs() < 180,
        "native scale timeout at step {}",
        proof.index
    );
    assert!(status.error.is_none(), "layout error: {:?}", status.error);
    assert!(
        status.action_error.is_none(),
        "action error: {:?}",
        status.action_error
    );
    if status.loading {
        if let Some(focus) = proof.last_scene {
            // Layout work must leave the last complete semantic scene visible.
            assert!(status.ready);
            assert_eq!(status.root.as_deref(), Some(id(focus).as_str()));
            assert_eq!(objects.iter().count(), status.object_count);
            assert_eq!(relations.iter().count(), status.relation_count);
            proof.worker_preservation_samples += 1;
        }
        return;
    }
    if !status.ready {
        return;
    }
    assert_eq!(
        status.model_id.as_deref(),
        Some(proof.manifest.model_id.as_str())
    );
    assert_eq!(status.revision, Some(proof.manifest.revision));
    assert_eq!(status.lens, Some(SemanticLens::Logical));
    if proof.initial_acceptance_ms.is_none() {
        proof.initial_acceptance_ms = Some(proof.started.elapsed().as_secs_f64() * 1000.0);
        return;
    }
    if let Some(expected) = proof.awaiting_capture {
        if captures.records.len() < expected {
            return;
        }
        proof.awaiting_capture = None;
    }
    if proof.initial_scene.is_none() {
        proof.initial_settled_frames += 1;
        if proof.initial_settled_frames < SETTLE_FRAMES {
            return;
        }
        check_scene(&status, &objects, &relations, proof.counts.entities, 0);
        let geometry = scene_geometry(&objects, &parts, &meshes, &relations);
        proof.initial_scene = Some(serde_json::json!({
            "focus":id(0), "objects":status.object_count, "routes":status.relation_count,
            "acceptance_ms":proof.initial_acceptance_ms,
            "layout_jobs":status.layout_timings.iter().filter(|timing| timing.action_id.is_none() && timing.revision == status.revision).map(timing_record).collect::<Vec<_>>(),
            "settled_scene_ms":proof.started.elapsed().as_secs_f64()*1000.0,
            "settlement_frames":SETTLE_FRAMES, "geometry_hash":geometry_hash(&geometry),
            "has_more_relationship_pages":status.has_next_relations,
        }));
        proof.geometry.insert(0, geometry);
        proof.navigation.push(0);
        proof.last_scene = Some(0);
        schedule_capture(&mut commands, &mut proof, &captures, "initial-root");
        return;
    }
    if proof.index == proof.steps.len() {
        write_report(&proof, &captures);
        proof.finished = true;
        exit.write(AppExit::Success);
        return;
    }
    let index = proof.index;
    if proof.ticket.is_none() {
        proof.action_started = Instant::now();
        proof.acceptance_ms = None;
        proof.settled_frames = 0;
        proof.ticket = Some(actions.submit(proof.steps[index].action.clone()).unwrap());
        return;
    }
    if !status.action_finished(proof.ticket.unwrap()) {
        return;
    }
    if proof.acceptance_ms.is_none() {
        proof.acceptance_ms = Some(proof.action_started.elapsed().as_secs_f64() * 1000.0);
        return;
    }
    proof.settled_frames += 1;
    if proof.settled_frames < SETTLE_FRAMES {
        return;
    }
    let step = &proof.steps[index];
    check_scene(
        &status,
        &objects,
        &relations,
        proof.counts.entities,
        step.focus,
    );
    if let Some(hit) = step.search_hit {
        assert!(
            status
                .search_results
                .iter()
                .any(|result| result.id == id(hit)),
            "exact source search failed"
        );
        assert!(status.search_results.len() <= 20);
    }
    let current = scene_geometry(&objects, &parts, &meshes, &relations);
    let old = proof.geometry.get(&step.focus);
    let distinct_since = proof
        .navigation
        .iter()
        .rposition(|focus| *focus == step.focus)
        .map(|position| {
            proof.navigation[position + 1..]
                .iter()
                .copied()
                .collect::<BTreeSet<_>>()
                .len()
        });
    match step.visit {
        Visit::First => assert!(old.is_none(), "first visit classification was wrong"),
        Visit::ResidentReturn => assert!(distinct_since.unwrap() < CACHE_CAPACITY),
        Visit::EvictionExpectedReturn => assert!(distinct_since.unwrap() >= CACHE_CAPACITY),
        Visit::Search => {}
    }
    if let Some(old) = old {
        assert_eq!(
            old, &current,
            "return changed object bounds, transforms, membership or connector geometry at {}",
            step.name
        );
    }
    let name = step.name.clone();
    let focus = step.focus;
    let screenshot = step.screenshot;
    let visit = step.visit;
    let routed = relations
        .iter()
        .map(|relation| relation.semantic_ids.len())
        .sum::<usize>();
    let record = serde_json::json!({
        "step":name, "focus":id(focus), "visit":format!("{visit:?}"),
        "objects":status.object_count, "routes":status.relation_count,
        "original_routed_relationships":routed, "total_children":status.total_children,
        "has_more_relationship_pages":status.has_next_relations,
        "acceptance_ms":proof.acceptance_ms,
        "layout_jobs":status.layout_timings.iter().filter(|timing| timing.action_id == proof.ticket && timing.revision == status.revision).map(timing_record).collect::<Vec<_>>(),
        "elapsed_ms_including_settlement":proof.action_started.elapsed().as_secs_f64()*1000.0,
        "settlement_frames":SETTLE_FRAMES, "geometry_hash":geometry_hash(&current),
        "exact_geometry_return_verified":old.is_some(), "distinct_focuses_since_previous":distinct_since,
        "screenshot":screenshot,
    });
    println!(
        "Scale native {}: {} objects, {} routes, {:.1} ms acceptance",
        name,
        status.object_count,
        status.relation_count,
        proof.acceptance_ms.unwrap()
    );
    proof.records.push(record);
    proof.routed_memberships += routed;
    proof.geometry.insert(focus, current);
    if visit != Visit::Search {
        proof.navigation.push(focus);
    }
    proof.last_scene = Some(focus);
    proof.ticket = None;
    proof.index += 1;
    if screenshot {
        schedule_capture(&mut commands, &mut proof, &captures, &name);
    }
}

fn check_scene(
    status: &LayoutStatus,
    objects: &Query<(&PlacedObject, &Transform, &Children)>,
    relations: &Query<&PlacedRelation>,
    count: usize,
    focus: usize,
) {
    assert_eq!(status.root.as_deref(), Some(id(focus).as_str()));
    assert_eq!(status.child_page, 0);
    assert_eq!(status.relation_page, 0);
    assert_eq!(objects.iter().count(), status.object_count);
    assert_eq!(relations.iter().count(), status.relation_count);
    assert!(status.object_count > 0 && status.object_count <= 250);
    let child_start = focus * FANOUT + 1;
    assert_eq!(
        status.total_children,
        count.saturating_sub(child_start).min(FANOUT)
    );
    for (object, transform, _) in objects.iter() {
        assert!(!object.semantic_ids.is_empty());
        assert!(transform.translation.is_finite());
        for semantic in &object.semantic_ids {
            let index = semantic_index(semantic, count);
            assert_eq!(
                object.kind,
                if index < first_leaf(count) {
                    "scope"
                } else {
                    "service"
                }
            );
        }
    }
    for relation in relations.iter() {
        assert!(relation.points.len() >= 2 && !relation.semantic_ids.is_empty());
        assert!(relation.points.iter().all(|point| point.is_finite()));
        semantic_index(&relation.source, count);
        semantic_index(&relation.target, count);
        for semantic in &relation.semantic_ids {
            check_relation_id(semantic, count);
        }
    }
}
fn semantic_index(semantic: &str, count: usize) -> usize {
    let number: usize = semantic.strip_prefix("entity:").unwrap().parse().unwrap();
    assert!(number < count);
    assert_eq!(semantic, id(number));
    number
}
fn check_relation_id(semantic: &str, count: usize) {
    let fields: Vec<_> = semantic.split(':').collect();
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0], "relation");
    let source: usize = fields[2].parse().unwrap();
    let target: usize = fields[3].parse().unwrap();
    assert!(source < count && target < count);
    let leaf = first_leaf(count);
    match fields[1] {
        "peer" => {
            assert!(source >= leaf && (source - leaf).is_multiple_of(8));
            assert_eq!(target, if source + 1 < count { source + 1 } else { leaf });
        }
        "shared" => {
            assert!(source >= leaf + 64 && (source - leaf - 64).is_multiple_of(64));
            assert_eq!(target, leaf);
        }
        _ => panic!("structural or invented dependency drawn as a connector"),
    }
}

fn scene_geometry(
    objects: &Query<(&PlacedObject, &Transform, &Children)>,
    parts: &Query<(&Mesh3d, &Transform)>,
    meshes: &Assets<Mesh>,
    relations: &Query<&PlacedRelation>,
) -> Geometry {
    let mut result = BTreeMap::new();
    for (object, transform, children) in objects.iter() {
        let mut boxes = vec![];
        for child in children.iter() {
            let (mesh, local) = parts.get(child).expect("rectangular object part");
            let mesh = meshes.get(&mesh.0).expect("resident mesh");
            let Some(VertexAttributeValues::Float32x3(positions)) =
                mesh.attribute(Mesh::ATTRIBUTE_POSITION)
            else {
                panic!("rectangular mesh positions")
            };
            let mut lo = Vec3::splat(f32::INFINITY);
            let mut hi = Vec3::splat(f32::NEG_INFINITY);
            for point in positions {
                let point = Vec3::from_array(*point);
                lo = lo.min(point);
                hi = hi.max(point);
            }
            assert!(lo.is_finite() && hi.is_finite());
            boxes.push(serde_json::json!({"min_bits":bits(lo), "max_bits":bits(hi), "transform":transform_bits(local)}));
        }
        assert!(!boxes.is_empty());
        boxes.sort_by_key(serde_json::Value::to_string);
        result.insert(
            format!("object:{}", object.id),
            serde_json::json!({
                "semantic_ids":object.semantic_ids, "kind":object.kind,
                "transform":transform_bits(transform), "mesh_bounds":boxes,
            }),
        );
    }
    for relation in relations.iter() {
        result.insert(format!("relation:{}", relation.id), serde_json::json!({
            "source":relation.source, "target":relation.target, "semantic_ids":relation.semantic_ids,
            "kind":relation.kind, "points_bits":relation.points.iter().copied().map(bits).collect::<Vec<_>>(),
        }));
    }
    result
}
fn bits(vector: Vec3) -> [u32; 3] {
    vector.to_array().map(f32::to_bits)
}
fn transform_bits(transform: &Transform) -> serde_json::Value {
    serde_json::json!({"translation":bits(transform.translation), "scale":bits(transform.scale), "rotation":transform.rotation.to_array().map(f32::to_bits)})
}
fn geometry_hash(geometry: &Geometry) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in serde_json::to_vec(geometry).unwrap() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn schedule_capture(commands: &mut Commands, proof: &mut Proof, captures: &Captures, name: &str) {
    let path = proof.output.join(format!("{name}.png"));
    let name = name.to_owned();
    commands.spawn(Screenshot::primary_window()).observe(move |capture: On<ScreenshotCaptured>, mut captures: ResMut<Captures>| {
        let image = capture.image.clone().try_into_dynamic().unwrap();
        let colored = image.to_rgb8().pixels().filter(|pixel| {
            let c = pixel.0;
            *c.iter().max().unwrap() > 60 && c.iter().max().unwrap() - c.iter().min().unwrap() > 20
        }).count();
        assert!(colored > 1500, "native frame lacks visible colored geometry");
        image.save(&path).unwrap();
        let milliseconds = captures.started.elapsed().as_secs_f64() * 1000.0;
        captures.records.push(serde_json::json!({"name":name,"capture_completed_ms":milliseconds,"colored_pixels":colored}));
    });
    proof.awaiting_capture = Some(captures.records.len() + 1);
    // The scheduling frame touches screenshot work even though capture was not
    // pending when this update's interval was first sampled.
    if let Some(sample) = proof.samples.last_mut() {
        sample.capture = true;
    }
}

fn write_report(proof: &Proof, captures: &Captures) {
    assert!(proof.geometry.len() > 20);
    assert!(proof.worker_preservation_samples > 0);
    assert!(proof.routed_memberships > 0);
    assert_eq!(captures.records.len(), 4);
    let worker: Vec<_> = proof
        .samples
        .iter()
        .filter(|sample| sample.worker && !sample.capture)
        .map(|sample| sample.milliseconds)
        .collect();
    let idle: Vec<_> = proof
        .samples
        .iter()
        .filter(|sample| !sample.worker && !sample.capture)
        .map(|sample| sample.milliseconds)
        .collect();
    let acceptance = |visit: Visit| {
        proof
            .records
            .iter()
            .filter(|record| record["visit"] == format!("{visit:?}"))
            .filter_map(|record| record["acceptance_ms"].as_f64())
            .collect::<Vec<_>>()
    };
    let cache_acceptance = |outcome: &str| {
        proof
            .records
            .iter()
            .filter(|record| {
                record["layout_jobs"].as_array().is_some_and(|jobs| {
                    jobs.iter()
                        .any(|job| job["cache_outcome"] == outcome && job["outcome"] == "accepted")
                })
            })
            .filter_map(|record| record["acceptance_ms"].as_f64())
            .collect::<Vec<_>>()
    };
    let report = serde_json::json!({
        "input_mode":scripted_input::INPUT_MODE,
        "source":proof.source, "model_id":proof.manifest.model_id, "revision":proof.manifest.revision,
        "layout_version":visualyze_layout::LAYOUT_VERSION,"quality_version":visualyze_layout::QUALITY_VERSION,
        "source_counts":proof.counts,"process_id":std::process::id(), "preflight_metadata_ms":proof.preflight_ms,
        "initial_acceptance_ms":proof.initial_acceptance_ms,
        "initial_capture_completed_ms":captures.records[0]["capture_completed_ms"],
        "initial_scene":proof.initial_scene,
        "total_ms":proof.started.elapsed().as_secs_f64()*1000.0,
        "distinct_focuses_validated":proof.geometry.len(),"actual_layout_cache_capacity":CACHE_CAPACITY,
        "worker_samples_preserving_accepted_scene":proof.worker_preservation_samples,
        "first_visit_acceptance_ms":interval_summary(&acceptance(Visit::First)),
        "resident_return_acceptance_ms":interval_summary(&acceptance(Visit::ResidentReturn)),
        "eviction_expected_return_acceptance_ms":interval_summary(&acceptance(Visit::EvictionExpectedReturn)),
        "actual_cache_hit_acceptance_ms":interval_summary(&cache_acceptance("hit")),
        "actual_cache_miss_acceptance_ms":interval_summary(&cache_acceptance("miss")),
        "worker_update_intervals_ms_excluding_capture_edges":interval_summary(&worker),
        "idle_update_intervals_ms_excluding_capture_edges":interval_summary(&idle),
        "capture_edge_intervals_excluded":proof.samples.iter().filter(|sample| sample.capture).count(),
        "steps":proof.records,"captures":captures.records,
        "peak_rss_measurement":"External /usr/bin/time -l maximum resident set size; join by process_id/run output. No resident-memory estimate is substituted.",
        "notes":[
            "Synthetic sparse 32-way ownership workload with declared peer/shared-leaf dependencies. This is not production complexity or human comprehension evidence.",
            "Preflight reads only bounded manifest/footer bytes and checks generator counts; the real native worker performs the sole full index. No provider index is pre-opened. OS filesystem cache temperature is uncontrolled and must be reported by the runner.",
            "Initial acceptance starts before app/plugin construction and includes GPU startup plus worker indexing/projection/validation. First screenshot completion is reported separately. Per-action acceptance is observed after ticket completion with loading false, before three subsequent settlement frames or screenshot work.",
            "CPU update intervals include benchmark instrumentation and are not GPU presentation latency. Intervals touching screenshot scheduling/pending/completion are excluded from worker/idle summaries.",
            "Visit labels describe expected residency from the runtime capacity of16 and navigation history. Each layout_jobs record now reports the actual runtime cache decision and matching action/revision identity; use those fields for cache-hit measurements. Worker stages are nonoverlapping: indexing, snapshot read, source lock wait, projection, restoration, solve and preparation residual; worker_total includes those stages. Publication wait, CPU acceptance, scene construction and save follow worker completion. Deferred command application and GPU presentation are not timed. Exact rendered mesh bounds/transforms, IDs, and connector point bits are compared on all returns, including after pressure exceeds16 distinct focuses.",
            "Every accepted scene is bounded to250 object representations. Partial relationship pages remain explicitly reported; the test does not claim every model relationship is simultaneously resident. Original rendered membership IDs are checked against the generator's supplied dependency definitions."
        ]
    });
    let path = proof.output.join("native-report.json");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    serde_json::to_writer_pretty(file, &report).unwrap();
    println!("Synthetic scale native proof: {}", path.display());
}
fn interval_summary(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |p: f64| {
        sorted
            .get((sorted.len().saturating_sub(1) as f64 * p).round() as usize)
            .copied()
    };
    serde_json::json!({"samples":sorted.len(),"median":percentile(0.5),"p95":percentile(0.95),"p99":percentile(0.99),"max":sorted.last()})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_plan_distinguishes_resident_and_eviction_expected_returns() {
        for count in [10_000, 100_000, 1_000_000] {
            let mut history = vec![0];
            let mut seen = BTreeSet::from([0]);
            for step in steps(count) {
                let intervening =
                    history
                        .iter()
                        .rposition(|focus| *focus == step.focus)
                        .map(|index| {
                            history[index + 1..]
                                .iter()
                                .copied()
                                .collect::<BTreeSet<_>>()
                                .len()
                        });
                match step.visit {
                    Visit::First => {
                        assert!(seen.insert(step.focus));
                    }
                    Visit::ResidentReturn => assert!(intervening.unwrap() < CACHE_CAPACITY),
                    Visit::EvictionExpectedReturn => {
                        assert!(intervening.unwrap() >= CACHE_CAPACITY)
                    }
                    Visit::Search => {}
                }
                if step.visit != Visit::Search {
                    history.push(step.focus);
                }
            }
            assert!(seen.len() > 20);
            assert_eq!(
                steps(count).iter().filter(|step| step.screenshot).count(),
                3
            );
        }
    }

    #[test]
    fn supplied_synthetic_dependency_memberships_are_checked_at_each_scale() {
        for count in [10_000, 100_000, 1_000_000] {
            let leaf = first_leaf(count);
            check_relation_id(&format!("relation:peer:{leaf:08}:{:08}", leaf + 1), count);
            check_relation_id(
                &format!("relation:shared:{:08}:{leaf:08}", leaf + 64),
                count,
            );
            assert_eq!(semantic_index(&id(count - 1), count), count - 1);
        }
    }
}

// Values come from the matching runtime job, not inferred navigation history.
fn timing_record(timing: &LayoutTiming) -> serde_json::Value {
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
        "restoration_ms":timing.restoration_ms,
        "solve_ms":timing.solve_ms,
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
