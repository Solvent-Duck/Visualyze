//! Semantic source adapter for the experimental viewer. Bundle indexing and
//! projection run on its worker; the render thread only sees completed results.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
    io,
    ops::{ControlFlow, Deref},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bevy::prelude::Resource;
use visualyze_layout::{
    LayoutError, Projection,
    detail::{PlanBudget, plan_provider, plan_snapshot},
    projection::{self, ViewState},
    provider_projection::{Continuation, ProviderViewRequest},
};
use visualyze_model::{
    architecture::{Architecture, Basis, RelationKind},
    provider::{self, LocalBundle, ModelProvider, SemanticLens},
};

#[derive(Resource, Clone)]
pub(crate) struct LayoutInput(pub Arc<Source>, Arc<Mutex<ProjectionCache>>);

/// Per-call measurements, never cumulative source state shared by other jobs.
#[derive(Debug, Default)]
pub(crate) struct SourceTimings {
    pub indexing_ms: f64,
    pub snapshot_read_ms: f64,
    pub lock_wait_ms: f64,
    /// An exact projection memo was returned after checking the source.
    pub projection_cache_hit: bool,
}

pub(crate) enum Source {
    Snapshot(Box<Architecture>),
    SnapshotFile {
        path: PathBuf,
        model: Box<Mutex<Option<Architecture>>>,
    },
    Bundle {
        path: PathBuf,
        provider: Box<Mutex<Option<LocalBundle>>>,
        bytes_read: AtomicU64,
        total_bytes: AtomicU64,
        indexing: AtomicBool,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayLabel {
    pub label: String,
    pub kind_label: String,
    pub basis: Basis,
    pub original_kind: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayClaim {
    pub id: String,
    pub source: String,
    pub target: String,
    pub basis: Basis,
    /// Connector appearance only. Inspectors use kind_label and original endpoints.
    pub visual_kind: RelationKind,
    pub kind_label: String,
    pub label: Option<String>,
    pub evidence_ids: Vec<String>,
    pub aspect: Option<String>,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayPage {
    pub page_index: usize,
    pub page_size: usize,
    pub total_children: usize,
    pub has_next: bool,
    pub has_previous: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayRelationPage {
    pub next: Option<provider::PageCursor>,
    pub loaded_records: usize,
    pub internal_count: usize,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct DisplaySearchResult {
    pub id: String,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DesktopProjection {
    pub lens: SemanticLens,
    pub graph: Projection,
    pub group_catalog: Vec<projection::DisplayGroup>,
    pub labels: BTreeMap<String, DisplayLabel>,
    pub claims: BTreeMap<String, DisplayClaim>,
    pub detail_note: String,
    /// Boxes the projector opened on its own to show supplied structure while
    /// the view stays readable (ADR 0007). Never includes a box the user
    /// expanded or collapsed. Recomputed from the user's view on every
    /// projection; it is not saved.
    pub auto_opened: BTreeSet<String>,
    pub page: Option<DisplayPage>,
    pub relation_page: Option<DisplayRelationPage>,
    // Detail residency can change independently of supplied semantic records.
    // Recheck these bounded IDs on memo hits rather than cache a stale count.
    bundle_detail_ids: Vec<String>,
}

const PROJECTION_CACHE_ENTRIES: usize = 16;
const PROJECTION_CACHE_BYTES: usize = 32 * 1024 * 1024;
const PROJECTION_KEY_BYTES: usize = 512 * 1024;

#[derive(Default)]
struct ProjectionCache {
    entries: VecDeque<ProjectionEntry>,
    bytes: usize,
}
struct ProjectionEntry {
    key: Vec<u8>,
    projection: Arc<DesktopProjection>,
    bytes: usize,
}
impl ProjectionCache {
    fn get(&self, key: &[u8]) -> Option<Arc<DesktopProjection>> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| entry.projection.clone())
    }
    fn touch(&mut self, key: &[u8]) {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            let entry = self.entries.remove(index).unwrap();
            self.entries.push_back(entry);
        }
    }
    fn insert(
        &mut self,
        key: Vec<u8>,
        projection: &DesktopProjection,
        cancel: &AtomicBool,
    ) -> Result<(), LayoutError> {
        check_cancel(cancel)?;
        let Some(bytes) = projection_charge(&key, projection) else {
            return Ok(());
        };
        let projection = Arc::new(projection.clone());
        check_cancel(cancel)?;
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key) {
            self.bytes -= self.entries.remove(index).unwrap().bytes;
        }
        while self.entries.len() >= PROJECTION_CACHE_ENTRIES
            || self.bytes.saturating_add(bytes) > PROJECTION_CACHE_BYTES
        {
            let Some(old) = self.entries.pop_front() else {
                return Ok(());
            };
            self.bytes -= old.bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(ProjectionEntry {
            key,
            projection,
            bytes,
        });
        Ok(())
    }
}

/// Exact serialization, not a hash. Retained and local group order is included:
/// offscreen definitions and retention limits can affect the resulting view.
fn projection_request_key(view: &ViewState) -> Option<Vec<u8>> {
    #[derive(serde::Serialize)]
    struct Request<'a> {
        focus: &'a Option<String>,
        lens: &'a Option<SemanticLens>,
        child_page: usize,
        relation_cursor: &'a Option<provider::PageCursor>,
        policy: projection::LayoutPolicy,
        effective_expanded: std::collections::BTreeSet<String>,
        expanded: &'a std::collections::BTreeSet<String>,
        collapsed: &'a std::collections::BTreeSet<String>,
        pins: &'a BTreeMap<String, visualyze_layout::Point>,
        layers: &'a BTreeMap<String, i32>,
        groups: &'a [projection::DisplayGroup],
        retained: &'a [projection::DisplayGroup],
        dismissed: &'a std::collections::BTreeSet<String>,
        visible_budget: usize,
    }
    struct BoundedKey(Vec<u8>);
    impl io::Write for BoundedKey {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > PROJECTION_KEY_BYTES {
                return Err(io::Error::other("projection memo key exceeds its limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let request = Request {
        focus: &view.focus,
        lens: &view.lens,
        child_page: view.child_page,
        relation_cursor: &view.relation_cursor,
        policy: view.policy,
        effective_expanded: view.expanded_ids(),
        expanded: &view.expanded,
        collapsed: &view.collapsed,
        pins: &view.pins,
        layers: &view.layer_preferences,
        groups: &view.group_overrides,
        retained: view.retained_suggestions(),
        dismissed: &view.dismissed_groups,
        visible_budget: view.visible_budget,
    };
    let mut key = BoundedKey(Vec::new());
    serde_json::to_writer(&mut key, &request).ok()?;
    Some(key.0)
}

/// Conservative retained-memory charge, independent of allocation timing. Debug
/// visits every owned field without constructing a second giant String. Four
/// times that text plus per-container bookkeeping covers cloned strings, vector
/// slots and tree nodes. This is an estimate, not process RSS; oversize entries
/// bypass the cache, and transient caller results are not retained here.
fn projection_charge(key: &[u8], projection: &DesktopProjection) -> Option<usize> {
    struct Count(usize);
    impl std::fmt::Write for Count {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0 = self.0.saturating_add(text.len());
            if self.0 > PROJECTION_CACHE_BYTES / 4 {
                return Err(std::fmt::Error);
            }
            Ok(())
        }
    }
    let mut count = Count(0);
    write!(&mut count, "{projection:?}").ok()?;
    let containers = projection.nodes.len()
        + projection.edges.len()
        + projection.labels.len()
        + projection.claims.len()
        + projection.relation_metadata.len()
        + projection.semantic_to_display.len()
        + projection.references.len()
        + projection.suggested_groups.len()
        + projection.group_catalog.len()
        + projection.diagnostics.len()
        + projection.bundle_detail_ids.len();
    let string_slots = projection
        .nodes
        .iter()
        .map(|n| n.semantic_ids.len())
        .sum::<usize>()
        + projection
            .edges
            .iter()
            .map(|e| e.semantic_ids.len())
            .sum::<usize>()
        + projection
            .suggested_groups
            .iter()
            .map(|g| g.members.len())
            .sum::<usize>()
        + projection
            .claims
            .values()
            .map(|c| c.evidence_ids.len())
            .sum::<usize>()
        + projection
            .relation_metadata
            .values()
            .map(|r| r.evidence_ids.len())
            .sum::<usize>()
        + projection
            .group_catalog
            .iter()
            .map(|g| g.members.len())
            .sum::<usize>()
        + projection.internal_relation_ids.len();
    let bytes = key
        .len()
        .saturating_mul(2)
        .saturating_add(count.0.saturating_mul(4))
        .saturating_add(string_slots.saturating_mul(32))
        .saturating_add(containers.saturating_mul(256))
        .saturating_add(1024);
    (bytes <= PROJECTION_CACHE_BYTES).then_some(bytes)
}

impl Deref for DesktopProjection {
    type Target = Projection;
    fn deref(&self) -> &Self::Target {
        &self.graph
    }
}

impl LayoutInput {
    fn from_source(source: Source) -> Self {
        Self(
            Arc::new(source),
            Arc::new(Mutex::new(ProjectionCache::default())),
        )
    }
    pub fn snapshot(model: Architecture) -> Self {
        Self::from_source(Source::Snapshot(Box::new(model)))
    }

    pub fn snapshot_file(model: Architecture, path: PathBuf) -> Self {
        Self::from_source(Source::SnapshotFile {
            path,
            model: Box::new(Mutex::new(Some(model))),
        })
    }

    /// Cheap candidate creation only: no filesystem work and no mutation of the
    /// accepted source. The layout worker validates/loads the candidate on demand.
    pub fn reload_candidate(&self) -> Result<Self, LayoutError> {
        match self.0.as_ref() {
            Source::Snapshot(_) => Err(LayoutError::Invalid(
                "This embedded or stdin snapshot has no explicit file to reload.".into(),
            )),
            Source::SnapshotFile { path, .. } => Ok(Self::from_source(Source::SnapshotFile {
                path: path.clone(),
                model: Box::new(Mutex::new(None)),
            })),
            Source::Bundle { path, .. } => Ok(Self::bundle(path.clone())),
        }
    }

    pub fn bundle(path: PathBuf) -> Self {
        Self::from_source(Source::Bundle {
            path,
            provider: Box::new(Mutex::new(None)),
            bytes_read: AtomicU64::new(0),
            total_bytes: AtomicU64::new(0),
            indexing: AtomicBool::new(false),
        })
    }

    fn indexed_bundle(
        &self,
        cancel: &AtomicBool,
    ) -> Result<MutexGuard<'_, Option<LocalBundle>>, LayoutError> {
        self.indexed_bundle_timed(cancel, &mut SourceTimings::default())
    }

    fn indexed_bundle_timed(
        &self,
        cancel: &AtomicBool,
        timings: &mut SourceTimings,
    ) -> Result<MutexGuard<'_, Option<LocalBundle>>, LayoutError> {
        let Source::Bundle {
            path,
            provider,
            bytes_read,
            total_bytes,
            indexing,
        } = self.0.as_ref()
        else {
            return Err(LayoutError::Invalid(
                "Expected a local bundle source".into(),
            ));
        };
        let started = Instant::now();
        let locked = lock_source(provider, cancel);
        timings.lock_wait_ms += started.elapsed().as_secs_f64() * 1000.0;
        let mut cached = locked?;
        if cached.is_none() {
            let started = Instant::now();
            bytes_read.store(0, Ordering::Relaxed);
            total_bytes.store(0, Ordering::Relaxed);
            indexing.store(true, Ordering::Relaxed);
            let indexed =
                LocalBundle::open_with_progress(path, provider::Limits::default(), |progress| {
                    total_bytes.store(progress.total_bytes, Ordering::Relaxed);
                    bytes_read.store(progress.bytes_read, Ordering::Relaxed);
                    if cancel.load(Ordering::Relaxed) {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                });
            indexing.store(false, Ordering::Relaxed);
            timings.indexing_ms += started.elapsed().as_secs_f64() * 1000.0;
            let indexed = indexed.map_err(bundle_error)?;
            check_cancel(cancel)?;
            *cached = Some(indexed);
        }
        Ok(cached)
    }

    /// Load/index on the calling worker before projection timing begins. A warm
    /// shared source records zero loading time; waiting for its lock is separate.
    pub fn prepare(
        &self,
        cancel: &AtomicBool,
        timings: &mut SourceTimings,
    ) -> Result<(), LayoutError> {
        check_cancel(cancel)?;
        match self.0.as_ref() {
            Source::Snapshot(_) => Ok(()),
            Source::Bundle { .. } => {
                drop(self.indexed_bundle_timed(cancel, timings)?);
                Ok(())
            }
            Source::SnapshotFile { path, model } => {
                let started = Instant::now();
                let locked = lock_source(model, cancel);
                timings.lock_wait_ms += started.elapsed().as_secs_f64() * 1000.0;
                let mut guard = locked?;
                if guard.is_none() {
                    let started = Instant::now();
                    let loaded = read_snapshot(path, cancel);
                    timings.snapshot_read_ms += started.elapsed().as_secs_f64() * 1000.0;
                    *guard = Some(loaded?);
                }
                Ok(())
            }
        }
    }

    /// None is absent/off-lens; Some(None) is a supplied root. Index-only for bundles.
    pub(crate) fn supplied_owner(
        &self,
        id: &str,
        lens: SemanticLens,
        cancel: &AtomicBool,
    ) -> Result<Option<Option<String>>, LayoutError> {
        check_cancel(cancel)?;
        match self.0.as_ref() {
            Source::Snapshot(model) => Ok(snapshot_owner(model, id, lens)),
            Source::SnapshotFile { path, model } => {
                let mut guard = lock_source(model, cancel)?;
                if guard.is_none() {
                    *guard = Some(read_snapshot(path, cancel)?);
                }
                Ok(snapshot_owner(guard.as_ref().unwrap(), id, lens))
            }
            Source::Bundle { .. } => {
                let mut guard = self.indexed_bundle(cancel)?;
                let provider = guard.as_mut().unwrap();
                let revision = provider.manifest().revision;
                if !provider.in_lens(revision, lens, id).map_err(bundle_error)? {
                    return Ok(None);
                }
                provider
                    .owner_id_in(revision, lens, id)
                    .map(Some)
                    .map_err(bundle_error)
            }
        }
    }

    /// A disappearing suggestion is presentation state, never a missing semantic
    /// entity. Recover only this case; invalid supplied/manual constraints still fail.
    pub fn recover_reload_group(
        &self,
        previous: &Self,
        view: &mut ViewState,
        group: &projection::DisplayGroup,
        cancel: &AtomicBool,
    ) -> Result<Option<String>, LayoutError> {
        if !group.suggested || view.focus.as_deref() != Some(&group.id) {
            return Ok(None);
        }
        let lens = view
            .lens
            .or(view.frame_lens)
            .unwrap_or(SemanticLens::Logical);
        let mut surviving = 0;
        for member in &group.members {
            if self
                .supplied_owner(member, lens, cancel)?
                .flatten()
                .as_deref()
                == Some(&group.parent)
            {
                surviving += 1;
            }
        }
        if surviving >= 2 && self.supplied_owner(&group.parent, lens, cancel)?.is_some() {
            return Ok(None);
        }
        let mut candidates = vec![group.parent.clone()];
        // Old immutable source metadata recovers ancestors absent in the new
        // revision. Bound the walk and fall back to surviving breadcrumb IDs.
        for _ in 0..64 {
            check_cancel(cancel)?;
            match previous.supplied_owner(candidates.last().unwrap(), lens, cancel) {
                Ok(Some(Some(parent))) if !candidates.contains(&parent) => candidates.push(parent),
                Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
                _ => break,
            }
        }
        candidates.extend(view.breadcrumbs().iter().rev().take(64).flatten().cloned());
        let mut destination = None;
        for id in candidates {
            if self.supplied_owner(&id, lens, cancel)?.is_some() {
                destination = Some(id);
                break;
            }
        }
        let mut trail = Vec::new();
        for id in view
            .breadcrumbs()
            .iter()
            .skip(view.breadcrumbs().len().saturating_sub(64))
            .flatten()
        {
            if self.supplied_owner(id, lens, cancel)?.is_some() && !trail.contains(id) {
                trail.push(id.clone());
            }
        }
        view.save_frame_pins();
        view.reset_focus();
        for id in trail {
            let arrived = destination.as_ref() == Some(&id);
            view.enter(id);
            if arrived {
                break;
            }
        }
        if let Some(id) = &destination {
            view.enter(id.clone());
        } else {
            view.reset_focus();
        }
        view.restore_frame_pins();
        Ok(Some(format!(
            "The suggested group {} no longer has two members under its supplied parent in this revision. Returned to {}. Pins remain saved in their original coordinate frames.",
            group.id,
            destination.as_deref().unwrap_or("the supplied overview")
        )))
    }

    /// Runs only on a worker. The index search reads matching summaries, never all
    /// descendant detail records. Cancellation is checked throughout each scan.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<DisplaySearchResult>, LayoutError> {
        check_cancel(cancel)?;
        if query.trim().is_empty() || query.len() > 256 || limit == 0 || limit > 20 {
            return Err(LayoutError::Invalid(
                "Search needs 1–256 bytes and 1–20 results".into(),
            ));
        }
        match self.0.as_ref() {
            Source::Snapshot(model) => search_snapshot(model, query, limit, cancel),
            Source::SnapshotFile { path, model } => {
                let mut guard = lock_source(model, cancel)?;
                if guard.is_none() {
                    *guard = Some(read_snapshot(path, cancel)?);
                }
                search_snapshot(guard.as_ref().unwrap(), query, limit, cancel)
            }
            Source::Bundle { provider, .. } => {
                // Index on the existing worker path if search was requested before
                // any view. Its default projection is discarded, never rendered.
                if lock_source(provider, cancel)?.is_none() {
                    self.project(&ViewState::default(), cancel)?;
                }
                let mut guard = lock_source(provider, cancel)?;
                let provider = guard
                    .as_mut()
                    .ok_or_else(|| LayoutError::Invalid("Bundle index unavailable".into()))?;
                let revision = provider.manifest().revision;
                let mut results = Vec::new();
                let mut after = None;
                loop {
                    check_cancel(cancel)?;
                    let page = provider
                        .search(
                            revision,
                            query,
                            provider::PageRequest {
                                limit: limit - results.len(),
                                after,
                            },
                            cancel,
                        )
                        .map_err(bundle_error)?;
                    results.extend(page.items.into_iter().map(|entity| DisplaySearchResult {
                        id: entity.id,
                        label: entity.label,
                        kind: bundle_entity_name(entity.kind).into(),
                    }));
                    if results.len() == limit {
                        break;
                    }
                    match page.next {
                        Some(cursor) => after = Some(cursor),
                        None => break,
                    }
                }
                Ok(results)
            }
        }
    }

    pub fn progress(&self) -> Option<(u64, u64)> {
        match self.0.as_ref() {
            Source::Snapshot(_) | Source::SnapshotFile { .. } => None,
            Source::Bundle {
                bytes_read,
                total_bytes,
                indexing,
                ..
            } => {
                if indexing.load(Ordering::Relaxed) {
                    let total = total_bytes.load(Ordering::Relaxed);
                    Some((bytes_read.load(Ordering::Relaxed).min(total), total))
                } else {
                    None
                }
            }
        }
    }

    pub fn project(
        &self,
        view: &ViewState,
        cancel: &AtomicBool,
    ) -> Result<DesktopProjection, LayoutError> {
        self.project_timed(view, cancel, &mut SourceTimings::default())
    }

    pub fn project_timed(
        &self,
        view: &ViewState,
        cancel: &AtomicBool,
        timings: &mut SourceTimings,
    ) -> Result<DesktopProjection, LayoutError> {
        timings.projection_cache_hit = false;
        check_cancel(cancel)?;
        let key = projection_request_key(view);
        if let Some(key) = &key {
            let cached = lock_source(&self.1, cancel)?.get(key);
            if let Some(cached) = cached {
                let mut projected = cached.as_ref().clone();
                match self.0.as_ref() {
                    Source::Snapshot(_) => (),
                    Source::SnapshotFile { .. } => self.prepare(cancel, timings)?,
                    Source::Bundle { .. } => {
                        let mut guard = self.indexed_bundle_timed(cancel, timings)?;
                        let provider = guard.as_mut().unwrap();
                        // prepare() only initializes a warm provider. A cached
                        // projection must still enforce its revision/file stamp.
                        provider
                            .in_lens(
                                projected.revision,
                                projected.lens,
                                &provider.manifest().root_id,
                            )
                            .map_err(bundle_error)?;
                        let mut unloaded = 0;
                        let mut absent = 0;
                        for id in &projected.bundle_detail_ids {
                            check_cancel(cancel)?;
                            match provider
                                .detail_state(projected.revision, id)
                                .map_err(bundle_error)?
                            {
                                provider::DetailState::Unloaded => unloaded += 1,
                                provider::DetailState::NotSupplied => absent += 1,
                                provider::DetailState::Loaded => (),
                            }
                        }
                        projected.detail_note = bundle_detail_note(
                            projected
                                .relation_page
                                .as_ref()
                                .is_some_and(|page| page.complete),
                            unloaded,
                            absent,
                            &projected.graph.diagnostics,
                        );
                    }
                }
                check_cancel(cancel)?;
                lock_source(&self.1, cancel)?.touch(key);
                timings.projection_cache_hit = true;
                return Ok(projected);
            }
        }
        let mut projected = match self.0.as_ref() {
            Source::Snapshot(model) => project_snapshot(model, view)?,
            Source::SnapshotFile { path, model } => {
                let started = Instant::now();
                let locked = lock_source(model, cancel);
                timings.lock_wait_ms += started.elapsed().as_secs_f64() * 1000.0;
                let mut guard = locked?;
                if guard.is_none() {
                    let started = Instant::now();
                    let loaded = read_snapshot(path, cancel);
                    timings.snapshot_read_ms += started.elapsed().as_secs_f64() * 1000.0;
                    *guard = Some(loaded?);
                }
                project_snapshot(guard.as_ref().unwrap(), view)?
            }
            Source::Bundle { .. } => {
                let mut cached = self.indexed_bundle_timed(cancel, timings)?;
                project_bundle(cached.as_mut().unwrap(), view, cancel)?
            }
        };
        for (id, layer) in &view.layer_preferences {
            if !(-16..=16).contains(layer) {
                return Err(LayoutError::Invalid(
                    "layer preferences must be between -16 and 16".into(),
                ));
            }
            if let Some(node) = projected.graph.nodes.iter_mut().find(|node| &node.id == id) {
                node.band = *layer;
            }
        }
        // Canonical BTree order makes stale-result identity independent of input
        // insertion order, including constraints on temporarily hidden objects.
        projected.graph.key.push_str(&format!(
            "|layers={:?}|pins={:?}|expanded={:?}|collapsed={:?}",
            view.layer_preferences, view.pins, view.expanded, view.collapsed
        ));
        check_cancel(cancel)?;
        if let Some(key) = key {
            let mut cache = lock_source(&self.1, cancel)?;
            check_cancel(cancel)?;
            cache.insert(key, &projected, cancel)?;
        }
        Ok(projected)
    }
}

fn search_snapshot(
    model: &Architecture,
    query: &str,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<DisplaySearchResult>, LayoutError> {
    let query = query.to_lowercase();
    let mut ranked = Vec::new();
    for entity in &model.snapshot().entities {
        check_cancel(cancel)?;
        let id = entity.id.to_lowercase();
        let label = entity.label.to_lowercase();
        if id.contains(&query) || label.contains(&query) {
            let rank = if id == query || label == query {
                0
            } else if id.starts_with(&query) || label.starts_with(&query) {
                1
            } else {
                2
            };
            ranked.push((rank, entity));
        }
    }
    ranked.sort_by(|(a, x), (b, y)| a.cmp(b).then_with(|| x.id.cmp(&y.id)));
    let results = ranked
        .into_iter()
        .take(limit)
        .map(|(_, entity)| DisplaySearchResult {
            id: entity.id.clone(),
            label: entity.label.clone(),
            kind: entity.kind.as_str().into(),
        })
        .collect();
    Ok(results)
}

fn read_snapshot(path: &std::path::Path, cancel: &AtomicBool) -> Result<Architecture, LayoutError> {
    check_cancel(cancel)?;
    let result = crate::input::load(&crate::input::Source::File(path.to_path_buf()));
    check_cancel(cancel)?;
    result.map_err(|error| LayoutError::Invalid(error.to_string()))
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), LayoutError> {
    if cancel.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}

fn lock_source<'a, T>(
    provider: &'a Mutex<T>,
    cancel: &AtomicBool,
) -> Result<MutexGuard<'a, T>, LayoutError> {
    loop {
        check_cancel(cancel)?;
        match provider.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => {
                return Err(LayoutError::Invalid(
                    "local bundle worker failed while holding its index".into(),
                ));
            }
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(5)),
        }
    }
}

