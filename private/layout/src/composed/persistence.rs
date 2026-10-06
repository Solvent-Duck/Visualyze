//! Versioned, explicitly located composed view state, separate from flat saves.
//!
//! `load` checks bounded storage, identities, numeric shape and reconstructs the
//! assembly through its checked constructors. It is not permission to display
//! the result: rebuild the projection from `view`, then call `validate` against
//! that supplied projection. Measurements are omitted from this format and
//! recomputed only after independent geometry validation. No file is migrated.
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use visualyze_model::provider::SemanticLens;

use super::{
    COMPOSITION_VERSION, CanonicalPins, ComposedMetrics, ComposedRoute, ComposedSolution,
    ComposedWork, CompositionContext, FrameFragment, validate_composed,
};
use crate::{
    Bounds, Edge, LAYOUT_VERSION, LayoutBox, LayoutError, LayoutSolution, Metrics, Point,
    Projection, QUALITY_VERSION,
    assembly::{
        AnchorPin, AnchorPinLink, AnchorPinRecord, Assembly, AssemblyInstance, CanonicalFrame,
        DyadicTransform, ExactPoint, MAX_COORDINATE, MAX_FRAMES, MAX_TOTAL_BOXES,
        MAX_TOTAL_ROUTE_POINTS, MAX_TOTAL_SEMANTIC_MEMBERS,
    },
    assembly_plan::{MAX_FRAME_STEPS, plan_frames},
    persistence::{DISMISSAL_ENCODING_VERSION, DismissedProposal},
    projection::{DisplayGroup, LayoutPolicy, ViewState},
    routing::{FragmentRequest, FragmentTerminal, RouteFragment},
};

pub const STATE_VERSION: u32 = 6;
pub const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
const FORMAT: &str = "visualyze-composed-view";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

/// In-memory form. `accepted` remains untrusted until `validate` succeeds for
/// the current supplied projection. Flat view pins are retained as view state;
/// only explicitly named `canonical_pins` constrain canonical coordinates.
#[derive(Debug, Clone)]
pub struct PersistedComposedView {
    pub version: u32,
    pub model_id: String,
    pub revision: u64,
    pub context: CompositionContext,
    pub view: ViewState,
    pub canonical_pins: CanonicalPins,
    pub anchor_pins: Vec<AnchorPin>,
    /// Bounded durable overrides for active, hidden and inactive view contexts.
    /// Geometry validation covers the accepted context; callers must recheck
    /// supplied ownership before activating any stored hidden constraints.
    pub constraint_sets: Vec<PersistedConstraintSet>,
    pub dismissed_proposals: Vec<DismissedProposal>,
    pub accepted: ComposedSolution,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersistedConstraintSet {
    pub context: CompositionContext,
    pub pins: CanonicalPins,
    pub anchors: Vec<AnchorPin>,
    pub ownership: BTreeMap<String, Vec<String>>,
    /// Durable display ancestry, separate from supplied architectural ownership.
    /// These records belong to this source context even when their constraints
    /// are effective in another view. Recheck every supplied path against the
    /// current model and the display path against the projection before use.
    pub presentation: BTreeMap<String, PinPresentation>,
}

/// Complete, bounded presentation evidence for one pinned representation.
/// Group definitions survive suggestion-cache eviction. `path` starts at the
/// displayed object, while `supplied` contains only object-first architectural
/// paths. No ID spelling establishes group or reference identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinPresentation {
    pub path: Vec<String>,
    pub groups: Vec<DisplayGroup>,
    pub reference: Option<PinReference>,
    pub supplied: BTreeMap<String, Vec<String>>,
}

/// A reference stays in its original display context. Current projections reuse
/// IDs for references to the same target, so even an ancestor's same-ID reference
/// is a different representation and never inherits this pin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinReference {
    pub origin: String,
    pub target: String,
}

impl PersistedComposedView {
    /// Validate original claims, all canonical geometry, joins, pins and work
    /// counters, then atomically replace diagnostic metrics with fresh values.
    /// The caller must supply the context's logical view/lens: a Projection has
    /// no typed lens field and an opaque projection key cannot prove that fact.
    pub fn validate(
        &mut self,
        projection: &Projection,
        cancelled: &AtomicBool,
    ) -> Result<(), PersistenceError> {
        check_shape(self, cancelled)?;
        if projection.model_id != self.context.model_id || projection.root != self.context.root {
            return Err(invalid("supplied projection differs from saved context"));
        }
        validate_composed(projection, &self.accepted, &self.canonical_pins, cancelled)?;
        for pin in &self.anchor_pins {
            check_cancel(cancelled)?;
            pin.validate(&self.accepted.assembly)
                .map_err(assembly_error)?;
        }
        let plan = plan_frames(projection, cancelled)?;
        let effective = effective_registry(self, cancelled)?;
        if let Some(set) = effective.as_ref() {
            let visible = |owner: &str, id: &str| {
                plan.frames
                    .get(owner)
                    .is_some_and(|frame| frame.projection.nodes.iter().any(|node| node.id == id))
            };
            let pins: CanonicalPins = set
                .pins
                .iter()
                .filter_map(|(owner, pins)| {
                    let pins: BTreeMap<_, _> = pins
                        .iter()
                        .filter(|(id, _)| visible(owner, id))
                        .map(|(id, point)| (id.clone(), *point))
                        .collect();
                    (!pins.is_empty()).then(|| (owner.clone(), pins))
                })
                .collect();
            let anchors: Vec<_> = set
                .anchors
                .iter()
                .filter(|pin| visible(pin.owner(), pin.object_id()))
                .cloned()
                .collect();
            if pins != self.canonical_pins || anchors != self.anchor_pins {
                return Err(invalid(
                    "active constraints differ from the durable registry",
                ));
            }
        }
        let metrics = super::presentation::measure_validated(&plan, &self.accepted, cancelled)?;
        check_cancel(cancelled)?;
        self.accepted.metrics = metrics;
        Ok(())
    }
}

// Versions 1/2 stored only current-view active constraints; version 3 inherited
// ancestor-owned pins downward. Preserve each interpretation while reading;
// the viewer repairs under current inheritance before publishing a new save.
// Versions 4/5 inherited only supplied pins; version 6 adds independently
// checked display ancestry. Version 5 records composed feedback work. Empty
// registries retain legacy explicit ownership.
fn effective_registry(
    state: &PersistedComposedView,
    cancelled: &AtomicBool,
) -> Result<Option<PersistedConstraintSet>, PersistenceError> {
    if state.version >= 3 && !state.constraint_sets.is_empty() {
        let effective = if state.version == 3 {
            super::constraints::resolve_descending_constraints(
                &state.context,
                &state.constraint_sets,
                cancelled,
            )
        } else if state.version < 6 {
            super::constraints::resolve_supplied_constraints(
                &state.context,
                &state.constraint_sets,
                cancelled,
            )
        } else {
            super::resolve_constraints(&state.context, &state.constraint_sets, cancelled)
        };
        return effective.map(Some).map_err(Into::into);
    }
    Ok(state
        .constraint_sets
        .iter()
        .find(|set| set.context == state.context)
        .cloned())
}

