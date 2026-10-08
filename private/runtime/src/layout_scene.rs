//! Experimental, cancellable layout viewer. The accepted scene remains visible
//! while another view is solved; only fully validated solutions are published.
use crate::{
    camera::{FIELD_OF_VIEW, OrbitCamera},
    connections,
    layout_input::{DesktopProjection, LayoutInput, SourceTimings},
    native_layout::{ComposedConstraints, LayoutData, NativeLayout},
    primitives,
    style::{self, GlassMaterial},
};
use bevy::{
    camera::CameraUpdateSystems,
    input::{
        ButtonState,
        keyboard::KeyboardInput,
        mouse::{MouseScrollUnit, MouseWheel},
    },
    prelude::*,
    window::PrimaryWindow,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use visualyze_layout::{
    self as layout, Algorithm, LayoutRequest, LayoutSolution, Point, QUALITY_VERSION,
    QualityProfile,
    persistence::{DISMISSAL_ENCODING_VERSION, DismissedProposal},
    projection::{NodeKind, Projection as ViewProjection, ViewState},
};
use visualyze_model::architecture::{Basis, EntityKind};
use visualyze_model::provider::SemanticLens;

#[cfg(test)]
#[path = "../examples/support/scripted_input.rs"]
mod scripted_proof_input;

#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LayoutEngine {
    #[default]
    Legacy,
    Floorplan,
    Composed,
    Layered,
    Constrained,
    Treemap,
}
impl LayoutEngine {
    fn algorithm(self) -> Algorithm {
        match self {
            Self::Layered => Algorithm::Layered,
            Self::Constrained => Algorithm::Constrained,
            Self::Treemap => Algorithm::Treemap,
            _ => Algorithm::Floorplan,
        }
    }
}
use crate::transition::{Choreography, Held, SceneTransition, Track};

/// Semantic identity remains independent of the geometry/color vocabulary.
#[derive(Component, Debug)]
pub struct PlacedObject {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub kind: String,
    pub basis: Basis,
    /// Half extents of the drawn box, for framing.
    pub half: Vec3,
}

/// A displayed object or connector drawn faded because a staged scene puts it
/// in context rather than in focus.
#[derive(Component, Debug)]
pub struct Dimmed;

/// Which supplied IDs a staged scene is about. Everything else is context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spotlight {
    pub lit: BTreeSet<String>,
    pub relations: BTreeSet<String>,
    pub context: SpotContext,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotContext {
    Dim,
    Hide,
    Show,
}

/// A view an agent asks for: where to stand, what to open, what to light.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StagedScene {
    /// None shows the system root.
    pub root: Option<String>,
    pub expand: Vec<String>,
    pub spotlight: Option<Spotlight>,
    pub explode: bool,
    /// Lay the scene out in a temporary flattened teaching view (ADR 0010) instead of
    /// the published model.
    pub(crate) teaching: Option<TeachingStage>,
    /// How the scene replaces the one on screen; None cuts.
    pub(crate) transition: Option<Choreography>,
}

/// The temporary layout source for one teaching view. It shares the published model's
/// ID and revision and is never saved; equality is identity of the derived source.
#[derive(Clone)]
pub(crate) struct TeachingStage {
    pub input: LayoutInput,
    pub policy: layout::projection::LayoutPolicy,
}
impl PartialEq for TeachingStage {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.input.0, &other.input.0) && self.policy == other.policy
    }
}
impl Eq for TeachingStage {}
impl std::fmt::Debug for TeachingStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TeachingStage({:?})", self.policy)
    }
}

/// What a teaching view replaced, restored when the explanation leaves it.
struct TeachingReturn {
    input: LayoutInput,
    view: ViewState,
    constraints: ConstraintRegistry,
}