fn snapshot_owner(model: &Architecture, id: &str, lens: SemanticLens) -> Option<Option<String>> {
    let snapshot = model.snapshot();
    let entity = snapshot.entities.iter().find(|entity| entity.id == id)?;
    let deployment = entity.kind == visualyze_model::architecture::EntityKind::Deployment;
    if (lens == SemanticLens::Logical && deployment)
        || (lens == SemanticLens::Deployment && !deployment && id != snapshot.system_id)
    {
        return None;
    }
    let mut relations: Vec<_> = snapshot.relations.iter().collect();
    relations.sort_by(|a, b| a.id.cmp(&b.id));
    let owner = if lens == SemanticLens::Deployment {
        relations
            .iter()
            .find(|relation| relation.source == id && relation.kind == RelationKind::DeployedIn)
            .map(|relation| relation.target.clone())
    } else {
        relations
            .iter()
            .rev()
            .find(|relation| {
                relation.target == id
                    && matches!(
                        relation.kind,
                        RelationKind::Contains | RelationKind::Exposes
                    )
            })
            .map(|relation| relation.source.clone())
    };
    Some(owner.or_else(|| (id != snapshot.system_id).then(|| snapshot.system_id.clone())))
}

fn project_snapshot(
    model: &Architecture,
    view: &ViewState,
) -> Result<DesktopProjection, LayoutError> {
    let lens = view.lens.unwrap_or_else(|| {
        if view.focus.as_ref().is_some_and(|id| {
            model.snapshot().entities.iter().any(|entity| {
                &entity.id == id
                    && entity.kind == visualyze_model::architecture::EntityKind::Deployment
            })
        }) {
            SemanticLens::Deployment
        } else {
            SemanticLens::Logical
        }
    });
    let mut effective_view = view.clone();
    effective_view.lens = Some(lens);
    let planned = plan_snapshot(model, &effective_view, PlanBudget::SNAPSHOT)?;
    let (graph, auto_opened) = (planned.result, planned.opened);
    let supplied: BTreeMap<_, _> = model
        .snapshot()
        .entities
        .iter()
        .map(|entity| (entity.id.as_str(), entity))
        .collect();
    let mut labels = BTreeMap::new();
    for node in &graph.nodes {
        let reference = graph.references.get(&node.id);
        let semantic = reference.map(String::as_str).unwrap_or(&node.id);
        let label = if let Some(entity) = supplied.get(semantic) {
            DisplayLabel {
                label: if reference.is_some() {
                    format!("Reference: {}", entity.label)
                } else {
                    entity.label.clone()
                },
                kind_label: entity.kind.as_str().into(),
                basis: entity.basis,
                original_kind: entity.kind.as_str().into(),
            }
        } else {
            let group = graph
                .suggested_groups
                .iter()
                .find(|group| group.id == node.id);
            DisplayLabel {
                label: format!(
                    "{} group | {} members",
                    if group.is_some_and(|group| group.suggested) {
                        "Suggested"
                    } else {
                        "Visual"
                    },
                    group.map_or(node.semantic_ids.len(), |group| group.members.len())
                ),
                kind_label: "visual group".into(),
                basis: Basis::Inferred,
                original_kind: "visual_group".into(),
            }
        };
        labels.insert(node.id.clone(), label);
    }
    let claims = graph
        .relation_metadata
        .values()
        .map(|relation| {
            (
                relation.id.clone(),
                DisplayClaim {
                    id: relation.id.clone(),
                    source: relation.source.clone(),
                    target: relation.target.clone(),
                    basis: relation.basis,
                    visual_kind: relation.kind,
                    kind_label: architecture_relation_name(relation.kind).into(),
                    label: relation.label.clone(),
                    evidence_ids: relation.evidence_ids.clone(),
                    aspect: relation.aspect.clone(),
                    rationale: relation.rationale.clone(),
                },
            )
        })
        .collect();
    let detail_note = graph.diagnostics.join(" ");
    Ok(DesktopProjection {
        lens,
        group_catalog: graph.suggested_groups.clone(),
        graph,
        labels,
        claims,
        detail_note,
        auto_opened,
        page: None,
        relation_page: None,
        bundle_detail_ids: vec![],
    })
}