#[derive(Debug)]
pub enum PersistenceError {
    Io(std::io::Error),
    Json(serde_json::Error),
    UnsupportedVersion(u32),
    TooLarge,
    Invalid(String),
    SourceChanged,
    Layout(LayoutError),
}
impl fmt::Display for PersistenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "composed view-state I/O: {e}"),
            Self::Json(e) => write!(f, "composed view-state JSON: {e}"),
            Self::UnsupportedVersion(v) => write!(
                f,
                "unsupported composed view-state version {v}; original file preserved"
            ),
            Self::TooLarge => f.write_str("composed view state exceeds 8 MiB"),
            Self::Invalid(s) => write!(f, "invalid composed view state: {s}"),
            Self::SourceChanged => {
                f.write_str("composed view-state file changed while saving; original preserved")
            }
            Self::Layout(e) => write!(f, "composed view-state validation: {e}"),
        }
    }
}
impl std::error::Error for PersistenceError {}
impl From<std::io::Error> for PersistenceError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for PersistenceError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<LayoutError> for PersistenceError {
    fn from(e: LayoutError) -> Self {
        Self::Layout(e)
    }
}
fn invalid(message: impl Into<String>) -> PersistenceError {
    PersistenceError::Invalid(message.into())
}
fn assembly_error(error: crate::assembly::AssemblyError) -> PersistenceError {
    invalid(error.to_string())
}
fn check_cancel(cancelled: &AtomicBool) -> Result<(), PersistenceError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled.into())
    } else {
        Ok(())
    }
}