/// A validated model supplied by a local agent, replacing the current one.
#[derive(Clone)]
pub struct Published(pub(crate) LayoutInput, pub(crate) String);
impl Published {
    pub(crate) fn new(
        model: visualyze_model::architecture::Architecture,
        path: Option<std::path::PathBuf>,
    ) -> Self {
        let id = model.snapshot().model_id.clone();
        Self(
            match path {
                Some(path) => LayoutInput::snapshot_file(model, path),
                None => LayoutInput::snapshot(model),
            },
            id,
        )
    }
}
impl std::fmt::Debug for Published {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Published({})", self.1)
    }
}
#[derive(Component, Debug)]
pub struct PlacedRelation {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub kind: String,
    pub source: String,
    pub target: String,
    pub basis: Basis,
    pub points: Vec<Vec3>,
}
/// Private local view state. None disables persistence for deterministic proofs.
#[derive(Resource, Clone)]
pub struct LayoutStorage(pub Option<std::path::PathBuf>);
impl Default for LayoutStorage {
    fn default() -> Self {
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|h| {
                std::path::PathBuf::from(h).join("Library/Application Support/Visualyze/layouts")
            })
        } else {
            None
        };
        Self(base)
    }
}
fn engine_projection(mut p: DesktopProjection, engine: LayoutEngine) -> DesktopProjection {
    p.graph
        .key
        .push_str(&format!("|engine:{engine:?}|quality:{QUALITY_VERSION}"));
    p
}
fn state_identity(model_id: &str, engine: LayoutEngine) -> String {
    quality_state_identity(model_id, engine, QUALITY_VERSION)
}
fn quality_state_identity(model_id: &str, engine: LayoutEngine, quality: u32) -> String {
    format!(
        "{model_id}|{engine:?}|layout:{}|quality:{quality}",
        layout::LAYOUT_VERSION
    )
}
// An explicit migration list bounds filesystem probes; never scan app data or
// infer identities from filenames. A present current file always takes priority.
const PREVIOUS_QUALITY_VERSIONS: &[u32] = &[4, 3, 2, 1];
fn saved_state_file(
    base: &std::path::Path,
    model_id: &str,
    engine: LayoutEngine,
) -> Result<Option<std::path::PathBuf>, layout::LayoutError> {
    let identities = std::iter::once(state_identity(model_id, engine))
        .chain(
            PREVIOUS_QUALITY_VERSIONS
                .iter()
                .map(|quality| quality_state_identity(model_id, engine, *quality)),
        )
        .chain([
            format!("{model_id}|{engine:?}|layout:{}", layout::LAYOUT_VERSION),
            format!("{model_id}|{engine:?}"),
        ]);
    for identity in identities {
        let path = saved_path(base, &identity);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Ok(Some(path)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(layout::LayoutError::Invalid(format!(
                    "Saved state preserved at {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(None)
}
/// Change only a recognized quality suffix. Different projection membership,
/// pin coordinates, policy, lens, engine or model still fails validation.
fn migrate_quality_key(solution: &mut LayoutSolution, current: &str) {
    if solution.projection_key == current {
        return;
    }
    let suffix = format!("|quality:{QUALITY_VERSION}");
    let Some(base) = current.strip_suffix(&suffix) else {
        return;
    };
    if solution.projection_key == base
        || PREVIOUS_QUALITY_VERSIONS
            .iter()
            .any(|quality| solution.projection_key == format!("{base}|quality:{quality}"))
    {
        solution.projection_key = current.into();
    }
}
/// A changed page may invalidate membership without changing coordinates. Do
/// not infer a frame from an arbitrary projection-key substring: use the saved
/// view's effective lens and the unique parentless box. Legacy state without a
/// lens can prove its frame only through an otherwise identical projection.
fn saved_repair_matches_frame(
    view: &ViewState,
    accepted: &LayoutSolution,
    current: &DesktopProjection,
) -> bool {
    let mut roots = accepted
        .boxes
        .values()
        .filter(|object| object.parent.is_none());
    accepted.model_id == current.model_id
        && accepted.version == layout::LAYOUT_VERSION
        && accepted.revision <= current.revision
        && roots.next().is_some_and(|root| root.id == current.root)
        && roots.next().is_none()
        && view
            .focus
            .as_ref()
            .is_none_or(|focus| focus == &current.root)
        && view.frame_lens.or(view.lens).map_or_else(
            || accepted.projection_key == current.key,
            |lens| lens == current.lens,
        )
}
fn saved_path(base: &std::path::Path, model_id: &str) -> std::path::PathBuf {
    let hash = model_id.bytes().fold(0xcbf29ce484222325u64, |hash, b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    base.join(format!("{hash:016x}.json"))
}
// Composed files never share a path with the unchanged flat-state schema.
// No fallback probe/migration treats an old flat arrangement as canonical data.
fn composed_state_path(base: &std::path::Path, model_id: &str) -> std::path::PathBuf {
    // Versions belong in the checked header: a future reader must find and
    // preserve incompatible constraints instead of silently starting afresh.
    saved_path(base, &format!("{model_id}|Composed|private-state"))
}
/// CPU timings for one actual layout job, never inferred from navigation history.
/// Worker stages are nonoverlapping; publication wait follows worker completion.
/// Acceptance, scene construction and save measure CPU work, not GPU presentation.
#[derive(Debug, Clone, Default)]
pub struct LayoutTiming {
    pub job_id: u64,
    pub action_id: Option<u64>,
    pub revision: Option<u64>,
    pub outcome: &'static str,
    pub cache_outcome: &'static str,
    pub indexing_ms: f64,
    pub snapshot_read_ms: f64,
    pub source_lock_wait_ms: f64,
    pub projection_ms: f64,
    pub projection_calls: u32,
    pub projection_cache_hits: u32,
    pub restoration_ms: f64,
    pub solve_ms: f64,
    /// Composed routing work for this job only; whole-view hits do no routing.
    pub routing_batches_reused: usize,
    pub routing_batches_computed: usize,
    pub preparation_ms: f64,
    pub worker_queue_ms: f64,
    pub worker_total_ms: f64,
    pub publication_wait_ms: f64,
    pub acceptance_ms: f64,
    pub scene_build_ms: f64,
    pub save_ms: f64,
    pub total_ms: f64,
    /// Cancellation snapshots have an unfinished stage, reported separately.
    pub partial_phase: Option<&'static str>,
    pub partial_phase_ms: f64,
    /// Process-local monotonic instants for opt-in native phase correlation.
    /// They are never serialized into saved model/view state.
    pub queued_at: Option<Instant>,
    pub worker_finished_at: Option<Instant>,
    pub observed_at: Option<Instant>,
}

struct JobTiming {
    record: LayoutTiming,
    queued: Instant,
    started: Option<Instant>,
    finished: Option<Instant>,
    active: Option<(&'static str, Instant)>,
}
impl Default for JobTiming {
    fn default() -> Self {
        Self {
            record: LayoutTiming::default(),
            queued: Instant::now(),
            started: None,
            finished: None,
            active: None,
        }
    }
}
fn timed_stage<T>(timing: &Mutex<JobTiming>, stage: &'static str, work: impl FnOnce() -> T) -> T {
    let started = Instant::now();
    timing.lock().unwrap().active = Some((stage, started));
    let result = work();
    let mut timing = timing.lock().unwrap();
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    match stage {
        "projection" => timing.record.projection_ms += elapsed,
        "restoration" => timing.record.restoration_ms += elapsed,
        "solve" => timing.record.solve_ms += elapsed,
        _ => (), // Source sub-stages are measured by the input adapter.
    }
    timing.active = None;
    result
}
fn timed_projection(
    timing: &Mutex<JobTiming>,
    input: &LayoutInput,
    view: &ViewState,
    cancel: &AtomicBool,
) -> Result<DesktopProjection, layout::LayoutError> {
    let started = Instant::now();
    timing.lock().unwrap().active = Some(("projection_or_source_wait", started));
    let mut source = SourceTimings::default();
    let result = input.project_timed(view, cancel, &mut source);
    let mut timing = timing.lock().unwrap();
    let source_ms = source.indexing_ms + source.snapshot_read_ms + source.lock_wait_ms;
    timing.record.indexing_ms += source.indexing_ms;
    timing.record.snapshot_read_ms += source.snapshot_read_ms;
    timing.record.source_lock_wait_ms += source.lock_wait_ms;
    timing.record.projection_ms += (started.elapsed().as_secs_f64() * 1000.0 - source_ms).max(0.0);
    timing.record.projection_calls += 1;
    timing.record.projection_cache_hits += u32::from(source.projection_cache_hit);
    timing.active = None;
    result
}
fn timing_snapshot(timing: &JobTiming, outcome: &'static str) -> LayoutTiming {
    timing_snapshot_at(timing, outcome, Instant::now())
}
fn timing_snapshot_at(timing: &JobTiming, outcome: &'static str, now: Instant) -> LayoutTiming {
    let mut record = timing.record.clone();
    record.outcome = outcome;
    record.queued_at = Some(timing.queued);
    record.worker_finished_at = timing.finished;
    record.observed_at = Some(now);
    record.total_ms = now.duration_since(timing.queued).as_secs_f64() * 1000.0;
    if let Some(started) = timing.started {
        record.worker_queue_ms = started.duration_since(timing.queued).as_secs_f64() * 1000.0;
        record.worker_total_ms = timing
            .finished
            .unwrap_or(now)
            .duration_since(started)
            .as_secs_f64()
            * 1000.0;
    }
    if let Some(finished) = timing.finished {
        record.publication_wait_ms = now.duration_since(finished).as_secs_f64() * 1000.0;
    }
    if let Some((phase, started)) = timing.active {
        record.partial_phase = Some(phase);
        record.partial_phase_ms = now.duration_since(started).as_secs_f64() * 1000.0;
    }
    let measured = record.indexing_ms
        + record.snapshot_read_ms
        + record.source_lock_wait_ms
        + record.projection_ms
        + record.restoration_ms
        + record.solve_ms
        + record.partial_phase_ms;
    record.preparation_ms = (record.worker_total_ms - measured).max(0.0);
    record
}
fn record_timing(status: &mut LayoutStatus, timing: LayoutTiming) {
    if timing.job_id == 0
        || status
            .layout_timings
            .iter()
            .any(|old| old.job_id == timing.job_id)
    {
        return;
    }
    status.layout_timings.push_back(timing);
    while status.layout_timings.len() > 64 {
        status.layout_timings.pop_front();
    }
}
/// Exposed for the native smoke check, without making layout types public model data.
#[derive(Resource, Debug, Default)]
pub struct LayoutStatus {
    /// Most recent 64 accepted, failed, cancelled or proposal layout jobs.
    pub layout_timings: VecDeque<LayoutTiming>,
    pub ready: bool,
    pub error: Option<String>,
    pub object_count: usize,
    pub relation_count: usize,
    /// Visual noise of the focused region of the accepted projection (ADR 0007):
    /// what the focused view shows at its own level, not the interiors of boxes
    /// expanded in it. Above `layout::noise::NOISE_THRESHOLD` the view is dense
    /// but still allowed.
    pub noise: f64,
    /// The densest expanded interior of the accepted projection, by region id
    /// and noise, when there is one. Each interior is scored on its own.
    pub densest_interior: Option<(String, f64)>,
    pub composition_frames: usize,
    pub maximum_scale_shift: u8,
    pub model_id: Option<String>,
    pub revision: Option<u64>,
    pub focus: Option<String>,
    pub root: Option<String>,
    pub lens: Option<SemanticLens>,
    pub loading: bool,
    /// Optional refinement runs after publication and never delays navigation.
    pub reorganizing: bool,
    pub reorganization_available: bool,
    pub selected: Option<String>,
    pub selected_relation: Option<String>,
    /// Original memberships of the displayed route containing the selected claim.
    pub selected_relation_members: Vec<String>,
    pub inspector_expanded: bool,
    pub inspector_scroll: f32,
    pub inspector_scroll_max: f32,
    pub inspector_hovered: bool,
    /// Main-thread state preparation + atomic save, including validation/fsync.
    pub save_attempts: u64,
    pub save_failures: u64,
    pub save_total_ms: f64,
    pub save_max_ms: f64,
    /// Most recent 64 attempts; diagnostics must remain bounded during long use.
    pub save_samples_ms: VecDeque<f64>,
    pub breadcrumbs: Vec<String>,
    pub child_page: usize,
    pub total_children: usize,
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub last_action_id: u64,
    pub completed_action_id: u64,
    /// Inclusive completed ranges beyond the frontier, bounded by queue holes.
    pub completed_interactions: BTreeMap<u64, u64>,
    pub action_error: Option<String>,
    pub search_results: Vec<ViewerSearchResult>,
    pub inspected_relation_ids: Vec<String>,
    pub relation_page: usize,
    pub has_next_relations: bool,
    pub has_previous_relations: bool,
    pub marked_group_members: Vec<String>,
    pub local_group_ids: Vec<String>,
}
impl LayoutStatus {
    pub fn action_finished(&self, id: u64) -> bool {
        id != 0
            && (id <= self.completed_action_id
                || self
                    .completed_interactions
                    .range(..=id)
                    .next_back()
                    .is_some_and(|(_, end)| id <= *end))
    }
}

/// Commands use supplied semantic identities and are processed in FIFO order.
/// A proof or alternate input surface can submit actions without emulating keys.
#[derive(Debug, Clone)]
pub enum ViewerAction {
    Enter(String),
    Jump(String),
    Back,
    Expand(String),
    Collapse(String),
    Select(String),
    SelectRelation(String),
    NextRelation,
    PreviousRelation,
    ClearRelationSelection,
    /// Expand both endpoints of the selected path so its member relations
    /// can be drawn as separate lines (ADR 0007 section 4).
    ExpandPathEndpoints,
    NextPage,
    PreviousPage,
    Search(String),
    Reload,
    /// Replace the model with an agent-supplied one, which may be a different model.
    Publish(Published),
    /// Show a scene an agent asked for, replacing navigation and emphasis.
    Stage(Box<StagedScene>),
    /// Return from a teaching view to the published model, if one is showing.
    LeaveTeaching,
    NextClaims,
    PreviousClaims,
    NextRelations,
    PreviousRelations,
    Cancel,
    SetPolicy(layout::projection::LayoutPolicy),
    SetLens(Option<SemanticLens>),
    SetSemanticZoom(bool),
    UpdateZoom {
        id: String,
        projected_pixels: f64,
    },
    Pin(String),
    Unpin(String),
    OpenPinOrigin(String),
    SetLayer {
        id: String,
        band: i32,
    },
    SetGroup {
        id: String,
        parent: String,
        members: Vec<String>,
    },
    RemoveGroup(String),
    ToggleGroupMember(String),
    ClearGroupMarks,
    GroupMarked,
    ToggleExplode,
    ToggleIsolate,
    ToggleDetails,
    Reorganize,
    Preview,
    Apply,
    Dismiss,
    Undo,
}

#[derive(Resource, Default)]
pub struct ViewerActions {
    next_id: u64,
    queue: VecDeque<(u64, ViewerAction)>,
}

impl ViewerActions {
    pub fn submit(&mut self, action: ViewerAction) -> Result<u64, &'static str> {
        if self.queue.len() >= 64
            && !matches!(
                action,
                ViewerAction::Cancel | ViewerAction::Reload | ViewerAction::Publish(_)
            )
        {
            return Err("viewer action queue is full");
        }
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or("viewer action sequence exhausted")?;
        let id = self.next_id;
        if matches!(
            action,
            ViewerAction::Cancel | ViewerAction::Reload | ViewerAction::Publish(_)
        ) {
            self.queue.clear();
            self.queue.push_front((id, action));
        } else {
            self.queue.push_back((id, action));
        }
        Ok(id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewerSearchResult {
    pub id: String,
    pub label: String,
    pub kind: String,
}
#[derive(Component)]
struct LayoutOwned;
/// The ground grid, which a transition replaces at once.
#[derive(Component)]
struct Ground;
#[derive(Component)]
struct LayoutText;
#[derive(Component)]
struct Label {
    point: Vec3,
    id: String,
}
/// A caption pill plus the stem that points down at its object.
const LABEL_SIZE: Vec2 = Vec2::new(208.0, 44.0);
const LABEL_STEM: f32 = 8.0;
/// Group marks tint objects amber, in both solid and glass form.
const MARK_COLOR: Color = Color::srgb(0.95, 0.74, 0.3);
#[derive(Component)]
struct Inspector;
#[derive(Component)]
struct InspectorPanel;
/// Exploration HUD: status, trail, hints, and Details. Hidden while an agent's
/// walkthrough presents, so the frame shows only the explanation.
#[derive(Component)]
pub(crate) struct DevChrome;
#[derive(Component)]
struct InspectorDisclosure;
#[derive(Component, Default)]
struct InspectorViewport {
    content_hash: u64,
}
const DETAILS_BOTTOM: f32 = 60.0;
const DETAILS_COLLAPSED_HEIGHT: f32 = 36.0;
const ALL_CONTROLS: &str = "Controls\nClick: select and isolate | click empty space: clear | double-click: enter | Tab: select | Enter: enter | Backspace: clear isolation, then return | /: search\nPageUp / PageDown: children | Shift+PageUp / PageDown: relationship pages\nE: expand / collapse | M: mark | G: group | Shift+G: ungroup | Shift+M: clear marks | Z: semantic zoom\nSpace: separate layers | I: isolate | J / K: select path | X: expand both ends of path | Shift+I: clear path | B / N: browse claims\nP: pin | Shift+P: open pin origin | [ / ]: layer | L: policy | C: semantic view | O: reorganize | V: preview | A: apply | D: dismiss | U: undo\nDrag: orbit | Wheel: zoom | T: top | F: side | R: home | F1: Details | F5: reload | Esc: cancel\nOver open Details: wheel or PageUp / PageDown scrolls; Home / End reaches the beginning / end.";
#[derive(Component)]
struct NavigationText;
#[derive(Component)]
struct SearchText;
type CompletedLayout = Result<(DesktopProjection, NativeLayout, ViewState), layout::LayoutError>;
struct Job {
    action_id: Option<u64>,
    cancel: Arc<AtomicBool>,
    receive: Mutex<mpsc::Receiver<CompletedLayout>>,
    reorganize: bool,
    replacement_input: Option<LayoutInput>,
    // Startup-only recovery is retained even if saved geometry cannot be solved.
    restored_view: Arc<Mutex<Option<ViewState>>>,
    restored_dismissals: Arc<Mutex<Option<VecDeque<DismissedProposal>>>>,
    timing: Arc<Mutex<JobTiming>>,
    canonical_cache: Arc<Mutex<Option<layout::composed::CanonicalPlacementCache>>>,
    restored_composed: Arc<Mutex<Option<Vec<ComposedConstraints>>>>,
}
struct SearchJob {
    action_id: Option<u64>,
    cancel: Arc<AtomicBool>,
    receive: Mutex<mpsc::Receiver<Result<Vec<ViewerSearchResult>, layout::LayoutError>>>,
}
impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
/// Remembers which relations an expanded path carried, so the status message
/// can report how many lines they became once the new view is accepted.
#[derive(Debug, Clone)]
struct PathExpansion {
    members: Vec<String>,
    expanded: Vec<String>,
    skipped: Vec<String>,
}
#[derive(Resource, Default)]
struct Viewer {
    view: ViewState,
    accepted_view: ViewState,
    accepted: Option<(DesktopProjection, NativeLayout)>,
    job: Option<Job>,
    next_job_id: u64,
    retired_timings: VecDeque<LayoutTiming>,
    pending_expansion: Option<String>,
    pending_path_expansion: Option<PathExpansion>,
    selected: Option<String>,
    selected_relation: Option<String>,
    inspector_expanded: bool,
    inspector_scroll: f32,
    inspector_scroll_max: f32,
    inspector_hovered: bool,
    proposal: Option<NativeLayout>,
    preview: bool,
    undo: Vec<(
        ViewState,
        DesktopProjection,
        NativeLayout,
        Arc<ConstraintRegistry>,
    )>,
    cache: BTreeMap<String, Arc<(DesktopProjection, NativeLayout)>>,
    cache_order: VecDeque<String>,
    explode: bool,
    isolate: bool,
    click: ClickState,
    dirty: bool,
    fit_requested: bool,
    origin: Vec3,
    origins: BTreeMap<(String, String), Vec3>,
    origin_order: VecDeque<(String, String)>,
    focus_labels: BTreeMap<(String, String), String>,
    focus_label_order: VecDeque<(String, String)>,
    storage: Option<std::path::PathBuf>,
    save_needed: bool,
    restore_attempted: bool,
    message: String,
    dismissed: VecDeque<DismissedProposal>,
    search_open: bool,
    search_query: String,
    search_results: Vec<ViewerSearchResult>,
    search_selected: usize,
    relation_page: usize,
    search_job: Option<SearchJob>,
    active_action: Option<u64>,
    zoom_pending: bool,
    group_marks: BTreeMap<String, String>,
    canonical_cache: Arc<layout::composed::CanonicalPlacementCache>,
    composed_constraints: ConstraintRegistry,
    accepted_composed_constraints: Arc<ConstraintRegistry>,
    spotlight: Option<Spotlight>,
    /// The next job replaces the model with a different one: no identity check
    /// against the accepted model and no reuse of its layout.
    switching_model: bool,
    /// Set while a teaching view stands in for the published model. Nothing is saved
    /// while it is set.
    teaching: Option<TeachingReturn>,
    /// The transition for the latest job's scene, and then for the next rebuild once
    /// that job is accepted. Any other job clears it.
    job_transition: Option<Choreography>,
    pending_transition: Option<Choreography>,
}
type ConstraintRegistry = BTreeMap<(SemanticLens, String), ComposedConstraints>;

/// Action processing through status publication. Readers of `LayoutStatus`
/// that must see a finished action's effects run after this set.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct LayoutUpdate;

pub(crate) struct LayoutScenePlugin;
impl Plugin for LayoutScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(crate::transition::TransitionPlugin)
            .init_resource::<Viewer>()
            .init_resource::<crate::icons::IconAtlas>()
            .init_resource::<LayoutStorage>()
            .init_resource::<LayoutStatus>()
            .init_resource::<ViewerActions>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (process_actions, controls, poll, navigation_status)
                    .chain()
                    .in_set(LayoutUpdate)
                    .before(crate::camera::CameraControls),
            )
            .add_systems(
                PostUpdate,
                (labels, semantic_zoom).after(CameraUpdateSystems),
            )
            .add_systems(PostUpdate, dim_context);
    }
}
fn setup(
    mut commands: Commands,
    model: Res<LayoutInput>,
    storage: Res<LayoutStorage>,
    engine: Res<LayoutEngine>,
    mut viewer: ResMut<Viewer>,
) {
    let orbit = OrbitCamera::for_scene(Vec3::splat(5.0), 1280.0 / 900.0);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: FIELD_OF_VIEW,
            ..default()
        }),
        style::camera_finish(),
        orbit.transform(),
        orbit,
    ));
    style::spawn_lights(&mut commands);
    style::spawn_backdrop(&mut commands);
    commands.spawn((
        Text::new("Visualyze / Refining assembly..."),
        TextFont {
            font_size: 18.0,
            ..default()
        },
        TextLayout::default().with_no_wrap(),
        TextColor(Color::srgb_u8(225, 231, 238)),
        Node {
            position_type: PositionType::Absolute,
            left: px(24),
            top: px(20),
            right: px(24),
            height: px(48),
            overflow: Overflow::clip(),
            ..default()
        },
        LayoutText,
        DevChrome,
    ));
    spawn_inspector(&mut commands);
    commands.spawn((DevChrome, Text::new("Click: select and isolate | click empty space: clear | double-click: enter | Enter: enter | Backspace: back\n/: find | E: expand | I: isolate | Drag: orbit | Wheel: zoom | J/K: path | P: pin | F5: reload | F1: Details and controls"),TextFont{font_size:11.0,..default()},TextLayout::default().with_no_wrap(),TextColor(Color::srgb_u8(145,165,181)),Node{position_type:PositionType::Absolute,left:px(24),bottom:px(16),right:px(24),height:px(32),overflow:Overflow::clip(),..default()}));
    commands.spawn((
        NavigationText,
        DevChrome,
        Text::new("/ Find component | PageUp / PageDown: browse children"),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextLayout::default().with_no_wrap(),
        TextColor(Color::srgb_u8(178, 201, 222)),
        Node {
            position_type: PositionType::Absolute,
            left: px(24),
            right: px(24),
            top: px(72),
            height: px(38),
            overflow: Overflow::clip(),
            ..default()
        },
    ));
    commands.spawn((
        SearchText,
        GlobalZIndex(20),
        Text::new(""),
        TextFont {
            font_size: 16.0,
            ..default()
        },
        TextColor(Color::srgb_u8(227, 235, 243)),
        BackgroundColor(Color::srgb_u8(25, 33, 44)),
        Node {
            position_type: PositionType::Absolute,
            display: Display::None,
            left: px(24),
            right: px(24),
            top: px(128),
            padding: UiRect::all(px(16)),
            ..default()
        },
    ));
    viewer.storage = storage.0.clone();
    start(&model, *engine, &mut viewer, false);
}

fn details_rect(viewport: Vec2, expanded: bool) -> Rect {
    let height = if expanded {
        (viewport.y * 0.25).clamp(120.0, 220.0)
    } else {
        DETAILS_COLLAPSED_HEIGHT
    };
    Rect::from_corners(
        Vec2::new(24.0, viewport.y - DETAILS_BOTTOM - height),
        Vec2::new((viewport.x - 24.0).max(24.0), viewport.y - DETAILS_BOTTOM),
    )
}
fn spawn_inspector(commands: &mut Commands) {
    commands
        .spawn((
            InspectorPanel,
            DevChrome,
            GlobalZIndex(10),
            BackgroundColor(Color::srgba(0.055, 0.07, 0.09, 0.97)),
            Node {
                position_type: PositionType::Absolute,
                left: px(24),
                right: px(24),
                bottom: px(DETAILS_BOTTOM),
                height: px(DETAILS_COLLAPSED_HEIGHT),
                padding: UiRect::all(px(8)),
                row_gap: px(6),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                ..default()
            },
        ))
        .with_children(|panel| {
            panel.spawn((
                InspectorDisclosure,
                Text::new("Details | F1 opens full names, relationships, status and controls"),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb_u8(214, 223, 235)),
                TextLayout::default().with_no_wrap(),
                Node {
                    height: px(18),
                    flex_shrink: 0.0,
                    overflow: Overflow::clip(),
                    ..default()
                },
            ));
            panel
                .spawn((
                    InspectorViewport::default(),
                    ScrollPosition::default(),
                    Node {
                        display: Display::None,
                        width: percent(100),
                        min_height: px(0),
                        flex_grow: 1.0,
                        flex_direction: FlexDirection::Column,
                        overflow: Overflow::scroll_y(),
                        ..default()
                    },
                ))
                .with_children(|viewport| {
                    viewport.spawn((
                        Inspector,
                        Text::new(""),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(Color::srgb_u8(190, 205, 220)),
                        TextLayout::new_with_linebreak(bevy::text::LineBreak::WordOrCharacter),
                        Node {
                            width: percent(100),
                            flex_shrink: 0.0,
                            ..default()
                        },
                    ));
                });
        });
}
fn details_scroll(
    offset: f32,
    maximum: f32,
    viewport: f32,
    wheel: f32,
    keys: &ButtonInput<KeyCode>,
    hovered: bool,
) -> f32 {
    let maximum = maximum.max(0.0);
    if !hovered {
        return offset.clamp(0.0, maximum);
    }
    if keys.just_pressed(KeyCode::Home) {
        return 0.0;
    }
    if keys.just_pressed(KeyCode::End) {
        return maximum;
    }
    let page = viewport * 0.85;
    (offset
        + wheel
        + if keys.just_pressed(KeyCode::PageDown) {
            page
        } else {
            0.0
        }
        - if keys.just_pressed(KeyCode::PageUp) {
            page
        } else {
            0.0
        })
    .clamp(0.0, maximum)
}
fn compact_ui_text(text: &str, maximum: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= maximum {
        return normalized;
    }
    format!(
        "{}...",
        normalized
            .chars()
            .take(maximum.saturating_sub(3))
            .collect::<String>()
    )
}
fn full_navigation(viewer: &Viewer, p: &DesktopProjection) -> String {
    let mut trail = viewer
        .accepted_view
        .breadcrumbs()
        .iter()
        .chain(std::iter::once(&viewer.accepted_view.focus))
        .map(|id| match id {
            None => "Overview".into(),
            Some(id) => {
                let label = viewer
                    .focus_labels
                    .get(&(p.model_id.clone(), id.clone()))
                    .map(String::as_str)
                    .or_else(|| p.labels.get(id).map(|label| label.label.as_str()))
                    .unwrap_or(id);
                format!("{label} [{id}]")
            }
        })
        .collect::<Vec<_>>()
        .join(" > ");
    let label = p
        .labels
        .get(&p.root)
        .map_or(p.root.as_str(), |label| label.label.as_str());
    trail.push_str(&format!(
        "\nCurrent assembly: {label} [{}] | {:?} view",
        p.root, p.lens
    ));
    if let Some(page) = &p.page {
        let first = if page.total_children == 0 {
            0
        } else {
            page.page_index * page.page_size + 1
        };
        let last = ((page.page_index + 1) * page.page_size).min(page.total_children);
        trail.push_str(&format!("\nChildren {first}-{last} of {}. PageUp / PageDown changes child pages outside this panel.", page.total_children));
    }
    if let Some(page) = &p.relation_page {
        trail.push_str(&format!(
            "\nRelationship page {}: {} records, {} internal{}.",
            viewer.accepted_view.relation_history.len() + 1,
            page.loaded_records,
            page.internal_count,
            if page.complete {
                ""
            } else {
                "; more available"
            }
        ));
    }
    trail
}
/// The status-line noise level. A view over the threshold is shown, never
/// refused, and is marked so it is visible: explicit expansion can cause it.
fn noise_label(noise: f64) -> String {
    if noise > layout::noise::NOISE_THRESHOLD {
        format!("noise {noise:.0} (dense)")
    } else {
        format!("noise {noise:.1}")
    }
}
/// A note for the densest expanded interior, shown only when it is over the
/// threshold. The focused view can be quiet while a box opened in it is not.
fn interior_note(status: &LayoutStatus) -> Option<String> {
    let (_, noise) = status.densest_interior.as_ref()?;
    (*noise > layout::noise::NOISE_THRESHOLD)
        .then(|| format!("densest interior {noise:.0} (dense)"))
}
fn brief_status(viewer: &Viewer, status: &LayoutStatus) -> String {
    if status.error.is_some() {
        return "Update unavailable - F1 opens full details".into();
    }
    if viewer.proposal.is_some() {
        return "Reorganization ready - V previews, A applies, D dismisses".into();
    }
    if let Some(job) = &viewer.job {
        return if job.reorganize {
            "Checking an alternative - accepted view stays available"
        } else {
            "Loading and refining - Esc cancels"
        }
        .into();
    }
    if let Some((p, s)) = &viewer.accepted {
        let mut line = format!(
            "{} objects | {} paths | revision {} | {}",
            s.geometry.boxes.len(),
            s.geometry.routes.len(),
            p.revision,
            noise_label(status.noise)
        );
        if let Some(note) = interior_note(status) {
            line.push_str(" | ");
            line.push_str(&note);
        }
        return line;
    }
    "Preparing the first view".into()
}

fn finish_action(viewer: &mut Viewer, status: &mut LayoutStatus, error: Option<String>) {
    finish_ticket(status, viewer.active_action.take(), error);
}
fn finish_ticket(status: &mut LayoutStatus, id: Option<u64>, error: Option<String>) {
    if let Some(id) = id {
        if id > status.completed_action_id {
            let (mut start, mut end) = (id, id);
            if let Some((&lo, &hi)) = status.completed_interactions.range(..=id).next_back()
                && hi.saturating_add(1) >= id
            {
                start = lo;
                end = end.max(hi);
                status.completed_interactions.remove(&lo);
            }
            if let Some((&lo, &hi)) = status.completed_interactions.range(id..).next()
                && lo <= end.saturating_add(1)
            {
                end = end.max(hi);
                status.completed_interactions.remove(&lo);
            }
            status.completed_interactions.insert(start, end);
        }
        while let Some((&start, &end)) = status.completed_interactions.first_key_value() {
            if start != status.completed_action_id.saturating_add(1) {
                break;
            }
            status.completed_action_id = end;
            status.completed_interactions.remove(&start);
        }
        status.action_error = error;
    }
}

fn live_interaction(action: &ViewerAction) -> bool {
    matches!(
        action,
        ViewerAction::Select(_)
            | ViewerAction::SelectRelation(_)
            | ViewerAction::NextRelation
            | ViewerAction::PreviousRelation
            | ViewerAction::ClearRelationSelection
            | ViewerAction::ToggleIsolate
            | ViewerAction::ToggleExplode
            | ViewerAction::ToggleDetails
            | ViewerAction::NextClaims
            | ViewerAction::PreviousClaims
            | ViewerAction::ToggleGroupMember(_)
            | ViewerAction::ClearGroupMarks
    )
}
fn process_actions(
    mut actions: ResMut<ViewerActions>,
    model: Res<LayoutInput>,
    engine: Res<LayoutEngine>,
    mut viewer: ResMut<Viewer>,
    mut status: ResMut<LayoutStatus>,
) {
    let cancel = actions
        .queue
        .front()
        .is_some_and(|(_, action)| matches!(action, ViewerAction::Cancel));
    let reload = actions.queue.front().is_some_and(|(_, action)| {
        matches!(action, ViewerAction::Reload | ViewerAction::Publish(_))
    });
    // Automatic reorganization is speculative. A user navigation, search or
    // constraint edit supersedes it immediately, including a result already in
    // its channel. Dropping the job cancels work and makes late results inert.
    if viewer
        .job
        .as_ref()
        .is_some_and(|job| job.reorganize && job.action_id.is_none())
        && actions
            .queue
            .front()
            .is_some_and(|(_, action)| !live_interaction(action))
    {
        retire_job(&mut viewer);
    }
    let pending = viewer.job.is_some() || viewer.search_job.is_some();
    let next = if !cancel && !reload && pending {
        actions
            .queue
            .iter()
            .position(|(_, action)| live_interaction(action))
            .and_then(|index| actions.queue.remove(index))
    } else {
        actions.queue.pop_front()
    };
    let Some((id, action)) = next else {
        return;
    };
    status.last_action_id = status.last_action_id.max(id);
    status.action_error = None;
    status.error = None;
    if cancel {
        actions.queue.clear();
        viewer.search_job.take();
        abandon_pending(&mut viewer);
        viewer.search_open = false;
        viewer.active_action = None;
        let cancelled_through = actions.next_id;
        status.completed_action_id = status.completed_action_id.max(cancelled_through);
        status
            .completed_interactions
            .retain(|_, end| *end > cancelled_through);
        viewer.message = "Cancelled; the accepted arrangement is preserved.".into();
        return;
    }
    if reload {
        // Reload supersedes every previously submitted navigation/refinement.
        status.completed_action_id = status.completed_action_id.max(id - 1);
        status.completed_interactions.retain(|_, end| *end >= id);
    }
    viewer.active_action = Some(id);
    match apply_action(action, &model, *engine, &mut viewer) {
        Ok(true) => {}
        Ok(false) => finish_action(&mut viewer, &mut status, None),
        Err(error) => {
            viewer.message = error.clone();
            finish_action(&mut viewer, &mut status, Some(error));
        }
    }
}

/// Returns true only when completion is deferred to a worker.
fn apply_action(
    action: ViewerAction,
    model: &LayoutInput,
    engine: LayoutEngine,
    v: &mut Viewer,
) -> Result<bool, String> {
    let expand_requested = matches!(&action, ViewerAction::Expand(_));
    let known_display = |id: &str, v: &Viewer| -> Result<String, String> {
        let (p, _) = v.accepted.as_ref().ok_or("No accepted assembly yet")?;
        if p.nodes.iter().any(|node| node.id == id) {
            Ok(id.to_owned())
        } else {
            p.semantic_to_display
                .get(id)
                .cloned()
                .ok_or_else(|| format!("Object is not in the current view: {id}"))
        }
    };
    match action {
        ViewerAction::Reload => {
            if v.teaching.is_some() {
                return Err(
                    "A teaching view is showing; stop the walkthrough before reloading".into(),
                );
            }
            v.search_job.take();
            v.search_results.clear();
            abandon_pending(v);
            v.view.reset_relation_pages();
            v.search_open = false;
            let candidate = model
                .reload_candidate()
                .map_err(|error| error.to_string())?;
            start_job(
                &candidate,
                engine,
                v,
                false,
                Some(candidate.clone()),
                Some(model.clone()),
            );
            return Ok(true);
        }
        ViewerAction::Publish(published) => {
            if let Some(back) = v.teaching.take() {
                v.view = back.view;
                v.composed_constraints = back.constraints;
            }
            v.search_job.take();
            v.search_results.clear();
            abandon_pending(v);
            v.view.reset_relation_pages();
            v.search_open = false;
            let same = v
                .accepted
                .as_ref()
                .is_some_and(|(p, _)| p.model_id == published.1);
            if !same {
                let policy = v.view.policy;
                v.view = ViewState::default();
                v.view.policy = policy;
                v.switching_model = true;
                v.restore_attempted = false;
                v.selected = None;
                v.isolate = false;
                v.explode = false;
                v.spotlight = None;
                v.group_marks.clear();
            }
            start_job(
                &published.0,
                engine,
                v,
                false,
                Some(published.0.clone()),
                same.then(|| model.clone()),
            );
            return Ok(true);
        }
        ViewerAction::Stage(scene) => {
            let scene = *scene;
            let transition = scene.transition;
            // Entering, changing, or leaving a teaching view swaps the layout source.
            // The published model's view and constraints wait aside until it returns.
            let source = match &scene.teaching {
                Some(stage) if Arc::ptr_eq(&stage.input.0, &model.0) => None,
                Some(stage) => {
                    if v.teaching.is_none() {
                        v.teaching = Some(TeachingReturn {
                            input: model.clone(),
                            view: v.view.clone(),
                            constraints: std::mem::take(&mut v.composed_constraints),
                        });
                    }
                    v.view = ViewState::default();
                    v.view.policy = stage.policy;
                    Some(stage.input.clone())
                }
                None => v.teaching.take().map(|back| {
                    v.view = back.view;
                    v.composed_constraints = back.constraints;
                    back.input
                }),
            };
            v.view.reset_focus();
            if let Some(root) = scene.root {
                v.view.enter(root);
            }
            v.view.expanded.clear();
            v.view.collapsed.clear();
            for id in scene.expand {
                v.view.set_expanded(id, true);
            }
            v.explode = scene.explode;
            v.isolate = false;
            v.selected = None;
            v.selected_relation = None;
            v.spotlight = scene.spotlight;
            v.dirty = true;
            match source {
                Some(input) => {
                    v.switching_model = true;
                    start_job(
                        &input,
                        engine,
                        v,
                        false,
                        Some(input.clone()),
                        Some(model.clone()),
                    );
                }
                None => start(model, engine, v, false),
            }
            // A transition starts from the scene on screen, so the old layout is not
            // redrawn under the new emphasis first.
            if transition.is_some() {
                v.job_transition = transition;
                v.dirty = false;
            }
            return Ok(true);
        }
        ViewerAction::LeaveTeaching => {
            let Some(back) = v.teaching.take() else {
                return Ok(false);
            };
            v.view = back.view;
            v.composed_constraints = back.constraints;
            v.selected = None;
            v.selected_relation = None;
            // The rest of the model returns around the part, dimmed rather than hidden.
            if let Some(spotlight) = &mut v.spotlight {
                spotlight.context = SpotContext::Dim;
            }
            v.switching_model = true;
            start_job(
                &back.input,
                engine,
                v,
                false,
                Some(back.input.clone()),
                Some(model.clone()),
            );
            v.job_transition = Some(Choreography::put_back());
            return Ok(true);
        }
        ViewerAction::Enter(id) | ViewerAction::Jump(id) => {
            let id = v
                .accepted
                .as_ref()
                .and_then(|(p, _)| p.references.get(&id))
                .cloned()
                .unwrap_or(id);
            if id.trim().is_empty() || id.len() > 4096 {
                return Err("Choose a valid supplied object ID".into());
            }
            v.view.save_frame_pins();
            v.view.enter(id);
            v.view.restore_frame_pins();
            v.selected = None;
            // Isolation belongs to the box it was set on; it does not follow
            // the user into a different view.
            v.isolate = false;
            v.search_open = false;
            v.group_marks.clear();
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::Back => {
            // Backspace first leaves isolation, then the navigation trail.
            if v.isolate {
                v.isolate = false;
                v.selected_relation = None;
                v.dirty = true;
                v.message = "Isolation cleared.".into();
                return Ok(false);
            }
            v.view.save_frame_pins();
            if !v.view.back() {
                v.message = "Already at the start of this navigation trail.".into();
                return Ok(false);
            }
            v.view.restore_frame_pins();
            v.selected = None;
            v.group_marks.clear();
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::Expand(id) | ViewerAction::Collapse(id) => {
            // A supplied member can be represented by a collapsed visual group.
            // Keep its semantic expansion request instead of expanding the whole
            // group and silently dropping the member's requested detail level.
            known_display(&id, v)?;
            if !expand_requested {
                collapse_group_members(v, &id);
            }
            let pending_expansion = expand_requested.then(|| id.clone());
            v.view.set_expanded(id, expand_requested);
            start(model, engine, v, false);
            v.pending_expansion = pending_expansion;
            return Ok(true);
        }
        ViewerAction::Select(id) => {
            v.selected = Some(known_display(&id, v)?);
            v.selected_relation = None;
            v.relation_page = 0;
            v.dirty = true;
        }
        ViewerAction::SelectRelation(id) => select_relation(v, &id)?,
        ViewerAction::NextRelation | ViewerAction::PreviousRelation => {
            let forward = matches!(action, ViewerAction::NextRelation);
            let claims = selected_claims(v);
            if claims.is_empty() {
                return Err("Select a component with a visible relationship first".into());
            }
            let current = v
                .selected_relation
                .as_ref()
                .and_then(|id| claims.iter().position(|claim| *claim == id));
            let next = match (current, forward) {
                (None, true) => 0,
                (None, false) => claims.len() - 1,
                (Some(index), true) => (index + 1) % claims.len(),
                (Some(index), false) => (index + claims.len() - 1) % claims.len(),
            };
            let id = claims[next].clone();
            select_relation(v, &id)?;
        }
        ViewerAction::ClearRelationSelection => {
            v.selected_relation = None;
            v.dirty = true;
        }
        ViewerAction::ExpandPathEndpoints => {
            let id = v
                .selected_relation
                .clone()
                .ok_or("Select a path first (J / K), then X expands both of its endpoints")?;
            let (p, _) = v.accepted.as_ref().ok_or("No accepted assembly yet")?;
            let edge = selected_path_edge(v)
                .ok_or_else(|| format!("Relationship {id} has no visible path to expand"))?;
            let members = edge.semantic_ids.clone();
            let mut expanded = Vec::new();
            let mut skipped = Vec::new();
            for end in [&edge.source, &edge.target] {
                match endpoint_expansion_blocker(p, end) {
                    None => expanded.push(end.clone()),
                    Some(reason) => skipped.push(format!("{end} {reason}")),
                }
            }
            if expanded.is_empty() {
                return Err(format!(
                    "Neither end of the selected path can be expanded: {}.",
                    skipped.join("; ")
                ));
            }
            for end in &expanded {
                v.view.set_expanded(end.clone(), true);
            }
            start(model, engine, v, false);
            v.pending_expansion = Some(expanded.join(" and "));
            v.pending_path_expansion = Some(PathExpansion {
                members,
                expanded,
                skipped,
            });
            return Ok(true);
        }
        ViewerAction::NextClaims | ViewerAction::PreviousClaims => {
            let next = matches!(action, ViewerAction::NextClaims);
            let count = paged_claims(v).len();
            let last = count.saturating_sub(1) / CLAIMS_PER_PAGE;
            v.relation_page = if next {
                (v.relation_page + 1).min(last)
            } else {
                v.relation_page.saturating_sub(1)
            };
        }
        ViewerAction::NextRelations => {
            let next = v
                .accepted
                .as_ref()
                .and_then(|(p, _)| p.relation_page.as_ref())
                .and_then(|page| page.next.clone())
                .ok_or("No more relationship pages")?;
            if v.view.relation_history.len() >= 64 {
                return Err(
                    "Relationship page history is full; narrow the view before continuing".into(),
                );
            }
            v.view.relation_history.push(v.view.relation_cursor.take());
            v.view.relation_cursor = Some(next);
            v.relation_page = 0;
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::PreviousRelations => {
            v.view.relation_cursor = v
                .view
                .relation_history
                .pop()
                .ok_or("Already at the first relationship page")?;
            v.relation_page = 0;
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::NextPage | ViewerAction::PreviousPage => {
            let next = matches!(action, ViewerAction::NextPage);
            let page = v
                .accepted
                .as_ref()
                .and_then(|(p, _)| p.page.as_ref())
                .ok_or("This view has no child pages")?;
            if (next && !page.has_next) || (!next && !page.has_previous) {
                return Err("No more children in that direction".into());
            }
            v.view.child_page = if next {
                page.page_index + 1
            } else {
                page.page_index.saturating_sub(1)
            };
            v.view.reset_relation_pages();
            v.selected = None;
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::Search(query) => {
            let query = query.trim();
            if query.is_empty() || query.len() > 256 {
                return Err("Search needs 1-256 characters".into());
            }
            v.search_open = true;
            v.search_query = query.to_owned();
            v.search_results.clear();
            v.search_selected = 0;
            let input = model.clone();
            let query = query.to_owned();
            let cancel = Arc::new(AtomicBool::new(false));
            let signal = cancel.clone();
            let (send, receive) = mpsc::sync_channel(1);
            std::thread::spawn(move || {
                let result = input.search(&query, 20, &signal).map(|hits| {
                    hits.into_iter()
                        .map(|hit| ViewerSearchResult {
                            id: hit.id,
                            label: hit.label,
                            kind: hit.kind,
                        })
                        .collect()
                });
                if !signal.load(Ordering::Relaxed) {
                    let _ = send.send(result);
                }
            });
            v.search_job = Some(SearchJob {
                action_id: v.active_action.take(),
                cancel,
                receive: Mutex::new(receive),
            });
            return Ok(true);
        }
        ViewerAction::SetPolicy(policy) => {
            v.view.policy = policy;
            v.view.child_page = 0;
            v.view.reset_relation_pages();
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::SetLens(lens) => {
            v.view.save_frame_pins();
            v.view.lens = lens;
            v.view.reset_focus();
            v.view.restore_frame_pins();
            v.selected = None;
            v.group_marks.clear();
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::SetSemanticZoom(enabled) => {
            v.view.semantic_zoom = enabled;
            v.accepted_view.semantic_zoom = enabled;
            v.save_needed = true;
            v.message = format!(
                "Semantic zoom {}. Explicit expand/collapse choices take precedence.",
                if enabled { "on" } else { "off" }
            );
        }
        ViewerAction::UpdateZoom {
            id,
            projected_pixels,
        } => {
            let id = known_display(&id, v)?;
            if v.view.semantic_zoom && v.view.update_zoom(&id, projected_pixels) {
                v.view.reset_relation_pages();
                start(model, engine, v, false);
                return Ok(true);
            }
        }
        ViewerAction::OpenPinOrigin(id) => {
            let origin = pin_origin(v, &id).ok_or("No saved pin for the selected component")?;
            return apply_action(ViewerAction::Jump(origin), model, engine, v);
        }
        ViewerAction::Pin(id) => {
            let id = known_display(&id, v)?;
            if let Some((p, s)) = &v.accepted
                && let Some(solution) = s.composed_solution()
            {
                if inherited_pin(v, &id) {
                    return Err(pin_origin_message(v, &id));
                }
                let owner = solution
                    .assembly
                    .frame_ids()
                    .find(|frame| {
                        solution
                            .assembly
                            .frame(frame)
                            .is_ok_and(|f| f.solution().boxes.contains_key(&id))
                            && (*frame != id || *frame == solution.assembly.root())
                    })
                    .ok_or("No canonical owner for the selected component")?;
                let pin = layout::assembly::AnchorPin::capture(
                    &solution.assembly,
                    owner,
                    solution.assembly.root(),
                    &id,
                )
                .map_err(|e| e.to_string())?;
                let mut constraints = s.owned_constraints().unwrap();
                if !v
                    .composed_constraints
                    .contains_key(&(p.lens, p.root.clone()))
                    && v.composed_constraints.len() >= 64
                {
                    return Err("The durable pin registry already contains 64 view contexts; unpin an unused context before adding another".into());
                }
                constraints.anchors.retain(|old| old.object_id() != id);
                constraints
                    .pins
                    .entry(owner.to_owned())
                    .or_default()
                    .insert(id, pin.local_point());
                constraints.anchors.push(pin);
                let mut sets: Vec<_> = v
                    .composed_constraints
                    .values()
                    .filter(|set| set.context != constraints.context)
                    .map(ComposedConstraints::persisted)
                    .collect();
                sets.push(constraints.persisted());
                layout::composed::persistence::validate_constraint_registry(
                    &p.model_id,
                    &sets,
                    &AtomicBool::new(false),
                )
                .map_err(|error| {
                    format!("Pin was not applied; existing constraints retained: {error}")
                })?;
                v.composed_constraints
                    .insert((p.lens, p.root.clone()), constraints);
                start(model, engine, v, false);
                return Ok(true);
            }
            let point = v
                .accepted
                .as_ref()
                .and_then(|(_, s)| s.flat())
                .and_then(|s| s.boxes.get(&id))
                .ok_or("No accepted bounds to pin")?
                .bounds
                .center();
            v.view.pins.insert(id, point);
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::Unpin(id) => {
            if inherited_pin(v, &id) {
                return Err(pin_origin_message(v, &id));
            }
            if let Some((p, _)) = &v.accepted
                && let Some(constraints) = v.composed_constraints.get_mut(&(p.lens, p.root.clone()))
            {
                constraints.anchors.retain(|pin| pin.object_id() != id);
                for pins in constraints.pins.values_mut() {
                    pins.remove(&id);
                }
                constraints.pins.retain(|_, pins| !pins.is_empty());
                constraints.ownership.remove(&id);
                constraints.presentation.remove(&id);
            }
            v.view.pins.remove(&id);
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::SetLayer { id, band } => {
            if !(-16..=16).contains(&band) {
                return Err("Layer preference must be between -16 and 16".into());
            }
            let id = known_display(&id, v)?;
            v.view.layer_preferences.insert(id, band);
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::SetGroup {
            id,
            parent,
            members,
        } => {
            set_group(v, id, parent, members)?;
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::RemoveGroup(id) => {
            let suggestion = v
                .accepted
                .as_ref()
                .and_then(|(p, _)| {
                    p.suggested_groups
                        .iter()
                        .find(|group| group.id == id && group.suggested)
                })
                .cloned();
            if !v.view.group_overrides.iter().any(|group| group.id == id) && suggestion.is_none() {
                return Err("Select a visual group to remove".into());
            }
            if suggestion.is_some() && v.view.dismissed_groups.len() >= 1024 {
                return Err("This view already has 1024 dismissed suggestions".into());
            }
            v.view.save_frame_pins();
            if let Some(group) = suggestion {
                v.view.dismiss_suggestion(&group);
            } else {
                v.view.remove_local_group(&id);
            }
            v.view.restore_frame_pins();
            v.group_marks.clear();
            v.selected = None;
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::ToggleGroupMember(id) => {
            let id = known_display(&id, v)?;
            if v.group_marks.remove(&id).is_some() {
                v.dirty = true;
                return Ok(false);
            }
            if v.group_marks.len() >= 64 {
                return Err("A visual group supports up to 64 members".into());
            }
            let (p, _) = v.accepted.as_ref().ok_or("No accepted assembly")?;
            let node = p
                .nodes
                .iter()
                .find(|node| node.id == id)
                .ok_or("Unknown component")?;
            if p.suggested_groups.iter().any(|group| group.id == id)
                || p.references.contains_key(&id)
                || id == p.root
            {
                return Err("Mark supplied sibling components; reference ports and visual groups are not members".into());
            }
            let parent = node
                .parent
                .as_ref()
                .ok_or("This component has no supplied parent")?;
            if p.suggested_groups.iter().any(|group| group.id == *parent) {
                return Err(
                    "Remove this visual group before marking a new group of its members".into(),
                );
            }
            if v.group_marks.values().any(|owner| owner != parent) {
                return Err(
                    "Marked components must share a parent. Shift+M clears the current marks."
                        .into(),
                );
            }
            v.group_marks.insert(id, parent.clone());
            v.message = format!(
                "{} components marked. G creates a local visual group; Shift+M clears marks.",
                v.group_marks.len()
            );
            v.dirty = true;
        }
        ViewerAction::ClearGroupMarks => {
            v.group_marks.clear();
            v.dirty = true;
        }
        ViewerAction::GroupMarked => {
            if v.group_marks.len() < 2 {
                return Err("Mark at least two sibling components with M first".into());
            }
            let parent = v.group_marks.values().next().unwrap().clone();
            let members: Vec<_> = v.group_marks.keys().cloned().collect();
            let base = local_group_id(&parent, &members);
            let mut id = base.clone();
            let mut suffix = 0;
            while v.view.group_overrides.iter().any(|group| group.id == id)
                || v.accepted
                    .as_ref()
                    .is_some_and(|(p, _)| p.nodes.iter().any(|node| node.id == id))
            {
                suffix += 1;
                id = format!("{base}:{suffix}");
            }
            set_group(v, id, parent, members)?;
            v.group_marks.clear();
            start(model, engine, v, false);
            return Ok(true);
        }
        ViewerAction::ToggleExplode => {
            if let Some(layout) = displayed_solution(v) {
                layout
                    .geometry
                    .validate_float_geometry(!v.explode, v.origin)
                    .map_err(|e| e.to_string())?;
            }
            v.explode = !v.explode;
            v.dirty = true;
            v.fit_requested = true;
        }
        ViewerAction::ToggleIsolate => {
            v.isolate = !v.isolate;
            v.dirty = true;
        }
        ViewerAction::ToggleDetails => {
            v.inspector_expanded = !v.inspector_expanded;
            if !v.inspector_expanded {
                v.inspector_hovered = false;
            }
        }
        ViewerAction::Reorganize => {
            if !can_reorganize(v) {
                return Err("Wait for an accepted view before reorganizing".into());
            }
            start(model, engine, v, true);
            return Ok(true);
        }
        ViewerAction::Preview => {
            if !v
                .proposal
                .as_ref()
                .zip(v.accepted.as_ref())
                .is_some_and(|(p, (_, s))| same_view(p, s))
            {
                return Err("No current reorganization proposal".into());
            }
            if !v.preview {
                v.proposal
                    .as_ref()
                    .unwrap()
                    .geometry
                    .validate_float_geometry(v.explode, v.origin)
                    .map_err(|e| e.to_string())?;
            }
            v.preview = !v.preview;
            v.dirty = true;
        }
        ViewerAction::Apply => {
            let candidate = v.proposal.clone().ok_or("No reorganization proposal")?;
            let (p, old) = v.accepted.clone().ok_or("No accepted view")?;
            if !same_view(&candidate, &old) {
                return Err("Reorganization belongs to a different view".into());
            }
            candidate
                .validate_constraints(
                    &p,
                    &visible_pins(&v.view, &p),
                    if engine == LayoutEngine::Composed {
                        Some(
                            requested_composed_constraints(
                                &p,
                                v.view.policy,
                                &v.composed_constraints,
                            )
                            .map_err(|error| error.to_string())?,
                        )
                    } else {
                        None
                    }
                    .as_ref(),
                )
                .map_err(|e| e.to_string())?;
            candidate
                .geometry
                .validate_float_geometry(v.explode, v.origin)
                .map_err(|e| e.to_string())?;
            let canonical_cache = accepted_canonical_cache(v, &p, &candidate, engine)?;
            clear_preview(v);
            remember(v);
            if let Some(cache) = canonical_cache {
                v.canonical_cache = Arc::new(cache);
                v.cache.clear();
                v.cache_order.clear();
            }
            cache_layout(v, p.clone(), candidate.clone());
            v.accepted = Some((p, candidate));
            restore_accepted_constraints(v);
            v.dirty = true;
            v.save_needed = true;
        }
        ViewerAction::Dismiss => {
            if let Some((proposal, (projection, accepted))) =
                v.proposal.as_ref().zip(v.accepted.as_ref())
            {
                if !same_view(proposal, accepted) {
                    return Err("Reorganization belongs to a different view".into());
                }
                let key = proposal_identity(projection, proposal);
                v.dismissed.retain(|old| *old != key);
                v.dismissed.push_back(key);
                while v.dismissed.len() > 32 {
                    v.dismissed.pop_front();
                }
                v.save_needed = true;
            }
            clear_preview(v);
        }
        ViewerAction::Undo => {
            let (view, p, old, registry) = v.undo.last().cloned().ok_or("Nothing to undo")?;
            old.validate(&p, &visible_pins(&view, &p))
                .map_err(|e| e.to_string())?;
            old.geometry
                .validate_float_geometry(v.explode, candidate_origin(v, &p, &old))
                .map_err(|e| e.to_string())?;
            let canonical_cache = accepted_canonical_cache(v, &p, &old, engine)?;
            v.undo.pop();
            v.composed_constraints = registry.as_ref().clone();
            clear_preview(v);
            if let Some(cache) = canonical_cache {
                v.canonical_cache = Arc::new(cache);
                v.cache.clear();
                v.cache_order.clear();
            }
            select_origin(v, &p, &old);
            v.view = view.clone();
            v.accepted_view = view;
            cache_layout(v, p.clone(), old.clone());
            v.accepted = Some((p, old));
            restore_accepted_constraints(v);
            v.selected = None;
            v.selected_relation = None;
            v.group_marks.clear();
            v.dirty = true;
            v.fit_requested = true;
            v.save_needed = true;
        }
        ViewerAction::Cancel => unreachable!("cancellation bypasses queued actions"),
    }
    Ok(false)
}

fn expansion_toggle(viewer: &Viewer, id: &str) -> ViewerAction {
    let displayed_open_group = viewer.accepted.as_ref().is_some_and(|(p, _)| {
        p.suggested_groups.iter().any(|group| group.id == id)
            && p.nodes.iter().any(|node| node.id == id && !node.solid)
    });
    // A box the projector opened on its own is open as far as the user can
    // tell: E collapses it, and the explicit collapse keeps it closed. Expand
    // on one makes the opening explicit.
    let opened_automatically = viewer
        .accepted
        .as_ref()
        .is_some_and(|(p, _)| p.auto_opened.contains(id));
    if displayed_open_group || opened_automatically || viewer.view.is_expanded(id) {
        ViewerAction::Collapse(id.to_owned())
    } else {
        ViewerAction::Expand(id.to_owned())
    }
}

fn collapse_group_members(viewer: &mut Viewer, id: &str) {
    let Some((p, _)) = &viewer.accepted else {
        return;
    };
    let Some(group) = p.suggested_groups.iter().find(|group| group.id == id) else {
        return;
    };
    let mut descendants: BTreeSet<_> = group.members.iter().cloned().collect();
    descendants.insert(id.to_owned());
    // Walk the bounded displayed tree and its semantic aliases. This includes
    // automatic detail choices and internal IDs represented by collapsed nodes.
    loop {
        let before = descendants.len();
        for node in &p.nodes {
            if node
                .parent
                .as_ref()
                .is_some_and(|parent| descendants.contains(parent))
            {
                descendants.insert(node.id.clone());
                descendants.extend(node.semantic_ids.iter().cloned());
            }
        }
        if descendants.len() == before {
            break;
        }
    }
    let expanded: Vec<_> = viewer
        .view
        .expanded_ids()
        .into_iter()
        .filter(|expanded| {
            descendants.contains(expanded)
                || p.semantic_to_display
                    .get(expanded)
                    .is_some_and(|display| descendants.contains(display))
        })
        .collect();
    for expanded in expanded {
        viewer.view.set_expanded(expanded, false);
    }
}

fn local_group_id(parent: &str, members: &[String]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for text in std::iter::once(parent).chain(members.iter().map(String::as_str)) {
        for byte in text.bytes().chain(std::iter::once(0)) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    format!("view:local-group:{hash:016x}")
}

fn set_group(
    viewer: &mut Viewer,
    id: String,
    parent: String,
    mut members: Vec<String>,
) -> Result<(), String> {
    let valid =
        |text: &str| !text.is_empty() && text.len() <= 4096 && !text.chars().any(char::is_control);
    if !valid(&id)
        || !id.starts_with("view:")
        || !valid(&parent)
        || !(2..=64).contains(&members.len())
        || members
            .iter()
            .any(|member| !valid(member) || member == &id || member == &parent)
    {
        return Err("A local group needs a view: ID and 2-64 supplied sibling IDs".into());
    }
    members.sort();
    if members.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("A component may occur only once in a group".into());
    }
    let existing = viewer
        .view
        .group_overrides
        .iter()
        .position(|group| group.id == id);
    if existing.is_none() && viewer.view.group_overrides.len() >= 64 {
        return Err("This view already has 64 local groups".into());
    }
    if viewer
        .view
        .group_overrides
        .iter()
        .any(|group| group.id != id && group.members.iter().any(|member| members.contains(member)))
    {
        return Err(
            "A member already belongs to another local group; remove that group before regrouping"
                .into(),
        );
    }
    let group = layout::projection::DisplayGroup {
        id,
        parent,
        members,
        suggested: false,
    };
    if let Some(index) = existing {
        viewer.view.group_overrides[index] = group;
    } else {
        viewer.view.group_overrides.push(group);
    }
    viewer.view.child_page = 0;
    viewer.view.reset_relation_pages();
    viewer.selected = None;
    viewer.selected_relation = None;
    Ok(())
}

fn start(model: &LayoutInput, engine: LayoutEngine, v: &mut Viewer, reorganize: bool) {
    start_job(model, engine, v, reorganize, None, None);
}

fn start_job(
    model: &LayoutInput,
    engine: LayoutEngine,
    v: &mut Viewer,
    reorganize: bool,
    replacement_input: Option<LayoutInput>,
    previous_input: Option<LayoutInput>,
) {
    if reorganize && !can_reorganize(v) {
        return;
    }
    retire_job(v);
    v.job_transition = None;
    v.pending_expansion = None;
    v.pending_path_expansion = None;
    clear_preview(v);
    if !reorganize {
        v.dirty |= v.selected_relation.take().is_some();
    }
    let input = model.clone();
    let reload = replacement_input.is_some();
    let switching = std::mem::take(&mut v.switching_model);
    let accepted_identity = v
        .accepted
        .as_ref()
        .filter(|_| !switching)
        .map(|(p, _)| (p.model_id.clone(), p.revision));
    let mut view = v.view.clone();
    let storage = v.storage.clone();
    let restore_saved = !v.restore_attempted;
    let cached = v.cache.clone();
    let cached_order = v.cache_order.clone();
    let canonical_cache = v.canonical_cache.clone();
    let composed_constraints = v.composed_constraints.clone();
    let previous = v.accepted.as_ref().filter(|_| !switching).map(|(p, s)| {
        v.cache
            .get(&s.projection_key)
            .cloned()
            .unwrap_or_else(|| Arc::new((p.clone(), s.clone())))
    });
    v.next_job_id = v.next_job_id.saturating_add(1);
    let timing = Arc::new(Mutex::new(JobTiming {
        record: LayoutTiming {
            job_id: v.next_job_id,
            action_id: v.active_action,
            cache_outcome: "not_checked",
            ..default()
        },
        ..default()
    }));
    let worker_timing = timing.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = cancel.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let restored_view = Arc::new(Mutex::new(None));
    let recovered = restored_view.clone();
    let restored_dismissals = Arc::new(Mutex::new(None));
    let recovered_dismissals = restored_dismissals.clone();
    let restored_composed = Arc::new(Mutex::new(None));
    let recovered_composed = restored_composed.clone();
    let published_cache = Arc::new(Mutex::new(None));
    let worker_cache = published_cache.clone();
    std::thread::spawn(move || {
        worker_timing.lock().unwrap().started = Some(Instant::now());
        let result = (|| {
            let mut restored_previous = None;
            let mut source_timings = SourceTimings::default();
            let prepared = timed_stage(&worker_timing, "source_preparation", || {
                input.prepare(&signal, &mut source_timings)
            });
            {
                let mut timing = worker_timing.lock().unwrap();
                timing.record.indexing_ms = source_timings.indexing_ms;
                timing.record.snapshot_read_ms = source_timings.snapshot_read_ms;
                timing.record.source_lock_wait_ms = source_timings.lock_wait_ms;
            }
            prepared?;
            let recovery = if let (Some(old_input), Some(old)) = (&previous_input, &previous) {
                if let Some(group) = old
                    .0
                    .suggested_groups
                    .iter()
                    .find(|group| view.focus.as_ref() == Some(&group.id))
                {
                    input.recover_reload_group(old_input, &mut view, group, &signal)?
                } else {
                    None
                }
            } else {
                None
            };
            if engine == LayoutEngine::Composed {
                crate::pin_provenance::seed_groups(&mut view, &composed_constraints, None)?;
            }
            let mut p = engine_projection(
                timed_projection(&worker_timing, &input, &view, &signal)?,
                engine,
            );
            if engine == LayoutEngine::Composed
                && crate::pin_provenance::seed_groups(
                    &mut view,
                    &composed_constraints,
                    Some(p.lens),
                )?
            {
                p = engine_projection(
                    timed_projection(&worker_timing, &input, &view, &signal)?,
                    engine,
                );
            }
            worker_timing.lock().unwrap().record.revision = Some(p.revision);
            if view.frame_lens != Some(p.lens) {
                let migrate_legacy = view.frame_lens.is_none() && previous.is_none();
                view.frame_lens = Some(p.lens);
                if migrate_legacy {
                    // Older private state used a single unqualified pin frame.
                    view.save_frame_pins();
                } else {
                    view.restore_frame_pins();
                    p = engine_projection(
                        timed_projection(&worker_timing, &input, &view, &signal)?,
                        engine,
                    );
                }
            }
            if let Some(note) = recovery {
                p.detail_note.push(' ');
                p.detail_note.push_str(&note);
            }
            if reload
                && let Some((model_id, revision)) = accepted_identity.as_ref()
                && (p.model_id != *model_id || p.revision <= *revision)
            {
                return Err(layout::LayoutError::Invalid(
                    "Reload requires the same model ID and a newer supplied revision; accepted model preserved".into(),
                ));
            }
            if engine == LayoutEngine::Composed {
                use layout::composed::{
                    CanonicalPins, ComposedRequest, ComposedReuse, CompositionContext,
                    cache_accepted_composed, persistence as composed_state, solve_composed_cached,
                    solve_composed_reorganized,
                };
                let mut restored = None;
                if !reload
                    && restore_saved
                    && previous.is_none()
                    && let Some(base) = &storage
                {
                    let path = composed_state_path(base, &p.model_id);
                    let present = match std::fs::symlink_metadata(&path) {
                        Ok(_) => true,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                        Err(error) => {
                            return Err(layout::LayoutError::Invalid(format!(
                                "Saved composed state preserved at {}: {error}",
                                path.display()
                            )));
                        }
                    };
                    if present {
                        let mut saved = timed_stage(&worker_timing, "restoration", || {
                            composed_state::load(&path, &signal)
                        })
                        .map_err(|error| match error {
                            composed_state::PersistenceError::Layout(
                                layout::LayoutError::Cancelled,
                            ) => layout::LayoutError::Cancelled,
                            error => layout::LayoutError::Invalid(format!(
                                "Saved composed state preserved at {}: {error}",
                                path.display()
                            )),
                        })?;
                        if saved.model_id != p.model_id || saved.revision > p.revision {
                            return Err(layout::LayoutError::Invalid(format!(
                                "Saved composed state preserved at {}: model differs or saved revision is newer",
                                path.display()
                            )));
                        }
                        view = saved.view.clone();
                        *recovered.lock().unwrap() = Some(view.clone());
                        *recovered_dismissals.lock().unwrap() =
                            Some(saved.dismissed_proposals.iter().cloned().collect());
                        let mut retained_constraints: Vec<ComposedConstraints> = saved
                            .constraint_sets
                            .iter()
                            .cloned()
                            .map(Into::into)
                            .collect();
                        if (saved.version < 3 || retained_constraints.is_empty())
                            && (!saved.canonical_pins.is_empty() || !saved.anchor_pins.is_empty())
                            && !retained_constraints
                                .iter()
                                .any(|set| set.context == saved.context)
                        {
                            retained_constraints.push(ComposedConstraints {
                                context: saved.context.clone(),
                                pins: saved.canonical_pins.clone(),
                                anchors: saved.anchor_pins.clone(),
                                ownership: BTreeMap::new(),
                                presentation: BTreeMap::new(),
                            });
                        }
                        let protected: ConstraintRegistry = retained_constraints
                            .iter()
                            .cloned()
                            .map(|set| ((set.context.lens, set.context.root.clone()), set))
                            .collect();
                        crate::pin_provenance::seed_groups(
                            &mut view,
                            &protected,
                            Some(saved.context.lens),
                        )?;
                        *recovered.lock().unwrap() = Some(view.clone());
                        *recovered_composed.lock().unwrap() = Some(retained_constraints);
                        p = engine_projection(
                            timed_projection(&worker_timing, &input, &view, &signal)?,
                            engine,
                        );
                        worker_timing.lock().unwrap().record.revision = Some(p.revision);
                        let context = CompositionContext {
                            model_id: p.model_id.clone(),
                            root: p.root.clone(),
                            lens: p.lens,
                            policy: view.policy,
                        };
                        if saved.context != context {
                            return Err(layout::LayoutError::Invalid(
                                "Saved composed arrangement cannot be repaired in a different model, root, lens or policy; original state file preserved".into()
                            ));
                        }
                        let registry = recovered_composed
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .iter()
                            .cloned()
                            .map(|set| ((set.context.lens, set.context.root.clone()), set))
                            .collect();
                        let (full_constraints, owned_constraints) =
                            worker_composed_constraints(&input, &p, &context, &registry, &signal)?;
                        let active = full_constraints.active(&p)?;
                        if let Some(sets) = recovered_composed.lock().unwrap().as_mut()
                            && let Some(set) = sets.iter_mut().find(|set| set.context == context)
                        {
                            *set = owned_constraints.clone();
                        }
                        if saved.version < composed_state::STATE_VERSION
                            && saved.revision == p.revision
                            && saved.accepted.projection_key == p.key
                        {
                            // Validate the old format under its original pin semantics;
                            // an upgrade must not turn corruption into a repair request.
                            saved.validate(&p, &signal).map_err(|error| {
                                layout::LayoutError::Invalid(format!(
                                    "Saved composed geometry failed validation: {error}"
                                ))
                            })?;
                        }
                        if saved.version == composed_state::STATE_VERSION
                            && saved.revision == p.revision
                            && saved.accepted.projection_key == p.key
                        {
                            if saved.canonical_pins != active.pins
                                || saved.anchor_pins != active.anchors
                            {
                                return Err(layout::LayoutError::Invalid("Saved visible constraints differ from the durable registry; original state file preserved".into()));
                            }
                            // An exact-identity file with invalid geometry is corrupt, not
                            // an instruction to replace the user's arrangement with a fresh solve.
                            timed_stage(&worker_timing, "restoration", || {
                                saved.validate(&p, &signal)
                            })
                            .map_err(|error| match error {
                                composed_state::PersistenceError::Layout(
                                    layout::LayoutError::Cancelled,
                                ) => layout::LayoutError::Cancelled,
                                error => layout::LayoutError::Invalid(format!(
                                    "Saved composed geometry failed validation: {error}"
                                )),
                            })?;
                            let mut cache = (*canonical_cache).clone();
                            timed_stage(&worker_timing, "restoration", || {
                                cache_accepted_composed(
                                    ComposedRequest {
                                        projection: &p,
                                        algorithm: engine.algorithm(),
                                        quality: QualityProfile::default(),
                                        pins: &saved.canonical_pins,
                                        work_budget: 192,
                                        cancelled: &signal,
                                    },
                                    &saved.context,
                                    &saved.accepted,
                                    &mut cache,
                                )
                            })?;
                            let accepted = NativeLayout::composed(
                                saved.accepted,
                                saved.canonical_pins,
                                saved.anchor_pins,
                                saved.context,
                                &signal,
                            )?
                            .retain_constraints(&p, full_constraints)?
                            .retain_owned_constraints(owned_constraints)?;
                            *worker_cache.lock().unwrap() = Some(cache);
                            worker_timing.lock().unwrap().record.cache_outcome = "saved_exact";
                            p.detail_note.push_str(" Restored exact composed arrangement, canonical pins and local overrides; quality measurements refreshed.");
                            return Ok((p, accepted, view));
                        }
                        worker_timing.lock().unwrap().record.cache_outcome = "saved_repair";
                        p.detail_note.push_str(" Repairing the saved composed arrangement from its previous canonical positions and constraints.");
                        restored = Some(saved);
                    }
                }
                let context = CompositionContext {
                    model_id: p.model_id.clone(),
                    root: p.root.clone(),
                    lens: p.lens,
                    policy: view.policy,
                };
                let registry = if restored.is_some() {
                    recovered_composed
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .iter()
                        .cloned()
                        .map(|set| ((set.context.lens, set.context.root.clone()), set))
                        .collect()
                } else {
                    composed_constraints.clone()
                };
                let (effective, owned_constraints) =
                    worker_composed_constraints(&input, &p, &context, &registry, &signal)?;
                let inherited = effective
                    .pins
                    .values()
                    .map(BTreeMap::len)
                    .sum::<usize>()
                    .saturating_sub(owned_constraints.pins.values().map(BTreeMap::len).sum());
                if inherited > 0 {
                    p.detail_note.push_str(&format!(" {inherited} pins are inherited from other regions. Select a pinned component and press Shift+P to open its origin and edit the pin."));
                }
                let desired = effective;
                let active = desired.active(&p)?;
                let pins: CanonicalPins = active.pins;
                let anchors = active.anchors;
                let hidden = desired.pins.values().map(BTreeMap::len).sum::<usize>()
                    - pins.values().map(BTreeMap::len).sum::<usize>();
                if hidden > 0 {
                    p.detail_note.push_str(&format!(" {hidden} pins remain saved outside the current detail or page; reopening restores their exact constraints."));
                }
                if !reorganize
                    && restored.is_none()
                    && let Some((old, s)) = cached.get(&p.key).map(Arc::as_ref)
                    && old.revision == p.revision
                    && s.validate_constraints(&p, &BTreeMap::new(), Some(&desired))
                        .is_ok()
                {
                    worker_timing.lock().unwrap().record.cache_outcome = "hit";
                    let accepted = s.clone().retain_owned_constraints(owned_constraints)?;
                    return Ok((p, accepted, view));
                }
                // A pinned view can be revisited after inspecting another frame.
                // Use its most recently accepted same-context predecessor, never
                // the unrelated active frame's coordinates. The core revalidates
                // each retained frame against new pins/locks independently.
                let previous_composed = restored
                    .as_ref()
                    .map(|saved| (&saved.context, &saved.accepted))
                    .or_else(|| {
                        previous
                            .iter()
                            .map(|old| &old.1)
                            .chain(
                                cached_order
                                    .iter()
                                    .rev()
                                    .filter_map(|key| cached.get(key))
                                    .map(|old| &old.1),
                            )
                            .find_map(|s| match &s.data {
                                LayoutData::Composed {
                                    solution,
                                    context: old,
                                    ..
                                } if old == &context => Some((old, solution.as_ref())),
                                _ => None,
                            })
                    });
                let mut cache = (*canonical_cache).clone();
                let result = timed_stage(&worker_timing, "solve", || {
                    let request = ComposedRequest {
                        projection: &p,
                        algorithm: engine.algorithm(),
                        quality: QualityProfile::default(),
                        pins: &pins,
                        work_budget: 192,
                        cancelled: &signal,
                    };
                    let reuse = ComposedReuse {
                        context: &context,
                        previous: previous_composed,
                        anchor_pins: &anchors,
                    };
                    if reorganize {
                        solve_composed_reorganized(request, reuse, &cache)
                    } else {
                        solve_composed_cached(request, reuse, &mut cache)
                    }
                })?;
                {
                    let mut timing = worker_timing.lock().unwrap();
                    timing.record.routing_batches_reused = result.work.cache.routing_hits;
                    timing.record.routing_batches_computed = result.work.cache.routing_misses;
                }
                if restored.is_none() {
                    worker_timing.lock().unwrap().record.cache_outcome = if reorganize {
                        "bypassed_reorganization"
                    } else if result.work.cache.misses == 0 {
                        "canonical_reuse"
                    } else {
                        "miss"
                    };
                }
                let mut accepted = NativeLayout::composed(result, pins, anchors, context, &signal)?;
                accepted = accepted
                    .retain_constraints(&p, desired)?
                    .retain_owned_constraints(owned_constraints)?;
                if !reorganize {
                    *worker_cache.lock().unwrap() = Some(cache);
                }
                return Ok((p, accepted, view));
            }
            if !reload
                && restore_saved
                && previous.is_none()
                && let Some(base) = &storage
                && let Some(path) = saved_state_file(base, &p.model_id, engine)?
            {
                let mut saved = timed_stage(&worker_timing, "restoration", || {
                    layout::persistence::load(&path)
                })
                .map_err(|error| {
                    layout::LayoutError::Invalid(format!(
                        "Saved state preserved at {}: {error}",
                        path.display()
                    ))
                })?;
                if saved.model_id != p.model_id || saved.revision > p.revision {
                    return Err(layout::LayoutError::Invalid(format!(
                        "Saved state preserved at {}: model differs or saved revision is newer",
                        path.display()
                    )));
                }
                *recovered_dismissals.lock().unwrap() = Some(saved.dismissed_proposals.into());
                // Pins and view corrections are authoritative even when a new
                // geometry profile needs to solve again. Never silently fall back
                // to an unconstrained default view after an upgrade.
                view = saved.view;
                let saved_frame = view.clone();
                *recovered.lock().unwrap() = Some(view.clone());
                p = engine_projection(
                    timed_projection(&worker_timing, &input, &view, &signal)?,
                    engine,
                );
                if view.frame_lens != Some(p.lens) {
                    view.frame_lens = Some(p.lens);
                    view.save_frame_pins();
                }
                *recovered.lock().unwrap() = Some(view.clone());
                migrate_quality_key(&mut saved.accepted, &p.key);
                if timed_stage(&worker_timing, "restoration", || {
                    layout::validate(&p, &saved.accepted, &visible_pins(&view, &p), 2)
                })
                .is_ok()
                {
                    worker_timing.lock().unwrap().record.cache_outcome = "saved_exact";
                    saved.accepted.metrics = layout::measure(&p, &saved.accepted, None);
                    p.detail_note.push_str(" Restored accepted arrangement and local overrides; quality measurements refreshed.");
                    return Ok((p, saved.accepted.into(), view));
                }
                if !saved_repair_matches_frame(&saved_frame, &saved.accepted, &p) {
                    return Err(layout::LayoutError::Invalid(
                        "Saved arrangement cannot be repaired in a different or unverified coordinate frame; original state file preserved".into(),
                    ));
                }
                // Membership/key changes are handled by the normal local-repair
                // solver. Never show old routes or bypass either prior-layout
                // shape checks or final geometry/pin validation.
                restored_previous = Some(saved.accepted);
                worker_timing.lock().unwrap().record.cache_outcome = "saved_repair";
                p.detail_note.push_str(" Saved geometry needs local repair; previous positions and compatible sizes seed refinement, with saved pins and local overrides retained. Previous state file preserved on failure.");
            }
            let pins = visible_pins(&view, &p);
            if !reorganize
                && let Some((old, s)) = cached.get(&p.key).map(Arc::as_ref)
                && old.revision == p.revision
                && timed_stage(&worker_timing, "restoration", || s.validate(&p, &pins)).is_ok()
            {
                worker_timing.lock().unwrap().record.cache_outcome = "hit";
                return Ok((p, s.clone(), view));
            }
            {
                let mut timing = worker_timing.lock().unwrap();
                if timing.record.cache_outcome == "not_checked" {
                    timing.record.cache_outcome = if reorganize {
                        "bypassed_reorganization"
                    } else {
                        "miss"
                    };
                }
            }
            let previous = previous
                .as_ref()
                .filter(|old| !reorganize && old.0.root == p.root && old.0.lens == p.lens)
                .and_then(|old| old.1.flat())
                .or(restored_previous.as_ref());
            let solution = timed_stage(&worker_timing, "solve", || {
                layout::solve(LayoutRequest {
                    projection: &p,
                    algorithm: engine.algorithm(),
                    pins: &pins,
                    previous,
                    quality: QualityProfile::default(),
                    work_budget: if reorganize { 192 } else { 48 },
                    cancelled: &signal,
                })
            })?;
            Ok((p, solution.into(), view))
        })();
        let result = result.map_err(|error| {
            if recovered.lock().unwrap().is_some() {
                layout::LayoutError::Invalid(format!(
                    "{error}; saved pins and local overrides retained, original state file preserved"
                ))
            } else {
                error
            }
        });
        worker_timing.lock().unwrap().finished = Some(Instant::now());
        if !signal.load(Ordering::Relaxed) {
            let _ = send.send(result);
        }
    });
    v.job = Some(Job {
        action_id: v.active_action.take(),
        cancel,
        receive: Mutex::new(receive),
        reorganize,
        replacement_input,
        restored_view,
        restored_dismissals,
        restored_composed,
        timing,
        canonical_cache: published_cache,
    });
    v.message = if reload {
        "Reloading and validating a newer supplied revision... Esc cancels"
    } else if reorganize {
        "Looking for a better arrangement..."
    } else {
        "Loading supplied records and refining assembly... Esc cancels"
    }
    .into();
}
fn remember(v: &mut Viewer) {
    if let Some((p, s)) = &v.accepted {
        if v.undo.len() >= 32 {
            v.undo.remove(0);
        }
        v.undo.push((
            v.accepted_view.clone(),
            p.clone(),
            s.clone(),
            v.accepted_composed_constraints.clone(),
        ));
    }
}
fn cache_layout(v: &mut Viewer, p: DesktopProjection, s: NativeLayout) {
    let key = s.projection_key.clone();
    v.cache.insert(key.clone(), Arc::new((p, s)));
    v.cache_order.retain(|k| k != &key);
    v.cache_order.push_back(key);
    while v.cache_order.len() > 16 {
        if let Some(key) = v.cache_order.pop_front() {
            v.cache.remove(&key);
        }
    }
}
fn retire_job(v: &mut Viewer) {
    if let Some(job) = v.job.take() {
        job.cancel.store(true, Ordering::Relaxed);
        let record = timing_snapshot(&job.timing.lock().unwrap(), "cancelled");
        v.retired_timings.push_back(record);
        while v.retired_timings.len() > 64 {
            v.retired_timings.pop_front();
        }
    }
}
fn abandon_pending(v: &mut Viewer) {
    if v.accepted.is_none()
        && let Some(job) = &v.job
    {
        if let Some(view) = job.restored_view.lock().unwrap().clone() {
            v.accepted_view = view;
        }
        if let Some(sets) = job.restored_composed.lock().unwrap().clone() {
            for constraints in sets {
                v.composed_constraints.insert(
                    (constraints.context.lens, constraints.context.root.clone()),
                    constraints,
                );
            }
        }
    }
    retire_job(v);
    v.job_transition = None;
    v.pending_expansion = None;
    v.pending_path_expansion = None;
    clear_preview(v);
    v.view = v.accepted_view.clone();
    if v.accepted.is_some() {
        v.composed_constraints = v.accepted_composed_constraints.as_ref().clone();
    }
    restore_accepted_constraints(v);
    if v.selected.as_ref().is_some_and(|id| {
        v.accepted
            .as_ref()
            .is_none_or(|(p, _)| !p.nodes.iter().any(|node| &node.id == id))
    }) {
        v.selected = None;
    }
    retain_valid_relation_selection(v);
    v.group_marks.clear();
    v.dirty = true;
}
fn restore_accepted_constraints(v: &mut Viewer) {
    if let Some((p, s)) = &v.accepted
        && let Some(constraints) = s.owned_constraints()
    {
        let key = (p.lens, p.root.clone());
        if constraints.pins.is_empty() && constraints.anchors.is_empty() {
            v.composed_constraints.remove(&key);
        } else {
            v.composed_constraints.insert(key, constraints);
        }
        v.accepted_composed_constraints = Arc::new(v.composed_constraints.clone());
    }
}

fn requested_composed_constraints(
    projection: &DesktopProjection,
    policy: layout::projection::LayoutPolicy,
    registry: &ConstraintRegistry,
) -> Result<ComposedConstraints, layout::LayoutError> {
    ComposedConstraints::resolve(
        &layout::composed::CompositionContext {
            model_id: projection.model_id.clone(),
            root: projection.root.clone(),
            lens: projection.lens,
            policy,
        },
        registry,
        &AtomicBool::new(false),
    )
}

/// Ownership is refreshed on the worker before deriving effective inherited
/// constraints. Derived pins never get installed as new owned registry entries.
fn worker_composed_constraints(
    input: &LayoutInput,
    p: &DesktopProjection,
    context: &layout::composed::CompositionContext,
    registry: &ConstraintRegistry,
    cancelled: &AtomicBool,
) -> Result<(ComposedConstraints, ComposedConstraints), layout::LayoutError> {
    let mut owned = registry
        .get(&(p.lens, p.root.clone()))
        .cloned()
        .unwrap_or_else(|| ComposedConstraints::empty(context.clone()));
    if owned.context != *context && (!owned.pins.is_empty() || !owned.anchors.is_empty()) {
        return Err(layout::LayoutError::Invalid(
            "Pinned composed geometry belongs to a different typed view; constraints retained"
                .into(),
        ));
    }
    owned.context = context.clone();
    verify_constraint_ownership(input, p, &mut owned, cancelled)?;
    let mut checked = registry.clone();
    checked.insert((p.lens, p.root.clone()), owned.clone());
    // Do not spend a durable context slot on an empty current view.
    if owned.pins.is_empty() && owned.anchors.is_empty() {
        checked.remove(&(p.lens, p.root.clone()));
    }
    // Classify related contexts using current supplied ancestry in both
    // directions before filtering stored paths. Otherwise moving a pinned
    // source region into the target could silently omit its old constraints.
    let has_foreign = checked.values().any(|set| {
        set.context.model_id == context.model_id
            && set.context.lens == context.lens
            && set.context.root != context.root
            && (!set.ownership.is_empty() || !set.presentation.is_empty())
    });
    let ancestors = if has_foreign {
        crate::pin_provenance::context_ancestors(input, &p.root, p.lens, &checked, p, cancelled)?
    } else {
        BTreeSet::new()
    };
    for set in checked.values() {
        if set.context.model_id != context.model_id
            || set.context.lens != context.lens
            || set.context.root == context.root
            || (set.ownership.is_empty() && set.presentation.is_empty())
        {
            continue;
        }
        let formerly_related = set
            .ownership
            .values()
            .chain(set.presentation.values().map(|pin| &pin.path))
            .any(|path| path.contains(&context.root));
        let currently_related = ancestors.contains(&set.context.root)
            || crate::pin_provenance::context_ancestors(
                input,
                &set.context.root,
                p.lens,
                &checked,
                p,
                cancelled,
            )?
            .contains(&context.root);
        if !formerly_related && !currently_related && set.presentation.is_empty() {
            continue;
        }
        let mut supplied = set.clone();
        // Legacy display pins without captured provenance stay source-owned.
        // New display bindings are checked even when their representation is
        // absent, so a changed target cannot masquerade as another page.
        for pins in supplied.pins.values_mut() {
            pins.retain(|id, _| {
                supplied.ownership.contains_key(id) || supplied.presentation.contains_key(id)
            });
        }
        supplied.pins.retain(|_, pins| !pins.is_empty());
        supplied.anchors.retain(|pin| {
            supplied
                .pins
                .get(pin.owner())
                .is_some_and(|pins| pins.contains_key(pin.object_id()))
        });
        verify_constraint_ownership(input, p, &mut supplied, cancelled)?;
    }
    let mut effective = ComposedConstraints::resolve(context, &checked, cancelled)?;
    verify_constraint_ownership(input, p, &mut effective, cancelled)?;
    Ok((effective, owned))
}

/// Recheck semantic and presentation provenance on the worker before activating pins.
fn verify_constraint_ownership(
    input: &LayoutInput,
    p: &DesktopProjection,
    constraints: &mut ComposedConstraints,
    cancelled: &AtomicBool,
) -> Result<(), layout::LayoutError> {
    crate::pin_provenance::verify(input, p, constraints, cancelled)
}
fn accepted_canonical_cache(
    v: &Viewer,
    p: &DesktopProjection,
    s: &NativeLayout,
    engine: LayoutEngine,
) -> Result<Option<layout::composed::CanonicalPlacementCache>, String> {
    let LayoutData::Composed {
        solution,
        pins,
        context,
        ..
    } = &s.data
    else {
        return Ok(None);
    };
    let mut cache = (*v.canonical_cache).clone();
    layout::composed::cache_accepted_composed(
        layout::composed::ComposedRequest {
            projection: p,
            algorithm: engine.algorithm(),
            quality: QualityProfile::default(),
            pins,
            work_budget: 192,
            cancelled: &AtomicBool::new(false),
        },
        context,
        solution,
        &mut cache,
    )
    .map_err(|e| e.to_string())?;
    Ok(Some(cache))
}
fn is_pinned(v: &Viewer, id: &str) -> bool {
    if let Some((p, s)) = &v.accepted
        && s.composed_solution().is_some()
    {
        let contains = |constraints: &ComposedConstraints| {
            constraints.pins.values().any(|pins| pins.contains_key(id))
                || constraints.anchors.iter().any(|pin| pin.object_id() == id)
        };
        return v
            .composed_constraints
            .get(&(p.lens, p.root.clone()))
            .is_some_and(contains)
            || (s.composed_constraints().as_ref().is_some_and(contains)
                && !s.owned_constraints().as_ref().is_some_and(contains));
    }
    v.view.pins.contains_key(id)
}
fn inherited_pin(v: &Viewer, id: &str) -> bool {
    v.accepted.as_ref().is_some_and(|(_, s)| {
        s.composed_solution().is_some()
            && is_pinned(v, id)
            && s.owned_constraints().is_some_and(|owned| {
                !owned.pins.values().any(|pins| pins.contains_key(id))
                    && !owned.anchors.iter().any(|pin| pin.object_id() == id)
            })
    })
}
fn pin_origin(v: &Viewer, id: &str) -> Option<String> {
    let (p, accepted) = v.accepted.as_ref()?;
    let effective = accepted.composed_constraints()?;
    // Prefer local ownership among records that actually contribute this pin.
    // A same-ID group/reference override may be retained but inapplicable here.
    v.composed_constraints
        .get(&(p.lens, p.root.clone()))
        .into_iter()
        .chain(v.composed_constraints.values())
        .find(|set| {
            set.context.model_id == effective.context.model_id
                && set.context.lens == effective.context.lens
                && set.context.policy == effective.context.policy
                && set.pins.iter().any(|(owner, pins)| {
                    pins.get(id).is_some_and(|point| {
                        effective.pins.get(owner).and_then(|pins| pins.get(id)) == Some(point)
                            && (set.context.root == p.root
                                || set.ancestry(id).is_some_and(|path| {
                                    let indices = (
                                        path.iter().position(|item| item == owner),
                                        path.iter().position(|item| item == &p.root),
                                        path.iter().position(|item| item == &set.context.root),
                                    );
                                    effective.ancestry(id) == Some(path)
                                        && matches!(indices, (Some(owner), Some(target), Some(source))
                                            if owner <= target.min(source))
                                }))
                    })
                })
        })
        .map(|set| set.context.root.clone())
}
fn pin_origin_message(v: &Viewer, id: &str) -> String {
    let origin = pin_origin(v, id).unwrap_or_else(|| "its original region".into());
    format!(
        "This pin belongs to {origin}. Press Shift+P to open that region and change or remove it. Its position stays fixed within that region."
    )
}
fn clear_preview(v: &mut Viewer) {
    v.dirty |= v.preview;
    v.proposal = None;
    v.preview = false;
}
fn can_reorganize(v: &Viewer) -> bool {
    v.accepted.is_some() && v.job.is_none()
}
fn same_view(a: &NativeLayout, b: &NativeLayout) -> bool {
    a.model_id == b.model_id
        && a.revision == b.revision
        && a.version == b.version
        && a.projection_key == b.projection_key
}
fn displayed_solution(v: &Viewer) -> Option<&NativeLayout> {
    let (_, accepted) = v.accepted.as_ref()?;
    Some(if v.preview {
        v.proposal
            .as_ref()
            .filter(|p| same_view(p, accepted))
            .unwrap_or(accepted)
    } else {
        accepted
    })
}
fn candidate_origin(v: &Viewer, p: &DesktopProjection, s: &NativeLayout) -> Vec3 {
    let key = (p.model_id.clone(), format!("{:?}|{}", p.lens, p.root));
    v.origins
        .get(&key)
        .copied()
        .unwrap_or_else(|| s.geometry.center())
}
fn select_origin(v: &mut Viewer, p: &DesktopProjection, s: &NativeLayout) {
    let key = (p.model_id.clone(), format!("{:?}|{}", p.lens, p.root));
    let center = s.geometry.center();
    v.origin = *v.origins.entry(key.clone()).or_insert(center);
    v.origin_order.retain(|k| k != &key);
    v.origin_order.push_back(key);
    while v.origin_order.len() > 16 {
        if let Some(key) = v.origin_order.pop_front() {
            v.origins.remove(&key);
        }
    }
}
fn remember_focus_label(viewer: &mut Viewer, projection: &DesktopProjection) {
    let Some(label) = projection.labels.get(&projection.root) else {
        return;
    };
    let key = (projection.model_id.clone(), projection.root.clone());
    viewer.focus_labels.insert(key.clone(), label.label.clone());
    viewer.focus_label_order.retain(|old| old != &key);
    viewer.focus_label_order.push_back(key);
    while viewer.focus_label_order.len() > 64 {
        if let Some(key) = viewer.focus_label_order.pop_front() {
            viewer.focus_labels.remove(&key);
        }
    }
}
fn visible_id(v: &Viewer, p: &DesktopProjection, id: &str) -> bool {
    if !v.isolate {
        return true;
    }
    if let Some(selected) = &v.selected_relation {
        return p
            .edges
            .iter()
            .filter(|edge| edge.semantic_ids.contains(selected))
            .any(|edge| {
                id == edge.source
                    || id == edge.target
                    || display_ancestor(p, id, &edge.source)
                    || display_ancestor(p, id, &edge.target)
            });
    }
    v.selected
        .as_ref()
        .is_none_or(|f| isolation_shows(p, f, id))
}
/// `id` is the selected box or one of its displayed descendants.
fn within_selection(p: &DesktopProjection, selected: &str, id: &str) -> bool {
    id == selected || display_ancestor(p, selected, id)
}
/// Isolating a box shows it, everything displayed inside it, and the boxes its
/// connectors reach. For an opened box those connectors include its
/// descendants', and the far end's ancestors stay so no line ends in a hidden
/// container (not the focused root, nor one that also holds the selection).
/// A box with nothing displayed inside it isolates as a plain box.
fn isolation_shows(p: &DesktopProjection, selected: &str, id: &str) -> bool {
    if within_selection(p, selected, id) {
        return true;
    }
    let opened = p
        .nodes
        .iter()
        .any(|node| node.parent.as_deref() == Some(selected));
    p.edges.iter().any(|e| {
        let other = if within_selection(p, selected, &e.source) {
            &e.target
        } else if within_selection(p, selected, &e.target) {
            &e.source
        } else {
            return false;
        };
        other == id
            || (opened
                && id != p.root
                && !display_ancestor(p, id, selected)
                && display_ancestor(p, id, other))
    })
}
fn display_ancestor(p: &DesktopProjection, ancestor: &str, id: &str) -> bool {
    let mut current = id;
    for _ in 0..p.nodes.len() {
        let Some(parent) = p
            .nodes
            .iter()
            .find(|node| node.id == current)
            .and_then(|node| node.parent.as_deref())
        else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        current = parent;
    }
    false
}
fn visible_edge(v: &Viewer, p: &DesktopProjection, edge: &layout::Edge) -> bool {
    !v.isolate
        || if let Some(id) = &v.selected_relation {
            edge.semantic_ids.contains(id)
        } else {
            v.selected.as_ref().is_none_or(|id| {
                within_selection(p, id, &edge.source) || within_selection(p, id, &edge.target)
            })
        }
}
/// Makes a drawn connector read as the selected path: its private materials
/// become the selection colour. Arrow/dash geometry is unchanged.
const DIMMED_ALPHA: f32 = 0.14;

/// Display IDs a spotlight lights, plus the containers that frame them.
struct Spotlit {
    focus: BTreeSet<String>,
    frames: BTreeSet<String>,
    relations: BTreeSet<String>,
    context: SpotContext,
}

fn spotlit(
    spotlight: &Spotlight,
    p: &DesktopProjection,
    parents: &BTreeMap<&str, &str>,
) -> Spotlit {
    let mut focus = BTreeSet::new();
    for id in &spotlight.lit {
        let display = if p.nodes.iter().any(|n| n.id == *id) {
            Some(id.clone())
        } else {
            p.semantic_to_display.get(id).cloned()
        };
        focus.extend(display);
    }
    let mut frames = BTreeSet::new();
    for id in &focus {
        let mut at = id.as_str();
        while let Some(parent) = parents.get(at) {
            if !frames.insert((*parent).to_owned()) {
                break;
            }
            at = parent;
        }
    }
    Spotlit {
        focus,
        frames,
        relations: spotlight.relations.clone(),
        context: spotlight.context,
    }
}

/// Fade every standard material under a dimmed root, after it is spawned.
fn dim_context(
    added: Query<Entity, Added<Dimmed>>,
    children: Query<&Children>,
    handles: Query<&MeshMaterial3d<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for root in &added {
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            let Ok(handle) = handles.get(entity) else {
                continue;
            };
            if let Some(material) = materials.get_mut(&handle.0)
                && material.alpha_mode != AlphaMode::Blend
            {
                material.base_color = material.base_color.with_alpha(DIMMED_ALPHA);
                material.emissive *= 0.15;
                material.alpha_mode = AlphaMode::Blend;
            }
        }
    }
}

fn highlight_path(commands: &mut Commands, root: Entity) {
    commands.queue(move |world: &mut World| {
        let handles: Vec<_> = world
            .get::<Children>(root)
            .into_iter()
            .flat_map(|children| children.iter())
            .filter_map(|child| {
                world
                    .get::<MeshMaterial3d<StandardMaterial>>(child)
                    .map(|material| material.0.clone())
            })
            .collect();
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        for handle in handles {
            if let Some(material) = materials.get_mut(&handle) {
                material.base_color = style::PATH_HIGHLIGHT;
                material.emissive = LinearRgba::rgb(0.15, 0.09, 0.02);
            }
        }
    });
}
fn select_relation(viewer: &mut Viewer, id: &str) -> Result<(), String> {
    let (p, _) = viewer.accepted.as_ref().ok_or("No accepted assembly yet")?;
    let edge = p.edges.iter().find(|edge| edge.semantic_ids.iter().any(|member| member == id))
        .filter(|_| p.claims.contains_key(id))
        .ok_or("This relationship has no loaded visible path; enter its region or change relationship page")?;
    if viewer
        .selected
        .as_ref()
        .is_none_or(|selected| *selected != edge.source && *selected != edge.target)
    {
        viewer.selected = Some(edge.source.clone());
    }
    viewer.selected_relation = Some(id.into());
    viewer.relation_page = paged_claims(viewer)
        .iter()
        .position(|claim| claim.as_str() == id)
        .unwrap_or(0)
        / CLAIMS_PER_PAGE;
    viewer.message = format!(
        "Selected relationship {id}. J / K changes path; X expands both ends of its path; I isolates it; Shift+I clears path selection."
    );
    viewer.dirty = true;
    Ok(())
}
fn retain_valid_relation_selection(viewer: &mut Viewer) {
    if viewer.selected_relation.as_ref().is_some_and(|id| {
        viewer.accepted.as_ref().is_none_or(|(p, _)| {
            !p.claims.contains_key(id)
                || !p.edges.iter().any(|edge| {
                    edge.semantic_ids.contains(id)
                        && viewer.selected.as_ref().is_some_and(|selected| {
                            *selected == edge.source || *selected == edge.target
                        })
                })
        })
    }) {
        viewer.selected_relation = None;
        viewer.dirty = true;
    }
}
/// Largest cursor travel between press and release that still counts as a
/// click (logical pixels); anything further is a drag and only orbits.
const CLICK_DRAG_THRESHOLD: f32 = 5.0;
/// Longest gap between two clicks on one box that makes them a double click.
const DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(400);

/// What a completed left-button press and release means for the scene.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Click {
    /// A drag, or a press that began outside the scene: no selection change.
    None,
    /// A click on empty space.
    Clear,
    Single(String),
    Double(String),
}
#[derive(Debug, Clone, PartialEq)]
struct LastClick {
    id: String,
    at: Duration,
}
#[derive(Debug, Clone, Default)]
struct ClickState {
    /// Cursor position when the left button went down, unless that was over
    /// Details.
    press: Option<Vec2>,
    last: Option<LastClick>,
}
/// Decides what a left press/release pair means. `hit` is the box under the
/// release position, `last` the previous click that could pair into a double
/// click, and `now` the time of this release.
fn classify_click(
    press: Option<Vec2>,
    release: Vec2,
    hit: Option<&str>,
    last: &mut Option<LastClick>,
    now: Duration,
) -> Click {
    let Some(press) = press else {
        return Click::None;
    };
    if press.distance(release) >= CLICK_DRAG_THRESHOLD {
        *last = None;
        return Click::None;
    }
    let Some(id) = hit else {
        *last = None;
        return Click::Clear;
    };
    let double = last
        .as_ref()
        .is_some_and(|l| l.id == id && now.saturating_sub(l.at) <= DOUBLE_CLICK_WINDOW);
    if double {
        *last = None;
        Click::Double(id.to_owned())
    } else {
        *last = Some(LastClick {
            id: id.to_owned(),
            at: now,
        });
        Click::Single(id.to_owned())
    }
}
/// Applies a click to the viewer. A double click returns the Enter action the
/// Enter key would submit; its first click already isolated the box.
fn apply_click(v: &mut Viewer, click: Click) -> Option<ViewerAction> {
    let double = matches!(click, Click::Double(_));
    match click {
        Click::None => None,
        Click::Clear => {
            let changed = v.isolate || v.selected.is_some() || v.selected_relation.is_some();
            v.isolate = false;
            v.selected = None;
            v.selected_relation = None;
            v.relation_page = 0;
            v.dirty |= changed;
            None
        }
        Click::Single(id) | Click::Double(id) => {
            let changed =
                !v.isolate || v.selected.as_ref() != Some(&id) || v.selected_relation.is_some();
            v.isolate = true;
            v.selected = Some(id.clone());
            v.selected_relation = None;
            v.relation_page = 0;
            v.dirty |= changed;
            double.then_some(ViewerAction::Enter(id))
        }
    }
}
/// The box under the cursor, if any.
fn pick_at(
    v: &Viewer,
    camera: &Camera,
    transform: &GlobalTransform,
    cursor: Vec2,
) -> Option<String> {
    let (p, _) = v.accepted.as_ref()?;
    let s = displayed_solution(v)?;
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    pick_box(v, p, s, ray.origin, *ray.direction)
}
/// Solid boxes win, nearest first. When the ray hits none, the innermost
/// (smallest) visible frame it passes through is picked, so boxes opened
/// automatically or with E can be selected and entered too. The focused root
/// is never picked.
fn pick_box(
    v: &Viewer,
    p: &DesktopProjection,
    s: &NativeLayout,
    origin: Vec3,
    direction: Vec3,
) -> Option<String> {
    let hits = |solid: bool| {
        s.geometry
            .boxes
            .iter()
            .filter(move |(id, b)| {
                b.solid == solid && (solid || **id != p.root) && visible_id(v, p, id)
            })
            .filter_map(|(id, b)| {
                let lo = s.geometry.display(b.min, v.explode, v.origin);
                let hi = s.geometry.display(b.max, v.explode, v.origin);
                let size = hi - lo;
                ray_box(origin, direction, lo, hi)
                    .map(|d| (d, size.x * size.y * size.z, id.clone()))
            })
    };
    hits(true)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)))
        .or_else(|| {
            hits(false).min_by(|a, b| {
                a.1.total_cmp(&b.1)
                    .then(a.0.total_cmp(&b.0))
                    .then(a.2.cmp(&b.2))
            })
        })
        .map(|(_, _, id)| id)
}
fn ray_box(origin: Vec3, direction: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut near = 0.0_f32;
    let mut far = f32::INFINITY;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-7 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if far < near {
                return None;
            }
        }
    }
    Some(near)
}
fn visible_pins(view: &ViewState, p: &ViewProjection) -> BTreeMap<String, Point> {
    view.pins
        .iter()
        .filter(|(id, _)| p.nodes.iter().any(|n| n.id == **id))
        .map(|(id, point)| (id.clone(), *point))
        .collect()
}
fn selected_claims(viewer: &Viewer) -> Vec<&String> {
    let Some((projection, _)) = &viewer.accepted else {
        return vec![];
    };
    let Some(selected) = &viewer.selected else {
        return vec![];
    };
    projection
        .edges
        .iter()
        .filter(|edge| edge.source == *selected || edge.target == *selected)
        .flat_map(|edge| &edge.semantic_ids)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}
fn claim_text(claim: &crate::layout_input::DisplayClaim) -> String {
    format!(
        "{}: {} --{}--> {}{} | {:?} | evidence {:?} | aspect {:?} | {}",
        claim.id,
        claim.source,
        claim.kind_label,
        claim.target,
        claim
            .label
            .as_ref()
            .map_or(String::new(), |label| format!(" ({label})")),
        claim.basis,
        claim.evidence_ids,
        claim.aspect,
        claim.rationale.as_deref().unwrap_or("")
    )
}
/// Claims shown per Details page and stepped by B / N.
const CLAIMS_PER_PAGE: usize = 3;
/// The aggregated connector the selected relationship is drawn on, if loaded.
fn selected_path_edge(viewer: &Viewer) -> Option<&layout::Edge> {
    let id = viewer.selected_relation.as_ref()?;
    let (p, _) = viewer.accepted.as_ref()?;
    p.edges.iter().find(|edge| edge.semantic_ids.contains(id))
}
/// What B / N browse: the members of the selected path when one is selected
/// (so an aggregated line can be decomposed), otherwise the selected
/// component's linked claims.
fn paged_claims(viewer: &Viewer) -> Vec<&String> {
    selected_path_edge(viewer).map_or_else(
        || selected_claims(viewer),
        |edge| edge.semantic_ids.iter().collect(),
    )
}
/// Why expanding the displayed endpoint `id` cannot reveal anything, or `None`
/// when it is a collapsed container whose supplied children are hidden in it.
fn endpoint_expansion_blocker(p: &DesktopProjection, id: &str) -> Option<&'static str> {
    if p.references.contains_key(id) || id.starts_with("view:reference:") {
        return Some("is a reference to an object outside this view; Enter it to see its interior");
    }
    if !p.nodes.iter().any(|node| node.id == id) {
        return Some("is not in the current view");
    }
    // A collapsed container maps its hidden supplied descendants onto itself;
    // an expanded one, or a leaf box, has nothing besides its own identity.
    let hides_children = p
        .semantic_to_display
        .iter()
        .any(|(semantic, display)| display == id && semantic != id);
    (!hides_children)
        .then_some("has no collapsed supplied children (a leaf box or already expanded)")
}
fn path_expansion_message(p: &DesktopProjection, expansion: &PathExpansion) -> String {
    let lines = p
        .edges
        .iter()
        .filter(|edge| {
            edge.semantic_ids
                .iter()
                .any(|id| expansion.members.contains(id))
        })
        .count();
    let both = expansion.skipped.is_empty();
    let mut message = if both {
        "Expanded both ends of the selected path".to_owned()
    } else {
        format!("Expanded {}", expansion.expanded.join(" and "))
    };
    message.push_str(&if lines == 0 {
        format!(
            "; its {} relationships are no longer drawn as lines in this view",
            expansion.members.len()
        )
    } else {
        format!(
            "; its {} relationships are now shown as {} line{}",
            expansion.members.len(),
            lines,
            if lines == 1 { "" } else { "s" }
        )
    });
    if !both {
        message.push_str(&format!(
            " (not expanded: {})",
            expansion.skipped.join("; ")
        ));
    }
    message.push('.');
    message
}
/// Details text for a selected path: how many relationships its line carries
/// and one page of them with their original predicates and endpoints.
fn path_details(viewer: &Viewer, p: &DesktopProjection) -> Option<String> {
    let selected = p.claims.get(viewer.selected_relation.as_ref()?)?;
    let edge = selected_path_edge(viewer)?;
    let count = edge.semantic_ids.len();
    let mut text = format!(
        "Selected path: {}\nThis line carries {} relationship{}. X expands both ends so they become separate lines; J / K selects a claim; I isolates; Shift+I clears.\n",
        claim_text(selected),
        count,
        if count == 1 { "" } else { "s" }
    );
    if count > 1 {
        let pages = count.div_ceil(CLAIMS_PER_PAGE);
        text.push_str(&format!(
            "Relationships on this line (page {} of {}, B / N to browse):\n",
            viewer.relation_page.min(pages - 1) + 1,
            pages
        ));
        for id in edge
            .semantic_ids
            .iter()
            .skip(viewer.relation_page * CLAIMS_PER_PAGE)
            .take(CLAIMS_PER_PAGE)
        {
            match p.claims.get(id) {
                Some(claim) => text.push_str(&format!(
                    "{} {}\n",
                    if *id == selected.id { ">" } else { "-" },
                    claim_text(claim)
                )),
                None => text.push_str(&format!("- {id}: details not loaded in this view\n")),
            }
        }
    }
    Some(text)
}
#[cfg(test)]
fn vec(p: Point) -> Vec3 {
    Vec3::new(p.x as f32, p.y as f32, p.z as f32) / layout::UNITS_PER_SCENE_UNIT
}
fn kind(k: NodeKind) -> EntityKind {
    match k {
        NodeKind::Scope | NodeKind::Group => EntityKind::Scope,
        NodeKind::Client => EntityKind::Client,
        NodeKind::Service => EntityKind::Service,
        NodeKind::Datastore => EntityKind::Datastore,
        NodeKind::Interface => EntityKind::Interface,
        NodeKind::Concern => EntityKind::Concern,
        NodeKind::Deployment => EntityKind::Deployment,
    }
}
fn object_color(original_kind: Option<&str>, fallback: EntityKind) -> Color {
    let rgb = match original_kind {
        Some("code repository" | "code_repository") => [110, 145, 174],
        Some("code package" | "code_package") => [104, 148, 220],
        Some("code file" | "code_file") => [65, 174, 164],
        Some("code type" | "code_type") => [190, 134, 218],
        Some("code function" | "code_function") => [232, 170, 88],
        Some("visual_group" | "visual group") => [180, 155, 88],
        _ => return primitives::color(fallback),
    };
    Color::srgb_u8(rgb[0], rgb[1], rgb[2])
}
fn proposal_identity(p: &DesktopProjection, s: &NativeLayout) -> DismissedProposal {
    if let Some(flat) = s.flat() {
        return flat_proposal_identity(p, flat);
    }
    let mut signature = [0xcbf29ce484222325u64, 0x84222325cbf29ce4u64];
    let mut field = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            for lane in &mut signature {
                *lane = (*lane ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
        }
    };
    field(b"visualyze-composed-dismissal-v1");
    let mut nodes: Vec<_> = p.nodes.iter().collect();
    nodes.sort_by_key(|node| &node.id);
    for node in nodes {
        let b = &s.geometry.boxes[&node.id];
        field(node.id.as_bytes());
        field(node.parent.as_deref().unwrap_or("").as_bytes());
        field(format!("{:?}:{}:{}", node.kind, node.band, b.solid).as_bytes());
        for point in [b.min, b.max] {
            for coordinate in point.to_array() {
                field(&coordinate.to_bits().to_le_bytes());
            }
        }
        let mut members = node.semantic_ids.clone();
        members.sort();
        for member in members {
            field(member.as_bytes());
        }
    }
    let mut edges: Vec<_> = p.edges.iter().collect();
    edges.sort_by_key(|edge| &edge.id);
    for edge in edges {
        field(edge.id.as_bytes());
        field(edge.source.as_bytes());
        field(edge.target.as_bytes());
        let mut members = edge.semantic_ids.clone();
        members.sort();
        for id in members {
            field(id.as_bytes());
            if let Some(claim) = p.claims.get(&id) {
                field(
                    format!(
                        "{}:{}:{}:{:?}:{:?}:{:?}:{:?}",
                        claim.source,
                        claim.target,
                        claim.kind_label,
                        claim.aspect,
                        claim.basis,
                        claim.label,
                        claim.rationale
                    )
                    .as_bytes(),
                );
                let mut evidence = claim.evidence_ids.clone();
                evidence.sort();
                for id in evidence {
                    field(id.as_bytes());
                }
            }
        }
    }
    DismissedProposal {
        encoding_version: DISMISSAL_ENCODING_VERSION,
        layout_version: s.version,
        model_id: s.model_id.clone(),
        root: p.root.clone(),
        lens: p.lens,
        signature,
    }
}
fn flat_proposal_identity(p: &DesktopProjection, s: &LayoutSolution) -> DismissedProposal {
    // Two stable FNV lanes with explicit length framing. This is compact local
    // prompt suppression, not a security hash or a substitute for validation.
    // Do not include revision/quality score or route points: an identical box
    // arrangement and original claims remain dismissed after a harmless reload
    // or reroute. Change the encoding version whenever this field set changes.
    let mut signature = [0xcbf29ce484222325u64, 0x84222325cbf29ce4u64];
    let mut field = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            for lane in &mut signature {
                *lane = (*lane ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
            }
        }
    };
    field(b"visualyze-dismissal-v1");
    let nodes: BTreeMap<_, _> = p
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    for (id, object) in &s.boxes {
        field(b"box");
        field(id.as_bytes());
        field(object.parent.as_deref().unwrap_or("").as_bytes());
        field(&[u8::from(object.solid)]);
        for point in [object.bounds.min, object.bounds.max] {
            for coordinate in [point.x, point.y, point.z] {
                field(&coordinate.to_le_bytes());
            }
        }
        if let Some(node) = nodes.get(id.as_str()) {
            field(format!("{:?}", node.kind).as_bytes());
            field(&node.band.to_le_bytes());
        }
        let mut members: Vec<_> = object.semantic_ids.iter().collect();
        members.sort();
        for member in members {
            field(b"object-member");
            field(member.as_bytes());
        }
    }
    let mut routes: Vec<_> = s.routes.iter().collect();
    routes.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
    for route in routes {
        field(b"route-membership");
        field(route.edge_id.as_bytes());
        let mut members: Vec<_> = route.semantic_ids.iter().collect();
        members.sort();
        for member in members {
            field(b"claim");
            field(member.as_bytes());
            if let Some(claim) = p.claims.get(member) {
                field(claim.source.as_bytes());
                field(claim.target.as_bytes());
                field(claim.kind_label.as_bytes());
                field(format!("{:?}:{:?}", claim.aspect, claim.basis).as_bytes());
                field(claim.label.as_deref().unwrap_or("").as_bytes());
                field(claim.rationale.as_deref().unwrap_or("").as_bytes());
                let mut evidence: Vec<_> = claim.evidence_ids.iter().collect();
                evidence.sort();
                for id in evidence {
                    field(b"evidence");
                    field(id.as_bytes());
                }
            }
        }
    }
    DismissedProposal {
        encoding_version: DISMISSAL_ENCODING_VERSION,
        layout_version: s.version,
        model_id: s.model_id.clone(),
        root: p.root.clone(),
        lens: p.lens,
        signature,
    }
}
// A monotone piecewise translation opens empty horizontal gaps. Within each
// solid object's vertical interval the translation is constant, preserving its
// dimensions and orthogonal route attachments.
#[cfg(test)]
fn inspection_offset(y: f32, solution: &LayoutSolution, explode: bool) -> f32 {
    if !explode {
        return 0.0;
    }
    let mut intervals: Vec<_> = solution
        .boxes
        .values()
        .filter(|b| b.solid)
        .map(|b| (b.bounds.min.y as f32 / 10.0, b.bounds.max.y as f32 / 10.0))
        .collect();
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f32, f32)> = vec![];
    for (a, b) in intervals {
        if let Some(last) = merged.last_mut()
            && a <= last.1
        {
            last.1 = last.1.max(b);
        } else {
            merged.push((a, b));
        }
    }
    let mut shift = 0.0;
    for pair in merged.windows(2) {
        let lo = pair[0].1;
        let hi = pair[1].0;
        if y >= hi {
            shift += 1.5;
        } else if y > lo {
            shift += 1.5 * (y - lo) / (hi - lo);
        }
    }
    shift
}
#[cfg(test)]
fn display_point(p: Point, s: &LayoutSolution, explode: bool, center: Vec3) -> Vec3 {
    let mut p = vec(p);
    p.y += inspection_offset(p.y, s, explode);
    p - center
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn poll(
    mut commands: Commands,
    mut viewer: ResMut<Viewer>,
    mut model: ResMut<LayoutInput>,
    engine: Res<LayoutEngine>,
    mut status: ResMut<LayoutStatus>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut glasses: ResMut<Assets<GlassMaterial>>,
    // Absent in headless scenes, which draw no icons.
    mut images: Option<ResMut<Assets<Image>>>,
    mut icons: Option<ResMut<crate::icons::IconAtlas>>,
    owned: Query<
        (
            Entity,
            Option<&PlacedObject>,
            Option<&Transform>,
            Has<Label>,
            Has<Ground>,
        ),
        (With<LayoutOwned>, Without<Camera3d>),
    >,
    // Absent in headless scenes that play no transitions.
    mut transition: Option<ResMut<SceneTransition>>,
    mut camera: Query<(&mut OrbitCamera, &mut Transform), With<Camera3d>>,
    mut text: Query<&mut Text, With<LayoutText>>,
    mut inspector: Query<&mut Text, (With<Inspector>, Without<LayoutText>)>,
) {
    for timing in viewer.retired_timings.drain(..) {
        record_timing(&mut status, timing);
    }
    let mut completed_timing = None;
    let result = viewer
        .job
        .as_ref()
        .and_then(|j| match j.receive.lock().unwrap().try_recv() {
            Ok(r) => Some(r),
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(layout::LayoutError::Invalid(
                "layout worker stopped".into(),
            ))),
            Err(mpsc::TryRecvError::Empty) => None,
        });
    if let Some(result) = result {
        let acceptance_started = Instant::now();
        let job = viewer.job.take().unwrap();
        if viewer.accepted.is_none()
            && let Some(dismissed) = job.restored_dismissals.lock().unwrap().take()
        {
            viewer.dismissed = dismissed;
        }
        if viewer.accepted.is_none()
            && let Some(sets) = job.restored_composed.lock().unwrap().take()
        {
            for constraints in sets {
                viewer.composed_constraints.insert(
                    (constraints.context.lens, constraints.context.root.clone()),
                    constraints,
                );
            }
        }
        // One cut partitions publication wait from acceptance. Restoring local
        // metadata and acquiring this mutex after the cut belong to acceptance
        // only, even when taking the snapshot itself is delayed.
        let mut timing =
            timing_snapshot_at(&job.timing.lock().unwrap(), "failed", acceptance_started);
        let queued = job.timing.lock().unwrap().queued;
        let expansion = viewer.pending_expansion.take();
        let path_expansion = viewer.pending_path_expansion.take();
        viewer.restore_attempted = true;
        let mut refine_reloaded = false;
        match result {
            Ok((projection, mut s, mut view)) => {
                let origin = candidate_origin(&viewer, &projection, &s);
                let desired = if *engine == LayoutEngine::Composed {
                    requested_composed_constraints(
                        &projection,
                        view.policy,
                        &viewer.composed_constraints,
                    )
                    .map(Some)
                } else {
                    Ok(None)
                };
                let validation = desired
                    .and_then(|desired| {
                        s.validate_constraints(
                            &projection,
                            &visible_pins(&view, &projection),
                            desired.as_ref(),
                        )
                    })
                    .and_then(|()| s.geometry.validate_float_geometry(viewer.explode, origin));
                match validation {
                    Ok(()) => {
                        timing.revision = Some(projection.revision);
                        status.error = None;
                        if job.reorganize {
                            timing.outcome = "no_proposal";
                            s.refresh_metrics(&projection);
                            if let Some((_, old)) = &viewer.accepted
                                && same_view(&s, old)
                            {
                                let improvement =
                                    1.0 - s.metrics.structural_cost / old.metrics.structural_cost;
                                if improvement >= 0.15
                                    && !viewer
                                        .dismissed
                                        .contains(&proposal_identity(&projection, &s))
                                    && s.metrics.occlusion <= old.metrics.occlusion + 0.05
                                    && s.metrics.aspect_ratio
                                        <= old.metrics.aspect_ratio.max(3.0) * 1.1
                                    && s.metrics.congestion <= old.metrics.congestion * 1.1 + 0.1
                                    && s.preserves_interior_scale(old)
                                {
                                    viewer.message = format!(
                                        "Reorganization available: {:.0}% lower quality cost; length {} to {}, bends {} to {}; congestion {:.2} to {:.2}, crossings {} to {}; space {:.0} to {:.0}; {} objects move. V previews, A applies.",
                                        improvement * 100.0,
                                        old.metrics.route_length,
                                        s.metrics.route_length,
                                        old.metrics.bends,
                                        s.metrics.bends,
                                        old.metrics.congestion,
                                        s.metrics.congestion,
                                        old.metrics.crossing_points,
                                        s.metrics.crossing_points,
                                        old.metrics.volume,
                                        s.metrics.volume,
                                        s.geometry
                                            .boxes
                                            .iter()
                                            .filter(|(id, b)| old
                                                .geometry
                                                .boxes
                                                .get(*id)
                                                .is_none_or(|o| o.min + o.max != b.min + b.max))
                                            .count()
                                    );
                                    viewer.proposal = Some(s);
                                    timing.outcome = "proposal";
                                } else {
                                    viewer.message="No new arrangement meets the 15% improvement and readability threshold.".into();
                                }
                            } else {
                                viewer.message =
                                    "Discarded a reorganization for a different view.".into();
                            }
                        } else {
                            timing.outcome = "accepted";
                            viewer.pending_transition = viewer.job_transition.take();
                            if let Some(cache) = job.canonical_cache.lock().unwrap().take() {
                                viewer.canonical_cache = Arc::new(cache);
                            }
                            if let Some(replacement) = &job.replacement_input {
                                *model = replacement.clone();
                                viewer.cache.clear();
                                viewer.cache_order.clear();
                                viewer.undo.clear();
                                viewer.focus_labels.clear();
                                viewer.focus_label_order.clear();
                                refine_reloaded = true;
                            }
                            viewer.message = format!(
                                "{} objects | {} visible relationships | revision {} | {:?} policy",
                                s.geometry.boxes.len(),
                                s.geometry.routes.len(),
                                s.revision,
                                viewer.view.policy
                            );
                            cache_layout(&mut viewer, projection.clone(), s.clone());
                            if job.replacement_input.is_none() {
                                remember(&mut viewer);
                            }
                            viewer.fit_requested =
                                viewer.accepted.as_ref().is_none_or(|(old, _)| {
                                    old.model_id != projection.model_id
                                        || old.root != projection.root
                                        || old.lens != projection.lens
                                });
                            if viewer.fit_requested {
                                viewer.group_marks.clear();
                            }
                            select_origin(&mut viewer, &projection, &s);
                            remember_focus_label(&mut viewer, &projection);
                            view.refresh_group_definitions(&projection.group_catalog);
                            view.remember_groups(&projection);
                            viewer.view = view.clone();
                            viewer.accepted_view = view;
                            viewer.save_needed = true;
                            viewer.accepted = Some((projection.clone(), s));
                            restore_accepted_constraints(&mut viewer);
                            if viewer.selected.as_ref().is_some_and(|id| {
                                !projection.nodes.iter().any(|node| node.id == *id)
                            }) {
                                viewer.selected = None;
                            }
                            retain_valid_relation_selection(&mut viewer);
                            viewer.relation_page = viewer.relation_page.min(
                                paged_claims(&viewer).len().saturating_sub(1) / CLAIMS_PER_PAGE,
                            );
                            if let Some(expansion) = &path_expansion {
                                viewer.message = path_expansion_message(&projection, expansion);
                            }
                            viewer.dirty = true;
                        }
                    }
                    Err(error) => {
                        timing.outcome = "discarded";
                        if !job.reorganize {
                            abandon_pending(&mut viewer);
                        }
                        viewer.message = format!(
                            "Discarded a stale or invalid layout result; accepted arrangement preserved: {}",
                            error
                        );
                        if !job.reorganize || job.action_id.is_some() {
                            status.error = Some(viewer.message.clone());
                        }
                    }
                }
            }
            Err(e) if job.reorganize => {
                // A failed optional search must not roll back the accepted view
                // or its live selection. Explicit requests still get an error.
                viewer.message =
                    format!("Accepted arrangement preserved; reorganization search: {e}");
                if job.action_id.is_some() {
                    status.error = Some(e.to_string());
                }
            }
            Err(e) => {
                abandon_pending(&mut viewer);
                if viewer.accepted.is_none()
                    && let Some(view) = job.restored_view.lock().unwrap().take()
                {
                    viewer.view = view.clone();
                    viewer.accepted_view = view;
                }
                viewer.message = if let Some(id) = expansion {
                    let next = if viewer.selected.as_deref() == Some(&id) {
                        "Press Enter to inspect the selected component separately"
                    } else {
                        "Select the component and press Enter to inspect it separately"
                    };
                    format!(
                        "Could not expand {id} while keeping the current arrangement and constraints. Your previous view is unchanged. {next}, or adjust its pins/layer and try again.\nTechnical detail: {e}"
                    )
                } else {
                    e.to_string()
                };
                status.error = Some(viewer.message.clone());
            }
        }
        let error = status.error.clone();
        finish_ticket(&mut status, job.action_id, error);
        timing.acceptance_ms = acceptance_started.elapsed().as_secs_f64() * 1000.0;
        completed_timing = Some((timing, queued));
        if refine_reloaded {
            // The reload is accepted and its ticket is complete before optional
            // refinement begins. Only this publication schedules it; neither a
            // proposal, its dismissal, application nor an idle frame repeats it.
            start(&model, *engine, &mut viewer, true);
        }
    }
    let scene_started = Instant::now();
    if viewer.dirty {
        viewer.dirty = false;
        let choreography = viewer.pending_transition.take();
        if let Some((p, _)) = &viewer.accepted {
            let s = displayed_solution(&viewer).unwrap();
            // With a transition, outgoing objects and connectors stay to play their
            // part; everything else is replaced at once.
            let staging = match (choreography, transition.as_deref_mut()) {
                (Some(c), Some(t)) => Some((t.begin(c), c)),
                (_, Some(t)) => {
                    t.cut();
                    None
                }
                _ => None,
            };
            let mut outgoing: BTreeMap<String, (Entity, Vec3, Vec3)> = BTreeMap::new();
            let mut leaving: BTreeMap<Entity, Vec3> = BTreeMap::new();
            for (entity, object, transform, label, ground) in &owned {
                match (staging, object, transform) {
                    (Some(_), Some(object), Some(transform)) if !label => {
                        let at = transform.translation;
                        for key in std::iter::once(&object.id).chain(&object.semantic_ids) {
                            outgoing
                                .entry(key.clone())
                                .or_insert((entity, at, object.half));
                        }
                        leaving.insert(entity, at);
                        commands
                            .entity(entity)
                            .remove::<(LayoutOwned, PlacedObject)>();
                    }
                    (Some((generation, c)), None, Some(transform)) if !label && !ground => {
                        commands
                            .entity(entity)
                            .remove::<(LayoutOwned, PlacedRelation)>()
                            .insert(Track::fade(
                                generation,
                                c.connectors_out,
                                transform.translation,
                                (1.0, 0.0),
                            ));
                    }
                    _ => {
                        commands.entity(entity).despawn();
                    }
                }
            }
            let center = viewer.origin;
            let mut scene_bounds = Vec3::ONE;
            let focus = viewer.selected.as_ref();
            let parents: BTreeMap<&str, &str> = p
                .nodes
                .iter()
                .filter_map(|n| Some((n.id.as_str(), n.parent.as_deref()?)))
                .collect();
            let spot = viewer
                .spotlight
                .as_ref()
                .map(|spotlight| spotlit(spotlight, p, &parents));
            for n in &p.nodes {
                let b = &s.geometry.boxes[&n.id];
                let lo = s.geometry.display(b.min, viewer.explode, center);
                let hi = s.geometry.display(b.max, viewer.explode, center);
                let position = (lo + hi) * 0.5;
                let half = (hi - lo) * 0.5;
                scene_bounds = scene_bounds.max(position.abs() + half + Vec3::splat(1.0));
                if !visible_id(&viewer, p, &n.id) {
                    continue;
                }
                let (lit, dimmed) = match &spot {
                    None => (false, false),
                    Some(spot) => {
                        let lit = spot.focus.contains(&n.id);
                        let framed = lit || spot.frames.contains(&n.id);
                        match spot.context {
                            SpotContext::Hide if !framed && n.id != p.root => continue,
                            SpotContext::Dim => (lit, !framed && n.id != p.root),
                            _ => (lit, false),
                        }
                    }
                };
                let display = p.labels.get(&n.id);
                let k = match display.map(|d| d.original_kind.as_str()) {
                    Some("table" | "column") => EntityKind::Datastore,
                    Some(
                        "message_channel" | "message channel" | "message_type" | "message type",
                    ) => EntityKind::Interface,
                    Some("deployment_instance" | "deployment instance") => EntityKind::Deployment,
                    _ => kind(n.kind),
                };
                let basis = display.map_or(Basis::Unknown, |d| d.basis);
                let title = display.map_or_else(
                    || n.id.clone(),
                    |d| format!("{} / {}", d.kind_label, d.label),
                );
                let root = commands
                    .spawn((
                        LayoutOwned,
                        PlacedObject {
                            id: n.id.clone(),
                            semantic_ids: n.semantic_ids.clone(),
                            kind: display
                                .map_or_else(|| "unknown".into(), |d| d.original_kind.clone()),
                            basis,
                            half,
                        },
                        Transform::from_translation(position),
                        Visibility::default(),
                    ))
                    .id();
                if let Some((generation, c)) = staging {
                    // Travel from the matching outgoing object, or arrive fresh.
                    let matched = std::iter::once(&n.id)
                        .chain(&n.semantic_ids)
                        .find_map(|key| outgoing.get(key))
                        .copied()
                        .filter(|(entity, ..)| leaving.contains_key(entity));
                    let track = match matched {
                        Some((old, from, from_half)) => {
                            leaving.remove(&old);
                            commands.entity(old).despawn();
                            Track::travel(generation, c.travel, from, position)
                                .resizing(from_half, half)
                                .glowing(c.highlight)
                        }
                        None => Track::arrive(generation, c.arrive, position),
                    };
                    commands.entity(root).insert((
                        Transform::from_translation(track.from).with_scale(track.scale.0),
                        track,
                    ));
                }
                let color = object_color(display.map(|label| label.original_kind.as_str()), k);
                let marked = viewer.group_marks.contains_key(&n.id);
                if dimmed {
                    commands.entity(root).insert(Dimmed);
                }
                if b.solid {
                    let material = materials.add(StandardMaterial {
                        base_color: if dimmed {
                            color.with_alpha(DIMMED_ALPHA)
                        } else {
                            color
                        },
                        alpha_mode: if dimmed {
                            AlphaMode::Blend
                        } else {
                            AlphaMode::Opaque
                        },
                        perceptual_roughness: 0.6,
                        // Curved corners must not flash to white under the key light.
                        reflectance: 0.25,
                        emissive: if marked {
                            LinearRgba::rgb(0.25, 0.2, 0.05)
                        } else if focus == Some(&n.id) {
                            // Glow in the object's own hue so selection never washes it grey.
                            color.to_linear() * 0.3
                        } else if lit {
                            color.to_linear() * 0.18
                        } else {
                            LinearRgba::BLACK
                        },
                        ..default()
                    });
                    for part in primitives::solid(k, half) {
                        commands.spawn((
                            Mesh3d(meshes.add(part.mesh())),
                            MeshMaterial3d(material.clone()),
                            Transform::from_translation(part.position),
                            ChildOf(root),
                        ));
                    }
                    if let (Some(images), Some(icons), Some(display)) =
                        (images.as_deref_mut(), icons.as_deref_mut(), display)
                        && let Some(icon) =
                            crate::icons::icon_for(&display.technology, display.subtype.as_deref())
                    {
                        let side = crate::icons::decal_half(half) * 2.0;
                        let inlay = crate::icons::tint(color);
                        commands.spawn((
                            Mesh3d(meshes.add(Plane3d::default().mesh().size(side, side))),
                            MeshMaterial3d(materials.add(StandardMaterial {
                                base_color: if dimmed {
                                    inlay.with_alpha(DIMMED_ALPHA)
                                } else {
                                    inlay
                                },
                                base_color_texture: Some(icons.handle(icon, images)),
                                unlit: true,
                                alpha_mode: AlphaMode::Blend,
                                depth_bias: 4.0,
                                ..default()
                            })),
                            // Just proud of the top face, so it never fights the box.
                            Transform::from_translation(Vec3::Y * (half.y + 0.004)),
                            ChildOf(root),
                        ));
                    }
                } else if n.id != p.root {
                    // The focused root encloses the whole view and is named by
                    // the navigation trail; drawing its walls adds nothing.
                    let depth = nesting_depth(&parents, &p.root, &n.id);
                    let (tint, emphasis) = if marked {
                        (MARK_COLOR, 1.6)
                    } else if focus == Some(&n.id) || lit {
                        (color, 1.7)
                    } else if dimmed {
                        (color, 0.3)
                    } else {
                        (color, 1.0)
                    };
                    let (shell, tray) = primitives::container(half);
                    for (part, glass) in [
                        (shell, GlassMaterial::shell(tint, depth, emphasis)),
                        (tray, GlassMaterial::tray(tint, depth, emphasis)),
                    ] {
                        commands.spawn((
                            Mesh3d(meshes.add(part.mesh())),
                            MeshMaterial3d(glasses.add(glass)),
                            Transform::from_translation(part.position),
                            ChildOf(root),
                        ));
                    }
                }
                if (b.solid && !dimmed) || focus == Some(&n.id) || lit {
                    let caption = label_caption(&if marked {
                        format!("[marked] {title}")
                    } else {
                        title
                    });
                    let label = spawn_label(
                        &mut commands,
                        Label {
                            point: position + Vec3::Y * half.y,
                            id: n.id.clone(),
                        },
                        &caption,
                        display.is_some(),
                        if marked { MARK_COLOR } else { color },
                    );
                    if let Some((generation, c)) = staging {
                        commands.entity(label).insert(Held {
                            generation,
                            at: c.labels_in,
                        });
                    }
                }
            }
            if let Some((generation, c)) = staging {
                for (entity, at) in std::mem::take(&mut leaving) {
                    commands
                        .entity(entity)
                        .insert(Track::leave(generation, c.leave, at));
                }
            }
            // Filter by isolation first, then merge coincident segments, so an
            // isolated path always draws as a normal single route.
            let mut visible = Vec::new();
            for r in &s.geometry.routes {
                let edge = p.edges.iter().find(|e| e.id == r.edge_id).unwrap();
                if !visible_edge(&viewer, p, edge) {
                    continue;
                }
                let edge_lit = spot.as_ref().is_none_or(|spot| {
                    r.semantic_ids.iter().any(|id| spot.relations.contains(id))
                        || (spot.focus.contains(&edge.source) && spot.focus.contains(&edge.target))
                });
                if !edge_lit
                    && spot
                        .as_ref()
                        .is_some_and(|spot| spot.context == SpotContext::Hide)
                {
                    continue;
                }
                let edge_dimmed = !edge_lit
                    && spot
                        .as_ref()
                        .is_some_and(|spot| spot.context == SpotContext::Dim);
                let selected = viewer
                    .selected_relation
                    .as_ref()
                    .filter(|id| r.semantic_ids.contains(id));
                let relation = &p.claims[selected.unwrap_or(&r.semantic_ids[0])];
                let points: Vec<_> = r
                    .points
                    .iter()
                    .map(|p| s.geometry.display(*p, viewer.explode, center))
                    .collect();
                for point in &points {
                    scene_bounds = scene_bounds.max(point.abs() + Vec3::splat(0.5));
                }
                visible.push((r, relation, selected.is_some(), points, edge_dimmed));
            }
            let sources: Vec<_> = visible
                .iter()
                .map(|(r, relation, _, points, _)| connections::SegmentSource {
                    kind: relation.visual_kind,
                    basis: relation.basis,
                    points,
                    scales: &r.segment_scales,
                    relations: r.semantic_ids.len(),
                })
                .collect();
            let shared = connections::shared_segments(&sources);
            let skips = connections::skip_masks(&sources, &shared);
            // A shared trunk is drawn once; it is highlighted when any route
            // through it is the selected path.
            let shared_selected: Vec<bool> = shared
                .iter()
                .map(|segment| segment.members.iter().any(|&(route, _)| visible[route].2))
                .collect();
            for (segment, selected) in shared.iter().zip(shared_selected) {
                let root = connections::draw_shared_segment(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    segment,
                );
                if selected {
                    highlight_path(&mut commands, root);
                }
                if segment.members.iter().all(|&(route, _)| visible[route].4) {
                    commands.entity(root).insert(Dimmed);
                }
                commands.entity(root).insert(LayoutOwned);
                if let Some((generation, c)) = staging {
                    commands.entity(root).insert(Track::fade(
                        generation,
                        c.connectors_in,
                        Vec3::ZERO,
                        (0.0, 1.0),
                    ));
                }
            }
            for ((r, relation, selected, points, dimmed), skip) in visible.into_iter().zip(&skips) {
                let route = connections::Route {
                    relation: connections::RenderedRelation {
                        id: relation.id.clone(),
                        kind: relation.visual_kind,
                        source: relation.source.clone(),
                        target: relation.target.clone(),
                        basis: relation.basis,
                        points: points.clone(),
                    },
                    label: None,
                    label_position: Vec3::ZERO,
                };
                let root = connections::draw_skipping(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &route,
                    &r.segment_scales,
                    connections::aggregate_thickness(r.semantic_ids.len()),
                    skip,
                );
                if selected {
                    // Keep existing arrow/dash geometry and its semantic basis.
                    // Only the private route material becomes more prominent.
                    highlight_path(&mut commands, root);
                }
                if dimmed {
                    commands.entity(root).insert(Dimmed);
                }
                commands
                    .entity(root)
                    .remove::<connections::RenderedRelation>()
                    .insert((
                        LayoutOwned,
                        PlacedRelation {
                            id: relation.id.clone(),
                            kind: relation.kind_label.clone(),
                            source: relation.source.clone(),
                            target: relation.target.clone(),
                            basis: relation.basis,
                            semantic_ids: r.semantic_ids.clone(),
                            points,
                        },
                    ));
                if let Some((generation, c)) = staging {
                    commands.entity(root).insert(Track::fade(
                        generation,
                        c.connectors_in,
                        Vec3::ZERO,
                        (0.0, 1.0),
                    ));
                }
            }
            // A faint ground grid just below the lowest content seats the view.
            let reach = scene_bounds.x.max(scene_bounds.z) * 1.6;
            commands.spawn((
                LayoutOwned,
                Ground,
                Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(reach)))),
                MeshMaterial3d(glasses.add(GlassMaterial::grid(
                    style::grid_spacing(scene_bounds.x.max(scene_bounds.z) * 2.0),
                    reach,
                    Vec2::ZERO,
                ))),
                Transform::from_xyz(0.0, 0.5 - scene_bounds.y, 0.0),
            ));
            for (mut orbit, mut transform) in &mut camera {
                if viewer.fit_requested {
                    *orbit = OrbitCamera::for_scene(scene_bounds, 1280.0 / 900.0);
                } else {
                    orbit.update_bounds(scene_bounds);
                }
                *transform = orbit.transform();
            }
            status.ready = true;
            status.object_count = s.geometry.boxes.len();
            status.relation_count = s.geometry.routes.len();
            status.noise = layout::noise::visual_noise(&p.graph).total;
            status.densest_interior =
                layout::noise::densest_interior(&p.graph).map(|(id, noise)| (id, noise.total));
            status.composition_frames = s
                .composed_solution()
                .map_or(0, |solution| solution.assembly.frame_ids().count());
            status.maximum_scale_shift = s
                .composed_solution()
                .map_or(0, |solution| solution.metrics.maximum_scale_shift);
            viewer.fit_requested = false;
        }
    }
    if let Some((timing, _)) = &mut completed_timing {
        timing.scene_build_ms = scene_started.elapsed().as_secs_f64() * 1000.0;
    }
    if viewer.save_needed {
        viewer.save_needed = false;
        // A teaching view is temporary: it never overwrites the model's saved layout.
        if viewer.teaching.is_none()
            && let (Some(base), Some((_, accepted))) = (&viewer.storage, &viewer.accepted)
        {
            let save_started = std::time::Instant::now();
            let saved = std::fs::create_dir_all(base)
                .map_err(|e| e.to_string())
                .and_then(|()| match &accepted.data {
                    LayoutData::Flat(s) => {
                        let state = layout::persistence::PersistedView {
                            version: layout::persistence::STATE_VERSION,
                            model_id: s.model_id.clone(),
                            revision: s.revision,
                            view: viewer.accepted_view.clone(),
                            accepted: s.as_ref().clone(),
                            dismissed_proposals: viewer.dismissed.iter().cloned().collect(),
                        };
                        layout::persistence::save(
                            saved_path(base, &state_identity(&s.model_id, *engine)),
                            &state,
                        )
                        .map_err(|e| e.to_string())
                    }
                    LayoutData::Composed {
                        solution,
                        pins,
                        anchors,
                        context,
                        ..
                    } => {
                        let state = layout::composed::persistence::PersistedComposedView {
                            version: layout::composed::persistence::STATE_VERSION,
                            model_id: solution.model_id.clone(),
                            revision: solution.revision,
                            context: context.clone(),
                            view: viewer.accepted_view.clone(),
                            canonical_pins: pins.clone(),
                            anchor_pins: anchors.clone(),
                            constraint_sets: viewer
                                .composed_constraints
                                .values()
                                .map(ComposedConstraints::persisted)
                                .collect(),
                            dismissed_proposals: viewer.dismissed.iter().cloned().collect(),
                            accepted: solution.as_ref().clone(),
                        };
                        layout::composed::persistence::save(
                            composed_state_path(base, &solution.model_id),
                            &state,
                            &AtomicBool::new(false),
                        )
                        .map_err(|e| e.to_string())
                    }
                });
            let elapsed_ms = save_started.elapsed().as_secs_f64() * 1000.0;
            if let Some((timing, _)) = &mut completed_timing {
                timing.save_ms = elapsed_ms;
            }
            status.save_attempts = status.save_attempts.saturating_add(1);
            status.save_failures = status
                .save_failures
                .saturating_add(u64::from(saved.is_err()));
            status.save_total_ms += elapsed_ms;
            status.save_max_ms = status.save_max_ms.max(elapsed_ms);
            status.save_samples_ms.push_back(elapsed_ms);
            while status.save_samples_ms.len() > 64 {
                status.save_samples_ms.pop_front();
            }
            if let Err(e) = saved {
                viewer
                    .message
                    .push_str(&format!(" | Local state was not saved: {e}"));
            }
        }
    }
    if let Some((mut timing, queued)) = completed_timing {
        timing.total_ms = queued.elapsed().as_secs_f64() * 1000.0;
        record_timing(&mut status, timing);
    }
    for mut text in &mut text {
        let progress = viewer
            .job
            .as_ref()
            .and_then(|job| job.replacement_input.as_ref())
            .unwrap_or(&model)
            .progress();
        text.0 = if let Some((read, total)) = progress {
            format!(
                "Visualyze / Indexing supplied bundle: {:.0}%\nEsc cancels",
                read as f64 / total.max(1) as f64 * 100.0
            )
        } else {
            format!(
                "Visualyze / Experimental assembly\n{}",
                brief_status(&viewer, &status)
            )
        };
    }
    for mut text in &mut inspector {
        let mut details = if let Some((p, s)) = &viewer.accepted {
            if let Some(id) = &viewer.selected {
                let label = p.labels.get(id);
                let claims = selected_claims(&viewer);
                let path = path_details(&viewer, p);
                let linked = if path.is_some() {
                    String::new()
                } else {
                    claims
                        .iter()
                        .skip(viewer.relation_page * CLAIMS_PER_PAGE)
                        .take(CLAIMS_PER_PAGE)
                        .filter_map(|id| p.claims.get(*id))
                        .map(claim_text)
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                let browse = if path.is_some() {
                    "J / K selects path".to_owned()
                } else {
                    format!(
                        "page {}, B / N to browse; J / K selects path",
                        viewer.relation_page + 1
                    )
                };
                let selected = path.unwrap_or_default();
                let automatic = if p.auto_opened.contains(id) {
                    "Opened automatically to show supplied structure within the readability budget; E collapses it.\n"
                } else {
                    ""
                };
                // An expanded box is its own region with its own readability budget.
                let interior = layout::noise::region_noise(&p.graph)
                    .get(id)
                    .filter(|_| *id != p.graph.root)
                    .map_or_else(String::new, |noise| {
                        format!(
                            "Interior: {}{}\n",
                            noise.summary(),
                            if noise.over_threshold() {
                                " (dense)"
                            } else {
                                ""
                            }
                        )
                    });
                format!(
                    "{} / {}\nSelected component: {}\n{}{} | {} linked claims ({}) | {}\n{}{}{}{}\n{}",
                    p.model_id,
                    viewer.view.focus.as_deref().unwrap_or("Overview"),
                    label.map_or("unknown", |label| label.label.as_str()),
                    id,
                    if inherited_pin(&viewer, id) {
                        format!(
                            " (pinned within {}; Shift+P opens origin)",
                            pin_origin(&viewer, id).unwrap_or_default()
                        )
                    } else if is_pinned(&viewer, id) {
                        " (pinned)".into()
                    } else {
                        String::new()
                    },
                    claims.len(),
                    browse,
                    label.map_or("unknown", |l| l.original_kind.as_str()),
                    automatic,
                    interior,
                    selected,
                    linked,
                    p.detail_note
                )
            } else {
                format!(
                    "{} / {} | {} internal membership IDs loaded | route length {:.1} | {} bends\n{}",
                    p.model_id,
                    viewer.view.focus.as_deref().unwrap_or("Overview"),
                    p.internal_relation_ids.len(),
                    s.metrics.route_length as f64 / 10.0,
                    s.metrics.bends,
                    p.detail_note
                )
            }
        } else {
            "".into()
        };
        if let Some((p, _)) = &viewer.accepted {
            details.push_str(&format!(
                "\n\nFull navigation trail\n{}",
                full_navigation(&viewer, p)
            ));
        }
        details.push_str(&format!("\n\nStatus\n{}\n\n{ALL_CONTROLS}", viewer.message));
        if text.0 != details {
            text.0 = details;
        }
    }
}

/// The palette consumes text input before shortcuts, including camera controls.
fn append_search_text(query: &mut String, text: &str) {
    for c in text.chars().filter(|c| !c.is_control()) {
        if query.len() + c.len_utf8() <= 256 {
            query.push(c);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn controls(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time<Real>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut keyboard: MessageReader<KeyboardInput>,
    mut wheel: MessageReader<MouseWheel>,
    mut input_lock: ResMut<crate::camera::CameraInputLock>,
    mut actions: ResMut<ViewerActions>,
    mut viewer: ResMut<Viewer>,
    mut panels: Query<&mut Node, (With<InspectorPanel>, Without<InspectorViewport>)>,
    mut scrollable: Query<
        (
            &ComputedNode,
            &mut Node,
            &mut ScrollPosition,
            &mut InspectorViewport,
        ),
        Without<InspectorPanel>,
    >,
    content: Query<&Text, With<Inspector>>,
    mut disclosures: Query<&mut Text, (With<InspectorDisclosure>, Without<Inspector>)>,
) {
    let rect = details_rect(
        Vec2::new(window.width(), window.height()),
        viewer.inspector_expanded,
    );
    let over_details = viewer.inspector_expanded
        && !panels.is_empty()
        && window
            .cursor_position()
            .is_some_and(|point| rect.contains(point));
    for mut panel in &mut panels {
        panel.height = px(rect.height());
    }
    let mut wheel_delta = 0.0;
    let mut wheel_zoom = false;
    for event in wheel.read() {
        wheel_zoom |= event.y != 0.0;
        wheel_delta -= event.y
            * match event.unit {
                MouseScrollUnit::Line => 28.0,
                MouseScrollUnit::Pixel => 1.0,
            };
    }
    let mut content_hash = 0xcbf29ce484222325_u64;
    for text in &content {
        for byte in text.0.bytes() {
            content_hash = (content_hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    viewer.inspector_hovered = over_details && window.focused && !viewer.search_open;
    for (computed, mut node, mut scroll, mut viewport) in &mut scrollable {
        if !viewer.inspector_expanded {
            node.display = Display::None;
            continue;
        }
        if node.display == Display::None {
            scroll.y = viewer.inspector_scroll;
        }
        node.display = Display::Flex;
        // Hidden UI has no measured viewport. Retain the last offset until the
        // newly opened viewport has been laid out instead of clamping it to zero.
        if computed.size().y <= 0.0 {
            continue;
        }
        if viewport.content_hash != content_hash {
            scroll.y = 0.0;
            viewport.content_hash = content_hash;
        }
        let maximum = ((computed.content_size().y - computed.size().y)
            * computed.inverse_scale_factor())
        .max(0.0);
        scroll.y = details_scroll(
            scroll.y,
            maximum,
            computed.size().y * computed.inverse_scale_factor(),
            wheel_delta,
            &keys,
            viewer.inspector_hovered,
        );
        viewer.inspector_scroll = scroll.y;
        viewer.inspector_scroll_max = maximum;
    }
    for mut disclosure in &mut disclosures {
        disclosure.0 = if !viewer.inspector_expanded {
            "Details | F1 opens full names, relationships, status and controls".into()
        } else if viewer.inspector_scroll_max > 0.5 {
            format!(
                "Details | F1 closes | Hover + wheel / PgUp / PgDn / Home / End | {:.0}%",
                viewer.inspector_scroll / viewer.inspector_scroll_max * 100.0
            )
        } else {
            "Details | F1 closes | Full names, relationships, status and controls".into()
        };
    }
    input_lock.0 = viewer.search_open || over_details;
    let zoom_input = wheel_zoom
        || keys.pressed(KeyCode::Equal)
        || keys.pressed(KeyCode::Minus)
        || keys.pressed(KeyCode::NumpadAdd)
        || keys.pressed(KeyCode::NumpadSubtract);
    viewer.zoom_pending = zoom_input && window.focused && !input_lock.0;
    if !window.focused {
        keyboard.clear();
        return;
    }
    if !viewer.search_open && keys.just_pressed(KeyCode::Slash) {
        viewer.search_open = true;
        viewer.search_query.clear();
        viewer.search_results.clear();
        viewer.search_selected = 0;
        input_lock.0 = true;
        keyboard.clear();
        return;
    }
    if viewer.search_open {
        if keys.just_pressed(KeyCode::Escape) {
            let _ = actions.submit(ViewerAction::Cancel);
            keyboard.clear();
            return;
        }
        let mut changed = false;
        // Search workers retain their own query. Editing is disabled until the
        // bounded worker completes or Escape cancels it, avoiding stale hits.
        if viewer.search_job.is_none() {
            for event in keyboard.read().filter(|e| e.state == ButtonState::Pressed) {
                if event.key_code == KeyCode::Backspace {
                    changed |= viewer.search_query.pop().is_some();
                } else if let Some(text) = &event.text {
                    let before = viewer.search_query.len();
                    append_search_text(&mut viewer.search_query, text);
                    changed |= viewer.search_query.len() != before;
                }
            }
        } else {
            keyboard.clear();
        }
        if changed {
            viewer.search_results.clear();
            viewer.search_selected = 0;
        }
        if keys.just_pressed(KeyCode::ArrowDown) && !viewer.search_results.is_empty() {
            viewer.search_selected =
                (viewer.search_selected + 1).min(viewer.search_results.len() - 1);
        }
        if keys.just_pressed(KeyCode::ArrowUp) {
            viewer.search_selected = viewer.search_selected.saturating_sub(1);
        }
        if keys.just_pressed(KeyCode::Enter) && viewer.search_job.is_none() {
            let action = viewer
                .search_results
                .get(viewer.search_selected)
                .map(|hit| ViewerAction::Jump(hit.id.clone()))
                .unwrap_or_else(|| ViewerAction::Search(viewer.search_query.clone()));
            let _ = actions.submit(action);
        }
        return;
    }
    keyboard.clear();
    if keys.just_pressed(KeyCode::Escape) {
        let _ = actions.submit(ViewerAction::Cancel);
        return;
    }
    if keys.just_pressed(KeyCode::Tab)
        && let Some((p, _)) = &viewer.accepted
    {
        let ids: Vec<_> = p
            .nodes
            .iter()
            .filter(|n| n.id != p.root)
            .map(|n| n.id.clone())
            .collect();
        if !ids.is_empty() {
            let next = viewer
                .selected
                .as_ref()
                .and_then(|s| ids.iter().position(|id| id == s))
                .map_or(0, |i| (i + 1) % ids.len());
            viewer.selected = Some(ids[next].clone());
            viewer.selected_relation = None;
            viewer.relation_page = 0;
            viewer.dirty = true;
        }
    }
    // A click is decided on release, so a drag that starts on a box only
    // orbits. A press over Details is never a click on the scene.
    if mouse.just_pressed(MouseButton::Left) {
        viewer.click.press = window.cursor_position().filter(|_| !over_details);
    }
    if mouse.just_released(MouseButton::Left) {
        let press = viewer.click.press.take();
        if let (Some(_), Some(cursor)) = (press, window.cursor_position()) {
            let hit = pick_at(&viewer, camera.0, camera.1, cursor);
            let mut last = viewer.click.last.take();
            let click = classify_click(press, cursor, hit.as_deref(), &mut last, time.elapsed());
            viewer.click.last = last;
            if let Some(action) = apply_click(&mut viewer, click) {
                let _ = actions.submit(action);
            }
        } else {
            viewer.click.last = None;
        }
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
    for (key, action) in [
        (KeyCode::Backspace, ViewerAction::Back),
        (KeyCode::Space, ViewerAction::ToggleExplode),
        (
            KeyCode::KeyI,
            if shift {
                ViewerAction::ClearRelationSelection
            } else {
                ViewerAction::ToggleIsolate
            },
        ),
        (KeyCode::KeyO, ViewerAction::Reorganize),
        (KeyCode::KeyV, ViewerAction::Preview),
        (KeyCode::KeyA, ViewerAction::Apply),
        (KeyCode::KeyD, ViewerAction::Dismiss),
        (KeyCode::KeyU, ViewerAction::Undo),
        (KeyCode::F1, ViewerAction::ToggleDetails),
        (KeyCode::F5, ViewerAction::Reload),
        (KeyCode::KeyB, ViewerAction::PreviousClaims),
        (KeyCode::KeyN, ViewerAction::NextClaims),
        (KeyCode::KeyJ, ViewerAction::PreviousRelation),
        (KeyCode::KeyK, ViewerAction::NextRelation),
    ] {
        if keys.just_pressed(key) {
            let _ = actions.submit(action);
        }
    }
    if keys.just_pressed(KeyCode::KeyM) {
        if shift {
            let _ = actions.submit(ViewerAction::ClearGroupMarks);
        } else if let Some(id) = viewer.selected.clone() {
            let _ = actions.submit(ViewerAction::ToggleGroupMember(id));
        }
    }
    if keys.just_pressed(KeyCode::KeyG) {
        if shift {
            if let Some(id) = viewer
                .selected
                .as_ref()
                .filter(|id| {
                    viewer.accepted.as_ref().is_some_and(|(p, _)| {
                        p.suggested_groups.iter().any(|group| &group.id == *id)
                    })
                })
                .or_else(|| {
                    viewer.view.focus.as_ref().filter(|id| {
                        viewer.accepted.as_ref().is_some_and(|(p, _)| {
                            p.suggested_groups.iter().any(|group| &group.id == *id)
                        })
                    })
                })
                .cloned()
            {
                let _ = actions.submit(ViewerAction::RemoveGroup(id));
            }
        } else {
            let _ = actions.submit(ViewerAction::GroupMarked);
        }
    }
    if keys.just_pressed(KeyCode::PageDown) && !over_details {
        let _ = actions.submit(if shift {
            ViewerAction::NextRelations
        } else {
            ViewerAction::NextPage
        });
    }
    if keys.just_pressed(KeyCode::PageUp) && !over_details {
        let _ = actions.submit(if shift {
            ViewerAction::PreviousRelations
        } else {
            ViewerAction::PreviousPage
        });
    }
    if let Some(id) = viewer.selected.clone() {
        if keys.just_pressed(KeyCode::Enter) {
            let _ = actions.submit(ViewerAction::Enter(id.clone()));
        }
        if keys.just_pressed(KeyCode::KeyE) {
            let action = expansion_toggle(&viewer, &id);
            let _ = actions.submit(action);
        }
        if keys.just_pressed(KeyCode::KeyP) {
            let action = if shift {
                ViewerAction::OpenPinOrigin(id.clone())
            } else if is_pinned(&viewer, &id) {
                ViewerAction::Unpin(id.clone())
            } else {
                ViewerAction::Pin(id.clone())
            };
            let _ = actions.submit(action);
        }
        if keys.just_pressed(KeyCode::BracketLeft) || keys.just_pressed(KeyCode::BracketRight) {
            let old = viewer
                .accepted
                .as_ref()
                .and_then(|(p, _)| p.nodes.iter().find(|n| n.id == id))
                .map_or(0, |n| n.band);
            let delta = if keys.just_pressed(KeyCode::BracketRight) {
                1
            } else {
                -1
            };
            let _ = actions.submit(ViewerAction::SetLayer {
                id,
                band: (old + delta).clamp(-16, 16),
            });
        }
    }
    if keys.just_pressed(KeyCode::KeyX) {
        let _ = actions.submit(ViewerAction::ExpandPathEndpoints);
    }
    if keys.just_pressed(KeyCode::KeyL) {
        use layout::projection::LayoutPolicy;
        let policy = match viewer.view.policy {
            LayoutPolicy::Crud => LayoutPolicy::Pipeline,
            LayoutPolicy::Pipeline => LayoutPolicy::Peers,
            LayoutPolicy::Peers => LayoutPolicy::Code,
            LayoutPolicy::Code => LayoutPolicy::Deployment,
            LayoutPolicy::Deployment => LayoutPolicy::Crud,
        };
        let _ = actions.submit(ViewerAction::SetPolicy(policy));
    }
    if keys.just_pressed(KeyCode::KeyZ) {
        let _ = actions.submit(ViewerAction::SetSemanticZoom(!viewer.view.semantic_zoom));
    }
    if keys.just_pressed(KeyCode::KeyC) {
        let lens = match viewer.view.lens {
            None => Some(SemanticLens::Logical),
            Some(SemanticLens::Logical) => Some(SemanticLens::Code),
            Some(SemanticLens::Code) => Some(SemanticLens::Deployment),
            Some(SemanticLens::Deployment) => None,
        };
        let _ = actions.submit(ViewerAction::SetLens(lens));
    }
}

/// Only a zoom gesture can change detail. Orbiting never triggers layout work,
/// and just the selected region participates in the two-threshold hysteresis.
fn semantic_zoom(
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut viewer: ResMut<Viewer>,
    mut actions: ResMut<ViewerActions>,
) {
    if !std::mem::take(&mut viewer.zoom_pending)
        || !viewer.view.semantic_zoom
        || viewer.job.is_some()
        || viewer.search_open
        || !actions.queue.is_empty()
    {
        return;
    }
    let Some(id) = viewer.selected.as_ref() else {
        return;
    };
    let Some((p, solution)) = viewer.accepted.as_ref() else {
        return;
    };
    // Reference ports and the current frame cannot be expanded in place.
    if id == &p.root || p.references.contains_key(id) {
        return;
    }
    let Some(bounds) = solution.geometry.boxes.get(id) else {
        return;
    };
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for x in [bounds.min.x, bounds.max.x] {
        for y in [bounds.min.y, bounds.max.y] {
            for z in [bounds.min.z, bounds.max.z] {
                let point = solution.geometry.display(
                    bevy::math::DVec3::new(x, y, z),
                    viewer.explode,
                    viewer.origin,
                );
                let Ok(pixel) = camera.0.world_to_viewport(camera.1, point) else {
                    return;
                };
                lo = lo.min(pixel);
                hi = hi.max(pixel);
            }
        }
    }
    let pixels = f64::from((hi - lo).max_element());
    let _ = actions.submit(ViewerAction::UpdateZoom {
        id: id.clone(),
        projected_pixels: pixels,
    });
}

#[allow(clippy::type_complexity)]
fn navigation_status(
    mut viewer: ResMut<Viewer>,
    mut status: ResMut<LayoutStatus>,
    mut navigation: Query<&mut Text, (With<NavigationText>, Without<SearchText>)>,
    mut search: Query<(&mut Text, &mut Node), (With<SearchText>, Without<NavigationText>)>,
) {
    let result =
        viewer
            .search_job
            .as_ref()
            .and_then(|job| match job.receive.lock().unwrap().try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(layout::LayoutError::Invalid(
                    "search worker stopped".into(),
                ))),
                Err(mpsc::TryRecvError::Empty) => None,
            });
    if let Some(result) = result {
        let job = viewer.search_job.take().unwrap();
        match result {
            Ok(hits) => {
                viewer.message = format!(
                    "{} search results. Choose a component and press Enter.",
                    hits.len()
                );
                viewer.search_results = hits;
                viewer.search_selected = 0;
                finish_ticket(&mut status, job.action_id, None);
            }
            Err(e) => {
                viewer.message = e.to_string();
                finish_ticket(&mut status, job.action_id, Some(e.to_string()));
            }
        }
    }
    status.loading = viewer.job.is_some() || viewer.search_job.is_some();
    status.reorganizing = viewer.job.as_ref().is_some_and(|job| job.reorganize);
    status.reorganization_available = viewer
        .proposal
        .as_ref()
        .zip(viewer.accepted.as_ref())
        .is_some_and(|(proposal, (_, accepted))| same_view(proposal, accepted));
    status.selected = viewer.selected.clone();
    status.inspector_expanded = viewer.inspector_expanded;
    status.inspector_scroll = viewer.inspector_scroll;
    status.inspector_scroll_max = viewer.inspector_scroll_max;
    status.inspector_hovered = viewer.inspector_hovered;
    status.selected_relation = viewer.selected_relation.clone();
    status.selected_relation_members = viewer.accepted.as_ref().map_or(Vec::new(), |(p, _)| {
        p.edges
            .iter()
            .filter(|edge| {
                viewer
                    .selected_relation
                    .as_ref()
                    .is_some_and(|id| edge.semantic_ids.contains(id))
            })
            .flat_map(|edge| edge.semantic_ids.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    });
    status.search_results = viewer.search_results.clone();
    status.marked_group_members = viewer.group_marks.keys().cloned().collect();
    status.local_group_ids = viewer
        .accepted_view
        .group_overrides
        .iter()
        .map(|group| group.id.clone())
        .collect();
    status.inspected_relation_ids = paged_claims(&viewer)
        .into_iter()
        .skip(viewer.relation_page * CLAIMS_PER_PAGE)
        .take(CLAIMS_PER_PAGE)
        .cloned()
        .collect();
    if let Some(id) = &viewer.selected_relation
        && !status.inspected_relation_ids.contains(id)
    {
        status.inspected_relation_ids.push(id.clone());
    }
    if let Some((p, _)) = &viewer.accepted {
        status.model_id = Some(p.model_id.clone());
        status.revision = Some(p.revision);
        status.focus = viewer.accepted_view.focus.clone();
        status.root = Some(p.root.clone());
        status.lens = Some(p.lens);
        status.breadcrumbs = viewer
            .accepted_view
            .breadcrumbs()
            .iter()
            .chain(std::iter::once(&viewer.accepted_view.focus))
            .map(|id| id.clone().unwrap_or_else(|| "Overview".into()))
            .collect();
        status.child_page = p.page.as_ref().map_or(0, |page| page.page_index);
        status.total_children = p.page.as_ref().map_or(0, |page| page.total_children);
        status.has_next_page = p.page.as_ref().is_some_and(|page| page.has_next);
        status.has_previous_page = p.page.as_ref().is_some_and(|page| page.has_previous);
        status.relation_page = viewer.accepted_view.relation_history.len();
        status.has_previous_relations = !viewer.accepted_view.relation_history.is_empty();
        status.has_next_relations = p
            .relation_page
            .as_ref()
            .is_some_and(|page| page.next.is_some());
        let label = p
            .labels
            .get(&p.root)
            .map_or(p.root.as_str(), |label| label.label.as_str());
        let children = p.page.as_ref().map_or("none".into(), |page| {
            if page.total_children == 0 {
                "none".into()
            } else {
                format!(
                    "{}/{}",
                    page.page_index + 1,
                    page.total_children.div_ceil(page.page_size.max(1))
                )
            }
        });
        for mut text in &mut navigation {
            text.0 = format!(
                "{:?}{} | {}\nBackspace: back | /: find | Children {} | Relations {}{} | Full trail in Details",
                p.lens,
                if viewer.accepted_view.lens.is_none() {
                    " (Auto)"
                } else {
                    ""
                },
                compact_ui_text(label, 30),
                children,
                status.relation_page + 1,
                if status.has_next_relations { "+" } else { "" }
            );
        }
    }
    for (mut text, mut node) in &mut search {
        node.display = if viewer.search_open {
            Display::Flex
        } else {
            Display::None
        };
        if !viewer.search_open {
            continue;
        }
        let first = viewer.search_selected.saturating_sub(5);
        let rows = viewer
            .search_results
            .iter()
            .enumerate()
            .skip(first)
            .take(10)
            .map(|(index, hit)| {
                format!(
                    "{} {} / {}\n    {}",
                    if index == viewer.search_selected {
                        ">"
                    } else {
                        " "
                    },
                    hit.label,
                    hit.kind,
                    hit.id
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        text.0 = format!(
            "Find component: {}\n{}\n{}",
            viewer.search_query,
            if viewer.search_job.is_some() {
                "Searching supplied records... Esc cancels"
            } else if viewer.search_results.is_empty() {
                "Type a name or ID, then Enter to search. Esc closes."
            } else {
                "Up / Down chooses | Enter opens | Type to change search | Esc closes"
            },
            rows
        );
    }
}

// A fixed two-line caption makes screen-space packing independent of font
// layout timing. Full identities and claims remain available in the inspector.
fn label_caption(title: &str) -> String {
    let normalized = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let shorten = |text: &str| {
        let mut characters = text.chars();
        let first: String = characters.by_ref().take(27).collect();
        if characters.next().is_some() {
            format!("{}...", first.chars().take(24).collect::<String>())
        } else {
            first
        }
    };
    if let Some((kind, name)) = normalized.split_once(" / ") {
        return format!("{}\n{}", shorten(kind), shorten(name));
    }
    let chars: Vec<_> = normalized.chars().collect();
    if chars.len() <= 27 {
        return normalized;
    }
    let first: String = chars[..27].iter().collect();
    let mut second: String = chars[27..chars.len().min(54)].iter().collect();
    if chars.len() > 54 {
        second = format!("{}...", second.chars().take(24).collect::<String>());
    }
    format!("{first}\n{second}")
}

/// The caption sits above its anchor; the stem's foot is the anchor itself.
fn label_rect(point: Vec2, viewport: Vec2, occupied: &[Rect]) -> Option<Rect> {
    let min = point - Vec2::new(LABEL_SIZE.x * 0.5, LABEL_SIZE.y);
    let rect = Rect::from_corners(min, min + LABEL_SIZE);
    let padded = Rect::from_corners(rect.min - Vec2::splat(4.0), rect.max + Vec2::splat(4.0));
    (rect.min.x >= 12.0
        && rect.min.y >= 12.0
        && rect.max.x <= viewport.x - 12.0
        && rect.max.y <= viewport.y - 12.0
        && occupied.iter().all(|other| {
            padded.max.x <= other.min.x
                || padded.min.x >= other.max.x
                || padded.max.y <= other.min.y
                || padded.min.y >= other.max.y
        }))
    .then_some(rect)
}

/// A rounded caption pill edged in the object's colour: kind above, name
/// below. The fixed outer box is what `labels` packs; the pill shrinks to fit.
fn spawn_label(
    commands: &mut Commands,
    label: Label,
    caption: &str,
    kinded: bool,
    accent: Color,
) -> Entity {
    let (header, name) = match caption.split_once('\n') {
        Some((kind, name)) if kinded => (kind, Some(name)),
        _ => (caption, None),
    };
    commands
        .spawn((
            LayoutOwned,
            label,
            Node {
                position_type: PositionType::Absolute,
                width: px(LABEL_SIZE.x),
                height: px(LABEL_SIZE.y),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::FlexEnd,
                ..default()
            },
        ))
        .with_children(|label| {
            label
                .spawn((
                    BackgroundColor(Color::srgba(0.055, 0.07, 0.095, 0.86)),
                    BorderColor::all(accent.with_alpha(0.6)),
                    Node {
                        max_width: px(LABEL_SIZE.x),
                        max_height: px(LABEL_SIZE.y - LABEL_STEM),
                        padding: UiRect::axes(px(9), px(3)),
                        border: UiRect::all(px(1)),
                        border_radius: BorderRadius::all(px(7)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                ))
                .with_children(|pill| {
                    let mut text = pill.spawn((
                        Text::new(header),
                        TextFont {
                            font_size: if name.is_some() { 10.0 } else { 12.0 },
                            ..default()
                        },
                        TextColor(if name.is_some() {
                            accent.mix(&Color::WHITE, 0.35)
                        } else {
                            Color::srgb_u8(222, 231, 241)
                        }),
                        TextLayout::new_with_justify(Justify::Center).with_no_wrap(),
                    ));
                    if let Some(name) = name {
                        text.with_child((
                            TextSpan::new(format!("\n{name}")),
                            TextFont {
                                font_size: 12.0,
                                ..default()
                            },
                            TextColor(Color::srgb_u8(222, 231, 241)),
                        ));
                    }
                });
            label.spawn((
                Node {
                    width: px(1.5),
                    height: px(LABEL_STEM),
                    ..default()
                },
                BackgroundColor(accent.with_alpha(0.75)),
            ));
        })
        .id()
}

/// Containers below the focused root: 0 for its direct children.
fn nesting_depth(parents: &BTreeMap<&str, &str>, root: &str, id: &str) -> usize {
    let mut depth = 0;
    let mut current = parents.get(id);
    while let Some(&id) = current {
        if id == root || depth > parents.len() {
            break;
        }
        depth += 1;
        current = parents.get(id);
    }
    depth
}

#[allow(clippy::type_complexity)]
fn labels(
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    viewer: Res<Viewer>,
    mut labels: Query<(&Label, &mut Node), Without<Held>>,
    panels: Query<
        (&ComputedNode, &Node),
        (
            Without<Label>,
            Or<(
                With<LayoutText>,
                With<NavigationText>,
                With<Inspector>,
                With<InspectorPanel>,
                With<SearchText>,
            )>,
        ),
    >,
) {
    let Some(viewport) = camera.0.logical_viewport_size() else {
        return;
    };
    let mut occupied = Vec::new();
    for (computed, node) in &panels {
        if node.display == Display::None {
            continue;
        }
        let size = computed.size() * computed.inverse_scale_factor();
        let x = match node.left {
            Val::Px(x) => x,
            _ => 0.0,
        };
        let y = match (node.top, node.bottom) {
            (Val::Px(top), _) => top,
            (_, Val::Px(bottom)) => viewport.y - bottom - size.y,
            _ => continue,
        };
        occupied.push(Rect::from_corners(Vec2::new(x, y), Vec2::new(x, y) + size));
    }
    // The fixed keyboard guide is below the inspector and has no world label.
    occupied.push(Rect::from_corners(
        Vec2::new(0.0, viewport.y - 96.0),
        viewport,
    ));
    let mut order: Vec<_> = labels.iter_mut().collect();
    order.sort_by(|(a, _), (b, _)| {
        (viewer.selected.as_ref() != Some(&a.id))
            .cmp(&(viewer.selected.as_ref() != Some(&b.id)))
            .then_with(|| {
                a.point
                    .distance_squared(camera.1.translation())
                    .total_cmp(&b.point.distance_squared(camera.1.translation()))
            })
            .then(a.id.cmp(&b.id))
    });
    for (label, mut node) in order {
        let rect = camera
            .0
            .world_to_viewport(camera.1, label.point)
            .ok()
            .and_then(|point| label_rect(point, viewport, &occupied));
        if let Some(rect) = rect {
            node.display = Display::Flex;
            node.left = px(rect.min.x);
            node.top = px(rect.min.y);
            occupied.push(rect);
        } else {
            node.display = Display::None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("composed_state_tests.rs");
    include!("presentation_pin_tests.rs");
    include!("pin_provenance_tests.rs");
    include!("aggregated_path_tests.rs");
    include!("planned_detail_tests.rs");
    include!("click_isolation_tests.rs");
    include!("teaching_view_tests.rs");

    // These regressions exercise the flat engine. Mutations must rebuild both
    // authoritative solver data and the renderer geometry, just as acceptance does.
    fn edit_flat(solution: &mut NativeLayout, edit: impl FnOnce(&mut LayoutSolution)) {
        let mut flat = solution.flat().expect("flat-engine test fixture").clone();
        edit(&mut flat);
        *solution = flat.into();
    }

    #[test]
    fn scripted_proof_blocks_key_v_but_handles_queued_action_and_normal_controls_stay_enabled() {
        use bevy::input::{InputPlugin, keyboard::Key};
        for isolated in [false, true] {
            let file = LocalTestFile::new();
            pipeline_file(&file.0, 1, false);
            let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
            app.add_plugins(InputPlugin)
                .init_resource::<Time<Real>>()
                .init_resource::<crate::camera::CameraInputLock>()
                .add_systems(Update, controls.before(process_actions));
            if isolated {
                scripted_proof_input::install(&mut app);
                assert!(scripted_proof_input::INPUT_MODE.contains("Scripted ViewerActions"));
            }
            let window = app
                .world_mut()
                .spawn((Window::default(), PrimaryWindow))
                .id();
            app.world_mut().spawn((
                Camera3d::default(),
                Camera::default(),
                GlobalTransform::default(),
            ));
            let before = app.world().resource::<Viewer>().accepted.clone().unwrap().1;
            let ticket = app
                .world_mut()
                .resource_mut::<ViewerActions>()
                .submit(ViewerAction::ToggleDetails)
                .unwrap();
            app.world_mut()
                .resource_mut::<Messages<KeyboardInput>>()
                .write(KeyboardInput {
                    key_code: KeyCode::KeyV,
                    logical_key: Key::Character("v".into()),
                    state: ButtonState::Pressed,
                    text: Some("v".into()),
                    repeat: false,
                    window,
                });
            app.update();
            // The runtime consumes one queued action per update. A native KeyV
            // may enqueue Preview behind the direct action in the control case.
            app.update();
            let status = app.world().resource::<LayoutStatus>();
            assert!(
                status.inspector_expanded,
                "direct queued action must still execute"
            );
            assert_eq!(status.completed_action_id, ticket + u64::from(!isolated));
            assert_eq!(
                status.action_error.as_deref(),
                (!isolated).then_some("No current reorganization proposal")
            );
            let accepted = &app
                .world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1;
            assert_eq!(accepted.flat().unwrap().boxes, before.flat().unwrap().boxes);
            assert_eq!(
                accepted.flat().unwrap().routes,
                before.flat().unwrap().routes
            );
        }
    }

    #[test]
    fn details_geometry_is_bounded_and_scroll_can_reach_both_ends() {
        for viewport in [
            Vec2::new(640.0, 480.0),
            Vec2::new(1280.0, 900.0),
            Vec2::new(1920.0, 1600.0),
        ] {
            let rect = details_rect(viewport, true);
            assert!(rect.height() <= 220.0 && rect.height() >= 120.0);
            assert!(rect.min.y > 116.0 && rect.max.y <= viewport.y - DETAILS_BOTTOM);
            let collapsed = details_rect(viewport, false);
            assert_eq!(collapsed.height(), DETAILS_COLLAPSED_HEIGHT);
            assert_eq!(collapsed.max, rect.max);
            assert!(collapsed.min.y > rect.min.y);
        }
        let mut keys = ButtonInput::default();
        keys.press(KeyCode::End);
        assert_eq!(details_scroll(0.0, 5000.0, 180.0, 0.0, &keys, true), 5000.0);
        assert_eq!(details_scroll(0.0, 5000.0, 180.0, 0.0, &keys, false), 0.0);
        keys.reset_all();
        keys.press(KeyCode::Home);
        assert_eq!(details_scroll(5000.0, 5000.0, 180.0, 0.0, &keys, true), 0.0);
        keys.reset_all();
        assert_eq!(details_scroll(0.0, 5000.0, 180.0, -100.0, &keys, true), 0.0);
        assert_eq!(
            details_scroll(0.0, 5000.0, 180.0, 9000.0, &keys, true),
            5000.0
        );
    }

    #[test]
    fn inspector_uses_a_clipped_scroll_view_and_wraps_long_identifiers() {
        let mut app = App::new();
        app.add_systems(Startup, |mut commands: Commands| {
            spawn_inspector(&mut commands)
        });
        app.update();
        let world = app.world_mut();
        let mut panels = world.query_filtered::<&Node, With<InspectorPanel>>();
        let panel = panels.single(world).unwrap();
        assert_eq!(panel.height, px(DETAILS_COLLAPSED_HEIGHT));
        assert_eq!(panel.overflow, Overflow::clip());
        let mut viewports =
            world.query_filtered::<(&Node, &ScrollPosition), With<InspectorViewport>>();
        assert_eq!(
            viewports.single(world).unwrap().0.overflow.y,
            OverflowAxis::Scroll
        );
        assert_eq!(viewports.single(world).unwrap().0.display, Display::None);
        let mut contents = world.query_filtered::<(&Node, &TextLayout), With<Inspector>>();
        let (content, layout) = contents.single(world).unwrap();
        assert_eq!(content.flex_shrink, 0.0);
        assert_eq!(layout.linebreak, bevy::text::LineBreak::WordOrCharacter);
    }

    #[test]
    fn compact_top_rows_keep_full_labels_status_and_navigation_reachable_in_details() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let status_message = "Complete diagnostic with evidence and relation IDs. ".repeat(300);
        let supplied_label = "Long supplied assembly name 界é".repeat(100);
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.message = status_message.clone();
            let p = &mut viewer.accepted.as_mut().unwrap().0;
            p.labels.get_mut(&p.graph.root).unwrap().label = supplied_label.clone();
        }
        let header = app.world_mut().spawn((LayoutText, Text::default())).id();
        let navigation = app
            .world_mut()
            .spawn((NavigationText, Text::default()))
            .id();
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        app.update();
        let details = &app.world().get::<Text>(inspector).unwrap().0;
        assert!(details.contains(&status_message));
        assert!(details.contains(&supplied_label));
        assert!(details.contains(ALL_CONTROLS));
        let header = &app.world().get::<Text>(header).unwrap().0;
        assert_eq!(header.lines().count(), 2);
        assert!(header.lines().all(|line| line.chars().count() < 70));
        let navigation = &app.world().get::<Text>(navigation).unwrap().0;
        assert_eq!(navigation.lines().count(), 2);
        assert!(navigation.contains("...") && navigation.contains("Full trail in Details"));
        assert!(navigation.lines().next().unwrap().chars().count() <= 52);
    }

    #[test]
    fn hovered_details_consumes_scroll_and_page_keys_without_moving_camera_or_navigating() {
        use bevy::input::mouse::AccumulatedMouseMotion;
        let mut app = App::new();
        app.insert_resource(Viewer {
            inspector_expanded: true,
            ..default()
        })
        .init_resource::<ViewerActions>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<ButtonInput<MouseButton>>()
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .init_resource::<AccumulatedMouseMotion>()
        .add_message::<MouseWheel>()
        .add_message::<KeyboardInput>()
        .add_plugins(crate::camera::OrbitCameraPlugin)
        .add_systems(Update, controls.before(crate::camera::CameraControls));
        let mut window = Window {
            focused: true,
            resolution: bevy::window::WindowResolution::new(1280, 900),
            ..default()
        };
        window.set_cursor_position(Some(details_rect(Vec2::new(1280.0, 900.0), true).center()));
        let window_id = app.world_mut().spawn((window, PrimaryWindow)).id();
        let orbit = OrbitCamera::for_scene(Vec3::splat(3.0), 1280.0 / 900.0);
        let before = orbit.transform();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera::default(),
                GlobalTransform::default(),
                before,
                orbit,
            ))
            .id();
        app.world_mut().spawn((InspectorPanel, Node::default()));
        let viewport = app
            .world_mut()
            .spawn((
                InspectorViewport::default(),
                ScrollPosition::default(),
                Node::default(),
                ComputedNode {
                    size: Vec2::new(1000.0, 180.0),
                    content_size: Vec2::new(1000.0, 1600.0),
                    ..default()
                },
            ))
            .id();
        app.world_mut()
            .spawn((Inspector, Text::new("Full inspector content")));
        app.world_mut()
            .spawn((InspectorDisclosure, Text::default()));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::PageDown);
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -2.0,
            window: window_id,
        });
        app.update();
        assert!(app.world().get::<ScrollPosition>(viewport).unwrap().y > 180.0);
        assert!(app.world().resource::<ViewerActions>().queue.is_empty());
        assert!(app.world().resource::<crate::camera::CameraInputLock>().0);
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), before);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::End);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            1420.0
        );
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), before);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Home);
        app.update();
        assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);
        assert_eq!(*app.world().get::<Transform>(camera).unwrap(), before);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .get_mut::<Window>(window_id)
            .unwrap()
            .set_cursor_position(Some(Vec2::new(300.0, 200.0)));
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -1.0,
            window: window_id,
        });
        app.update();
        assert!(!app.world().resource::<crate::camera::CameraInputLock>().0);
        assert_ne!(*app.world().get::<Transform>(camera).unwrap(), before);
        assert_eq!(app.world().get::<ScrollPosition>(viewport).unwrap().y, 0.0);

        // Closing the panel releases the old panel area and even the compact
        // header to camera/page shortcuts, without throwing away its offset.
        app.world_mut().resource_mut::<Viewer>().inspector_expanded = false;
        app.world_mut().resource_mut::<Viewer>().inspector_scroll = 500.0;
        app.world_mut()
            .get_mut::<ScrollPosition>(viewport)
            .unwrap()
            .y = 500.0;
        app.world_mut()
            .get_mut::<Window>(window_id)
            .unwrap()
            .set_cursor_position(Some(details_rect(Vec2::new(1280.0, 900.0), false).center()));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::PageDown);
        app.world_mut().write_message(MouseWheel {
            unit: MouseScrollUnit::Line,
            x: 0.0,
            y: -1.0,
            window: window_id,
        });
        let camera_before_collapse = *app.world().get::<Transform>(camera).unwrap();
        app.update();
        assert!(!app.world().resource::<crate::camera::CameraInputLock>().0);
        assert!(!app.world().resource::<Viewer>().inspector_hovered);
        assert_eq!(
            app.world().get::<Node>(viewport).unwrap().display,
            Display::None
        );
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            500.0
        );
        assert_ne!(
            *app.world().get::<Transform>(camera).unwrap(),
            camera_before_collapse
        );
        assert!(matches!(
            app.world().resource::<ViewerActions>().queue.back(),
            Some((_, ViewerAction::NextPage))
        ));

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F1);
        app.update();
        assert!(matches!(
            app.world().resource::<ViewerActions>().queue.back(),
            Some((_, ViewerAction::ToggleDetails))
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .reset_all();
        app.world_mut().resource_mut::<Viewer>().inspector_expanded = true;
        app.update();
        assert_eq!(
            app.world().get::<Node>(viewport).unwrap().display,
            Display::Flex
        );
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().y,
            500.0
        );
    }

    #[test]
    fn details_toggle_stays_responsive_during_refinement_without_altering_scene() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        assert!(!app.world().resource::<LayoutStatus>().inspector_expanded);
        let accepted = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        let (_send, receive) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        app.world_mut().resource_mut::<Viewer>().job = Some(Job {
            action_id: None,
            cancel: cancel.clone(),
            receive: Mutex::new(receive),
            reorganize: true,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        for expanded in [true, false] {
            let ticket = app
                .world_mut()
                .resource_mut::<ViewerActions>()
                .submit(ViewerAction::ToggleDetails)
                .unwrap();
            app.update();
            let status = app.world().resource::<LayoutStatus>();
            assert_eq!(status.inspector_expanded, expanded);
            assert!(status.action_finished(ticket) && status.reorganizing);
            assert!(!cancel.load(Ordering::Relaxed));
            assert_eq!(
                app.world()
                    .resource::<Viewer>()
                    .accepted
                    .as_ref()
                    .unwrap()
                    .1,
                accepted
            );
        }
    }

    #[test]
    fn captions_have_two_bounded_lines_without_splitting_unicode() {
        assert_eq!(label_caption("Auth"), "Auth");
        let caption = label_caption(&"é界".repeat(40));
        assert_eq!(caption.lines().count(), 2);
        assert!(caption.lines().all(|line| line.chars().count() <= 27));
        assert!(caption.ends_with("..."));
        assert_eq!(
            label_caption("code package / Reference: bevy_ui"),
            "code package\nReference: bevy_ui"
        );
        assert_eq!(label_caption("one\n two\tthree"), "one two three");
    }

    #[test]
    fn label_packing_reserves_entire_caption_and_inspector_bounds() {
        let viewport = Vec2::new(1000.0, 800.0);
        let first = label_rect(Vec2::new(350.0, 200.0), viewport, &[]).unwrap();
        // The previous point-only rule admitted captions just 23 pixels apart.
        assert!(label_rect(Vec2::new(350.0, 223.0), viewport, &[first]).is_none());
        assert!(label_rect(Vec2::new(350.0, 250.0), viewport, &[first]).is_some());
        let inspector = Rect::from_corners(Vec2::new(24.0, 600.0), Vec2::new(976.0, 704.0));
        // Captions sit above their anchor, so one anchored just below the
        // inspector's top edge would overlap it.
        assert!(label_rect(Vec2::new(350.0, 610.0), viewport, &[inspector]).is_none());
        assert!(label_rect(Vec2::new(350.0, 590.0), viewport, &[inspector]).is_some());
        // A caption never leaves the top of the viewport.
        assert!(label_rect(Vec2::new(350.0, 40.0), viewport, &[]).is_none());
        assert!(label_rect(Vec2::new(30.0, 300.0), viewport, &[]).is_none());
        assert!(label_rect(Vec2::new(950.0, 300.0), viewport, &[]).is_none());
    }
    struct LocalTestFile(std::path::PathBuf);
    impl LocalTestFile {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            Self(std::env::temp_dir().join(format!(
                    "visualyze-reload-{}-{}-{}.jsonl",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                )))
        }
        fn write(&self, bytes: &[u8]) {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.0)
                .unwrap()
                .write_all(bytes)
                .unwrap();
        }
        fn replace(&self, bytes: &[u8]) {
            let staged = Self::new();
            staged.write(bytes);
            std::fs::rename(&staged.0, &self.0).unwrap();
        }
    }
    impl Drop for LocalTestFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn pipeline_file(path: &std::path::Path, revision: u64, reversed: bool) {
        use visualyze_model::provider::*;
        let mut writer = BundleWriter::create(
            path,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "reload-pipeline".into(),
                revision,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture],
                coverage: Coverage::Complete,
                scope_description: "Declared reload and pin regression".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        for (id, kind) in [
            ("a", EntityKind::Service),
            ("b", EntityKind::Service),
            ("root", EntityKind::Scope),
        ] {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        id: id.into(),
                        kind,
                        label: id.into(),
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: None,
                }))
                .unwrap();
        }
        let (source, target) = if reversed { ("b", "a") } else { ("a", "b") };
        for (id, kind, source, target) in [
            ("flow", RelationKind::Requests, source, target),
            ("owns:a", RelationKind::Contains, "root", "a"),
            ("owns:b", RelationKind::Contains, "root", "b"),
        ] {
            writer
                .push(&BundleRecord::Relation(RelationRecord {
                    id: id.into(),
                    kind,
                    source: source.into(),
                    target: target.into(),
                    claim: Claim::declared(),
                    label: None,
                    aspect: None,
                }))
                .unwrap();
        }
        writer.finish().unwrap();
    }
    /// A suggested group as the fallback grouper would have produced and the view
    /// retained. In a bundle, paging by noise keeps every page of one layer within
    /// the fallback noise, so suggested groups only appear on later pages of a
    /// level that mixes layers (ADR 0007), and the code lens has a single layer.
    /// These tests are about what happens to a retained suggestion (pins, focus,
    /// reload, dismissal), not about when one is made, so they seed it.
    fn seeded_suggestion(parent: &str, members: &[&str]) -> layout::projection::DisplayGroup {
        layout::projection::DisplayGroup {
            id: format!("view:suggested-group:{parent}:{}", members[0]),
            parent: parent.into(),
            members: members.iter().map(|id| (*id).to_owned()).collect(),
            suggested: true,
        }
    }
    fn grouped_code_file(path: &std::path::Path) {
        grouped_code_revision(path, 1, 130, true);
    }
    fn grouped_code_revision(path: &std::path::Path, revision: u64, files: usize, package: bool) {
        use visualyze_model::provider::*;
        let mut writer = BundleWriter::create(
            path,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "grouped-code-actions".into(),
                revision,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture, Profile::Code],
                coverage: Coverage::Complete,
                scope_description: "Declared code expansion and collapse regression".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        let mut entities = BTreeMap::from([
            ("root".to_owned(), EntityKind::Scope),
            ("repo".to_owned(), EntityKind::CodeRepository),
            ("package".to_owned(), EntityKind::CodePackage),
        ]);
        let mut relations = BTreeMap::from([
            (
                "owns:repo".to_owned(),
                (RelationKind::Contains, "root".to_owned(), "repo".to_owned()),
            ),
            (
                "owns:package".to_owned(),
                (
                    RelationKind::OwnsCode,
                    "repo".to_owned(),
                    "package".to_owned(),
                ),
            ),
        ]);
        if !package {
            entities.remove("package");
            relations.remove("owns:package");
        }
        for index in 0..files {
            let file = format!("file:{index:03}");
            let function = format!("function:{index:03}");
            entities.insert(file.clone(), EntityKind::CodeFile);
            entities.insert(function.clone(), EntityKind::CodeFunction);
            relations.insert(
                format!("owns:{file}"),
                (RelationKind::OwnsCode, "package".into(), file.clone()),
            );
            relations.insert(
                format!("owns:{function}"),
                (RelationKind::OwnsCode, file, function),
            );
        }
        for (id, kind) in entities {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        label: id.clone(),
                        id,
                        kind,
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: None,
                }))
                .unwrap();
        }
        for (id, (kind, source, target)) in relations {
            writer
                .push(&BundleRecord::Relation(RelationRecord {
                    id,
                    kind,
                    source,
                    target,
                    claim: Claim::declared(),
                    label: None,
                    aspect: None,
                }))
                .unwrap();
        }
        writer.finish().unwrap();
    }
    /// The data-messaging example with `extra` more services in its root scope.
    /// More than 24 same-layer children are noisy enough (ADR 0007) to be paged,
    /// which the small example never is.
    fn paged_messaging_file(path: &std::path::Path, extra: usize) {
        use visualyze_model::provider::*;
        let mut manifest = None;
        let mut records = Vec::new();
        for line in include_str!("../../../public/examples/data-messaging.bundle.jsonl").lines() {
            match serde_json::from_str::<BundleRecord>(line).unwrap() {
                BundleRecord::Manifest(value) => manifest = Some(value),
                BundleRecord::Complete(_) => (),
                record => records.push(record),
            }
        }
        for index in 0..extra {
            let id = format!("service:extra:{index:03}");
            records.push(BundleRecord::Entity(EntityRecord {
                summary: EntitySummary {
                    label: id.clone(),
                    id: id.clone(),
                    kind: EntityKind::Service,
                    boundary: Boundary::Internal,
                    claim: Claim::declared(),
                },
                detail: None,
            }));
            records.push(BundleRecord::Relation(RelationRecord {
                id: format!("rel:contains:{id}"),
                kind: RelationKind::Contains,
                source: "scope:orders".into(),
                target: id,
                claim: Claim::declared(),
                label: None,
                aspect: None,
            }));
        }
        records.sort_by_key(|record| match record {
            BundleRecord::Evidence(record) => (0, record.id.clone()),
            BundleRecord::Entity(record) => (1, record.summary.id.clone()),
            BundleRecord::Relation(record) => (2, record.id.clone()),
            _ => unreachable!(),
        });
        let mut writer = BundleWriter::create(path, manifest.unwrap(), Limits::default()).unwrap();
        for record in records {
            writer.push(&record).unwrap();
        }
        writer.finish().unwrap();
    }
    fn aggregated_claim_file(path: &std::path::Path) {
        use visualyze_model::provider::*;
        let mut manifest = None;
        let mut records = Vec::new();
        for line in include_str!("../../../public/examples/data-messaging.bundle.jsonl").lines() {
            match serde_json::from_str::<BundleRecord>(line).unwrap() {
                BundleRecord::Manifest(value) => manifest = Some(value),
                BundleRecord::Complete(_) => (),
                record => records.push(record),
            }
        }
        records.push(BundleRecord::Evidence(EvidenceRecord {
            id: "evidence:selected-claim".into(),
            description: "Synthetic selected-claim evidence".into(),
            reference: None,
        }));
        records.push(BundleRecord::Relation(RelationRecord {
            id: "rel:publishes-detail".into(),
            kind: RelationKind::Publishes,
            source: "service:orders".into(),
            target: "channel:orders".into(),
            claim: Claim {
                basis: Basis::Inferred,
                evidence_ids: vec!["evidence:selected-claim".into()],
                rationale: Some("Explicit synthetic inference for inspection".into()),
            },
            label: Some("Detailed publication claim".into()),
            aspect: None,
        }));
        records.sort_by_key(|record| match record {
            BundleRecord::Evidence(record) => (0, record.id.clone()),
            BundleRecord::Entity(record) => (1, record.summary.id.clone()),
            BundleRecord::Relation(record) => (2, record.id.clone()),
            _ => unreachable!(),
        });
        let mut writer = BundleWriter::create(path, manifest.unwrap(), Limits::default()).unwrap();
        for record in records {
            writer.push(&record).unwrap();
        }
        writer.finish().unwrap();
    }
    fn action_app(input: LayoutInput, mut view: ViewState) -> App {
        // Tests exercise the real worker, queue, publication and rendering ECS
        // without starting a window or a renderer.
        view.semantic_zoom = false;
        let p = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Floorplan,
        );
        view.frame_lens = Some(p.lens);
        let s = layout::solve(LayoutRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 4,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        let mut app = App::new();
        app.insert_resource(input)
            .insert_resource(LayoutEngine::Floorplan)
            .insert_resource(Viewer {
                view: view.clone(),
                accepted_view: view,
                accepted: Some((p, s.into())),
                dirty: true,
                ..default()
            })
            .init_resource::<LayoutStatus>()
            .init_resource::<ViewerActions>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<crate::style::GlassMaterial>>()
            .add_systems(Update, (process_actions, poll, navigation_status).chain());
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            let (p, s) = viewer.accepted.clone().unwrap();
            cache_layout(&mut viewer, p, s);
        }
        app.update();
        app
    }
    fn complete(app: &mut App, action: ViewerAction) -> u64 {
        let ticket = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(action)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            let status = app.world().resource::<LayoutStatus>();
            if status.completed_action_id >= ticket && !status.loading {
                return ticket;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "action timed out: {}",
                app.world().resource::<Viewer>().message
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    struct LocalTestDirectory(std::path::PathBuf);
    impl LocalTestDirectory {
        fn new() -> Self {
            let path = LocalTestFile::new().0.clone().with_extension("states");
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for LocalTestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn cold_app(input: LayoutInput, directory: &std::path::Path) -> App {
        let mut viewer = Viewer {
            storage: Some(directory.into()),
            ..default()
        };
        start(&input, LayoutEngine::Floorplan, &mut viewer, false);
        let mut app = App::new();
        app.insert_resource(input)
            .insert_resource(LayoutEngine::Floorplan)
            .insert_resource(viewer)
            .init_resource::<LayoutStatus>()
            .init_resource::<ViewerActions>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<crate::style::GlassMaterial>>()
            .add_systems(Update, (process_actions, poll, navigation_status).chain());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            if app.world().resource::<Viewer>().job.is_none() {
                return app;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "startup restore timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    #[test]
    fn publication_and_acceptance_use_one_exact_cut_despite_late_snapshot() {
        let queued = Instant::now();
        let after_ms = |milliseconds| queued + std::time::Duration::from_millis(milliseconds);
        let timing = JobTiming {
            record: LayoutTiming {
                indexing_ms: 3.0,
                projection_ms: 1.0,
                solve_ms: 1.0,
                ..default()
            },
            queued,
            started: Some(after_ms(2)),
            finished: Some(after_ms(7)),
            active: None,
        };
        let acceptance_started = after_ms(10);
        // Simulate three milliseconds of metadata restoration/mutex work before
        // the snapshot is taken. The same earlier cut is still authoritative.
        let snapshot_taken = after_ms(13);
        let acceptance_finished = after_ms(16);
        let mut measured = timing_snapshot_at(&timing, "accepted", acceptance_started);
        measured.acceptance_ms = acceptance_finished
            .duration_since(acceptance_started)
            .as_secs_f64()
            * 1000.0;
        measured.scene_build_ms = 1.0;
        measured.save_ms = 2.0;
        measured.total_ms = 19.0;
        assert_eq!(measured.publication_wait_ms, 3.0);
        assert_eq!(measured.queued_at, Some(queued));
        assert_eq!(measured.worker_finished_at, Some(after_ms(7)));
        assert_eq!(measured.observed_at, Some(acceptance_started));
        assert_eq!(measured.acceptance_ms, 6.0);
        assert_eq!(measured.worker_total_ms, 5.0);
        assert_eq!(measured.preparation_ms, 0.0);
        let accounted = measured.worker_queue_ms
            + measured.worker_total_ms
            + measured.publication_wait_ms
            + measured.acceptance_ms
            + measured.scene_build_ms
            + measured.save_ms;
        assert!((accounted - measured.total_ms).abs() <= f64::EPSILON * measured.total_ms * 64.0);
        // Regress the former boundary: using snapshot time instead would count
        // restoration/mutex work in both publication wait and acceptance.
        let late = timing_snapshot_at(&timing, "accepted", snapshot_taken);
        assert_eq!(late.publication_wait_ms - measured.publication_wait_ms, 3.0);
    }
    #[test]
    fn projection_reacquisition_reports_contended_source_wait_separately() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        input
            .prepare(&AtomicBool::new(false), &mut SourceTimings::default())
            .unwrap();
        let crate::layout_input::Source::Bundle { provider, .. } = input.0.as_ref() else {
            unreachable!()
        };
        let held = provider.lock().unwrap();
        let timing = Arc::new(Mutex::new(JobTiming::default()));
        let worker_timing = timing.clone();
        let worker_input = input.clone();
        let worker = std::thread::spawn(move || {
            worker_timing.lock().unwrap().started = Some(Instant::now());
            let result = timed_projection(
                &worker_timing,
                &worker_input,
                &ViewState::default(),
                &AtomicBool::new(false),
            );
            worker_timing.lock().unwrap().finished = Some(Instant::now());
            result
        });
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while timing.lock().unwrap().active.is_none() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // Contend the second acquisition, after prepare() already indexed and
        // released this source. This models a search/retiring worker taking it.
        std::thread::sleep(std::time::Duration::from_millis(25));
        drop(held);
        worker.join().unwrap().unwrap();
        let measured = timing_snapshot(&timing.lock().unwrap(), "accepted");
        assert!(measured.source_lock_wait_ms >= 20.0, "{measured:?}");
        assert_eq!(measured.indexing_ms, 0.0);
        assert_eq!(measured.snapshot_read_ms, 0.0);
        assert!(measured.projection_ms > 0.0);
        assert!(
            (measured.projection_ms + measured.source_lock_wait_ms + measured.preparation_ms
                - measured.worker_total_ms)
                .abs()
                < 0.001
        );
    }
    #[test]
    fn job_timings_measure_indexing_actual_cache_hits_and_matching_actions() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let directory = LocalTestDirectory::new();
        let mut app = cold_app(LayoutInput::bundle(file.0.clone()), &directory.0);
        let initial = app.world().resource::<LayoutStatus>().layout_timings[0].clone();
        assert_eq!(initial.cache_outcome, "miss");
        assert_eq!(initial.outcome, "accepted");
        assert_eq!(initial.action_id, None);
        assert_eq!(initial.revision, Some(1));
        assert!(initial.indexing_ms > 0.0 && initial.projection_ms > 0.0);
        assert!(initial.solve_ms > 0.0 && initial.save_ms > 0.0);
        let enter = complete(&mut app, ViewerAction::Enter("a".into()));
        let back = complete(&mut app, ViewerAction::Back);
        let status = app.world().resource::<LayoutStatus>();
        let entered = status
            .layout_timings
            .iter()
            .find(|record| record.action_id == Some(enter))
            .unwrap();
        assert_eq!(entered.cache_outcome, "miss");
        assert_eq!(entered.indexing_ms, 0.0);
        let returned = status
            .layout_timings
            .iter()
            .find(|record| record.action_id == Some(back))
            .unwrap();
        assert_eq!(returned.cache_outcome, "hit");
        assert_eq!(returned.solve_ms, 0.0);
        assert_eq!(returned.indexing_ms, 0.0);
        assert!(returned.restoration_ms > 0.0);
        for record in &status.layout_timings {
            assert_eq!(record.revision, Some(1));
            let stages = record.indexing_ms
                + record.snapshot_read_ms
                + record.source_lock_wait_ms
                + record.projection_ms
                + record.restoration_ms
                + record.solve_ms
                + record.preparation_ms;
            let roundoff = f64::EPSILON * record.worker_total_ms.max(1.0) * 64.0;
            assert!((stages - record.worker_total_ms).abs() <= roundoff);
            let phases = record.worker_queue_ms
                + record.worker_total_ms
                + record.publication_wait_ms
                + record.acceptance_ms
                + record.scene_build_ms
                + record.save_ms;
            let roundoff = f64::EPSILON * phases.max(record.total_ms).max(1.0) * 64.0;
            assert!(phases <= record.total_ms + roundoff, "{record:?}");
            assert!(record.partial_phase.is_none());
        }
        let failed = complete(&mut app, ViewerAction::Jump("absent".into()));
        let record = app
            .world()
            .resource::<LayoutStatus>()
            .layout_timings
            .back()
            .unwrap();
        assert_eq!(record.action_id, Some(failed));
        assert_eq!(record.outcome, "failed");
        assert_eq!(record.cache_outcome, "not_checked");
        assert_eq!(record.revision, None); // Projection never returned a revision.
        assert!(record.projection_ms > 0.0);
        assert_eq!(record.save_ms, 0.0);
        let corrupt = LocalTestFile::new();
        corrupt.write(b"invalid bundle\n");
        let directory = LocalTestDirectory::new();
        let failed_source = cold_app(LayoutInput::bundle(corrupt.0.clone()), &directory.0);
        let record = failed_source
            .world()
            .resource::<LayoutStatus>()
            .layout_timings
            .back()
            .unwrap();
        assert_eq!(record.outcome, "failed");
        assert!(record.indexing_ms > 0.0);
        assert_eq!(record.projection_ms, 0.0);
        assert_eq!(record.solve_ms, 0.0);
        assert_eq!(record.revision, None);
    }
    #[test]
    fn cancelled_job_timings_are_frozen_and_late_results_cannot_overwrite_them() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let (p, s) = app.world().resource::<Viewer>().accepted.clone().unwrap();
        let now = Instant::now();
        let timing = Arc::new(Mutex::new(JobTiming {
            record: LayoutTiming {
                job_id: 100,
                action_id: Some(88),
                revision: Some(1),
                cache_outcome: "miss",
                ..default()
            },
            queued: now - std::time::Duration::from_millis(6),
            started: Some(now - std::time::Duration::from_millis(5)),
            finished: None,
            active: Some(("solve", now - std::time::Duration::from_millis(4))),
        }));
        let (send, receive) = mpsc::sync_channel(1);
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.next_job_id = 100;
            viewer.job = Some(Job {
                action_id: Some(88),
                cancel: Arc::new(AtomicBool::new(false)),
                receive: Mutex::new(receive),
                reorganize: false,
                replacement_input: None,
                restored_view: default(),
                restored_dismissals: default(),
                restored_composed: default(),
                canonical_cache: default(),
                timing: timing.clone(),
            });
        }
        complete(&mut app, ViewerAction::Cancel);
        let cancelled = app
            .world()
            .resource::<LayoutStatus>()
            .layout_timings
            .back()
            .unwrap()
            .clone();
        assert_eq!(cancelled.outcome, "cancelled");
        assert_eq!(cancelled.partial_phase, Some("solve"));
        assert!(cancelled.partial_phase_ms >= 4.0);
        assert!(send.send(Ok((p, s, ViewState::default()))).is_err());
        // A retired worker still owns its timing Arc until it finishes. Its late
        // writes must not mutate the published cancellation or any newer job.
        timing.lock().unwrap().record.revision = Some(999);
        timing.lock().unwrap().record.indexing_ms = 9999.0;
        let ticket = complete(&mut app, ViewerAction::Enter("a".into()));
        let status = app.world().resource::<LayoutStatus>();
        let old = status
            .layout_timings
            .iter()
            .find(|record| record.job_id == 100)
            .unwrap();
        assert_eq!(old.revision, Some(1));
        assert_eq!(old.indexing_ms, cancelled.indexing_ms);
        assert_eq!(old.outcome, "cancelled");
        let latest = status.layout_timings.back().unwrap();
        assert_eq!(latest.action_id, Some(ticket));
        assert_eq!(latest.revision, Some(1));
        assert_eq!(latest.outcome, "accepted");
    }
    #[test]
    fn timing_history_is_bounded_and_duplicate_jobs_do_not_replace_records() {
        let mut status = LayoutStatus::default();
        for job_id in 1..=100 {
            record_timing(
                &mut status,
                LayoutTiming {
                    job_id,
                    outcome: "accepted",
                    ..default()
                },
            );
        }
        record_timing(
            &mut status,
            LayoutTiming {
                job_id: 100,
                outcome: "failed",
                ..default()
            },
        );
        assert_eq!(status.layout_timings.len(), 64);
        assert_eq!(status.layout_timings.front().unwrap().job_id, 37);
        assert_eq!(status.layout_timings.back().unwrap().outcome, "accepted");
    }
    fn saved_pipeline(input: &LayoutInput) -> layout::persistence::PersistedView {
        let app = action_app(input.clone(), ViewState::default());
        let viewer = app.world().resource::<Viewer>();
        let mut view = viewer.accepted_view.clone();
        let mut accepted = viewer.accepted.as_ref().unwrap().1.flat().unwrap().clone();
        // An obvious prior arrangement verifies that restoration does not merely
        // produce the same deterministic fresh layout and happen to pass.
        let offset = Point::new(400, 160, -240);
        for item in accepted.boxes.values_mut() {
            item.bounds = item.bounds.translated(offset);
        }
        for route in &mut accepted.routes {
            for point in &mut route.points {
                *point = *point + offset;
            }
            for port in &mut route.scope_ports {
                port.point = port.point + offset;
            }
        }
        view.pins
            .insert("a".into(), accepted.boxes["a"].bounds.center());
        view.save_frame_pins();
        let p = engine_projection(
            input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Floorplan,
        );
        accepted.projection_key = p.key.clone();
        layout::validate(&p, &accepted, &view.pins, 2).unwrap();
        accepted.metrics.structural_cost = 987654321.0;
        layout::persistence::PersistedView {
            version: layout::persistence::STATE_VERSION,
            model_id: accepted.model_id.clone(),
            revision: accepted.revision,
            view,
            accepted,
            dismissed_proposals: vec![],
        }
    }
    fn saved_region_membership_fixture(file: &LocalTestFile, extra_child: bool) -> LayoutInput {
        use visualyze_model::provider::*;
        let mut writer = BundleWriter::create(
            &file.0,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "saved-region-membership".into(),
                revision: 1,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture],
                coverage: Coverage::Complete,
                scope_description: "Declared startup page-membership regression".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        let mut entities = BTreeMap::from([
            ("a", (EntityKind::Scope, Some("root"))),
            ("a1", (EntityKind::Service, Some("a"))),
            ("b", (EntityKind::Scope, Some("root"))),
            ("b1", (EntityKind::Service, Some("b"))),
            ("root", (EntityKind::Scope, None)),
        ]);
        if extra_child {
            entities.insert("a2", (EntityKind::Service, Some("a")));
        }
        for (id, (kind, _)) in &entities {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        id: (*id).into(),
                        kind: *kind,
                        label: (*id).into(),
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: None,
                }))
                .unwrap();
        }
        for (id, (_, parent)) in &entities {
            if let Some(parent) = parent {
                writer
                    .push(&BundleRecord::Relation(RelationRecord {
                        id: format!("owns:{id}"),
                        kind: RelationKind::Contains,
                        source: (*parent).into(),
                        target: (*id).into(),
                        claim: Claim::declared(),
                        label: None,
                        aspect: None,
                    }))
                    .unwrap();
            }
        }
        writer.finish().unwrap();
        LayoutInput::bundle(file.0.clone())
    }
    #[test]
    fn startup_membership_change_repairs_from_saved_unpinned_geometry() {
        let old_file = LocalTestFile::new();
        let new_file = LocalTestFile::new();
        let old_input = saved_region_membership_fixture(&old_file, false);
        let new_input = saved_region_membership_fixture(&new_file, true);
        let mut view = ViewState::default();
        view.expanded = BTreeSet::from(["a".into(), "b".into()]);
        view.semantic_zoom = false;
        view.policy = layout::projection::LayoutPolicy::Peers;
        let p = engine_projection(
            old_input.project(&view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Floorplan,
        );
        view.frame_lens = Some(p.lens);
        // Deliberately nondefault size and distant position: a fresh solve
        // cannot coincidentally reproduce the unaffected unpinned neighbor.
        let mut seeded = p.clone();
        let leaf = seeded
            .graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "b1")
            .unwrap();
        leaf.preferred_size = leaf.min_size;
        let accepted = layout::solve(LayoutRequest {
            projection: &seeded,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::from([
                ("a".into(), Point::new(400, 160, -240)),
                ("b".into(), Point::new(1000, 160, -240)),
            ]),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 4,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        view.pins
            .insert("a".into(), accepted.boxes["a"].bounds.center());
        view.save_frame_pins();
        layout::validate(&p, &accepted, &view.pins, 2).unwrap();
        let saved = layout::persistence::PersistedView {
            version: layout::persistence::STATE_VERSION,
            model_id: accepted.model_id.clone(),
            revision: accepted.revision,
            view,
            accepted,
            dismissed_proposals: vec![],
        };
        for conflicting_pins in [false, true] {
            let directory = LocalTestDirectory::new();
            let mut saved = saved.clone();
            if conflicting_pins {
                saved.view.pins.insert("b".into(), saved.view.pins["a"]);
                saved.view.save_frame_pins();
            }
            let path = saved_path(
                &directory.0,
                &state_identity(&saved.model_id, LayoutEngine::Floorplan),
            );
            layout::persistence::save(&path, &saved).unwrap();
            let original_bytes = std::fs::read(&path).unwrap();
            let app = cold_app(new_input.clone(), &directory.0);
            let viewer = app.world().resource::<Viewer>();
            assert_eq!(
                app.world()
                    .resource::<LayoutStatus>()
                    .layout_timings
                    .back()
                    .unwrap()
                    .cache_outcome,
                "saved_repair"
            );
            assert_eq!(viewer.view.pins, saved.view.pins);
            if conflicting_pins {
                assert!(viewer.accepted.is_none());
                assert!(viewer.message.contains("original state file preserved"));
                assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
                assert_eq!(app.world().resource::<LayoutStatus>().save_attempts, 0);
            } else {
                let (projection, repaired) = viewer.accepted.as_ref().expect(&viewer.message);
                let repaired = repaired.flat().unwrap();
                assert_ne!(projection.key, saved.accepted.projection_key);
                assert!(repaired.boxes.contains_key("a2"));
                assert_eq!(repaired.boxes["b"], saved.accepted.boxes["b"]);
                assert_eq!(repaired.boxes["b1"], saved.accepted.boxes["b1"]);
                assert_eq!(repaired.boxes["a"].bounds.center(), saved.view.pins["a"]);
                layout::validate(projection, repaired, &saved.view.pins, 2).unwrap();
                assert!(projection.detail_note.contains("previous positions"));
                assert!(viewer.proposal.is_none());
            }
        }
    }
    #[test]
    fn saved_repair_rejects_a_different_or_unproven_coordinate_frame() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        let saved = saved_pipeline(&input);
        let current = engine_projection(
            input.project(&saved.view, &AtomicBool::new(false)).unwrap(),
            LayoutEngine::Floorplan,
        );
        assert!(saved_repair_matches_frame(
            &saved.view,
            &saved.accepted,
            &current
        ));
        let mut changed = current.clone();
        changed.graph.key.push_str("|changed-membership");
        assert!(saved_repair_matches_frame(
            &saved.view,
            &saved.accepted,
            &changed
        ));
        let mut other_view = saved.view.clone();
        other_view.frame_lens = Some(SemanticLens::Deployment);
        assert!(!saved_repair_matches_frame(
            &other_view,
            &saved.accepted,
            &changed
        ));
        other_view.frame_lens = None;
        other_view.lens = None;
        assert!(!saved_repair_matches_frame(
            &other_view,
            &saved.accepted,
            &changed
        ));
        assert!(saved_repair_matches_frame(
            &other_view,
            &saved.accepted,
            &current
        ));
        changed = current.clone();
        changed.graph.root = "a".into();
        assert!(!saved_repair_matches_frame(
            &saved.view,
            &saved.accepted,
            &changed
        ));
        let mut invalid = saved.accepted.clone();
        invalid.version += 1;
        assert!(!saved_repair_matches_frame(&saved.view, &invalid, &current));
    }
    #[test]
    fn prior_quality_and_legacy_state_files_preserve_valid_positions_and_pins() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        let original = saved_pipeline(&input);
        let engine = LayoutEngine::Floorplan;
        let current_suffix = format!("|quality:{QUALITY_VERSION}");
        let base_key = original
            .accepted
            .projection_key
            .strip_suffix(&current_suffix)
            .unwrap();
        let cases = [
            (
                quality_state_identity(&original.model_id, engine, 3),
                format!("{base_key}|quality:3"),
            ),
            (
                quality_state_identity(&original.model_id, engine, 2),
                format!("{base_key}|quality:2"),
            ),
            (
                quality_state_identity(&original.model_id, engine, 1),
                format!("{base_key}|quality:1"),
            ),
            (
                format!(
                    "{}|{engine:?}|layout:{}",
                    original.model_id,
                    layout::LAYOUT_VERSION
                ),
                base_key.into(),
            ),
            (format!("{}|{engine:?}", original.model_id), base_key.into()),
        ];
        for (identity, key) in cases {
            if identity == state_identity(&original.model_id, engine) {
                continue;
            }
            let directory = LocalTestDirectory::new();
            let mut saved = original.clone();
            saved.accepted.projection_key = key;
            let old_path = saved_path(&directory.0, &identity);
            layout::persistence::save(&old_path, &saved).unwrap();
            let before = std::fs::read(&old_path).unwrap();
            let app = cold_app(input.clone(), &directory.0);
            let viewer = app.world().resource::<Viewer>();
            assert_eq!(
                app.world()
                    .resource::<LayoutStatus>()
                    .layout_timings
                    .back()
                    .unwrap()
                    .cache_outcome,
                "saved_exact"
            );
            assert!(
                app.world().resource::<LayoutStatus>().error.is_none(),
                "{}",
                viewer.message
            );
            let (projection, restored) = viewer.accepted.as_ref().unwrap();
            let restored = restored.flat().unwrap();
            assert_eq!(restored.boxes, original.accepted.boxes);
            assert_eq!(restored.routes, original.accepted.routes);
            assert_eq!(viewer.view.pins, original.view.pins);
            assert_eq!(viewer.view.frame_pins, original.view.frame_pins);
            assert_eq!(restored.projection_key, original.accepted.projection_key);
            assert_eq!(
                restored.metrics,
                layout::measure(projection, restored, None)
            );
            assert_ne!(
                restored.metrics.structural_cost,
                saved.accepted.metrics.structural_cost
            );
            assert!(viewer.proposal.is_none());
            assert_eq!(std::fs::read(&old_path).unwrap(), before);
            let current = layout::persistence::load(saved_path(
                &directory.0,
                &state_identity(&saved.model_id, engine),
            ))
            .unwrap();
            assert_eq!(current.accepted.boxes, saved.accepted.boxes);
            assert_eq!(current.view.pins, saved.view.pins);
        }
    }
    #[test]
    fn incompatible_saved_geometry_keeps_constraints_and_original_file_on_failure() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        let directory = LocalTestDirectory::new();
        let mut saved = saved_pipeline(&input);
        // These conflicting pins cannot both fit. Re-solving must not drop them.
        saved.view.pins.insert("b".into(), saved.view.pins["a"]);
        saved.view.save_frame_pins();
        let old_path = saved_path(
            &directory.0,
            &quality_state_identity(&saved.model_id, LayoutEngine::Floorplan, 2),
        );
        layout::persistence::save(&old_path, &saved).unwrap();
        let before = std::fs::read(&old_path).unwrap();
        let mut app = cold_app(input, &directory.0);
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_none());
        assert_eq!(viewer.view.pins, saved.view.pins);
        assert!(
            viewer
                .message
                .contains("saved pins and local overrides retained"),
            "{}",
            viewer.message
        );
        assert_eq!(std::fs::read(&old_path).unwrap(), before);
        let current_path = saved_path(
            &directory.0,
            &state_identity(&saved.model_id, LayoutEngine::Floorplan),
        );
        assert!(!current_path.exists());
        // An explicit correction retries retained state instead of reading the
        // conflicting file again or losing the other pin.
        complete(&mut app, ViewerAction::Unpin("b".into()));
        let viewer = app.world().resource::<Viewer>();
        assert!(viewer.accepted.is_some(), "{}", viewer.message);
        assert_eq!(viewer.view.pins.get("a"), saved.view.pins.get("a"));
        assert!(!viewer.view.pins.contains_key("b"));
        assert_eq!(std::fs::read(&old_path).unwrap(), before);
    }
    #[test]
    fn present_current_file_is_never_bypassed_by_a_prior_quality_file() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let input = LayoutInput::bundle(file.0.clone());
        let directory = LocalTestDirectory::new();
        let saved = saved_pipeline(&input);
        let old_path = saved_path(
            &directory.0,
            &quality_state_identity(&saved.model_id, LayoutEngine::Floorplan, 2),
        );
        layout::persistence::save(&old_path, &saved).unwrap();
        let old_bytes = std::fs::read(&old_path).unwrap();
        let current_path = saved_path(
            &directory.0,
            &state_identity(&saved.model_id, LayoutEngine::Floorplan),
        );
        std::fs::write(&current_path, b"invalid current state").unwrap();
        let app = cold_app(input, &directory.0);
        assert!(app.world().resource::<Viewer>().accepted.is_none());
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .error
                .as_ref()
                .unwrap()
                .contains("Saved state preserved")
        );
        assert_eq!(
            std::fs::read(&current_path).unwrap(),
            b"invalid current state"
        );
        assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
    }
    #[test]
    fn quality_migration_does_not_retag_other_projection_or_unknown_profiles() {
        let (_, native) = solved();
        let mut solution = native.flat().unwrap().clone();
        let current = format!("projection|engine:Floorplan|quality:{QUALITY_VERSION}");
        for old in [
            "different|engine:Floorplan|quality:2",
            "projection|engine:Layered|quality:2",
            "projection|engine:Floorplan|quality:99",
        ] {
            solution.projection_key = old.into();
            migrate_quality_key(&mut solution, &current);
            assert_eq!(solution.projection_key, old);
        }
    }
    #[test]
    fn actions_are_bounded_fifo_with_cancel_available_even_when_full() {
        let mut actions = ViewerActions::default();
        for expected in 1..=64 {
            assert_eq!(actions.submit(ViewerAction::Back).unwrap(), expected);
        }
        assert!(actions.submit(ViewerAction::Back).is_err());
        assert_eq!(actions.submit(ViewerAction::Cancel).unwrap(), 65);
        assert_eq!(actions.queue.len(), 1);
        assert!(matches!(
            actions.queue.front().unwrap(),
            (65, ViewerAction::Cancel)
        ));
    }
    #[test]
    fn interaction_completion_ranges_stay_bounded_behind_a_pending_ticket() {
        let mut status = LayoutStatus::default();
        for id in 2..20_000 {
            finish_ticket(&mut status, Some(id), None);
        }
        assert_eq!(status.completed_action_id, 0);
        assert_eq!(status.completed_interactions.len(), 1);
        assert!(status.action_finished(19_999));
        assert!(!status.action_finished(1));
        finish_ticket(&mut status, Some(1), None);
        assert_eq!(status.completed_action_id, 19_999);
        assert!(status.completed_interactions.is_empty());
    }
    #[test]
    fn unicode_search_text_is_bounded_without_shortcut_control_characters() {
        let mut query = "x".repeat(254);
        append_search_text(&mut query, "é🦀\n\t");
        assert_eq!(query.len(), 256);
        assert_eq!(query.pop(), Some('é'));
        append_search_text(&mut query, "\r\n");
        assert_eq!(query.len(), 254);
    }
    #[test]
    fn native_actions_navigate_search_page_and_recover_from_invalid_jump() {
        // Thirty extra services in the root scope make the level noisy enough
        // to be paged by noise (24 per page), so there is a next page.
        let file = LocalTestFile::new();
        paged_messaging_file(&file.0, 30);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let original = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        assert!(app.world().resource::<LayoutStatus>().has_next_page);
        complete(&mut app, ViewerAction::NextPage);
        assert_eq!(app.world().resource::<LayoutStatus>().child_page, 1);
        assert!(app.world().resource::<LayoutStatus>().has_previous_page);
        complete(&mut app, ViewerAction::PreviousPage);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            original
        );
        complete(&mut app, ViewerAction::Search("Orders package".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .search_results
                .iter()
                .any(|hit| hit.id == "code:package")
        );
        complete(&mut app, ViewerAction::Jump("code:package".into()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("code:package")
        );
        complete(&mut app, ViewerAction::Enter("code:file".into()));
        complete(&mut app, ViewerAction::Enter("code:function".into()));
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.focus.as_deref(), Some("code:function"));
        assert_eq!(
            status.breadcrumbs,
            ["Overview", "code:package", "code:file", "code:function"]
        );
        let accepted = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        complete(&mut app, ViewerAction::Jump("not:supplied".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_some()
        );
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            accepted
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("code:function")
        );
        complete(&mut app, ViewerAction::Back);
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("code:file")
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
    }
    #[test]
    fn semantic_views_restore_their_own_pins_and_auto_follows_search_focus() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut app = action_app(LayoutInput::bundle(path), ViewState::default());
        assert_eq!(
            app.world().resource::<LayoutStatus>().lens,
            Some(SemanticLens::Logical)
        );
        complete(&mut app, ViewerAction::Pin("datastore:orders".into()));
        let point = app.world().resource::<Viewer>().view.pins["datastore:orders"];
        complete(
            &mut app,
            ViewerAction::SetGroup {
                id: "view:lens-group".into(),
                parent: "scope:orders".into(),
                members: vec!["client:web".into(), "service:orders".into()],
            },
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        complete(&mut app, ViewerAction::SetLens(Some(SemanticLens::Code)));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none(),
            "{:?}",
            app.world().resource::<LayoutStatus>().action_error
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().lens,
            Some(SemanticLens::Code)
        );
        assert!(app.world().resource::<Viewer>().view.pins.is_empty());
        complete(&mut app, ViewerAction::Jump("code:package".into()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("code:package")
        );
        complete(&mut app, ViewerAction::SetLens(Some(SemanticLens::Logical)));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        assert!(app.world().resource::<LayoutStatus>().focus.is_none());
        assert_eq!(
            app.world().resource::<Viewer>().view.pins["datastore:orders"],
            point
        );
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(
            app.world().resource::<LayoutStatus>().lens,
            Some(SemanticLens::Code)
        );
        complete(&mut app, ViewerAction::SetLens(None));
        assert_eq!(
            app.world().resource::<LayoutStatus>().lens,
            Some(SemanticLens::Logical)
        );
        assert_eq!(
            app.world().resource::<Viewer>().view.pins["datastore:orders"],
            point
        );
        complete(&mut app, ViewerAction::Jump("code:function".into()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().lens,
            Some(SemanticLens::Code)
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
    }
    #[test]
    fn entering_a_reference_follows_its_supplied_identity() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut app = action_app(LayoutInput::bundle(path), ViewState::default());
        complete(&mut app, ViewerAction::Enter("service:orders".into()));
        let (display_id, semantic_id) = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0
            .references
            .iter()
            .next()
            .map(|(display, semantic)| (display.clone(), semantic.clone()))
            .unwrap();
        complete(&mut app, ViewerAction::Enter(display_id));
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.focus.as_ref(), Some(&semantic_id));
        assert!(status.action_error.is_none());
    }
    #[test]
    fn exact_claim_selection_isolates_its_aggregate_and_preserves_semantics_and_geometry() {
        let file = LocalTestFile::new();
        aggregated_claim_file(&file.0);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let inspector = app.world_mut().spawn((Inspector, Text::default())).id();
        let original = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        complete(
            &mut app,
            ViewerAction::SelectRelation("rel:publishes-detail".into()),
        );
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(viewer.selected.as_deref(), Some("service:orders"));
        let p = &viewer.accepted.as_ref().unwrap().0;
        let claim = &p.claims["rel:publishes-detail"];
        let inspected = claim_text(claim);
        for expected in [
            "service:orders",
            "publishes",
            "channel:orders",
            "evidence:selected-claim",
            "Explicit synthetic inference",
        ] {
            assert!(inspected.contains(expected), "{inspected}");
        }
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(
            status.selected_relation.as_deref(),
            Some("rel:publishes-detail")
        );
        assert_eq!(
            status.selected_relation_members,
            vec!["rel:publishes", "rel:publishes-detail"]
        );
        let text = &app.world().get::<Text>(inspector).unwrap().0;
        assert!(text.contains("Selected component: Orders service"));
        assert!(text.contains("evidence:selected-claim") && text.contains("rel:publishes-detail"));
        complete(&mut app, ViewerAction::ToggleIsolate);
        let world = app.world_mut();
        let mut relations = world.query::<(&PlacedRelation, &Children)>();
        let displayed: Vec<_> = relations.iter(world).collect();
        assert_eq!(displayed.len(), 1);
        let (relation, children) = displayed[0];
        assert_eq!(relation.id, "rel:publishes-detail");
        assert_eq!(relation.basis, Basis::Inferred);
        assert_eq!(
            (relation.source.as_str(), relation.target.as_str()),
            ("service:orders", "channel:orders")
        );
        assert_eq!(
            relation.semantic_ids,
            vec!["rel:publishes", "rel:publishes-detail"]
        );
        let material = world
            .get::<MeshMaterial3d<StandardMaterial>>(children[0])
            .unwrap();
        assert_eq!(
            world
                .resource::<Assets<StandardMaterial>>()
                .get(&material.0)
                .unwrap()
                .base_color,
            style::PATH_HIGHLIGHT
        );
        let mut objects = world.query::<&PlacedObject>();
        let ids: std::collections::BTreeSet<_> = objects
            .iter(world)
            .map(|object| object.id.as_str())
            .collect();
        assert!(
            ids.contains("service:orders")
                && ids.contains("channel:orders")
                && ids.contains("scope:orders")
        );
        assert!(!ids.contains("client:web") && !ids.contains("datastore:orders"));
        assert_eq!(
            world.resource::<Viewer>().accepted.as_ref().unwrap().1,
            original
        );
        complete(&mut app, ViewerAction::SelectRelation("not-loaded".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_some()
        );
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .selected_relation
                .as_deref(),
            Some("rel:publishes-detail")
        );
        complete(&mut app, ViewerAction::ClearRelationSelection);
        let world = app.world_mut();
        assert!(world.resource::<Viewer>().isolate);
        assert!(world.query::<&PlacedRelation>().iter(world).count() > 1);
        assert_eq!(
            world.resource::<Viewer>().accepted.as_ref().unwrap().1,
            original
        );
    }
    #[test]
    fn claim_keyboard_actions_cycle_original_ids_and_navigation_clears_selection() {
        let file = LocalTestFile::new();
        aggregated_claim_file(&file.0);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        complete(&mut app, ViewerAction::Select("service:orders".into()));
        let claims: Vec<_> = selected_claims(app.world().resource::<Viewer>())
            .into_iter()
            .cloned()
            .collect();
        assert!(claims.contains(&"rel:publishes-detail".into()));
        for expected in &claims {
            complete(&mut app, ViewerAction::NextRelation);
            let status = app.world().resource::<LayoutStatus>();
            assert_eq!(status.selected_relation.as_ref(), Some(expected));
            assert!(status.inspected_relation_ids.contains(expected));
        }
        complete(&mut app, ViewerAction::NextRelation);
        assert_eq!(
            app.world()
                .resource::<LayoutStatus>()
                .selected_relation
                .as_ref(),
            claims.first()
        );
        complete(&mut app, ViewerAction::PreviousRelation);
        assert_eq!(
            app.world()
                .resource::<LayoutStatus>()
                .selected_relation
                .as_ref(),
            claims.last()
        );
        complete(&mut app, ViewerAction::Enter("datastore:orders".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .selected_relation
                .is_none()
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .selected_relation_members
                .is_empty()
        );
    }
    #[test]
    fn manual_group_limit_matches_the_provider_and_existing_groups_remain_editable() {
        let mut viewer = Viewer::default();
        for index in 0..64 {
            set_group(
                &mut viewer,
                format!("view:{index}"),
                "root".into(),
                vec![format!("a:{index}"), format!("b:{index}")],
            )
            .unwrap();
        }
        let error = set_group(
            &mut viewer,
            "view:65".into(),
            "root".into(),
            vec!["new:a".into(), "new:b".into()],
        )
        .unwrap_err();
        assert!(error.contains("64 local groups"));
        assert_eq!(viewer.view.group_overrides.len(), 64);
        set_group(
            &mut viewer,
            "view:0".into(),
            "root".into(),
            vec!["new:a".into(), "new:b".into()],
        )
        .unwrap();
        assert_eq!(viewer.view.group_overrides.len(), 64);
    }
    #[test]
    fn accepted_scene_interactions_remain_live_while_layout_is_pending() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let first = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::Reorganize)
            .unwrap();
        app.world_mut()
            .resource_mut::<ViewerActions>()
            .queue
            .pop_front();
        let (send, receive) = mpsc::sync_channel(1);
        let (p, s) = app.world().resource::<Viewer>().accepted.clone().unwrap();
        app.world_mut().resource_mut::<Viewer>().job = Some(Job {
            action_id: Some(first),
            cancel: Arc::new(AtomicBool::new(false)),
            receive: Mutex::new(receive),
            reorganize: false,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        let queued = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::Enter("a".into()))
            .unwrap();
        let selected = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::Select("b".into()))
            .unwrap();
        app.update();
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.loading && status.action_finished(selected));
        assert!(!status.action_finished(queued));
        assert_eq!(status.selected.as_deref(), Some("b"));
        let relation_selected = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::SelectRelation("flow".into()))
            .unwrap();
        app.update();
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.loading && status.action_finished(relation_selected));
        assert!(!status.action_finished(queued));
        assert_eq!(status.selected_relation.as_deref(), Some("flow"));
        let isolated = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::ToggleIsolate)
            .unwrap();
        app.update();
        assert!(app.world().resource::<Viewer>().isolate);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_finished(isolated)
        );
        assert!(app.world().resource::<LayoutStatus>().loading);
        send.send(Ok((p, s, ViewState::default()))).unwrap();
        app.update();
        assert_eq!(
            app.world().resource::<LayoutStatus>().completed_action_id,
            first
        );
        assert!(
            !app.world()
                .resource::<LayoutStatus>()
                .action_finished(queued)
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !app
            .world()
            .resource::<LayoutStatus>()
            .action_finished(queued)
        {
            app.update();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("a")
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().completed_action_id,
            isolated
        );
    }
    #[test]
    fn failed_expansion_keeps_selection_and_offers_separate_inspection() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let accepted = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        let (send, receive) = mpsc::sync_channel(1);
        send.send(Err(layout::LayoutError::Invalid(
            "declared expansion conflict".into(),
        )))
        .unwrap();
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.selected = Some("a".into());
            viewer.pending_expansion = Some("a".into());
            viewer.job = Some(Job {
                action_id: None,
                cancel: Arc::new(AtomicBool::new(false)),
                receive: Mutex::new(receive),
                reorganize: false,
                replacement_input: None,
                restored_view: default(),
                restored_dismissals: default(),
                restored_composed: default(),
                canonical_cache: default(),
                timing: default(),
            });
        }
        app.update();
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(viewer.selected.as_deref(), Some("a"));
        assert_eq!(viewer.accepted.as_ref().unwrap().1, accepted);
        assert!(
            viewer.message.contains("previous view is unchanged")
                && viewer.message.contains("Press Enter")
        );
        assert!(viewer.message.contains("declared expansion conflict"));
        complete(&mut app, ViewerAction::Enter("a".into()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_deref(),
            Some("a")
        );
    }

    #[test]
    fn reload_recovers_disappearing_suggested_focus_and_keeps_frame_pins() {
        for package_survives in [true, false] {
            let file = LocalTestFile::new();
            grouped_code_file(&file.0);
            let mut view = ViewState::default();
            view.protect_groups(&[seeded_suggestion(
                "package",
                &["file:000", "file:001", "file:002", "file:003"],
            )])
            .unwrap();
            let mut app = action_app(LayoutInput::bundle(file.0.clone()), view);
            complete(&mut app, ViewerAction::Jump("repo".into()));
            complete(&mut app, ViewerAction::Pin("repo".into()));
            complete(&mut app, ViewerAction::Enter("package".into()));
            complete(&mut app, ViewerAction::Pin("package".into()));
            let group = app
                .world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .0
                .suggested_groups[0]
                .clone();
            complete(&mut app, ViewerAction::Enter(group.id.clone()));
            complete(&mut app, ViewerAction::Pin(group.members[0].clone()));
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .is_none()
            );
            let before = app
                .world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1
                .clone();
            let group_pins = app.world().resource::<Viewer>().view.pins.clone();
            assert!(!group_pins.is_empty());
            file.replace(b"invalid replacement");
            complete(&mut app, ViewerAction::Reload);
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .is_some()
            );
            assert_eq!(
                app.world()
                    .resource::<Viewer>()
                    .accepted
                    .as_ref()
                    .unwrap()
                    .1,
                before
            );
            assert_eq!(
                app.world().resource::<Viewer>().view.focus.as_ref(),
                Some(&group.id)
            );
            assert_eq!(app.world().resource::<Viewer>().view.pins, group_pins);

            let replacement = LocalTestFile::new();
            grouped_code_revision(
                &replacement.0,
                2,
                usize::from(package_survives),
                package_survives,
            );
            file.replace(&std::fs::read(&replacement.0).unwrap());
            complete(&mut app, ViewerAction::Reload);
            let status = app.world().resource::<LayoutStatus>();
            assert!(status.action_error.is_none(), "{:?}", status.action_error);
            assert_eq!(status.revision, Some(2));
            let destination = if package_survives { "package" } else { "repo" };
            assert_eq!(status.focus.as_deref(), Some(destination));
            let viewer = app.world().resource::<Viewer>();
            assert!(
                viewer.undo.is_empty(),
                "old-revision undo must not survive publication"
            );
            assert_eq!(
                viewer.view.frame_pins[&format!("l:code|f:{}", group.id)],
                group_pins
            );
            assert_eq!(viewer.view.pins.len(), 1);
            assert!(viewer.view.pins.contains_key(destination));
            let p = &viewer.accepted.as_ref().unwrap().0;
            assert!(p.detail_note.contains("no longer has two members"));
            assert!(p.nodes.iter().all(|node| node.id != group.id));
            assert!(
                !viewer
                    .view
                    .breadcrumbs()
                    .iter()
                    .flatten()
                    .any(|id| id == &group.id)
            );
        }
    }

    #[test]
    fn synchronous_save_measurements_cover_real_success_failure_and_stay_bounded() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let directory = LocalTestDirectory::new();
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        app.world_mut().resource_mut::<Viewer>().storage = Some(directory.0.clone());
        for _ in 0..66 {
            app.world_mut().resource_mut::<Viewer>().save_needed = true;
            app.update();
        }
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.save_attempts, 66);
        assert_eq!(status.save_failures, 0);
        assert_eq!(status.save_samples_ms.len(), 64);
        assert!(status.save_total_ms >= status.save_max_ms && status.save_max_ms > 0.0);
        let path = saved_path(
            &directory.0,
            &state_identity("reload-pipeline", LayoutEngine::Floorplan),
        );
        assert_eq!(
            layout::persistence::load(&path).unwrap().model_id,
            "reload-pipeline"
        );
        std::fs::write(&path, b"corrupt existing state must be preserved").unwrap();
        app.world_mut().resource_mut::<Viewer>().save_needed = true;
        app.update();
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.save_attempts, 67);
        assert_eq!(status.save_failures, 1);
        assert_eq!(status.save_samples_ms.len(), 64);
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"corrupt existing state must be preserved"
        );
        assert!(
            app.world()
                .resource::<Viewer>()
                .message
                .contains("Local state was not saved")
        );
    }

    #[test]
    fn reload_atomically_switches_revision_and_discards_prior_workers() {
        let file = LocalTestFile::new();
        let bytes = include_str!("../../../public/examples/data-messaging.bundle.jsonl");
        file.write(bytes.as_bytes());
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let old_source = app.world().resource::<LayoutInput>().clone();
        let (stale_p, stale_s) = app.world().resource::<Viewer>().accepted.clone().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        send.send(Ok((stale_p, stale_s, ViewState::default())))
            .unwrap();
        let layout_cancel = Arc::new(AtomicBool::new(false));
        let search_cancel = Arc::new(AtomicBool::new(false));
        let (search_send, search_receive) = mpsc::sync_channel(1);
        search_send
            .send(Ok(vec![ViewerSearchResult {
                id: "stale".into(),
                label: "stale".into(),
                kind: "stale".into(),
            }]))
            .unwrap();
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.job = Some(Job {
                action_id: None,
                cancel: layout_cancel.clone(),
                receive: Mutex::new(receive),
                reorganize: false,
                replacement_input: None,
                restored_view: default(),
                restored_dismissals: default(),
                restored_composed: default(),
                canonical_cache: default(),
                timing: default(),
            });
            viewer.search_job = Some(SearchJob {
                action_id: None,
                cancel: search_cancel.clone(),
                receive: Mutex::new(search_receive),
            });
        }
        file.replace(
            bytes
                .replacen("\"revision\":1", "\"revision\":2", 1)
                .as_bytes(),
        );
        complete(&mut app, ViewerAction::Reload);
        let status = app.world().resource::<LayoutStatus>();
        assert_eq!(status.revision, Some(2));
        assert!(status.action_error.is_none());
        assert!(status.search_results.is_empty());
        assert!(layout_cancel.load(Ordering::Relaxed) && search_cancel.load(Ordering::Relaxed));
        assert!(!Arc::ptr_eq(
            &old_source.0,
            &app.world().resource::<LayoutInput>().0
        ));
        assert_eq!(
            old_source
                .project(&ViewState::default(), &AtomicBool::new(false))
                .unwrap()
                .revision,
            1
        );
        assert!(app.world().resource::<Viewer>().undo.is_empty());
        assert!(
            app.world()
                .resource::<Viewer>()
                .cache
                .values()
                .all(|entry| entry.0.revision == 2)
        );
        let accepted = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        let source = app.world().resource::<LayoutInput>().clone();
        for rejected in [
            "{".to_owned(),
            bytes.to_owned(),
            bytes
                .replacen("example:data-messaging", "other-model", 1)
                .replacen("\"revision\":1", "\"revision\":3", 1),
        ] {
            file.replace(rejected.as_bytes());
            complete(&mut app, ViewerAction::Reload);
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .is_some()
            );
            assert_eq!(
                app.world()
                    .resource::<Viewer>()
                    .accepted
                    .as_ref()
                    .unwrap()
                    .1,
                accepted
            );
            assert!(Arc::ptr_eq(
                &source.0,
                &app.world().resource::<LayoutInput>().0
            ));
        }
    }
    fn wait_layout_idle(app: &mut App) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.world().resource::<Viewer>().job.is_some() {
            app.update();
            assert!(
                std::time::Instant::now() < deadline,
                "optional refinement timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn reload_through_publication(app: &mut App, file: &LocalTestFile, revision: u64) {
        let staged = LocalTestFile::new();
        pipeline_file(&staged.0, revision, false);
        std::fs::rename(&staged.0, &file.0).unwrap();
        let ticket = app
            .world_mut()
            .resource_mut::<ViewerActions>()
            .submit(ViewerAction::Reload)
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            app.update();
            let status = app.world().resource::<LayoutStatus>();
            if status.action_finished(ticket) {
                assert!(status.action_error.is_none(), "{:?}", status.action_error);
                assert_eq!(status.revision, Some(revision));
                assert!(status.reorganizing);
                let viewer = app.world().resource::<Viewer>();
                let job = viewer
                    .job
                    .as_ref()
                    .expect("one automatic search after publication");
                assert!(job.reorganize && job.action_id.is_none());
                return;
            }
            assert!(std::time::Instant::now() < deadline, "reload timed out");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    fn generous_accepted_envelope(app: &mut App) {
        // A previously accepted large envelope is valid but makes a meaningful
        // compact alternative possible without inventing quality measurements.
        let mut viewer = app.world_mut().resource_mut::<Viewer>();
        let (p, accepted) = viewer.accepted.clone().unwrap();
        let mut s = accepted.flat().unwrap().clone();
        let root = s.boxes.get_mut(&p.root).unwrap();
        assert!(!root.solid);
        root.bounds.min = root.bounds.min - Point::new(800, 800, 800);
        root.bounds.max = root.bounds.max + Point::new(800, 800, 800);
        layout::validate(&p, &s, &visible_pins(&viewer.view, &p), 2).unwrap();
        s.metrics = layout::measure(&p, &s, None);
        let s: NativeLayout = s.into();
        cache_layout(&mut viewer, p.clone(), s.clone());
        viewer.accepted = Some((p, s));
    }
    #[test]
    fn reload_refines_once_and_preserves_geometry_until_explicit_apply() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        complete(&mut app, ViewerAction::Pin("a".into()));
        generous_accepted_envelope(&mut app);
        let pins = app.world().resource::<Viewer>().view.pins.clone();
        let before = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .flat()
            .unwrap()
            .boxes
            .clone();
        reload_through_publication(&mut app, &file, 2);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1
                .flat()
                .unwrap()
                .boxes,
            before
        );
        wait_layout_idle(&mut app);
        let viewer = app.world().resource::<Viewer>();
        let accepted = viewer.accepted.as_ref().unwrap().1.clone();
        let proposed = viewer
            .proposal
            .clone()
            .expect("valid compact proposal improves the generous frame");
        assert_eq!(accepted.flat().unwrap().boxes, before);
        assert_eq!(viewer.view.pins, pins);
        assert!(!viewer.preview);
        assert!(proposed.metrics.structural_cost <= accepted.metrics.structural_cost * 0.85);
        assert!(
            viewer.message.contains("length")
                && viewer.message.contains("bends")
                && viewer.message.contains("objects move")
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .reorganization_available
        );
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().resource::<Viewer>().job.is_none());
        complete(&mut app, ViewerAction::Preview);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            accepted
        );
        complete(&mut app, ViewerAction::Apply);
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(viewer.accepted.as_ref().unwrap().1, proposed);
        assert_eq!(viewer.view.pins, pins);
        assert!(viewer.job.is_none() && viewer.proposal.is_none());
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            accepted
        );
        assert!(app.world().resource::<Viewer>().job.is_none());
    }
    #[test]
    fn dismissed_background_arrangement_stays_suppressed_after_another_reload() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let directory = LocalTestDirectory::new();
        app.world_mut().resource_mut::<Viewer>().storage = Some(directory.0.clone());
        generous_accepted_envelope(&mut app);
        reload_through_publication(&mut app, &file, 2);
        wait_layout_idle(&mut app);
        let candidate = app.world().resource::<Viewer>().proposal.clone().unwrap();
        let key = proposal_identity(
            &app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .0,
            &candidate,
        );
        let before_routes = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .flat()
            .unwrap()
            .routes
            .clone();
        let before = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .flat()
            .unwrap()
            .boxes
            .clone();
        let saves = app.world().resource::<LayoutStatus>().save_attempts;
        complete(&mut app, ViewerAction::Dismiss);
        assert_eq!(
            app.world().resource::<LayoutStatus>().save_attempts,
            saves + 1
        );
        assert!(app.world().resource::<Viewer>().dismissed.contains(&key));
        let saved = layout::persistence::load(saved_path(
            &directory.0,
            &state_identity("reload-pipeline", LayoutEngine::Floorplan),
        ))
        .unwrap();
        assert_eq!(saved.dismissed_proposals, vec![key.clone()]);
        assert_eq!(saved.accepted.boxes, before);
        assert_eq!(saved.accepted.routes, before_routes);
        drop(app);
        let mut app = cold_app(LayoutInput::bundle(file.0.clone()), &directory.0);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1
                .flat()
                .unwrap()
                .boxes,
            before
        );
        assert!(app.world().resource::<Viewer>().dismissed.contains(&key));
        complete(&mut app, ViewerAction::Reorganize);
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1
                .flat()
                .unwrap()
                .routes,
            before_routes
        );
        assert!(app.world().resource::<Viewer>().proposal.is_none());
        reload_through_publication(&mut app, &file, 3);
        wait_layout_idle(&mut app);
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(
            viewer.accepted.as_ref().unwrap().1.flat().unwrap().boxes,
            before
        );
        assert!(viewer.proposal.is_none() && viewer.job.is_none());
        assert!(
            !app.world()
                .resource::<LayoutStatus>()
                .reorganization_available
        );
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().resource::<Viewer>().job.is_none());
    }
    #[test]
    fn navigation_cancels_automatic_refinement_and_discards_its_ready_result() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        reload_through_publication(&mut app, &file, 2);
        // Install a finished stale result to exercise channel publication ordering,
        // rather than depending on how quickly the real worker happens to run.
        let (send, receive) = mpsc::sync_channel(1);
        let (p, s) = app.world().resource::<Viewer>().accepted.clone().unwrap();
        send.send(Ok((p, s, app.world().resource::<Viewer>().view.clone())))
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        app.world_mut().resource_mut::<Viewer>().job = Some(Job {
            action_id: None,
            cancel: cancelled.clone(),
            receive: Mutex::new(receive),
            reorganize: true,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        complete(&mut app, ViewerAction::Enter("a".into()));
        assert!(cancelled.load(Ordering::Relaxed));
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(viewer.view.focus.as_deref(), Some("a"));
        assert_eq!(viewer.accepted.as_ref().unwrap().0.root, "a");
        assert!(viewer.proposal.is_none());
        assert_eq!(app.world().resource::<LayoutStatus>().revision, Some(2));
    }
    #[test]
    fn unsuccessful_optional_refinement_preserves_the_accepted_selection_and_view() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        complete(&mut app, ViewerAction::Select("a".into()));
        let before = app.world().resource::<Viewer>().accepted.clone().unwrap();
        let (send, receive) = mpsc::sync_channel(1);
        send.send(Err(layout::LayoutError::Unroutable(
            "bounded alternative exhausted".into(),
        )))
        .unwrap();
        app.world_mut().resource_mut::<Viewer>().job = Some(Job {
            action_id: None,
            cancel: Arc::new(AtomicBool::new(false)),
            receive: Mutex::new(receive),
            reorganize: true,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        app.update();
        let viewer = app.world().resource::<Viewer>();
        assert_eq!(viewer.accepted.as_ref().unwrap().1, before.1);
        assert_eq!(viewer.selected.as_deref(), Some("a"));
        assert!(viewer.proposal.is_none() && viewer.job.is_none());
        let status = app.world().resource::<LayoutStatus>();
        assert!(status.ready && status.error.is_none() && !status.reorganizing);
        assert!(viewer.message.contains("Accepted arrangement preserved"));
    }
    #[test]
    fn optional_proposals_recompute_scores_instead_of_trusting_cached_improvement() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let (projection, mut candidate) =
            app.world().resource::<Viewer>().accepted.clone().unwrap();
        edit_flat(&mut candidate, |s| s.metrics.structural_cost = 0.0);
        let (send, receive) = mpsc::sync_channel(1);
        send.send(Ok((
            projection,
            candidate,
            app.world().resource::<Viewer>().view.clone(),
        )))
        .unwrap();
        app.world_mut().resource_mut::<Viewer>().job = Some(Job {
            action_id: None,
            cancel: Arc::new(AtomicBool::new(false)),
            receive: Mutex::new(receive),
            reorganize: true,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        app.update();
        assert!(app.world().resource::<Viewer>().proposal.is_none());
        assert!(
            !app.world()
                .resource::<LayoutStatus>()
                .reorganization_available
        );
    }
    #[test]
    fn dismissal_identity_scopes_semantics_and_frames_but_ignores_reroutes() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let app = action_app(LayoutInput::bundle(file.0.clone()), ViewState::default());
        let (p, s) = app.world().resource::<Viewer>().accepted.as_ref().unwrap();
        let key = proposal_identity(p, s);
        let mut rerouted = s.clone();
        edit_flat(&mut rerouted, |flat| {
            flat.revision += 1;
            flat.metrics.structural_cost += 1.0;
            flat.projection_key.push_str("|different-quality-profile");
            flat.routes[0].points.insert(1, Point::new(800, 300, 200));
        });
        assert_eq!(proposal_identity(p, &rerouted), key);
        let mut different = p.clone();
        let claim = different.claims.get_mut("flow").unwrap();
        std::mem::swap(&mut claim.source, &mut claim.target);
        assert_ne!(proposal_identity(&different, s), key);
        different = p.clone();
        different.claims.get_mut("flow").unwrap().kind_label = "publishes".into();
        assert_ne!(proposal_identity(&different, s), key);
        different = p.clone();
        different.claims.get_mut("flow").unwrap().aspect = Some("authorization".into());
        assert_ne!(proposal_identity(&different, s), key);
        different = p.clone();
        different.graph.root = "different-frame".into();
        assert_ne!(proposal_identity(&different, s), key);
        different = p.clone();
        different.lens = SemanticLens::Deployment;
        assert_ne!(proposal_identity(&different, s), key);
        let mut other = s.clone();
        edit_flat(&mut other, |flat| flat.model_id = "different-model".into());
        assert_ne!(proposal_identity(p, &other), key);
        other = s.clone();
        edit_flat(&mut other, |flat| {
            flat.boxes.get_mut("a").unwrap().bounds.max.x += 1
        });
        assert_ne!(proposal_identity(p, &other), key);
        other = s.clone();
        edit_flat(&mut other, |flat| {
            flat.boxes
                .get_mut("a")
                .unwrap()
                .semantic_ids
                .push("additional-original".into())
        });
        assert_ne!(proposal_identity(p, &other), key);
    }
    #[test]
    fn reload_with_conflicting_established_pins_keeps_the_accepted_source() {
        let file = LocalTestFile::new();
        pipeline_file(&file.0, 1, false);
        let mut view = ViewState::default();
        view.policy = layout::projection::LayoutPolicy::Pipeline;
        let mut app = action_app(LayoutInput::bundle(file.0.clone()), view);
        complete(&mut app, ViewerAction::Pin("a".into()));
        complete(&mut app, ViewerAction::Pin("b".into()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        let accepted = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        let source = app.world().resource::<LayoutInput>().clone();
        let staged = LocalTestFile::new();
        pipeline_file(&staged.0, 2, true);
        std::fs::rename(&staged.0, &file.0).unwrap();
        assert_eq!(
            source
                .reload_candidate()
                .unwrap()
                .project(
                    &app.world().resource::<Viewer>().accepted_view,
                    &AtomicBool::new(false)
                )
                .unwrap()
                .revision,
            2
        );
        complete(&mut app, ViewerAction::Reload);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_some()
        );
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            accepted
        );
        assert!(Arc::ptr_eq(
            &source.0,
            &app.world().resource::<LayoutInput>().0
        ));
    }
    #[test]
    fn local_group_actions_are_reversible_and_reject_invalid_ownership() {
        let model = visualyze_model::architecture::Architecture::from_json(include_bytes!(
            "../../../public/examples/notes-crud.json"
        ))
        .unwrap();
        let mut app = action_app(LayoutInput::snapshot(model), ViewState::default());
        let original = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        complete(
            &mut app,
            ViewerAction::SetGroup {
                id: "view:invalid".into(),
                parent: "scope:notes".into(),
                members: vec!["client:web".into(), "interface:notes".into()],
            },
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_some()
        );
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            original
        );
        assert!(
            app.world()
                .resource::<Viewer>()
                .view
                .group_overrides
                .is_empty()
        );
        complete(
            &mut app,
            ViewerAction::ToggleGroupMember("client:web".into()),
        );
        complete(
            &mut app,
            ViewerAction::ToggleGroupMember("service:notes".into()),
        );
        assert_eq!(
            app.world()
                .resource::<LayoutStatus>()
                .marked_group_members
                .len(),
            2
        );
        complete(&mut app, ViewerAction::GroupMarked);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        let id = app.world().resource::<LayoutStatus>().local_group_ids[0].clone();
        complete(&mut app, ViewerAction::Enter(id.clone()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_ref(),
            Some(&id)
        );
        complete(&mut app, ViewerAction::RemoveGroup(id.clone()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .local_group_ids
                .is_empty()
        );
        assert!(
            !app.world()
                .resource::<LayoutStatus>()
                .breadcrumbs
                .contains(&id)
        );
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_ref(),
            Some(&id)
        );
        assert_eq!(app.world().resource::<LayoutStatus>().local_group_ids, [id]);
    }
    #[test]
    fn suggested_group_dismissal_survives_navigation_and_undo() {
        // Twenty-five extra services in the scope put 26 peers in one layer, so
        // the level is noisy enough for suggested fallback groups.
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        for index in 0..25 {
            let id = format!("service:extra:{index:02}");
            value["entities"].as_array_mut().unwrap().push(serde_json::json!({
                "id": id, "kind": "service", "label": id, "boundary": "internal", "basis": "declared"
            }));
            value["relations"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({
                    "id": format!("rel:contains-{id}"), "source": "scope:notes", "target": id,
                    "kind": "contains", "basis": "declared"
                }));
        }
        let model = visualyze_model::architecture::Architecture::from_json(
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        let mut app = action_app(LayoutInput::snapshot(model), ViewState::default());
        let id = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0
            .suggested_groups
            .iter()
            .find(|group| group.suggested)
            .unwrap()
            .id
            .clone();
        complete(&mut app, ViewerAction::Enter(id.clone()));
        complete(&mut app, ViewerAction::RemoveGroup(id.clone()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        assert!(
            app.world()
                .resource::<Viewer>()
                .view
                .dismissed_groups
                .contains(&id)
        );
        assert!(
            !app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .0
                .nodes
                .iter()
                .any(|node| node.id == id)
        );
        complete(&mut app, ViewerAction::Undo);
        assert!(
            !app.world()
                .resource::<Viewer>()
                .view
                .dismissed_groups
                .contains(&id)
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().focus.as_ref(),
            Some(&id)
        );
    }
    #[test]
    fn grouped_file_expansion_can_collapse_and_undo_fresh_or_retained_groups() {
        for retained in [false, true] {
            let file = LocalTestFile::new();
            grouped_code_file(&file.0);
            let mut view = ViewState::default();
            view.enter("package");
            view.protect_groups(&[seeded_suggestion(
                "package",
                &["file:000", "file:001", "file:002", "file:003"],
            )])
            .unwrap();
            let mut app = action_app(LayoutInput::bundle(file.0.clone()), view);
            let group = app
                .world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .0
                .suggested_groups
                .iter()
                .find(|group| group.members.iter().any(|id| id == "file:000"))
                .unwrap()
                .clone();
            if retained {
                let mut viewer = app.world_mut().resource_mut::<Viewer>();
                let p = viewer.accepted.as_ref().unwrap().0.graph.clone();
                viewer.view.remember_groups(&p);
                viewer.accepted_view = viewer.view.clone();
            }
            complete(&mut app, ViewerAction::Expand("file:000".into()));
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .is_none(),
                "{:?}",
                app.world().resource::<LayoutStatus>().action_error
            );
            let viewer = app.world().resource::<Viewer>();
            assert!(viewer.view.is_expanded("file:000"));
            assert!(!viewer.view.is_expanded(&group.id));
            let (p, expanded_layout) = viewer.accepted.as_ref().unwrap();
            assert!(p.nodes.iter().any(|node| node.id == "function:000"));
            assert!(
                !p.nodes
                    .iter()
                    .find(|node| node.id == group.id)
                    .unwrap()
                    .solid
            );
            let expanded_layout = expanded_layout.clone();
            let toggle = expansion_toggle(viewer, &group.id);
            assert!(matches!(&toggle, ViewerAction::Collapse(id) if id == &group.id));
            complete(&mut app, toggle);
            assert!(
                app.world()
                    .resource::<LayoutStatus>()
                    .action_error
                    .is_none(),
                "{:?}",
                app.world().resource::<LayoutStatus>().action_error
            );
            let viewer = app.world().resource::<Viewer>();
            assert!(!viewer.view.is_expanded("file:000"));
            let p = &viewer.accepted.as_ref().unwrap().0;
            assert!(
                p.nodes
                    .iter()
                    .find(|node| node.id == group.id)
                    .unwrap()
                    .solid
            );
            assert!(!p.nodes.iter().any(|node| node.id == "function:000"));
            assert!(matches!(
                expansion_toggle(viewer, &group.id),
                ViewerAction::Expand(_)
            ));
            complete(&mut app, ViewerAction::Undo);
            let viewer = app.world().resource::<Viewer>();
            assert!(viewer.view.is_expanded("file:000"));
            assert_eq!(viewer.accepted.as_ref().unwrap().1, expanded_layout);
        }
    }
    #[test]
    fn bundle_group_is_navigable_without_changing_supplied_relationships() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut app = action_app(LayoutInput::bundle(path), ViewState::default());
        complete(
            &mut app,
            ViewerAction::ToggleGroupMember("client:web".into()),
        );
        complete(
            &mut app,
            ViewerAction::ToggleGroupMember("service:orders".into()),
        );
        complete(&mut app, ViewerAction::GroupMarked);
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        let id = app.world().resource::<LayoutStatus>().local_group_ids[0].clone();
        let p = &app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0;
        assert_eq!(p.semantic_to_display["client:web"], id);
        assert_eq!(p.semantic_to_display["service:orders"], id);
        assert_eq!(p.labels[&id].original_kind, "visual_group");
        let claims = p.claims.clone();
        complete(&mut app, ViewerAction::Expand(id.clone()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        let p = &app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .0;
        assert!(p.nodes.iter().any(|node| node.id == "client:web"));
        for (claim_id, old) in &claims {
            if let Some(current) = p.claims.get(claim_id) {
                assert_eq!(
                    (&old.source, &old.target, &old.kind_label),
                    (&current.source, &current.target, &current.kind_label)
                );
            }
        }
        complete(&mut app, ViewerAction::Enter(id.clone()));
        assert_eq!(
            app.world().resource::<LayoutStatus>().root.as_ref(),
            Some(&id)
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
        complete(&mut app, ViewerAction::RemoveGroup(id.clone()));
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .local_group_ids
                .is_empty()
        );
        assert_eq!(
            app.world().resource::<LayoutStatus>().root.as_deref(),
            Some("scope:orders")
        );
        complete(&mut app, ViewerAction::Undo);
        assert_eq!(
            app.world().resource::<LayoutStatus>().root.as_ref(),
            Some(&id)
        );
        assert!(
            app.world()
                .resource::<LayoutStatus>()
                .action_error
                .is_none()
        );
    }
    #[test]
    fn queued_cancel_restores_the_previous_focus_and_completes_its_ticket() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut app = action_app(LayoutInput::bundle(path), ViewState::default());
        let before = app
            .world()
            .resource::<Viewer>()
            .accepted
            .as_ref()
            .unwrap()
            .1
            .clone();
        let (_sender, receiver) = mpsc::sync_channel(1);
        let signal = Arc::new(AtomicBool::new(false));
        {
            let mut viewer = app.world_mut().resource_mut::<Viewer>();
            viewer.view.enter("pending:frame");
            viewer.job = Some(Job {
                action_id: None,
                cancel: signal.clone(),
                receive: Mutex::new(receiver),
                reorganize: false,
                replacement_input: None,
                restored_view: default(),
                restored_dismissals: default(),
                restored_composed: default(),
                canonical_cache: default(),
                timing: default(),
            });
        }
        complete(&mut app, ViewerAction::Cancel);
        assert!(signal.load(Ordering::Relaxed));
        assert!(app.world().resource::<LayoutStatus>().focus.is_none());
        assert_eq!(
            app.world()
                .resource::<Viewer>()
                .accepted
                .as_ref()
                .unwrap()
                .1,
            before
        );
    }
    fn solved() -> (DesktopProjection, NativeLayout) {
        let model = visualyze_model::architecture::Architecture::from_json(include_bytes!(
            "../../../public/examples/sketch-crud.json"
        ))
        .unwrap();
        let input = LayoutInput::snapshot(model);
        let view = ViewState::default();
        let flag = AtomicBool::new(false);
        let p = input.project(&view, &flag).unwrap();
        let s = layout::solve(LayoutRequest {
            projection: &p,
            algorithm: Algorithm::Floorplan,
            pins: &BTreeMap::new(),
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &flag,
        })
        .unwrap();
        (p, s.into())
    }
    #[test]
    fn cancelled_navigation_restores_focus_breadcrumbs_and_pins() {
        let mut v = Viewer::default();
        v.view.pins.insert("a".into(), Point::new(2, 3, 4));
        v.accepted_view = v.view.clone();
        v.view.enter("cancelled");
        v.view.pins.clear();
        v.selected = Some("new".into());
        abandon_pending(&mut v);
        assert_eq!(v.view.focus, None);
        assert!(v.view.breadcrumbs().is_empty());
        assert_eq!(v.view.pins["a"], Point::new(2, 3, 4));
        assert!(v.selected.is_none());
    }
    #[test]
    fn preview_cancel_and_restart_restore_accepted_geometry() {
        let (p, s) = solved();
        let mut candidate = s.clone();
        edit_flat(&mut candidate, |flat| flat.metrics.route_length += 1);
        let mut v = Viewer {
            accepted: Some((p, s.clone())),
            proposal: Some(candidate),
            preview: true,
            ..default()
        };
        assert_ne!(displayed_solution(&v).unwrap(), &s);
        clear_preview(&mut v);
        assert!(v.dirty);
        assert_eq!(displayed_solution(&v).unwrap(), &s);
        v.proposal = Some(s.clone());
        v.preview = true;
        v.dirty = false;
        abandon_pending(&mut v);
        assert!(v.dirty && !v.preview && v.proposal.is_none());
        assert_eq!(displayed_solution(&v).unwrap(), &s);
    }
    #[test]
    fn reorganization_requires_an_idle_accepted_view_and_matching_identity() {
        let mut v = Viewer::default();
        assert!(!can_reorganize(&v));
        let (p, s) = solved();
        v.accepted = Some((p, s.clone()));
        assert!(can_reorganize(&v));
        let (_send, receive) = mpsc::sync_channel(1);
        v.job = Some(Job {
            action_id: None,
            cancel: Arc::new(AtomicBool::new(false)),
            receive: Mutex::new(receive),
            reorganize: false,
            replacement_input: None,
            restored_view: default(),
            restored_dismissals: default(),
            restored_composed: default(),
            canonical_cache: default(),
            timing: default(),
        });
        assert!(!can_reorganize(&v));
        let mut stale = s.clone();
        edit_flat(&mut stale, |flat| flat.projection_key.push_str("other"));
        assert!(!same_view(&stale, &s));
        v.preview = true;
        v.proposal = Some(stale);
        assert_eq!(displayed_solution(&v).unwrap(), &s);
    }
    #[test]
    fn expansion_does_not_recenter_established_world_geometry() {
        let (p, s) = solved();
        let mut v = Viewer::default();
        select_origin(&mut v, &p, &s);
        let origin = v.origin;
        let mut expanded = s.clone();
        edit_flat(&mut expanded, |flat| {
            flat.boxes.values_mut().next().unwrap().bounds.min.y -= 500
        });
        select_origin(&mut v, &p, &expanded);
        assert_eq!(origin, v.origin);
    }
    #[test]
    fn picking_intersects_visible_box_faces_in_depth_order() {
        let origin = Vec3::new(0.0, 0.0, 10.0);
        assert_eq!(
            ray_box(origin, -Vec3::Z, Vec3::splat(-1.0), Vec3::splat(1.0)),
            Some(9.0)
        );
        assert_eq!(
            ray_box(origin, Vec3::Z, Vec3::splat(-1.0), Vec3::splat(1.0)),
            None
        );
        assert_eq!(
            ray_box(
                origin + Vec3::X * 2.0,
                -Vec3::Z,
                Vec3::splat(-1.0),
                Vec3::splat(1.0)
            ),
            None
        );
        let near = ray_box(
            origin,
            -Vec3::Z,
            Vec3::new(-1.0, -1.0, 4.0),
            Vec3::new(1.0, 1.0, 6.0),
        )
        .unwrap();
        assert!(near < 9.0);
    }
    #[test]
    fn inspection_preserves_boxes_and_orthogonal_attachments() {
        let (_, native) = solved();
        let s = native.flat().unwrap().clone();
        let before = s.clone();
        for b in s.boxes.values().filter(|b| b.solid) {
            let lo = display_point(b.bounds.min, &s, true, Vec3::ZERO);
            let hi = display_point(b.bounds.max, &s, true, Vec3::ZERO);
            assert!((hi - lo).abs_diff_eq(vec(b.bounds.size()), 0.0001));
        }
        for r in &s.routes {
            for pair in r.points.windows(2) {
                let a = display_point(pair[0], &s, true, Vec3::ZERO);
                let b = display_point(pair[1], &s, true, Vec3::ZERO);
                assert_eq!(
                    [a.x != b.x, a.y != b.y, a.z != b.z]
                        .into_iter()
                        .filter(|v| *v)
                        .count(),
                    1
                );
            }
        }
        assert_eq!(s, before);
    }
    #[test]
    fn view_undo_remembers_constraints_and_is_bounded() {
        let (p, s) = solved();
        let mut v = Viewer {
            accepted: Some((p, s)),
            ..default()
        };
        v.accepted_view
            .pins
            .insert("saved".into(), Point::new(1, 2, 3));
        for _ in 0..100 {
            remember(&mut v);
        }
        assert_eq!(v.undo.len(), 32);
        let (view, _, _, _) = v.undo.pop().unwrap();
        assert_eq!(view.pins["saved"], Point::new(1, 2, 3));
    }
    #[test]
    fn restoring_evicted_undo_entries_keeps_the_cache_bounded() {
        let (p, s) = solved();
        let mut v = Viewer::default();
        for i in 0..100 {
            let mut p = p.clone();
            let mut s = s.clone();
            p.graph.key = format!("view-{i}");
            edit_flat(&mut s, |flat| flat.projection_key.clone_from(&p.graph.key));
            cache_layout(&mut v, p, s);
        }
        for i in 0..100 {
            let mut p = p.clone();
            let mut s = s.clone();
            p.graph.key = format!("view-{i}");
            edit_flat(&mut s, |flat| flat.projection_key.clone_from(&p.graph.key));
            cache_layout(&mut v, p, s);
            assert!(v.cache.len() <= 16);
            assert_eq!(v.cache.len(), v.cache_order.len());
            assert!(v.cache_order.iter().all(|key| v.cache.contains_key(key)));
        }
    }
    #[test]
    fn state_identity_cannot_escape_its_application_directory() {
        let root = std::path::Path::new("/tmp/layout-state");
        let path = saved_path(root, "../../arbitrary/path");
        assert_eq!(path.parent(), Some(root));
        assert_eq!(path, saved_path(root, "../../arbitrary/path"));
        assert_ne!(path, saved_path(root, "another-model"));
    }
}