fn project_bundle(
    provider: &mut LocalBundle,
    view: &ViewState,
    cancel: &AtomicBool,
) -> Result<DesktopProjection, LayoutError> {
    let lens = resolve_bundle_lens(provider, view)?;
    // The planner opens supplied structure on its own copy of the view, so the
    // request is built from whichever view it is projecting.
    let request = |view: &ViewState| ProviderViewRequest {
        focus: view.focus.clone(),
        lens: Some(lens),
        child_page: view.child_page,
        boundary_after: view.relation_cursor.clone(),
        policy: view.policy,
        expanded: view.expanded_ids(),
        groups: view.group_overrides.clone(),
        retained_groups: view.retained_suggestions().to_vec(),
        dismissed_groups: view.dismissed_groups.clone(),
        visible_budget: view.visible_budget.clamp(1, 250),
        ..ProviderViewRequest::default()
    };
    let planned = plan_provider(provider, view, request, PlanBudget::BUNDLE, cancel)?;
    let (projected, auto_opened) = (planned.result, planned.opened);
    let mut labels = BTreeMap::new();
    for node in &projected.graph.nodes {
        check_cancel(cancel)?;
        if let Some(group) = projected
            .graph
            .suggested_groups
            .iter()
            .find(|group| group.id == node.id)
        {
            labels.insert(
                node.id.clone(),
                DisplayLabel {
                    label: group_label(group, &projected.entities),
                    kind_label: if group.suggested {
                        "visual group (suggested)"
                    } else {
                        "visual group (local)"
                    }
                    .into(),
                    basis: if group.suggested {
                        Basis::Inferred
                    } else {
                        Basis::Declared
                    },
                    original_kind: "visual_group".into(),
                },
            );
            continue;
        }
        let reference = projected.graph.references.get(&node.id);
        let semantic = reference.map(String::as_str).unwrap_or(&node.id);
        let summary = if let Some(entity) = projected.entities.get(semantic) {
            Some(entity.clone())
        } else {
            provider
                .summary(projected.graph.revision, semantic)
                .map_err(bundle_error)?
        };
        let label = if let Some(summary) = summary {
            let kind = bundle_entity_name(summary.kind);
            DisplayLabel {
                label: if reference.is_some() {
                    format!("Reference: {}", summary.label)
                } else {
                    summary.label
                },
                kind_label: kind.into(),
                basis: basis(summary.claim.basis),
                original_kind: kind.into(),
            }
        } else {
            DisplayLabel {
                label: format!("Reference: {semantic}"),
                kind_label: "reference".into(),
                basis: Basis::Unknown,
                original_kind: "unloaded_reference".into(),
            }
        };
        labels.insert(node.id.clone(), label);
    }
    let claims = projected
        .relations
        .values()
        .map(|relation| {
            (
                relation.id.clone(),
                DisplayClaim {
                    id: relation.id.clone(),
                    source: relation.source.clone(),
                    target: relation.target.clone(),
                    basis: basis(relation.claim.basis),
                    visual_kind: bundle_relation_appearance(relation.kind),
                    kind_label: bundle_relation_name(relation.kind).into(),
                    label: relation.label.clone(),
                    evidence_ids: relation.claim.evidence_ids.clone(),
                    aspect: relation.aspect.clone(),
                    rationale: relation.claim.rationale.clone(),
                },
            )
        })
        .collect();
    let unloaded = projected
        .detail_states
        .values()
        .filter(|state| **state == provider::DetailState::Unloaded)
        .count();
    let absent = projected
        .detail_states
        .values()
        .filter(|state| **state == provider::DetailState::NotSupplied)
        .count();
    let detail_note = bundle_detail_note(
        projected.complete,
        unloaded,
        absent,
        &projected.graph.diagnostics,
    );
    let bundle_detail_ids = projected.detail_states.keys().cloned().collect();
    let relation_page = Some(DisplayRelationPage {
        next: projected
            .continuations
            .iter()
            .find_map(|continuation| match continuation {
                Continuation::Boundary { after } => Some(after.clone()),
                _ => None,
            }),
        loaded_records: projected.relations.len(),
        internal_count: projected.internal_relation_counts.values().sum(),
        complete: projected.complete,
    });
    let page = Some(DisplayPage {
        page_index: view.child_page,
        page_size: projected.child_page_size,
        total_children: projected.total_children,
        has_next: (view.child_page + 1).saturating_mul(projected.child_page_size)
            < projected.total_children,
        has_previous: view.child_page > 0,
    });
    Ok(DesktopProjection {
        lens,
        group_catalog: projected.group_catalog,
        graph: projected.graph,
        labels,
        claims,
        detail_note,
        auto_opened,
        page,
        relation_page,
        bundle_detail_ids,
    })
}

