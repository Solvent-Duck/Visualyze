//! Bounded placement and exact routing-batch reuse. Boundary requests are rebuilt.
use super::*;
use crate::{
    Bounds, LayoutSolution, QUALITY_VERSION,
    assembly::{AnchorPin, AnchorPinLink},
    assembly_plan::{FrameCacheKey, PlannedFrame},
    check_cancel,
    projection::LayoutPolicy,
};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
};
use visualyze_model::provider::SemanticLens;

/// Effective semantic lens and layout policy must come from the caller. Root
/// identity determines whether a prior accepted composition is a local repair.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompositionContext {
    pub model_id: String,
    pub root: String,
    pub lens: SemanticLens,
    pub policy: LayoutPolicy,
}
#[derive(Clone, Copy)]
pub struct ComposedReuse<'a> {
    pub context: &'a CompositionContext,
    pub previous: Option<(&'a CompositionContext, &'a ComposedSolution)>,
    pub anchor_pins: &'a [AnchorPin],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalCacheLimits {
    pub max_entries: usize,
    /// Conservative retained-data estimate, not a process-RSS measurement.
    pub max_bytes: usize,
}
impl Default for CanonicalCacheLimits {
    fn default() -> Self {
        Self {
            max_entries: 128,
            max_bytes: 32 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalCacheStats {
    pub hits: usize,
    pub previous_hits: usize,
    pub misses: usize,
    pub evictions: usize,
    pub entries: usize,
    pub estimated_bytes: usize,
    /// Exact complete routing batches reused after rebuilding boundary requests.
    #[serde(default)]
    pub routing_hits: usize,
    #[serde(default)]
    pub routing_misses: usize,
}
#[derive(Debug, Clone)]
pub struct CanonicalPlacementCache {
    limits: CanonicalCacheLimits,
    entries: VecDeque<Entry>,
    bytes: usize,
    // A viewer cache has one active model. Model switches discard prior entries;
    // the active model's revision high-water mark survives ordinary eviction.
    latest: Option<(String, u64)>,
}
impl Default for CanonicalPlacementCache {
    fn default() -> Self {
        Self::new(CanonicalCacheLimits::default()).unwrap()
    }
}
impl CanonicalPlacementCache {
    pub fn new(limits: CanonicalCacheLimits) -> Result<Self, LayoutError> {
        if limits.max_entries > 1024 || limits.max_bytes > 256 * 1024 * 1024 {
            return Err(invalid("canonical cache limits exceed supported maximum"));
        }
        Ok(Self {
            limits,
            entries: VecDeque::new(),
            bytes: 0,
            latest: None,
        })
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn estimated_bytes(&self) -> usize {
        self.bytes
            + self
                .latest
                .as_ref()
                .map_or(0, |(model, _)| model.len() + 64)
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.bytes = 0;
    }
    fn get(&self, key: &PlacementKey) -> Option<Placement> {
        self.entries
            .iter()
            .rev()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.placement.clone())
    }
    pub(super) fn routes(
        &self,
        model_id: &str,
        frame_id: &str,
        placement: &crate::assembly::CanonicalFrame,
        requests: &[FragmentRequest],
        clearance: i64,
    ) -> Option<Vec<RouteFragment>> {
        self.entries
            .iter()
            .rev()
            .find(|entry| {
                entry.key.geometry.model_id == model_id
                    && entry.key.geometry.frame_id == frame_id
                    && entry.placement.domain == placement.bounds()
                    && entry.placement.local.boxes == placement.solution().boxes
                    && entry.routing.clearance == clearance
                    && entry.routing.requests == requests
            })
            .map(|entry| entry.routing.fragments.clone())
    }
    fn insert(&mut self, entry: Entry) -> usize {
        if let Some(index) = self.entries.iter().position(|old| old.key == entry.key) {
            self.bytes -= self.entries.remove(index).unwrap().bytes;
        }
        let watermark_bytes = self
            .latest
            .as_ref()
            .map_or(0, |(model, _)| model.len() + 64);
        if self.limits.max_entries == 0 || entry.bytes + watermark_bytes > self.limits.max_bytes {
            return 0;
        }
        let mut evictions = 0;
        while self.entries.len() >= self.limits.max_entries
            || self.estimated_bytes() + entry.bytes > self.limits.max_bytes
        {
            self.bytes -= self.entries.pop_front().unwrap().bytes;
            evictions += 1;
        }
        self.bytes += entry.bytes;
        self.entries.push_back(entry);
        evictions
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlacementKey {
    lens: SemanticLens,
    policy: LayoutPolicy,
    geometry: FrameCacheKey,
    algorithm: Algorithm,
    settings: [u64; 10],
    pins: BTreeMap<String, Point>,
    domain_lock: Option<AnchorPinLink>,
    host_locks: BTreeMap<String, Bounds>,
}
#[derive(Debug, Clone)]
struct Entry {
    key: PlacementKey,
    placement: Placement,
    routing: Arc<RoutingBatch>,
    bytes: usize,
}
#[derive(Debug, Clone)]
struct RoutingBatch {
    clearance: i64,
    // Request order and all original memberships are part of the exact key.
    // The complete batch matters: lanes and congestion depend on earlier routes.
    requests: Vec<FragmentRequest>,
    fragments: Vec<RouteFragment>,
}
#[derive(Debug, Clone)]
pub(super) struct Placement {
    pub local: LayoutSolution,
    pub domain: Bounds,
}
pub(super) struct Prepared<'a> {
    pub reused: BTreeMap<String, (Placement, PlacementReuse)>,
    pub previous: Option<&'a ComposedSolution>,
    pub search: bool,
    pub locked_domains: BTreeSet<String>,
    pub locks: BTreeMap<String, AnchorPinLink>,
    pub host_bounds: BTreeMap<String, BTreeMap<String, Bounds>>,
    pub routing_cache: &'a CanonicalPlacementCache,
}

/// Reuse canonical placements and exactly matching whole routing batches. Local
/// cache keys bind dimensions, memberships, local claims, settings, canonical
/// pins and the requested total work allowance. View-root and revision changes
/// do not invalidate identical local inputs; cache hits are independently checked
/// against the new projection. Routing requests are rebuilt from the new frame
/// topology before comparing boxes, domains, clearance, terminals and original
/// memberships. The final complete composition is independently validated.
/// No winning dimension vector replaces its domain.
///
/// Previous geometry is trusted only as a candidate: same-root/same-lens repairs
/// retain legal frames/domains/transforms first. Navigation uses the placement
/// cache, not an implicit transform/pin migration. Accepted scenes are borrowed
/// and never modified. Cache publication is transactional with final validation
/// and cancellation; this function does not persist cache or accepted state.
pub fn solve_composed_cached(
    req: ComposedRequest<'_>,
    reuse: ComposedReuse<'_>,
    cache: &mut CanonicalPlacementCache,
) -> Result<ComposedSolution, LayoutError> {
    let (solution, staged) = solve_with_reuse(req, reuse, cache, false)?;
    if let Some(staged) = staged {
        *cache = staged;
    }
    Ok(solution)
}

/// Explicit bounded search, without publishing an unapproved arrangement into
/// the accepted-placement cache. Free frames use fresh candidate evaluation;
/// canonical pins and anchor-owned object positions remain hard constraints.
///
/// Every anchor link fixes its canonical domain and parent host envelope. This
/// first constrained search conservatively retains each complete hosting frame
/// while searching other frames, including pinned owner interiors that can fit
/// their locked domains. It does not claim to optimize every movable object in
/// those retained frames. Whole-composition validation and exact anchor checks
/// precede return, even when the resulting candidate provides no improvement.
/// The caller decides whether to offer/apply it under the unchanged quality gate.
pub fn solve_composed_reorganized(
    req: ComposedRequest<'_>,
    reuse: ComposedReuse<'_>,
    cache: &CanonicalPlacementCache,
) -> Result<ComposedSolution, LayoutError> {
    solve_with_reuse(req, reuse, cache, true).map(|(solution, _)| solution)
}

/// Publish an explicitly accepted composition's canonical placements without
/// solving, rerouting or changing the accepted result. Callers supply the same
/// typed context and search settings used for that arrangement, and validate
/// any presentation/anchor constraints before accepting it. Entries preserve
/// the full allowed-dimension domains and exact routing batches under the same
/// entry/byte limits. Routing batches cannot be reused by placement key alone.
///
/// Validation, bounded eviction and cancellation are transactional. This also
/// supports restoring a previously accepted arrangement through undo. The
/// caller must invalidate dependent whole-view caches separately.
pub fn cache_accepted_composed(
    req: ComposedRequest<'_>,
    context: &CompositionContext,
    solution: &ComposedSolution,
    cache: &mut CanonicalPlacementCache,
) -> Result<CanonicalCacheStats, LayoutError> {
    validate_cache_request(req, context, cache)?;
    if solution.work.budget != req.work_budget || solution.clearance != req.quality.clearance {
        return Err(invalid(
            "accepted composition differs from cache work/clearance settings",
        ));
    }
    validate_composed(req.projection, solution, req.pins, req.cancelled)?;
    let plan = crate::assembly_plan::plan_frames(req.projection, req.cancelled)?;
    super::validation::check_plan_capacity(&plan, req.cancelled)?;
    let mut keys = BTreeMap::new();
    for (id, frame) in &plan.frames {
        check_cancel(req.cancelled)?;
        let accepted = solution.assembly.frame(id).map_err(assembly_error)?;
        if revalidate(
            Placement {
                local: accepted.solution().clone(),
                domain: accepted.bounds(),
            },
            frame,
            req,
        )
        .is_none()
        {
            return Err(invalid(format!(
                "accepted frame {id} differs from cache geometry settings"
            )));
        }
        keys.insert(id.clone(), key_for(frame, req, context, &BTreeMap::new()));
    }
    let (staged, stats) = stage_placements(req, solution, keys, cache)?;
    check_cancel(req.cancelled)?;
    *cache = staged;
    Ok(stats)
}

fn validate_cache_request(
    req: ComposedRequest<'_>,
    context: &CompositionContext,
    cache: &CanonicalPlacementCache,
) -> Result<(), LayoutError> {
    check_cancel(req.cancelled)?;
    let quality = req.quality;
    if quality.clearance < 1
        || quality.clearance > 100
        || quality.gap < quality.clearance * 4
        || quality.gap > 1000
        || quality.routed_candidates == 0
        || quality.feedback_candidates > 16
        || quality.size_candidates > 28
        || !quality.movement_weight.is_finite()
        || quality.movement_weight < 0.0
        || !quality.shape_weight.is_finite()
        || quality.shape_weight < 0.0
    {
        return Err(invalid("invalid canonical cache quality profile"));
    }
    if context.model_id != req.projection.model_id || context.root != req.projection.root {
        return Err(invalid("typed composition context differs from model/root"));
    }
    if cache.latest.as_ref().is_some_and(|(model, revision)| {
        model == &req.projection.model_id && *revision > req.projection.revision
    }) {
        return Err(invalid("requested revision predates the canonical cache"));
    }
    Ok(())
}

fn solve_with_reuse(
    req: ComposedRequest<'_>,
    reuse: ComposedReuse<'_>,
    cache: &CanonicalPlacementCache,
    search: bool,
) -> Result<(ComposedSolution, Option<CanonicalPlacementCache>), LayoutError> {
    validate_cache_request(req, reuse.context, cache)?;
    if reuse.anchor_pins.len() > crate::assembly::MAX_TOTAL_BOXES {
        return Err(invalid("active anchor count exceeds budget"));
    }
    let previous = if let Some((context, previous)) = reuse.previous {
        if context.model_id != previous.model_id
            || context.root != previous.assembly.root()
            || previous.version != COMPOSITION_VERSION
            || previous.quality_version != QUALITY_VERSION
        {
            return Err(invalid(
                "previous composition context or version is invalid",
            ));
        }
        if previous.model_id == req.projection.model_id
            && previous.revision > req.projection.revision
        {
            return Err(invalid("previous composition belongs to a future revision"));
        }
        (context == reuse.context).then_some(previous)
    } else {
        None
    };
    if search && let Some(previous) = previous {
        if previous.revision != req.projection.revision
            || previous.projection_key != req.projection.key
        {
            return Err(invalid(
                "reorganization requires the accepted model revision and projection",
            ));
        }
        validate_composed(req.projection, previous, req.pins, req.cancelled)?;
    }
    let mut pins = req.pins.clone();
    let plan = crate::assembly_plan::plan_frames(req.projection, req.cancelled)?;
    super::validation::check_plan_capacity(&plan, req.cancelled)?;
    let mut locked_domains = BTreeSet::new();
    let mut locked_hosts = BTreeSet::new();
    let mut locks = BTreeMap::new();
    let mut host_bounds: BTreeMap<String, BTreeMap<String, Bounds>> = BTreeMap::new();
    for pin in reuse.anchor_pins {
        check_cancel(req.cancelled)?;
        let owner = plan
            .frames
            .get(pin.owner())
            .ok_or_else(|| invalid("active anchor owner is absent"))?;
        let object = owner
            .projection
            .nodes
            .iter()
            .find(|node| node.id == pin.object_id())
            .ok_or_else(|| invalid("active anchor object is absent from its owner"))?;
        if pin.model_id() != req.projection.model_id
            || !plan.frames.contains_key(pin.anchor())
            || object.semantic_ids.iter().cloned().collect::<BTreeSet<_>>() != *pin.semantic_ids()
        {
            return Err(invalid("active anchor model, frame or membership differs"));
        }
        let local = pins.entry(pin.owner().into()).or_default();
        if local
            .get(pin.object_id())
            .is_some_and(|point| *point != pin.local_point())
        {
            return Err(invalid("canonical and anchor pin coordinates conflict"));
        }
        local.insert(pin.object_id().into(), pin.local_point());
        for link in pin.links() {
            if plan
                .frames
                .get(&link.frame)
                .is_none_or(|frame| frame.parent_frame.as_ref() != Some(&link.parent))
            {
                return Err(invalid("active anchor frame ancestry differs"));
            }
            if locks.get(&link.frame).is_some_and(|old| old != link) {
                return Err(invalid("active anchors have conflicting frame locks"));
            }
            locks.insert(link.frame.clone(), link.clone());
            locked_domains.insert(link.frame.clone());
            locked_hosts.insert(link.parent.clone());
            host_bounds
                .entry(link.parent.clone())
                .or_default()
                .insert(link.frame.clone(), link.envelope);
        }
    }
    for (frame, bounds) in &host_bounds {
        let local = pins.entry(frame.clone()).or_default();
        for (id, bounds) in bounds {
            if local.get(id).is_some_and(|point| *point != bounds.center()) {
                return Err(invalid(
                    "canonical pin conflicts with an anchor host envelope",
                ));
            }
            local.insert(id.clone(), bounds.center());
        }
    }
    let req = ComposedRequest { pins: &pins, ..req };
    let mut keys = BTreeMap::new();
    let mut reused = BTreeMap::new();
    for (id, frame) in &plan.frames {
        check_cancel(req.cancelled)?;
        let key = key_for(frame, req, reuse.context, &locks);
        let accepted = previous
            .and_then(|old| old.assembly.frame(id).ok())
            .map(|frame| Placement {
                local: frame.solution().clone(),
                domain: frame.bounds(),
            })
            .filter(|placement| matches_locks(id, placement, &locks, &host_bounds));
        if search {
            if locked_hosts.contains(id)
                && let Some(placement) =
                    accepted.and_then(|placement| revalidate(placement, frame, req))
            {
                reused.insert(id.clone(), (placement, PlacementReuse::Previous));
            }
        } else if let Some(placement) =
            accepted.and_then(|placement| revalidate(placement, frame, req))
        {
            reused.insert(id.clone(), (placement, PlacementReuse::Previous));
        } else if let Some(placement) = cache
            .get(&key)
            .filter(|placement| matches_locks(id, placement, &locks, &host_bounds))
            .and_then(|placement| revalidate(placement, frame, req))
        {
            reused.insert(id.clone(), (placement, PlacementReuse::Cache));
        }
        keys.insert(id.clone(), key);
    }
    let prepared = Prepared {
        reused,
        previous,
        search,
        locked_domains,
        locks,
        host_bounds,
        routing_cache: cache,
    };
    let mut solution = super::solver::solve_prepared(req, Some(&prepared))?;
    for pin in reuse.anchor_pins {
        pin.validate(&solution.assembly).map_err(assembly_error)?;
    }
    if search {
        solution.work.cache = CanonicalCacheStats {
            previous_hits: solution.work.reused.len(),
            misses: solution
                .work
                .evaluations
                .values()
                .filter(|evaluations| **evaluations > 0)
                .count(),
            entries: cache.len(),
            estimated_bytes: cache.estimated_bytes(),
            routing_hits: solution.work.cache.routing_hits,
            routing_misses: solution.work.cache.routing_misses,
            ..Default::default()
        };
        check_cancel(req.cancelled)?;
        return Ok((solution, None));
    }
    let (staged, mut stats) = stage_placements(req, &solution, keys, cache)?;
    stats.routing_hits = solution.work.cache.routing_hits;
    stats.routing_misses = solution.work.cache.routing_misses;
    for id in solution.assembly.frame_ids() {
        match solution.work.reused.get(id) {
            Some(PlacementReuse::Cache) => stats.hits += 1,
            Some(PlacementReuse::Previous) => stats.previous_hits += 1,
            None => stats.misses += 1,
        }
    }
    solution.work.cache = stats;
    check_cancel(req.cancelled)?;
    Ok((solution, Some(staged)))
}

fn stage_placements(
    req: ComposedRequest<'_>,
    solution: &ComposedSolution,
    mut keys: BTreeMap<String, PlacementKey>,
    cache: &CanonicalPlacementCache,
) -> Result<(CanonicalPlacementCache, CanonicalCacheStats), LayoutError> {
    // Stage bounded cache mutation separately. Even an observed cancellation
    // during staging cannot change recency, evictions, or revision watermarks.
    let mut staged = cache.clone();
    let mut stats = CanonicalCacheStats::default();
    if staged
        .latest
        .as_ref()
        .is_some_and(|(model, _)| model != &req.projection.model_id)
    {
        stats.evictions += staged.len();
        staged.clear();
    }
    // The model watermark consumes the same retained-data allowance. A cache
    // too small even for that identity is disabled, with no hidden allocation.
    staged.latest = (req.projection.model_id.len() + 64 <= staged.limits.max_bytes)
        .then(|| (req.projection.model_id.clone(), req.projection.revision));
    for id in solution.assembly.frame_ids() {
        check_cancel(req.cancelled)?;
        let frame = solution.assembly.frame(id).map_err(assembly_error)?;
        let placement = Placement {
            local: frame.solution().clone(),
            domain: frame.bounds(),
        };
        let key = keys
            .remove(id)
            .ok_or_else(|| invalid("missing canonical placement key"))?;
        let fragments: Vec<_> = solution
            .routes
            .values()
            .flat_map(|route| &route.fragments)
            .filter(|fragment| fragment.frame_id == id)
            .collect();
        let routing = Arc::new(RoutingBatch {
            clearance: solution.clearance,
            requests: fragments
                .iter()
                .map(|fragment| fragment.request.clone())
                .collect(),
            fragments: fragments
                .iter()
                .map(|fragment| fragment.route.clone())
                .collect(),
        });
        let bytes = estimated_entry_bytes(&key, &placement)
            .saturating_add(estimated_routing_bytes(&routing));
        stats.evictions += staged.insert(Entry {
            key,
            placement,
            routing,
            bytes,
        });
    }
    stats.entries = staged.len();
    stats.estimated_bytes = staged.estimated_bytes();
    check_cancel(req.cancelled)?;
    Ok((staged, stats))
}

fn estimated_routing_bytes(routing: &RoutingBatch) -> usize {
    use crate::routing::FragmentTerminal;
    // Owned strings and vectors plus allocation/header slack. Arc copies share
    // immutable batches, but every retained entry is charged in full.
    let text = |value: &String| value.len().saturating_add(64);
    let terminal = |value: &FragmentTerminal| match value {
        FragmentTerminal::Box(id) => text(id).saturating_add(64),
        FragmentTerminal::Boundary { .. } => 128,
    };
    let mut bytes = 256usize;
    for request in &routing.requests {
        bytes = bytes.saturating_add(
            256 + text(&request.id) + terminal(&request.source) + terminal(&request.target),
        );
        for id in &request.semantic_ids {
            bytes = bytes.saturating_add(text(id));
        }
    }
    for fragment in &routing.fragments {
        bytes = bytes.saturating_add(
            256 + text(&fragment.id)
                + fragment
                    .points
                    .len()
                    .saturating_mul(2 * std::mem::size_of::<Point>()),
        );
        for id in &fragment.semantic_ids {
            bytes = bytes.saturating_add(text(id));
        }
    }
    bytes
}

fn key_for(
    frame: &PlannedFrame,
    req: ComposedRequest<'_>,
    context: &CompositionContext,
    locks: &BTreeMap<String, AnchorPinLink>,
) -> PlacementKey {
    let mut geometry = frame.cache_key.clone();
    // Hosting changes presentation, not canonical placement. Revision is safe
    // to omit only because all local geometry and original claims remain exact.
    geometry.parent_frame = None;
    geometry.revision = 0;
    PlacementKey {
        lens: context.lens,
        policy: context.policy,
        geometry,
        algorithm: req.algorithm,
        settings: [
            COMPOSITION_VERSION as u64,
            QUALITY_VERSION as u64,
            req.work_budget as u64,
            req.quality.clearance as u64,
            req.quality.gap as u64,
            req.quality.routed_candidates as u64,
            ((req.quality.feedback_candidates as u64) << 32) | req.quality.size_candidates as u64,
            req.quality.movement_weight.to_bits(),
            req.quality.shape_weight.to_bits(),
            2, // Boundary alignment and blocked-corridor recovery revision.
        ],
        pins: req
            .pins
            .get(&frame.projection.root)
            .cloned()
            .unwrap_or_default(),
        domain_lock: locks.get(&frame.projection.root).cloned(),
        host_locks: locks
            .values()
            .filter(|link| link.parent == frame.projection.root)
            .map(|link| (link.frame.clone(), link.envelope))
            .collect(),
    }
}
fn matches_locks(
    id: &str,
    placement: &Placement,
    locks: &BTreeMap<String, AnchorPinLink>,
    hosts: &BTreeMap<String, BTreeMap<String, Bounds>>,
) -> bool {
    locks
        .get(id)
        .is_none_or(|link| placement.domain == link.canonical_bounds)
        && hosts.get(id).is_none_or(|bounds| {
            bounds.iter().all(|(id, bounds)| {
                placement
                    .local
                    .boxes
                    .get(id)
                    .is_some_and(|object| object.bounds == *bounds)
            })
        })
}
fn revalidate(
    mut placement: Placement,
    frame: &PlannedFrame,
    req: ComposedRequest<'_>,
) -> Option<Placement> {
    placement.local.revision = frame.projection.revision;
    placement
        .local
        .projection_key
        .clone_from(&frame.projection.key);
    placement.local.routes.clear();
    let empty = BTreeMap::new();
    let pins = req.pins.get(&frame.projection.root).unwrap_or(&empty);
    crate::validation::validate_boxes(&frame.projection, &placement.local, pins).ok()?;
    for a in &frame.projection.nodes {
        for b in &frame.projection.nodes {
            if a.parent.is_some()
                && a.parent == b.parent
                && a.band < b.band
                && placement.local.boxes[&a.id].bounds.max.y + req.quality.gap
                    > placement.local.boxes[&b.id].bounds.min.y
            {
                return None;
            }
        }
    }
    Some(placement)
}
fn estimated_entry_bytes(key: &PlacementKey, placement: &Placement) -> usize {
    // Account for owned strings, collection nodes and geometry, with explicit
    // overhead per record. No eviction decision depends on allocator capacity.
    let text = |s: &String| s.len() + std::mem::size_of::<String>();
    let mut bytes = 1024 + text(&key.geometry.model_id) + text(&key.geometry.frame_id);
    for node in &key.geometry.nodes {
        bytes += 256 + text(&node.id) + node.parent.as_ref().map_or(0, text);
        bytes += node.semantic_ids.iter().map(text).sum::<usize>();
    }
    for edge in &key.geometry.edges {
        bytes += 128 + text(&edge.id) + text(&edge.source) + text(&edge.target);
        bytes += edge.semantic_ids.iter().map(text).sum::<usize>();
    }
    bytes += key.pins.keys().map(|id| text(id) + 96).sum::<usize>();
    bytes += key
        .host_locks
        .keys()
        .map(|id| text(id) + 128)
        .sum::<usize>();
    bytes += key
        .domain_lock
        .as_ref()
        .map_or(0, |link| 256 + text(&link.frame) + text(&link.parent));
    bytes += text(&placement.local.model_id) + text(&placement.local.projection_key);
    for (id, object) in &placement.local.boxes {
        bytes += 256 + text(id) + text(&object.id) + object.parent.as_ref().map_or(0, text);
        bytes += object.semantic_ids.iter().map(text).sum::<usize>();
    }
    bytes + placement.local.diagnostics.iter().map(text).sum::<usize>()
}