// Private wire types deliberately exclude measurements and unchecked Assembly
// deserialization. Exact numerators are decimal strings, avoiding JSON's f64
// conversion for values outside the u64/i64 range.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireState {
    format: String,
    version: u32,
    model_id: String,
    revision: u64,
    context: WireContext,
    view: ViewState,
    canonical_pins: CanonicalPins,
    anchor_pins: Vec<WirePin>,
    #[serde(default)]
    constraint_sets: Vec<WireConstraintSet>,
    dismissed_proposals: Vec<DismissedProposal>,
    accepted: WireSolution,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireConstraintSet {
    context: CompositionContext,
    pins: CanonicalPins,
    anchors: Vec<WireAnchorRecord>,
    ownership: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    presentation: Option<BTreeMap<String, PinPresentation>>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAnchorRecord {
    model_id: String,
    owner: String,
    anchor: String,
    object_id: String,
    members: Vec<String>,
    local_point: Point,
    anchored_point: WireExact,
    links: Vec<WireAnchorLink>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireAnchorLink {
    frame: String,
    parent: String,
    transform: WireTransform,
    envelope: Bounds,
    canonical_bounds: Bounds,
}
impl WireAnchorRecord {
    fn capture(pin: &AnchorPin) -> Self {
        let record = pin.to_record();
        Self {
            model_id: record.model_id,
            owner: record.owner,
            anchor: record.anchor,
            object_id: record.object_id,
            members: record.members,
            local_point: record.local_point,
            anchored_point: record.anchored_point.into(),
            links: record
                .links
                .into_iter()
                .map(|link| WireAnchorLink {
                    frame: link.frame,
                    parent: link.parent,
                    envelope: link.envelope,
                    canonical_bounds: link.canonical_bounds,
                    transform: WireTransform {
                        origin: link.transform.origin(),
                        translation: link.transform.translation().into(),
                        shift: link.transform.scale_shift(),
                    },
                })
                .collect(),
        }
    }
    fn restore(self) -> Result<AnchorPin, PersistenceError> {
        let links = self
            .links
            .into_iter()
            .map(|link| {
                Ok(AnchorPinLink {
                    frame: link.frame,
                    parent: link.parent,
                    envelope: link.envelope,
                    canonical_bounds: link.canonical_bounds,
                    transform: DyadicTransform::from_exact(
                        link.transform.origin,
                        link.transform.translation.decode()?,
                        link.transform.shift,
                    )
                    .map_err(assembly_error)?,
                })
            })
            .collect::<Result<Vec<_>, PersistenceError>>()?;
        AnchorPin::from_record(AnchorPinRecord {
            model_id: self.model_id,
            owner: self.owner,
            anchor: self.anchor,
            object_id: self.object_id,
            members: self.members,
            local_point: self.local_point,
            anchored_point: self.anchored_point.decode()?,
            links,
        })
        .map_err(assembly_error)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireContext {
    model_id: String,
    root: String,
    lens: SemanticLens,
    policy: LayoutPolicy,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSolution {
    model_id: String,
    revision: u64,
    projection_key: String,
    version: u32,
    quality_version: u32,
    clearance: i64,
    frames: Vec<WireFrame>,
    routes: BTreeMap<String, WireRoute>,
    work: ComposedWork,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFrame {
    id: String,
    bounds: Bounds,
    model_id: String,
    revision: u64,
    projection_key: String,
    version: u32,
    boxes: BTreeMap<String, LayoutBox>,
    diagnostics: Vec<String>,
    parent: Option<String>,
    transform: WireTransform,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTransform {
    origin: Point,
    translation: WireExact,
    shift: u8,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireExact {
    numerators: [String; 3],
    shift: u8,
}
impl From<ExactPoint> for WireExact {
    fn from(p: ExactPoint) -> Self {
        Self {
            numerators: p.numerators().map(|n| n.to_string()),
            shift: p.denominator_shift(),
        }
    }
}
impl WireExact {
    fn decode(&self) -> Result<ExactPoint, PersistenceError> {
        let mut values = [0i128; 3];
        for (value, text) in values.iter_mut().zip(&self.numerators) {
            if text.len() > 41 {
                return Err(invalid("excessive exact coordinate"));
            }
            *value = text
                .parse()
                .map_err(|_| invalid("invalid exact coordinate"))?;
            if value.to_string() != *text {
                return Err(invalid("noncanonical exact coordinate"));
            }
        }
        let result = ExactPoint::new(values, self.shift).map_err(assembly_error)?;
        if result.numerators() != values || result.denominator_shift() != self.shift {
            return Err(invalid("non-normalized exact coordinate"));
        }
        Ok(result)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WirePin {
    owner: String,
    anchor: String,
    object_id: String,
    local: Point,
    anchored: WireExact,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEdge {
    id: String,
    source: String,
    target: String,
    semantic_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bundle: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireRoute {
    edge: WireEdge,
    fragments: Vec<WireFragment>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFragment {
    frame_id: String,
    request_id: String,
    request_semantic_ids: Vec<String>,
    source: WireTerminal,
    target: WireTerminal,
    route_id: String,
    route_semantic_ids: Vec<String>,
    points: Vec<Point>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireTerminal {
    Box { id: String },
    Boundary { point: Point, inward: Point },
}
impl From<&FragmentTerminal> for WireTerminal {
    fn from(t: &FragmentTerminal) -> Self {
        match t {
            FragmentTerminal::Box(id) => Self::Box { id: id.clone() },
            FragmentTerminal::Boundary { point, inward } => Self::Boundary {
                point: *point,
                inward: *inward,
            },
        }
    }
}
impl From<WireTerminal> for FragmentTerminal {
    fn from(t: WireTerminal) -> Self {
        match t {
            WireTerminal::Box { id } => Self::Box(id),
            WireTerminal::Boundary { point, inward } => Self::Boundary { point, inward },
        }
    }
}

impl WireState {
    fn capture(
        state: &PersistedComposedView,
        cancelled: &AtomicBool,
    ) -> Result<Self, PersistenceError> {
        check_shape(state, cancelled)?; // Bound all clones before allocating them.
        let s = &state.accepted;
        let mut frames = Vec::new();
        for id in s.assembly.frame_ids() {
            check_cancel(cancelled)?;
            let frame = s.assembly.frame(id).map_err(assembly_error)?;
            let instance = s.assembly.instance(id).map_err(assembly_error)?;
            let local = frame.solution();
            frames.push(WireFrame {
                id: id.into(),
                bounds: frame.bounds(),
                model_id: local.model_id.clone(),
                revision: local.revision,
                projection_key: local.projection_key.clone(),
                version: local.version,
                boxes: local.boxes.clone(),
                diagnostics: local.diagnostics.clone(),
                parent: instance.parent.clone(),
                transform: WireTransform {
                    origin: instance.transform.origin(),
                    translation: instance.transform.translation().into(),
                    shift: instance.transform.scale_shift(),
                },
            });
        }
        let mut routes = BTreeMap::new();
        for (id, route) in &s.routes {
            check_cancel(cancelled)?;
            routes.insert(
                id.clone(),
                WireRoute {
                    edge: WireEdge {
                        id: route.edge.id.clone(),
                        source: route.edge.source.clone(),
                        target: route.edge.target.clone(),
                        semantic_ids: route.edge.semantic_ids.clone(),
                        bundle: route.edge.bundle.clone(),
                    },
                    fragments: route
                        .fragments
                        .iter()
                        .map(|f| WireFragment {
                            frame_id: f.frame_id.clone(),
                            request_id: f.request.id.clone(),
                            request_semantic_ids: f.request.semantic_ids.clone(),
                            source: (&f.request.source).into(),
                            target: (&f.request.target).into(),
                            route_id: f.route.id.clone(),
                            route_semantic_ids: f.route.semantic_ids.clone(),
                            points: f.route.points.clone(),
                        })
                        .collect(),
                },
            );
        }
        Ok(Self {
            format: FORMAT.into(),
            version: state.version,
            model_id: state.model_id.clone(),
            revision: state.revision,
            context: WireContext {
                model_id: state.context.model_id.clone(),
                root: state.context.root.clone(),
                lens: state.context.lens,
                policy: state.context.policy,
            },
            view: state.view.clone(),
            canonical_pins: state.canonical_pins.clone(),
            constraint_sets: state
                .constraint_sets
                .iter()
                .map(|set| WireConstraintSet {
                    context: set.context.clone(),
                    pins: set.pins.clone(),
                    ownership: set.ownership.clone(),
                    presentation: (!set.presentation.is_empty()).then(|| set.presentation.clone()),
                    anchors: set.anchors.iter().map(WireAnchorRecord::capture).collect(),
                })
                .collect(),
            dismissed_proposals: state.dismissed_proposals.clone(),
            anchor_pins: state
                .anchor_pins
                .iter()
                .map(|pin| WirePin {
                    owner: pin.owner().into(),
                    anchor: pin.anchor().into(),
                    object_id: pin.object_id().into(),
                    local: pin.local_point(),
                    anchored: pin.anchored_point().into(),
                })
                .collect(),
            accepted: WireSolution {
                model_id: s.model_id.clone(),
                revision: s.revision,
                projection_key: s.projection_key.clone(),
                version: s.version,
                quality_version: s.quality_version,
                clearance: s.clearance,
                frames,
                routes,
                work: s.work.clone(),
            },
        })
    }

    fn restore(self, cancelled: &AtomicBool) -> Result<PersistedComposedView, PersistenceError> {
        check_cancel(cancelled)?;
        if self.format != FORMAT {
            return Err(invalid("not a composed view-state file"));
        }
        if !matches!(self.version, 1 | 2 | 3 | 4 | 5 | STATE_VERSION) {
            return Err(PersistenceError::UnsupportedVersion(self.version));
        }
        if self.version == 1 && !self.constraint_sets.is_empty() {
            return Err(invalid(
                "version 1 cannot contain durable constraint registry records",
            ));
        }
        if self.constraint_sets.len() > 64 {
            return Err(invalid("too many stored constraint contexts"));
        }
        let mut constraint_sets = Vec::new();
        for set in self.constraint_sets {
            check_cancel(cancelled)?;
            if self.version < 6 && set.presentation.is_some() {
                return Err(invalid(
                    "legacy state cannot contain pin presentation metadata",
                ));
            }
            if set.anchors.len() > MAX_TOTAL_BOXES {
                return Err(invalid("too many stored anchors"));
            }
            constraint_sets.push(PersistedConstraintSet {
                context: set.context,
                pins: set.pins,
                ownership: set.ownership,
                presentation: set.presentation.unwrap_or_default(),
                anchors: set
                    .anchors
                    .into_iter()
                    .map(WireAnchorRecord::restore)
                    .collect::<Result<Vec<_>, _>>()?,
            });
        }
        let s = self.accepted;
        if s.frames.is_empty()
            || s.frames.len() > MAX_FRAMES
            || s.routes.len() > 4_096
            || s.frames.iter().map(|f| f.boxes.len()).sum::<usize>() > MAX_TOTAL_BOXES
        {
            return Err(invalid("saved frame or route count exceeds budget"));
        }
        let mut frames = Vec::new();
        let mut instances = Vec::new();
        for frame in s.frames {
            check_cancel(cancelled)?;
            // CanonicalFrame checks malformed bounds before arithmetic. Assembly
            // then checks exact envelopes, hosting, ownership and depth/scale.
            instances.push(AssemblyInstance {
                frame_id: frame.id.clone(),
                parent: frame.parent,
                transform: DyadicTransform::from_exact(
                    frame.transform.origin,
                    frame.transform.translation.decode()?,
                    frame.transform.shift,
                )
                .map_err(assembly_error)?,
            });
            frames.push(
                CanonicalFrame::with_root(
                    frame.id,
                    frame.bounds,
                    LayoutSolution {
                        model_id: frame.model_id,
                        revision: frame.revision,
                        projection_key: frame.projection_key,
                        version: frame.version,
                        boxes: frame.boxes,
                        routes: vec![],
                        metrics: Metrics::default(),
                        diagnostics: frame.diagnostics,
                    },
                )
                .map_err(assembly_error)?,
            );
        }
        let assembly = Assembly::new(frames, instances).map_err(assembly_error)?;
        let mut anchors = Vec::new();
        if self.anchor_pins.len() > MAX_TOTAL_BOXES {
            return Err(invalid("too many anchor pins"));
        }
        for pin in self.anchor_pins {
            check_cancel(cancelled)?;
            let captured = AnchorPin::capture(&assembly, &pin.owner, &pin.anchor, &pin.object_id)
                .map_err(assembly_error)?;
            if captured.local_point() != pin.local
                || captured.anchored_point() != pin.anchored.decode()?
            {
                return Err(invalid(
                    "saved anchor point differs from its exact assembly capture",
                ));
            }
            anchors.push(captured);
        }
        let mut routes = BTreeMap::new();
        for (id, route) in s.routes {
            check_cancel(cancelled)?;
            routes.insert(
                id,
                ComposedRoute {
                    edge: Edge {
                        bundle: route.edge.bundle,
                        id: route.edge.id,
                        source: route.edge.source,
                        target: route.edge.target,
                        semantic_ids: route.edge.semantic_ids,
                    },
                    fragments: route
                        .fragments
                        .into_iter()
                        .map(|f| FrameFragment {
                            frame_id: f.frame_id,
                            request: FragmentRequest {
                                id: f.request_id,
                                semantic_ids: f.request_semantic_ids,
                                source: f.source.into(),
                                target: f.target.into(),
                            },
                            route: RouteFragment {
                                id: f.route_id,
                                semantic_ids: f.route_semantic_ids,
                                points: f.points,
                            },
                        })
                        .collect(),
                },
            );
        }
        let state = PersistedComposedView {
            version: self.version,
            model_id: self.model_id,
            revision: self.revision,
            context: CompositionContext {
                model_id: self.context.model_id,
                root: self.context.root,
                lens: self.context.lens,
                policy: self.context.policy,
            },
            view: self.view,
            canonical_pins: self.canonical_pins,
            anchor_pins: anchors,
            constraint_sets,
            dismissed_proposals: self.dismissed_proposals,
            accepted: ComposedSolution {
                model_id: s.model_id,
                revision: s.revision,
                projection_key: s.projection_key,
                version: s.version,
                quality_version: s.quality_version,
                clearance: s.clearance,
                assembly,
                routes,
                metrics: ComposedMetrics::default(),
                work: s.work,
            },
        };
        check_shape(&state, cancelled)?;
        Ok(state)
    }
}

#[derive(Default)]
struct ShapeBudget {
    text: usize,
    members: usize,
    points: usize,
    fragments: usize,
}
impl ShapeBudget {
    fn text(&mut self, value: &str, maximum: usize) -> Result<(), PersistenceError> {
        if value.is_empty() || value.len() > maximum {
            return Err(invalid("empty or excessive saved identifier"));
        }
        self.text = self
            .text
            .saturating_add(value.len())
            .saturating_add(std::mem::size_of::<String>());
        if self.text > MAX_STATE_BYTES {
            return Err(PersistenceError::TooLarge);
        }
        Ok(())
    }
    fn members(
        &mut self,
        values: &[String],
        cancelled: &AtomicBool,
    ) -> Result<(), PersistenceError> {
        self.members = self.members.saturating_add(values.len());
        if values.is_empty() || self.members > MAX_TOTAL_SEMANTIC_MEMBERS {
            return Err(invalid("saved membership count exceeds budget"));
        }
        let mut distinct = BTreeSet::new();
        for id in values {
            check_cancel(cancelled)?;
            self.text(id, 4096)?;
            if !distinct.insert(id) {
                return Err(invalid("duplicate saved semantic membership"));
            }
        }
        Ok(())
    }
}
fn check_point(p: Point) -> Result<(), PersistenceError> {
    if [p.x, p.y, p.z]
        .into_iter()
        .any(|v| !(-MAX_COORDINATE..=MAX_COORDINATE).contains(&v))
    {
        Err(invalid("saved coordinate exceeds canonical numeric range"))
    } else {
        Ok(())
    }
}

fn check_shape(
    state: &PersistedComposedView,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    check_cancel(cancelled)?;
    if !matches!(state.version, 1 | 2 | 3 | 4 | 5 | STATE_VERSION) {
        return Err(PersistenceError::UnsupportedVersion(state.version));
    }
    if state.version < 6
        && state
            .constraint_sets
            .iter()
            .any(|set| !set.presentation.is_empty())
    {
        return Err(invalid(
            "legacy state cannot contain pin presentation metadata",
        ));
    }
    let s = &state.accepted;
    let mut budget = ShapeBudget::default();
    check_constraint_sets(
        &state.model_id,
        state.constraint_sets.iter(),
        &mut budget,
        cancelled,
    )?;
    budget.text(&state.model_id, 128)?;
    budget.text(&state.context.root, 4096)?;
    budget.text(&s.projection_key, 512 * 1024)?;
    if state.model_id != s.model_id
        || state.model_id != state.context.model_id
        || state.revision == 0
        || state.revision != s.revision
        || s.version != COMPOSITION_VERSION
        || s.quality_version != QUALITY_VERSION
        || state.context.root != s.assembly.root()
        || state.view.policy != state.context.policy
        || state
            .view
            .focus
            .as_ref()
            .is_some_and(|id| id != &state.context.root)
        || state
            .view
            .lens
            .is_some_and(|lens| lens != state.context.lens)
        || state
            .view
            .frame_lens
            .is_some_and(|lens| lens != state.context.lens)
        || !(0..=10_000_000).contains(&s.clearance)
    {
        return Err(invalid(
            "model, revision, context, version or clearance mismatch",
        ));
    }
    check_view(&state.view, &mut budget, cancelled)?;
    // This comparison requires no supplied model: the accepted canonical
    // frames already identify which stored pins are active. Reject a corrupt
    // contradictory registry during load and before replacing an existing file.
    let effective = effective_registry(state, cancelled)?;
    if let Some(set) = effective.as_ref() {
        let visible = |owner: &str, id: &str| {
            s.assembly
                .frame(owner)
                .is_ok_and(|frame| frame.solution().boxes.contains_key(id))
        };
        let active: CanonicalPins = set
            .pins
            .iter()
            .filter_map(|(owner, pins)| {
                let pins: BTreeMap<_, _> = pins
                    .iter()
                    .filter(|(id, _)| visible(owner, id))
                    .map(|(id, point)| (id.clone(), *point))
                    .collect();
                (!pins.is_empty()).then(|| (owner.clone(), pins))
            })
            .collect();
        let anchors: Vec<_> = set
            .anchors
            .iter()
            .filter(|pin| visible(pin.owner(), pin.object_id()))
            .cloned()
            .collect();
        if active != state.canonical_pins || anchors != state.anchor_pins {
            return Err(invalid(
                "active constraints differ from the durable registry",
            ));
        }
    }
    let frames: BTreeSet<_> = s.assembly.frame_ids().collect();
    if frames.is_empty()
        || frames.len() > MAX_FRAMES
        || s.routes.len() > 4096
        || s.work.allocated.len() != frames.len()
        || s.work.evaluations.len() != frames.len()
        || s.work.reused.len() > frames.len()
        || s.work
            .allocated
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != frames
        || s.work
            .evaluations
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            != frames
        || s.work.reused.keys().any(|id| !frames.contains(id.as_str()))
        || state.canonical_pins.len() > frames.len()
        || state.anchor_pins.len() > MAX_TOTAL_BOXES
    {
        return Err(invalid("invalid saved frame, work, route or pin keys"));
    }
    if s.work.feedback_improvements > s.work.feedback_evaluations
        || s.work.feedback_evaluations > s.work.budget
    {
        return Err(invalid(
            "invalid saved composed feedback evaluation counters",
        ));
    }
    let mut work = s.work.feedback_evaluations;
    for id in &frames {
        check_cancel(cancelled)?;
        budget.text(id, 4096)?;
        let local = s.assembly.frame(id).map_err(assembly_error)?.solution();
        if local.model_id != state.model_id
            || local.revision != state.revision
            || local.version != LAYOUT_VERSION
            || !local.routes.is_empty()
            || local.diagnostics.len() > 1024
        {
            return Err(invalid(
                "invalid canonical identity or duplicate kernel routes",
            ));
        }
        budget.text(&local.projection_key, 512 * 1024)?;
        for note in &local.diagnostics {
            budget.text(note, 512 * 1024)?;
        }
        let actual = s.work.evaluations[*id];
        let allocation = s.work.allocated[*id];
        work = work
            .checked_add(actual)
            .ok_or_else(|| invalid("saved work counter overflow"))?;
        if (actual == 0) != s.work.reused.contains_key(*id)
            || actual > allocation
            || allocation > s.work.budget
            || work > s.work.budget
        {
            return Err(invalid("saved placement evaluations exceed budget"));
        }
        for (key, object) in &local.boxes {
            check_cancel(cancelled)?;
            budget.text(key, 4096)?;
            budget.text(&object.id, 4096)?;
            if let Some(parent) = &object.parent {
                budget.text(parent, 4096)?;
            }
            budget.members(&object.semantic_ids, cancelled)?;
        }
    }
    for (frame, pins) in &state.canonical_pins {
        check_cancel(cancelled)?;
        budget.text(frame, 4096)?;
        let objects = &s
            .assembly
            .frame(frame)
            .map_err(assembly_error)?
            .solution()
            .boxes;
        if pins.len() > objects.len() {
            return Err(invalid("too many canonical pins"));
        }
        for (id, point) in pins {
            budget.text(id, 4096)?;
            check_point(*point)?;
            if objects.get(id).is_none_or(|b| b.bounds.center() != *point) {
                return Err(invalid(
                    "canonical pin differs from its saved object center",
                ));
            }
        }
    }
    let mut anchors = BTreeSet::new();
    for pin in &state.anchor_pins {
        check_cancel(cancelled)?;
        for id in [pin.owner(), pin.anchor(), pin.object_id()] {
            budget.text(id, 4096)?;
        }
        if !anchors.insert((pin.owner(), pin.anchor(), pin.object_id())) {
            return Err(invalid("duplicate anchor pin"));
        }
        pin.validate(&s.assembly).map_err(assembly_error)?;
        // Compare the entire transform/envelope signature, not just the point.
        if *pin
            != AnchorPin::capture(&s.assembly, pin.owner(), pin.anchor(), pin.object_id())
                .map_err(assembly_error)?
        {
            return Err(invalid("stale anchor capture"));
        }
    }
    for (id, route) in &s.routes {
        check_cancel(cancelled)?;
        for text in [id, &route.edge.id, &route.edge.source, &route.edge.target] {
            budget.text(text, 4096)?;
        }
        budget.members(&route.edge.semantic_ids, cancelled)?;
        budget.fragments = budget.fragments.saturating_add(route.fragments.len());
        if id != &route.edge.id
            || route.fragments.is_empty()
            || budget.fragments > MAX_FRAME_STEPS + 4096
        {
            return Err(invalid("invalid saved route identity or fragment count"));
        }
        let mut distinct = BTreeSet::new();
        for f in &route.fragments {
            check_cancel(cancelled)?;
            budget.text(&f.frame_id, 4096)?;
            if !frames.contains(f.frame_id.as_str())
                || !distinct.insert(&f.frame_id)
                || f.request.id != *id
                || f.route.id != *id
                || f.request.semantic_ids != route.edge.semantic_ids
                || f.route.semantic_ids != route.edge.semantic_ids
                || f.route.points.len() < 2
            {
                return Err(invalid(
                    "saved fragment frame, identity or membership mismatch",
                ));
            }
            budget.text(&f.request.id, 4096)?;
            budget.text(&f.route.id, 4096)?;
            budget.members(&f.request.semantic_ids, cancelled)?;
            budget.members(&f.route.semantic_ids, cancelled)?;
            let frame = s.assembly.frame(&f.frame_id).map_err(assembly_error)?;
            for terminal in [&f.request.source, &f.request.target] {
                match terminal {
                    FragmentTerminal::Box(id) => {
                        budget.text(id, 4096)?;
                        if !frame.solution().boxes.contains_key(id) {
                            return Err(invalid("unknown terminal box"));
                        }
                    }
                    FragmentTerminal::Boundary { point, inward } => {
                        check_point(*point)?;
                        if ![
                            Point::new(1, 0, 0),
                            Point::new(-1, 0, 0),
                            Point::new(0, 1, 0),
                            Point::new(0, -1, 0),
                            Point::new(0, 0, 1),
                            Point::new(0, 0, -1),
                        ]
                        .contains(inward)
                        {
                            return Err(invalid("saved boundary normal is not a unit axis"));
                        }
                    }
                }
            }
            budget.points = budget.points.saturating_add(f.route.points.len());
            if budget.points > MAX_TOTAL_ROUTE_POINTS {
                return Err(invalid("saved route point count exceeds budget"));
            }
            for chunk in f.route.points.chunks(256) {
                check_cancel(cancelled)?;
                for point in chunk {
                    check_point(*point)?;
                }
            }
        }
    }
    if state.dismissed_proposals.len() > 32 {
        return Err(invalid("too many dismissed proposals"));
    }
    let mut dismissed = BTreeSet::new();
    for proposal in &state.dismissed_proposals {
        budget.text(&proposal.root, 4096)?;
        if proposal.encoding_version != DISMISSAL_ENCODING_VERSION
            || proposal.layout_version != COMPOSITION_VERSION
            || proposal.model_id != state.model_id
            || !dismissed.insert(proposal)
        {
            return Err(invalid(
                "invalid, incompatible or duplicate dismissed proposal",
            ));
        }
    }
    check_cancel(cancelled)
}

/// Bound a registry before accepting an override, independently of disk I/O.
pub fn validate_constraint_registry(
    model_id: &str,
    sets: &[PersistedConstraintSet],
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    check_constraint_sets(
        model_id,
        sets.iter(),
        &mut ShapeBudget::default(),
        cancelled,
    )
}
pub(crate) fn validate_constraint_refs(
    model_id: &str,
    sets: &[&PersistedConstraintSet],
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    check_constraint_sets(
        model_id,
        sets.iter().copied(),
        &mut ShapeBudget::default(),
        cancelled,
    )
}
fn check_constraint_sets<'a>(
    model_id: &str,
    sets: impl ExactSizeIterator<Item = &'a PersistedConstraintSet>,
    budget: &mut ShapeBudget,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    if sets.len() > 64 {
        return Err(invalid("too many stored constraint contexts"));
    }
    let mut contexts = BTreeSet::new();
    let mut pin_count = 0usize;
    let mut anchor_count = 0usize;
    let mut protected_groups = BTreeMap::new();
    let mut supplied_paths = BTreeMap::new();
    for set in sets {
        check_cancel(cancelled)?;
        if set.context.model_id != model_id
            || !contexts.insert((set.context.lens, &set.context.root))
        {
            return Err(invalid("foreign or duplicate stored constraint context"));
        }
        budget.text(&set.context.model_id, 128)?;
        budget.text(&set.context.root, 4096)?;
        budget.text = budget.text.saturating_add(512);
        let mut objects = BTreeSet::new();
        for (owner, pins) in &set.pins {
            budget.text(owner, 4096)?;
            budget.text = budget.text.saturating_add(128);
            if pins.is_empty() {
                return Err(invalid("empty stored pin frame"));
            }
            for (id, point) in pins {
                check_cancel(cancelled)?;
                pin_count = pin_count.saturating_add(1);
                if pin_count > MAX_TOTAL_BOXES || !objects.insert(id.as_str()) {
                    return Err(invalid("too many or duplicate stored pinned objects"));
                }
                budget.text(id, 4096)?;
                budget.text = budget.text.saturating_add(128);
                check_point(*point)?;
            }
        }
        let mut anchored = BTreeSet::new();
        for pin in &set.anchors {
            check_cancel(cancelled)?;
            anchor_count = anchor_count.saturating_add(1);
            if anchor_count > MAX_TOTAL_BOXES
                || pin.model_id() != model_id
                || !anchored.insert((pin.owner(), pin.anchor(), pin.object_id()))
                || set
                    .pins
                    .get(pin.owner())
                    .and_then(|pins| pins.get(pin.object_id()))
                    .is_none_or(|point| *point != pin.local_point())
            {
                return Err(invalid("stored anchor differs from its canonical pin"));
            }
            for id in [pin.owner(), pin.anchor(), pin.object_id()] {
                budget.text(id, 4096)?;
            }
            for id in pin.semantic_ids() {
                budget.text(id, 4096)?;
            }
            for link in pin.links() {
                budget.text = budget.text.saturating_add(384);
                budget.text(&link.frame, 4096)?;
                budget.text(&link.parent, 4096)?;
            }
        }
        for (id, path) in &set.ownership {
            check_cancel(cancelled)?;
            budget.text(id, 4096)?;
            if !objects.contains(id.as_str())
                || path.is_empty()
                || path.len() > 65
                || path.first() != Some(id)
                || path.iter().collect::<BTreeSet<_>>().len() != path.len()
            {
                return Err(invalid("invalid stored supplied ownership path"));
            }
            for item in path {
                budget.text = budget.text.saturating_add(32);
                budget.text(item, 4096)?;
            }
        }
        check_presentations(
            set,
            &objects,
            &mut protected_groups,
            &mut supplied_paths,
            budget,
            cancelled,
        )?;
        if budget.text > MAX_STATE_BYTES {
            return Err(PersistenceError::TooLarge);
        }
    }
    Ok(())
}

fn same_group(a: &DisplayGroup, b: &DisplayGroup) -> bool {
    a.id == b.id
        && a.parent == b.parent
        && a.suggested == b.suggested
        && a.members.iter().collect::<BTreeSet<_>>() == b.members.iter().collect()
}

fn check_path(
    id: &str,
    path: &[String],
    maximum: usize,
    budget: &mut ShapeBudget,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    if path.is_empty()
        || path.len() > maximum
        || path[0] != id
        || path.iter().collect::<BTreeSet<_>>().len() != path.len()
    {
        return Err(invalid("invalid stored pin ancestry path"));
    }
    for item in path {
        check_cancel(cancelled)?;
        budget.text = budget.text.saturating_add(32);
        budget.text(item, 4096)?;
    }
    Ok(())
}

fn check_presentations<'a>(
    set: &'a PersistedConstraintSet,
    objects: &BTreeSet<&str>,
    protected: &mut BTreeMap<(SemanticLens, &'a str), &'a DisplayGroup>,
    supplied_paths: &mut BTreeMap<(SemanticLens, &'a str), &'a [String]>,
    budget: &mut ShapeBudget,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    if set.presentation.len() > objects.len() {
        return Err(invalid("orphan pin presentation metadata"));
    }
    for (id, metadata) in &set.presentation {
        check_cancel(cancelled)?;
        budget.text(id, 4096)?;
        budget.text = budget.text.saturating_add(256);
        if !objects.contains(id.as_str()) || metadata.groups.len() > 64 {
            return Err(invalid("orphan or excessive pin presentation metadata"));
        }
        // Up to 65 supplied IDs, 64 protected group frames and one reference.
        check_path(id, &metadata.path, 130, budget, cancelled)?;
        let owner = set
            .pins
            .iter()
            .find(|(_, pins)| pins.contains_key(id))
            .unwrap()
            .0;
        let owner_index = metadata.path.iter().position(|item| item == owner);
        let context_index = metadata
            .path
            .iter()
            .position(|item| item == &set.context.root);
        if owner_index
            .zip(context_index)
            .is_none_or(|(owner, context)| owner > context)
        {
            return Err(invalid(
                "pin presentation omits its canonical owner or source context",
            ));
        }
        let mut groups = BTreeMap::new();
        let mut members = BTreeSet::new();
        let mut permitted_supplied: BTreeSet<&str> =
            metadata.path.iter().map(String::as_str).collect();
        for group in &metadata.groups {
            check_cancel(cancelled)?;
            budget.text(&group.id, 256)?;
            budget.text(&group.parent, 4096)?;
            if group.members.len() < 2
                || group.members.len() > 128
                || !metadata.path.contains(&group.id)
                || groups.insert(group.id.as_str(), group).is_some()
                || group.id == group.parent
            {
                return Err(invalid("invalid or unrelated protected group definition"));
            }
            for member in &group.members {
                check_cancel(cancelled)?;
                budget.text(member, 4096)?;
                if member == &group.id
                    || member == &group.parent
                    || !members.insert(member.as_str())
                {
                    return Err(invalid(
                        "protected groups require distinct unshared supplied members",
                    ));
                }
                permitted_supplied.insert(member);
            }
            permitted_supplied.insert(&group.parent);
            if protected
                .insert((set.context.lens, &group.id), group)
                .is_some_and(|previous| !same_group(previous, group))
            {
                return Err(invalid("conflicting protected group definitions"));
            }
            if protected.len() > 64 {
                return Err(invalid(
                    "at most 64 distinct protected groups are supported",
                ));
            }
        }
        if metadata.supplied.len() > 64 * 129 + 130 {
            return Err(invalid("excessive pin supplied ancestry metadata"));
        }
        if let Some(reference) = &metadata.reference {
            budget.text(&reference.origin, 4096)?;
            budget.text(&reference.target, 4096)?;
            if reference.origin != set.context.root
                || reference.origin != *owner
                || metadata.path.get(1) != Some(&reference.origin)
                || reference.origin == reference.target
                || reference.target == *id
                || groups.contains_key(id.as_str())
                || set.ownership.contains_key(id)
            {
                return Err(invalid("invalid reference pin display origin or target"));
            }
            permitted_supplied.insert(&reference.target);
        } else if groups.contains_key(id.as_str()) && set.ownership.contains_key(id) {
            return Err(invalid(
                "a protected group cannot claim supplied object ownership",
            ));
        }
        for (supplied, path) in &metadata.supplied {
            check_cancel(cancelled)?;
            budget.text(supplied, 4096)?;
            if !permitted_supplied.contains(supplied.as_str())
                || groups.contains_key(supplied.as_str())
                || metadata.reference.is_some() && supplied == id
            {
                return Err(invalid("orphan or nonsupplied pin ancestry metadata"));
            }
            check_path(supplied, path, 65, budget, cancelled)?;
            if path.iter().any(|item| groups.contains_key(item.as_str()))
                || set
                    .ownership
                    .get(supplied)
                    .is_some_and(|ownership| ownership != path)
            {
                return Err(invalid("conflicting supplied pin presentation ancestry"));
            }
            check_shared_path(set.context.lens, path, supplied_paths, cancelled)?;
        }
        if let Some(path) = set.ownership.get(id) {
            check_shared_path(set.context.lens, path, supplied_paths, cancelled)?;
        }
        // Every group member is a supplied immediate child. Its complete path
        // must agree with the recorded parent path, not merely share an ID.
        for group in groups.values() {
            let parent = metadata
                .supplied
                .get(&group.parent)
                .ok_or_else(|| invalid("protected group lacks supplied parent ancestry"))?;
            for member in &group.members {
                let path = metadata
                    .supplied
                    .get(member)
                    .ok_or_else(|| invalid("protected group lacks supplied member ancestry"))?;
                if path.get(1..) != Some(parent.as_slice()) {
                    return Err(invalid("protected group members are not supplied siblings"));
                }
            }
        }
        let mut base = if let Some(reference) = &metadata.reference {
            if !metadata.supplied.contains_key(&reference.target) {
                return Err(invalid("reference pin lacks supplied target ancestry"));
            }
            let mut base = vec![id.clone()];
            if let Some(group) = groups.get(reference.origin.as_str()) {
                base.push(group.id.clone());
                base.extend(metadata.supplied[&group.parent].iter().cloned());
            } else {
                base.extend(
                    metadata
                        .supplied
                        .get(&reference.origin)
                        .ok_or_else(|| invalid("reference pin lacks supplied origin ancestry"))?
                        .iter()
                        .cloned(),
                );
            }
            base
        } else if let Some(group) = groups.get(id.as_str()) {
            let mut base = vec![id.clone()];
            base.extend(metadata.supplied[&group.parent].iter().cloned());
            base
        } else {
            set.ownership
                .get(id)
                .ok_or_else(|| invalid("supplied pin presentation lacks architectural ownership"))?
                .clone()
        };
        // Insert only declared visual group frames between supplied siblings
        // and their parent. This reconstructs the display chain without any
        // convention about group/reference ID strings.
        let mut display = Vec::with_capacity(base.len() + groups.len());
        for (index, item) in base.iter().enumerate() {
            display.push(item.clone());
            if let Some(group) = groups.values().find(|group| {
                group.members.contains(item) && base.get(index + 1) == Some(&group.parent)
            }) {
                display.push(group.id.clone());
            }
        }
        if display != metadata.path {
            return Err(invalid(
                "pin display ancestry differs from its protected presentation",
            ));
        }
        // Cross-check redundant supplied records along the pinned object's
        // chain too; all paths remain complete rather than arbitrary suffixes.
        if metadata.reference.is_some() || groups.contains_key(id.as_str()) {
            base.remove(0);
        }
        for (index, item) in base.iter().enumerate() {
            if let Some(path) = metadata.supplied.get(item)
                && path.as_slice() != &base[index..]
            {
                return Err(invalid("inconsistent supplied pin presentation suffix"));
            }
        }
        for anchor in set.anchors.iter().filter(|anchor| anchor.object_id() == id) {
            check_cancel(cancelled)?;
            if let Some(reference) = &metadata.reference
                && anchor.semantic_ids() != &BTreeSet::from([reference.target.clone()])
            {
                return Err(invalid("reference pin membership differs from its target"));
            }
            let mut previous = owner_index.unwrap();
            for link in anchor.links() {
                let frame = metadata.path.iter().position(|item| item == &link.frame);
                let parent = metadata.path.iter().position(|item| item == &link.parent);
                if frame != Some(previous) || parent.is_none_or(|parent| parent <= previous) {
                    return Err(invalid(
                        "pin anchor chain conflicts with presentation ancestry",
                    ));
                }
                previous = parent.unwrap();
            }
        }
    }
    Ok(())
}

fn check_shared_path<'a>(
    lens: SemanticLens,
    path: &'a [String],
    known: &mut BTreeMap<(SemanticLens, &'a str), &'a [String]>,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    for (index, id) in path.iter().enumerate() {
        check_cancel(cancelled)?;
        if known
            .insert((lens, id), &path[index..])
            .is_some_and(|previous| previous != &path[index..])
        {
            return Err(invalid("conflicting shared supplied presentation ancestry"));
        }
    }
    Ok(())
}

fn check_view(
    view: &ViewState,
    budget: &mut ShapeBudget,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    if view.visible_budget == 0
        || view.visible_budget > 1024
        || view.pins.len() > 4096
        || view.frame_pins.len() > 64
        || view.layer_preferences.len() > 4096
        || view.expanded.len() > 4096
        || view.collapsed.len() > 4096
        || view.group_overrides.len() > 1024
        || view.dismissed_groups.len() > 1024
        || view.retained_suggestions().len() > 64
        || view.breadcrumbs().len() > 256
        || view.child_page > 1_000_000
        || view.relation_history.len() > 64
        || view
            .relation_cursor
            .iter()
            .chain(view.relation_history.iter().flatten())
            .any(|c| !c.valid_shape())
        || view.expanded.iter().any(|id| view.collapsed.contains(id))
        || view
            .layer_preferences
            .values()
            .any(|band| !(-16..=16).contains(band))
    {
        return Err(invalid("view overrides exceed supported bounds"));
    }
    let expanded = view.expanded_ids();
    if expanded.len() > 4096 {
        return Err(invalid("too many automatic expansions"));
    }
    for id in view
        .focus
        .iter()
        .chain(view.breadcrumbs().iter().flatten())
        .chain(expanded.iter())
        .chain(view.collapsed.iter())
        .chain(view.dismissed_groups.iter())
        .chain(view.layer_preferences.keys())
    {
        check_cancel(cancelled)?;
        budget.text(id, 4096)?;
    }
    let mut pins_count = 0usize;
    for (frame, pins) in std::iter::once(("root", &view.pins))
        .chain(view.frame_pins.iter().map(|(id, pins)| (id.as_str(), pins)))
    {
        check_cancel(cancelled)?;
        budget.text(frame, 4128)?;
        let local = ["l:logical|", "l:code|", "l:deployment|"]
            .into_iter()
            .find_map(|p| frame.strip_prefix(p))
            .unwrap_or(frame);
        pins_count = pins_count.saturating_add(pins.len());
        if (local != "root" && local.strip_prefix("f:").is_none_or(str::is_empty))
            || pins.len() > 4096
            || pins_count > 20_480
        {
            return Err(invalid("invalid frame-scoped view pins"));
        }
        for (id, point) in pins {
            budget.text(id, 4096)?;
            check_point(*point)?;
        }
    }
    for group in view
        .group_overrides
        .iter()
        .chain(view.retained_suggestions())
    {
        check_cancel(cancelled)?;
        budget.text(&group.id, 4096)?;
        budget.text(&group.parent, 4096)?;
        if group.members.is_empty()
            || group.members.len() > 4096
            || group.members.iter().collect::<BTreeSet<_>>().len() != group.members.len()
        {
            return Err(invalid("invalid saved visual group"));
        }
        for id in &group.members {
            budget.text(id, 4096)?;
        }
    }
    Ok(())
}

/// Read a bounded regular file without modifying it. This first stage restores
/// view state but does not certify geometry against a supplied architecture.
pub fn load(
    path: impl AsRef<Path>,
    cancelled: &AtomicBool,
) -> Result<PersistedComposedView, PersistenceError> {
    check_cancel(cancelled)?;
    let path = path.as_ref();
    if !std::fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("state path must be a regular file"));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0004 | 0x0100);
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x0800 | 0x20000);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(invalid("state path is not a regular file"));
    }
    if metadata.len() > MAX_STATE_BYTES as u64 {
        return Err(PersistenceError::TooLarge);
    }
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65_536];
    loop {
        check_cancel(cancelled)?;
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        if read > MAX_STATE_BYTES - bytes.len() {
            return Err(PersistenceError::TooLarge);
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let value = strict_json::parse(&bytes, cancelled)?;
    if let Some(v) = value.get("version").and_then(serde_json::Value::as_u64)
        && v != 1
        && v != 2
        && v != 3
        && v != 4
        && v != 5
        && v != u64::from(STATE_VERSION)
    {
        return Err(PersistenceError::UnsupportedVersion(
            v.try_into().unwrap_or(u32::MAX),
        ));
    }
    let wire: WireState = serde_json::from_value(value)?;
    wire.restore(cancelled)
}

/// Atomically replace an explicit composed-state file. The caller validates the
/// accepted result against its projection first. Foreign-model, newer-revision,
/// corrupt, unknown-version and nonregular destinations are preserved. Routine
/// focus/lens changes for the same model may replace its own prior view state.
pub fn save(
    path: impl AsRef<Path>,
    state: &PersistedComposedView,
    cancelled: &AtomicBool,
) -> Result<(), PersistenceError> {
    if state.version != STATE_VERSION {
        return Err(invalid(
            "old composed state must be validated and upgraded before saving",
        ));
    }
    let path = path.as_ref();
    let wire = WireState::capture(state, cancelled)?;
    let mut encoded = BoundedBuffer {
        bytes: Vec::new(),
        cancelled,
    };
    if let Err(e) = serde_json::to_writer(&mut encoded, &wire) {
        check_cancel(cancelled)?;
        return if e.is_io() {
            Err(PersistenceError::TooLarge)
        } else {
            Err(e.into())
        };
    }
    strict_json::parse(&encoded.bytes, cancelled)?;
    let original = destination_stamp(path)?;
    if original.is_some() {
        let existing = load(path, cancelled)?;
        if existing.model_id != state.model_id {
            return Err(invalid("destination belongs to a different model"));
        }
        if existing.revision > state.revision {
            return Err(invalid("refusing to replace a newer saved revision"));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if path.file_name().is_none() {
        return Err(invalid("destination has no file name"));
    }
    let temporary = parent.join(format!(
        ".visualyze-composed-state-{}-{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    check_cancel(cancelled)?;
    let mut file = options.open(&temporary)?;
    let mut guard = Temporary {
        path: temporary,
        armed: true,
    };
    for chunk in encoded.bytes.chunks(65_536) {
        check_cancel(cancelled)?;
        file.write_all(chunk)?;
    }
    file.flush()?;
    file.sync_all()?;
    drop(file);
    check_cancel(cancelled)?;
    if destination_stamp(path)? != original {
        return Err(PersistenceError::SourceChanged);
    }
    std::fs::rename(&guard.path, path)?;
    guard.armed = false;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}
struct Temporary {
    path: PathBuf,
    armed: bool,
}
impl Drop for Temporary {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
fn destination_stamp(path: &Path) -> Result<Option<(u64, SystemTime)>, PersistenceError> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => Ok(Some((m.len(), m.modified()?))),
        Ok(_) => Err(invalid("destination is not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}
struct BoundedBuffer<'a> {
    bytes: Vec<u8>,
    cancelled: &'a AtomicBool,
}
impl Write for BoundedBuffer<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.cancelled.load(Ordering::Relaxed)
            || bytes.len() > MAX_STATE_BYTES - self.bytes.len()
        {
            return Err(std::io::Error::other(
                "composed state cancelled or byte limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

mod strict_json {
    use super::{PersistenceError, check_cancel};
    use serde::de::{DeserializeSeed, Error, MapAccess, SeqAccess, Visitor};
    use serde_json::{Map, Number, Value};
    use std::{
        fmt,
        sync::atomic::{AtomicBool, Ordering},
    };

    pub(super) fn parse(bytes: &[u8], cancelled: &AtomicBool) -> Result<Value, PersistenceError> {
        check_cancel(cancelled)?;
        let mut parser = serde_json::Deserializer::from_slice(bytes);
        let mut remaining = 1_000_000;
        let result = Seed {
            depth: 0,
            remaining: &mut remaining,
            cancelled,
        }
        .deserialize(&mut parser);
        check_cancel(cancelled)?;
        let value = result?;
        parser.end()?;
        Ok(value)
    }
    struct Seed<'a> {
        depth: usize,
        remaining: &'a mut usize,
        cancelled: &'a AtomicBool,
    }
    impl<'de> DeserializeSeed<'de> for Seed<'_> {
        type Value = Value;
        fn deserialize<D: serde::Deserializer<'de>>(self, parser: D) -> Result<Value, D::Error> {
            if *self.remaining == 0 || self.cancelled.load(Ordering::Relaxed) {
                return Err(D::Error::custom(
                    "composed state cancelled or JSON value budget exceeded",
                ));
            }
            *self.remaining -= 1;
            parser.deserialize_any(self)
        }
    }
    impl<'de> Visitor<'de> for Seed<'_> {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("bounded composed view-state JSON")
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
            if self.depth >= 24 {
                return Err(A::Error::custom("composed state nesting exceeds 24"));
            }
            let mut result = Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if result.contains_key(&key) {
                    return Err(A::Error::custom("duplicate composed state key"));
                }
                let value = map.next_value_seed(Seed {
                    depth: self.depth + 1,
                    remaining: self.remaining,
                    cancelled: self.cancelled,
                })?;
                result.insert(key, value);
            }
            Ok(Value::Object(result))
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
            if self.depth >= 24 {
                return Err(A::Error::custom("composed state nesting exceeds 24"));
            }
            let mut result = Vec::new();
            while let Some(value) = seq.next_element_seed(Seed {
                depth: self.depth + 1,
                remaining: self.remaining,
                cancelled: self.cancelled,
            })? {
                result.push(value);
            }
            Ok(Value::Array(result))
        }
        fn visit_str<E: Error>(self, v: &str) -> Result<Value, E> {
            Ok(Value::String(v.into()))
        }
        fn visit_string<E: Error>(self, v: String) -> Result<Value, E> {
            Ok(Value::String(v))
        }
        fn visit_bool<E: Error>(self, v: bool) -> Result<Value, E> {
            Ok(Value::Bool(v))
        }
        fn visit_u64<E: Error>(self, v: u64) -> Result<Value, E> {
            Ok(Value::Number(v.into()))
        }
        fn visit_i64<E: Error>(self, v: i64) -> Result<Value, E> {
            Ok(Value::Number(v.into()))
        }
        fn visit_f64<E: Error>(self, v: f64) -> Result<Value, E> {
            Number::from_f64(v)
                .map(Value::Number)
                .ok_or_else(|| E::custom("non-finite saved number"))
        }
        fn visit_unit<E: Error>(self) -> Result<Value, E> {
            Ok(Value::Null)
        }
    }
}