fn bundle_detail_note(
    complete: bool,
    unloaded: usize,
    absent: usize,
    diagnostics: &[String],
) -> String {
    format!(
        "{} {} additional details available; {} summaries have no supplied detail. {}",
        if complete {
            "Current region loaded."
        } else {
            "Partial region; more records are available."
        },
        unloaded,
        absent,
        diagnostics.join(" ")
    )
}

fn resolve_bundle_lens(
    provider: &mut LocalBundle,
    view: &ViewState,
) -> Result<SemanticLens, LayoutError> {
    if let Some(lens) = view.lens {
        return Ok(lens);
    }
    let focus = view.focus.as_ref().map(|id| {
        view.group_overrides
            .iter()
            .chain(view.retained_suggestions())
            .find(|group| &group.id == id)
            .map(|group| group.parent.as_str())
            .unwrap_or(id)
    });
    if let Some(id) = focus
        && let Some(entity) = provider
            .summary(provider.manifest().revision, id)
            .map_err(bundle_error)?
    {
        use provider::EntityKind::*;
        match entity.kind {
            CodeRepository | CodePackage | CodeFile | CodeType | CodeFunction => {
                return Ok(SemanticLens::Code);
            }
            Deployment | DeploymentInstance => return Ok(SemanticLens::Deployment),
            Scope => {}
            _ => return Ok(SemanticLens::Logical),
        }
    }
    let profiles = &provider.manifest().profiles;
    Ok(
        if profiles.contains(&provider::Profile::Code)
            && profiles.iter().all(|profile| {
                matches!(
                    profile,
                    provider::Profile::Architecture | provider::Profile::Code
                )
            })
        {
            SemanticLens::Code
        } else {
            SemanticLens::Logical
        },
    )
}

fn group_label(
    group: &projection::DisplayGroup,
    entities: &BTreeMap<String, provider::EntitySummary>,
) -> String {
    let labels: Vec<_> = group
        .members
        .iter()
        .filter_map(|id| entities.get(id))
        .map(|entity| entity.label.as_str())
        .collect();
    let identity = if let Some(first) = labels.first() {
        let common = labels
            .iter()
            .skip(1)
            .fold(first.to_string(), |prefix, label| {
                prefix
                    .chars()
                    .zip(label.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a)
                    .collect()
            });
        let common = common.trim_end_matches(['_', ':', '/', '-', ' ']);
        if common.chars().count() >= 8 && common.chars().count() * 2 >= first.chars().count() {
            common.chars().take(36).collect::<String>()
        } else {
            first.chars().take(32).collect::<String>()
        }
    } else {
        "Members".into()
    };
    format!(
        "{}: {} | {} members",
        if group.suggested {
            "Suggested"
        } else {
            "Local group"
        },
        identity,
        group.members.len()
    )
}

fn bundle_error(error: provider::BundleError) -> LayoutError {
    match error {
        provider::BundleError::Cancelled => LayoutError::Cancelled,
        _ => LayoutError::Invalid(error.to_string()),
    }
}

fn basis(basis: provider::Basis) -> Basis {
    match basis {
        provider::Basis::Supported => Basis::Supported,
        provider::Basis::Inferred => Basis::Inferred,
        provider::Basis::Declared => Basis::Declared,
        provider::Basis::Unknown => Basis::Unknown,
    }
}

fn architecture_relation_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Contains => "contains",
        RelationKind::Exposes => "exposes",
        RelationKind::Requests => "requests",
        RelationKind::Reads => "reads",
        RelationKind::Writes => "writes",
        RelationKind::DependsOn => "depends on",
        RelationKind::AppliesTo => "applies to",
        RelationKind::ImplementedBy => "implemented by",
        RelationKind::DeployedIn => "deployed in",
    }
}

fn bundle_relation_appearance(kind: provider::RelationKind) -> RelationKind {
    use provider::RelationKind as R;
    match kind {
        R::Contains | R::OwnsTable | R::OwnsColumn | R::OwnsCode => RelationKind::Contains,
        R::Exposes => RelationKind::Exposes,
        R::Requests => RelationKind::Requests,
        R::Reads => RelationKind::Reads,
        R::Writes => RelationKind::Writes,
        R::AppliesTo => RelationKind::AppliesTo,
        R::ImplementedBy => RelationKind::ImplementedBy,
        R::DeployedIn => RelationKind::DeployedIn,
        R::DependsOn
        | R::TableReferences
        | R::CodeReferences
        | R::Publishes
        | R::Subscribes
        | R::Carries
        | R::InstanceOf => RelationKind::DependsOn,
    }
}

fn bundle_relation_name(kind: provider::RelationKind) -> &'static str {
    use provider::RelationKind as R;
    match kind {
        R::Contains => "contains",
        R::Exposes => "exposes",
        R::Requests => "requests",
        R::Reads => "reads",
        R::Writes => "writes",
        R::DependsOn => "depends on",
        R::AppliesTo => "applies to",
        R::ImplementedBy => "implemented by",
        R::DeployedIn => "deployed in",
        R::OwnsTable => "owns table",
        R::OwnsColumn => "owns column",
        R::OwnsCode => "owns code",
        R::TableReferences => "table references",
        R::CodeReferences => "code references",
        R::Publishes => "publishes",
        R::Subscribes => "subscribes",
        R::Carries => "carries",
        R::InstanceOf => "instance of",
    }
}

fn bundle_entity_name(kind: provider::EntityKind) -> &'static str {
    use provider::EntityKind as E;
    match kind {
        E::Scope => "scope",
        E::Client => "client",
        E::Service => "service",
        E::Datastore => "datastore",
        E::Interface => "interface",
        E::Concern => "concern",
        E::Deployment => "deployment",
        E::Table => "table",
        E::Column => "column",
        E::CodeRepository => "code repository",
        E::CodePackage => "code package",
        E::CodeFile => "code file",
        E::CodeType => "code type",
        E::CodeFunction => "code function",
        E::MessageChannel => "message channel",
        E::MessageType => "message type",
        E::DeploymentInstance => "deployment instance",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use provider::{
        Boundary, BundleRecord, BundleWriter, Claim, Coverage, EntityRecord, EntitySummary,
        FORMAT_VERSION, Limits, Manifest, Profile, RelationRecord,
    };

    fn memo_fixture() -> LayoutInput {
        LayoutInput::bundle(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../public/examples/data-messaging.bundle.jsonl"),
        )
    }

    fn memo_project(source: &LayoutInput, view: &ViewState, hit: bool) -> DesktopProjection {
        let mut timing = SourceTimings::default();
        let p = source
            .project_timed(view, &AtomicBool::new(false), &mut timing)
            .unwrap();
        assert_eq!(timing.projection_cache_hit, hit);
        p
    }

    #[test]
    fn exact_projection_memo_reuses_source_clones_without_aliasing_returned_data() {
        let source = memo_fixture();
        let view = ViewState::default();
        let first = memo_project(&source, &view, false);
        let clone = source.clone();
        let mut second = memo_project(&clone, &view, true);
        second.labels.clear();
        second.graph.edges.clear();
        let third = memo_project(&source, &view, true);
        assert_eq!(third.key, first.key);
        assert_eq!(third.edges, first.edges);
        assert_eq!(third.labels.len(), first.labels.len());
        assert_eq!(third.claims.len(), first.claims.len());
        assert_eq!(source.1.lock().unwrap().entries.len(), 1);
    }

    #[test]
    fn exact_projection_memo_keeps_snapshot_sources_independent() {
        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        let source = LayoutInput::snapshot(model);
        let separate = LayoutInput::snapshot(
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap(),
        );
        let mut view = ViewState::default();
        let first = memo_project(&source, &view, false);
        assert_eq!(memo_project(&source, &view, true).edges, first.edges);
        assert_eq!(memo_project(&separate, &view, false).edges, first.edges);
        view.layer_preferences.insert("client:web".into(), 4);
        let changed = memo_project(&source, &view, false);
        assert_ne!(changed.key, first.key);
        assert_eq!(memo_project(&source, &view, true).key, changed.key);
    }

    #[test]
    fn exact_projection_memo_invalidates_grouping_policy_pages_and_overrides() {
        let source = memo_fixture();
        let base = ViewState::default();
        let first = memo_project(&source, &base, false);
        let mut variations = vec![];
        let mut v = base.clone();
        v.policy = projection::LayoutPolicy::Peers;
        variations.push(v);
        let mut v = base.clone();
        v.child_page = 1;
        variations.push(v);
        let mut v = base.clone();
        v.layer_preferences.insert("client:web".into(), 3);
        variations.push(v);
        let mut v = base.clone();
        v.pins
            .insert("client:web".into(), visualyze_layout::Point::ZERO);
        variations.push(v);
        let mut v = base.clone();
        v.set_expanded("datastore:orders", true);
        variations.push(v);
        let mut v = base.clone();
        v.set_expanded("datastore:orders", false);
        variations.push(v);
        let group = projection::DisplayGroup {
            id: "view:local-group:memo".into(),
            parent: first.root.clone(),
            members: vec!["client:web".into(), "service:orders".into()],
            suggested: false,
        };
        let mut v = base.clone();
        v.group_overrides.push(group.clone());
        variations.push(v);
        let mut v = base.clone();
        v.dismissed_groups
            .insert("view:suggested-group:memo".into());
        variations.push(v);
        let mut v = base.clone();
        v.visible_budget = 200;
        variations.push(v);
        let mut v = base.clone();
        v.lens = Some(SemanticLens::Code);
        variations.push(v);
        let mut v = base.clone();
        v.focus = Some("datastore:orders".into());
        variations.push(v);
        let mut v = base.clone();
        let mut graph = first.graph.clone();
        graph.suggested_groups = vec![projection::DisplayGroup {
            id: "view:suggested-group:memo".into(),
            suggested: true,
            ..group
        }];
        v.remember_groups(&graph);
        variations.push(v);
        for view in variations {
            memo_project(&source, &view, false);
            memo_project(&source, &view, true);
        }
        // Navigation bookkeeping does not enter the projection itself.
        let mut same = base.clone();
        same.enter("service:orders");
        assert!(same.back());
        same.semantic_zoom = false;
        same.frame_lens = Some(SemanticLens::Logical);
        same.frame_pins.insert("inactive".into(), BTreeMap::new());
        same.relation_history.push(None);
        assert_eq!(projection_request_key(&same), projection_request_key(&base));
        memo_project(&source, &same, true);
        // The legacy peer ceiling and group target are ignored by projection
        // (ADR 0007), so they must not split the memo either.
        let mut legacy = base.clone();
        legacy.group_target = 3;
        legacy.peer_ceiling = 3;
        assert_eq!(
            projection_request_key(&legacy),
            projection_request_key(&base)
        );
        memo_project(&source, &legacy, true);
    }

    #[test]
    fn exact_projection_memo_preserves_cursor_identity_and_detail_residency() {
        let source = memo_fixture();
        let mut view = ViewState::default();
        let first = memo_project(&source, &view, false);
        // A syntactically valid foreign cursor is not an alias for page one.
        view.relation_cursor = Some(
            serde_json::from_value(serde_json::json!({
                "model_id":first.model_id,"revision":first.revision,
                "query":"wrong-query","offset":1
            }))
            .unwrap(),
        );
        let mut timing = SourceTimings::default();
        assert!(
            source
                .project_timed(&view, &AtomicBool::new(false), &mut timing)
                .is_err()
        );
        assert!(!timing.projection_cache_hit);
        let Source::Bundle { provider, .. } = source.0.as_ref() else {
            unreachable!()
        };
        provider
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .clear_detail_cache();
        let hit = memo_project(&source, &ViewState::default(), true);
        assert_eq!(hit.edges, first.edges);
        let guard = provider.lock().unwrap();
        let provider = guard.as_ref().unwrap();
        let mut unloaded = 0;
        let mut absent = 0;
        for id in &hit.bundle_detail_ids {
            match provider.detail_state(hit.revision, id).unwrap() {
                provider::DetailState::Unloaded => unloaded += 1,
                provider::DetailState::NotSupplied => absent += 1,
                provider::DetailState::Loaded => (),
            }
        }
        assert_eq!(
            hit.detail_note,
            bundle_detail_note(
                hit.relation_page.as_ref().unwrap().complete,
                unloaded,
                absent,
                &hit.diagnostics
            )
        );
    }

    #[test]
    fn exact_projection_memo_rejects_source_mutation_and_reload_has_independent_memo() {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "visualyze-projection-memo-{}.jsonl",
            std::process::id()
        ));
        std::fs::copy(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../public/examples/data-messaging.bundle.jsonl"),
            &path,
        )
        .unwrap();
        let source = LayoutInput::bundle(path.clone());
        let view = ViewState::default();
        memo_project(&source, &view, false);
        memo_project(&source, &view, true);
        let replacement = source.reload_candidate().unwrap();
        assert!(!Arc::ptr_eq(&source.0, &replacement.0));
        assert!(!Arc::ptr_eq(&source.1, &replacement.1));
        memo_project(&replacement, &view, false);
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"\n")
            .unwrap();
        let mut timing = SourceTimings::default();
        assert!(
            source
                .project_timed(&view, &AtomicBool::new(false), &mut timing)
                .is_err()
        );
        assert!(!timing.projection_cache_hit);
        assert!(
            replacement
                .project_timed(&view, &AtomicBool::new(false), &mut timing)
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn exact_projection_memo_cancellation_does_not_publish_or_promote_entries() {
        let source = memo_fixture();
        let view = ViewState::default();
        let mut timing = SourceTimings::default();
        assert!(matches!(
            source.project_timed(&view, &AtomicBool::new(true), &mut timing),
            Err(LayoutError::Cancelled)
        ));
        assert!(source.1.lock().unwrap().entries.is_empty());
        let p = memo_project(&source, &view, false);
        let before = source.1.lock().unwrap().bytes;
        assert!(matches!(
            source.project_timed(&view, &AtomicBool::new(true), &mut timing),
            Err(LayoutError::Cancelled)
        ));
        assert!(!timing.projection_cache_hit);
        assert!(
            source
                .1
                .lock()
                .unwrap()
                .insert(vec![99], &p, &AtomicBool::new(true))
                .is_err()
        );
        assert_eq!(source.1.lock().unwrap().bytes, before);
        memo_project(&source, &view, true);
    }

    #[test]
    fn exact_projection_memo_bounds_entries_bytes_and_oversize_keys() {
        let source = memo_fixture();
        let p = memo_project(&source, &ViewState::default(), false);
        let mut cache = ProjectionCache::default();
        let cancel = AtomicBool::new(false);
        for i in 0..PROJECTION_CACHE_ENTRIES + 3 {
            cache.insert(vec![i as u8], &p, &cancel).unwrap();
        }
        assert_eq!(cache.entries.len(), PROJECTION_CACHE_ENTRIES);
        assert!(cache.get(&[0]).is_none());
        let mut large = p.clone();
        large.detail_note = "x".repeat(512 * 1024);
        for i in 40..64 {
            cache.insert(vec![i], &large, &cancel).unwrap();
        }
        assert!(cache.entries.len() < PROJECTION_CACHE_ENTRIES);
        assert!(cache.bytes <= PROJECTION_CACHE_BYTES);
        assert_eq!(
            cache.bytes,
            cache.entries.iter().map(|e| e.bytes).sum::<usize>()
        );
        let before = (cache.entries.len(), cache.bytes);
        large.detail_note = "x".repeat(PROJECTION_CACHE_BYTES / 4);
        cache.insert(vec![255], &large, &cancel).unwrap();
        assert_eq!((cache.entries.len(), cache.bytes), before);
        let mut oversized = ViewState::default();
        oversized
            .dismissed_groups
            .insert("x".repeat(PROJECTION_KEY_BYTES));
        assert!(projection_request_key(&oversized).is_none());
    }

    #[test]
    fn native_bundle_preserves_async_claims_and_reuses_its_index() {
        let path = std::env::temp_dir().join(format!(
            "visualyze-layout-input-{}.jsonl",
            std::process::id()
        ));
        let mut writer = BundleWriter::create(
            &path,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "async-test".into(),
                revision: 1,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture, Profile::Messaging],
                coverage: Coverage::Complete,
                scope_description: "Declared native adapter test".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        writer
            .push(&BundleRecord::Evidence(provider::EvidenceRecord {
                id: "evidence:events".into(),
                description: "Declared adapter test evidence".into(),
                reference: None,
            }))
            .unwrap();
        for (id, kind) in [
            ("queue", provider::EntityKind::MessageChannel),
            ("root", provider::EntityKind::Scope),
            ("service", provider::EntityKind::Service),
        ] {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        id: id.into(),
                        label: id.into(),
                        kind,
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: None,
                }))
                .unwrap();
        }
        for (id, kind, source, target) in [
            ("r1", provider::RelationKind::Contains, "root", "queue"),
            ("r2", provider::RelationKind::Contains, "root", "service"),
            ("r3", provider::RelationKind::Publishes, "service", "queue"),
        ] {
            writer
                .push(&BundleRecord::Relation(RelationRecord {
                    id: id.into(),
                    kind,
                    source: source.into(),
                    target: target.into(),
                    claim: if kind == provider::RelationKind::Publishes {
                        Claim {
                            basis: provider::Basis::Supported,
                            evidence_ids: vec!["evidence:events".into()],
                            rationale: Some("Explicit fixture event channel".into()),
                        }
                    } else {
                        Claim::declared()
                    },
                    label: Some("Events".into()),
                    aspect: None,
                }))
                .unwrap();
        }
        drop(writer.finish().unwrap());
        let source = LayoutInput::bundle(path.clone());
        let mut view = ViewState::default();
        let projection = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(projection.claims["r3"].kind_label, "publishes");
        assert_eq!(projection.claims["r3"].label.as_deref(), Some("Events"));
        assert_eq!(projection.claims["r3"].evidence_ids, ["evidence:events"]);
        assert!(projection.claims["r3"].aspect.is_none());
        assert_eq!(
            projection.claims["r3"].rationale.as_deref(),
            Some("Explicit fixture event channel")
        );
        assert_eq!(projection.claims["r3"].basis, Basis::Supported);
        assert_eq!(projection.labels["queue"].original_kind, "message channel");
        assert!(source.progress().is_none());
        if let Source::Bundle { provider, .. } = source.0.as_ref() {
            assert!(provider.lock().unwrap().is_some());
        }
        view.set_expanded("service", true);
        let expanded = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_ne!(projection.key, expanded.key);
        assert!(
            !expanded
                .detail_note
                .contains("Expansion in place is not available")
        );
        view.enter("service");
        let focused = source.project(&view, &AtomicBool::new(false)).unwrap();
        let queue_reference = focused
            .references
            .iter()
            .find(|(_, id)| *id == "queue")
            .unwrap()
            .0;
        assert_eq!(focused.labels[queue_reference].label, "Reference: queue");
        assert_eq!(
            focused.labels[queue_reference].original_kind,
            "message channel"
        );
        drop(source);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn snapshot_adapter_retains_original_claims_beneath_collapsed_ports() {
        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        let source = LayoutInput::snapshot(model);
        assert!(source.progress().is_none());
        let p = source
            .project(&ViewState::default(), &AtomicBool::new(false))
            .unwrap();
        assert_eq!(p.labels["service:notes"].label, "Notes service");
        assert_eq!(p.claims["rel:request-notes"].target, "interface:notes");
        assert_eq!(
            p.claims["rel:request-notes"].visual_kind,
            RelationKind::Requests
        );
        assert_eq!(
            p.claims["rel:auth-session"].aspect.as_deref(),
            Some("session_validation")
        );
        assert!(
            p.edges
                .iter()
                .any(|edge| edge.id == "rel:request-notes" && edge.target == "service:notes")
        );
    }

    #[test]
    fn bundle_search_before_display_prioritizes_exact_labels_without_loading_all_details() {
        let source = LayoutInput::bundle(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../public/examples/data-messaging.bundle.jsonl"),
        );
        let results = source.search("Orders", 1, &AtomicBool::new(false)).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "table:orders");
        assert_eq!(results[0].kind, "table");
        assert!(matches!(
            source.search("Orders", 1, &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
        assert!(
            source
                .search("Orders", 21, &AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn supplied_bundle_navigation_keeps_table_and_column_identity() {
        let source = LayoutInput::bundle(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../public/examples/data-messaging.bundle.jsonl"),
        );
        let mut view = ViewState::default();
        let overview = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert!(
            overview
                .claims
                .values()
                .any(|claim| claim.kind_label == "publishes")
        );
        view.set_expanded("datastore:orders", true);
        let mixed = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(mixed.labels["table:orders"].original_kind, "table");
        assert_eq!(
            mixed
                .nodes
                .iter()
                .find(|node| node.id == "table:orders")
                .unwrap()
                .parent
                .as_deref(),
            Some("datastore:orders")
        );
        assert!(
            mixed
                .nodes
                .iter()
                .find(|node| node.id == "service:orders")
                .unwrap()
                .solid
        );
        assert_eq!(mixed.lens, SemanticLens::Logical);
        assert!(!mixed.nodes.iter().any(|node| node.id == "code:repository"));
        assert!(
            !mixed
                .nodes
                .iter()
                .find(|node| node.id == "datastore:orders")
                .unwrap()
                .solid
        );
        view.enter("datastore:orders");
        let tables = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(tables.labels["table:orders"].label, "Orders");
        assert_eq!(tables.labels["table:orders"].original_kind, "table");
        assert!(
            tables
                .claims
                .values()
                .any(|claim| claim.kind_label == "table references")
        );
        view.enter("table:orders");
        let columns = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(columns.labels["column:customer-id"].original_kind, "column");
        assert!(view.back());
        let returned = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_eq!(returned.key, tables.key);
        assert_eq!(returned.labels["table:orders"].original_kind, "table");
    }

    #[test]
    fn bundle_input_does_not_open_or_index_until_the_worker_requests_a_view() {
        let source = LayoutInput::bundle(PathBuf::from("/not/a/supplied/bundle.jsonl"));
        assert!(source.progress().is_none());
        assert!(matches!(
            source.project(&ViewState::default(), &AtomicBool::new(true)),
            Err(LayoutError::Cancelled)
        ));
        assert!(
            source
                .project(&ViewState::default(), &AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn async_and_table_connector_labels_never_become_requests() {
        for (kind, label) in [
            (provider::RelationKind::Publishes, "publishes"),
            (provider::RelationKind::Subscribes, "subscribes"),
            (provider::RelationKind::TableReferences, "table references"),
        ] {
            assert_ne!(bundle_relation_appearance(kind), RelationKind::Requests);
            assert_eq!(bundle_relation_name(kind), label);
        }
    }

    #[test]
    fn local_constraints_change_the_snapshot_key_without_changing_supplied_claims() {
        let source = LayoutInput::snapshot(
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap(),
        );
        let mut view = ViewState::default();
        let before = source.project(&view, &AtomicBool::new(false)).unwrap();
        view.layer_preferences.insert("client:web".into(), 3);
        let after = source.project(&view, &AtomicBool::new(false)).unwrap();
        assert_ne!(before.key, after.key);
        assert_eq!(before.claims.len(), after.claims.len());
        assert_eq!(
            after
                .nodes
                .iter()
                .find(|node| node.id == "client:web")
                .unwrap()
                .band,
            3
        );
    }
}
