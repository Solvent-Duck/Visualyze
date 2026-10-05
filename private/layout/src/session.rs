//! Transactional acceptance of complete layouts and cancellable background work.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, TryRecvError},
    },
    thread,
};

use crate::{LayoutError, LayoutSolution};

#[derive(Debug, Clone, PartialEq)]
pub struct ReorganizationStats {
    pub improvement_fraction: f64,
    pub route_length_before: i64,
    pub route_length_after: i64,
    pub bends_before: usize,
    pub bends_after: usize,
    pub volume_before: f64,
    pub volume_after: f64,
    pub objects_moved: usize,
}

#[derive(Debug, Clone)]
pub struct Reorganization {
    pub id: String,
    pub stats: ReorganizationStats,
    pub candidate: LayoutSolution,
}

/// A candidate becomes visible only through these validating commit operations.
pub struct LayoutSession {
    model_id: String,
    revision: u64,
    projection_key: String,
    version: u32,
    accepted: Option<LayoutSolution>,
    proposal: Option<Reorganization>,
    dismissed: BTreeSet<String>,
    undo: VecDeque<LayoutSolution>,
    pub improvement_threshold: f64,
}

impl LayoutSession {
    pub fn new(model_id: String, revision: u64, projection_key: String, version: u32) -> Self {
        Self {
            model_id,
            revision,
            projection_key,
            version,
            accepted: None,
            proposal: None,
            dismissed: BTreeSet::new(),
            undo: VecDeque::new(),
            improvement_threshold: 0.15,
        }
    }

    pub fn accepted(&self) -> Option<&LayoutSolution> {
        self.accepted.as_ref()
    }

    pub fn proposal(&self) -> Option<&Reorganization> {
        self.proposal.as_ref()
    }

    /// Preserve the last scene until a valid replacement is ready. Clear actions
    /// which could otherwise apply a layout computed for obsolete constraints.
    pub fn set_context(
        &mut self,
        model_id: String,
        revision: u64,
        projection_key: String,
        version: u32,
    ) {
        if self.model_id == model_id
            && self.revision == revision
            && self.projection_key == projection_key
            && self.version == version
        {
            return;
        }
        self.model_id = model_id;
        self.revision = revision;
        self.projection_key = projection_key;
        self.version = version;
        self.proposal = None;
        self.dismissed.clear();
        self.undo.clear();
    }

    pub fn accept_initial(
        &mut self,
        candidate: LayoutSolution,
        validate: impl FnOnce(&LayoutSolution) -> Result<(), LayoutError>,
    ) -> Result<(), LayoutError> {
        if self
            .accepted
            .as_ref()
            .is_some_and(|accepted| self.matches(accepted))
        {
            return Err(LayoutError::Invalid(
                "a layout is already accepted; use repair or a reorganization proposal".into(),
            ));
        }
        self.check(&candidate)?;
        validate(&candidate)?;
        self.accepted = Some(candidate);
        self.proposal = None;
        Ok(())
    }

    /// Caller supplies the validator for the current model, pins, and projection.
    pub fn accept_repair(
        &mut self,
        candidate: LayoutSolution,
        validate: impl FnOnce(&LayoutSolution) -> Result<(), LayoutError>,
    ) -> Result<(), LayoutError> {
        self.check(&candidate)?;
        validate(&candidate)?;
        self.replace(candidate);
        Ok(())
    }

    pub fn offer(
        &mut self,
        candidate: LayoutSolution,
        validate: impl FnOnce(&LayoutSolution) -> Result<(), LayoutError>,
    ) -> Result<Option<&Reorganization>, LayoutError> {
        self.check(&candidate)?;
        validate(&candidate)?;
        let accepted = self.accepted.as_ref().ok_or_else(|| {
            LayoutError::Invalid("accept a layout before comparing reorganizations".into())
        })?;
        self.check(accepted)?;
        if !self.improvement_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.improvement_threshold)
        {
            return Err(LayoutError::Invalid(
                "improvement threshold must be between zero and one".into(),
            ));
        }
        if accepted.metrics.structural_cost <= 0.0 {
            return Ok(None);
        }
        let improvement = (accepted.metrics.structural_cost - candidate.metrics.structural_cost)
            / accepted.metrics.structural_cost;
        // Readability guards are independent of the weighted structural score.
        // Original membership is also checked: a shorter route obtained by
        // hiding a relationship can never become an offered improvement.
        // Congestion tolerance is a provisional metric policy: 10% relative
        // plus 0.1 absolute, not a human-calibrated comprehension threshold.
        if improvement + f64::EPSILON < self.improvement_threshold
            || candidate.metrics.occlusion > accepted.metrics.occlusion + 0.05
            || candidate.metrics.congestion > accepted.metrics.congestion * 1.1 + 0.1
            || candidate.metrics.aspect_ratio > accepted.metrics.aspect_ratio.max(3.0) * 1.1
            || membership(&candidate) != membership(accepted)
        {
            return Ok(None);
        }
        let id = fingerprint(&candidate);
        if self.dismissed.contains(&id) {
            return Ok(None);
        }
        let stats = ReorganizationStats {
            improvement_fraction: improvement,
            route_length_before: accepted.metrics.route_length,
            route_length_after: candidate.metrics.route_length,
            bends_before: accepted.metrics.bends,
            bends_after: candidate.metrics.bends,
            volume_before: accepted.metrics.volume,
            volume_after: candidate.metrics.volume,
            objects_moved: candidate
                .boxes
                .iter()
                .filter(|(id, object)| {
                    accepted
                        .boxes
                        .get(*id)
                        .is_none_or(|previous| previous.bounds.center() != object.bounds.center())
                })
                .count(),
        };
        if self.proposal.as_ref().is_some_and(|current| {
            current.candidate.metrics.structural_cost <= candidate.metrics.structural_cost
        }) {
            return Ok(self.proposal.as_ref());
        }
        self.proposal = Some(Reorganization {
            id,
            stats,
            candidate,
        });
        Ok(self.proposal.as_ref())
    }

    /// Explicitly apply the previewed ID, revalidating after any intervening edit.
    pub fn apply(
        &mut self,
        proposal_id: &str,
        validate: impl FnOnce(&LayoutSolution) -> Result<(), LayoutError>,
    ) -> Result<(), LayoutError> {
        let proposal = self
            .proposal
            .as_ref()
            .filter(|proposal| proposal.id == proposal_id)
            .ok_or_else(|| LayoutError::Invalid("reorganization is absent or stale".into()))?;
        self.check(&proposal.candidate)?;
        validate(&proposal.candidate)?;
        let candidate = self.proposal.take().unwrap().candidate;
        self.replace(candidate);
        Ok(())
    }

    pub fn dismiss(&mut self, proposal_id: &str) -> bool {
        if self
            .proposal
            .as_ref()
            .is_some_and(|proposal| proposal.id == proposal_id)
        {
            self.dismissed.insert(proposal_id.to_owned());
            self.proposal = None;
            true
        } else {
            false
        }
    }

    pub fn undo(
        &mut self,
        validate: impl FnOnce(&LayoutSolution) -> Result<(), LayoutError>,
    ) -> Result<bool, LayoutError> {
        let Some(previous) = self.undo.back() else {
            return Ok(false);
        };
        self.check(previous)?;
        validate(previous)?;
        self.accepted = self.undo.pop_back();
        self.proposal = None;
        Ok(true)
    }

    fn matches(&self, candidate: &LayoutSolution) -> bool {
        candidate.model_id == self.model_id
            && candidate.revision == self.revision
            && candidate.projection_key == self.projection_key
            && candidate.version == self.version
    }

    fn check(&self, candidate: &LayoutSolution) -> Result<(), LayoutError> {
        if !self.matches(candidate) {
            return Err(LayoutError::Invalid(
                "candidate belongs to a stale model, projection, or layout version".into(),
            ));
        }
        let metrics = &candidate.metrics;
        if metrics.route_length < 0
            || metrics.shared_lane_length < 0
            || metrics.displacement < 0
            || [
                metrics.structural_cost,
                metrics.volume,
                metrics.aspect_ratio,
                metrics.occlusion,
                metrics.congestion,
            ]
            .iter()
            .any(|number| !number.is_finite() || *number < 0.0)
        {
            return Err(LayoutError::Invalid(
                "candidate has invalid quality measurements".into(),
            ));
        }
        Ok(())
    }

    fn replace(&mut self, candidate: LayoutSolution) {
        if let Some(previous) = self.accepted.take()
            && self.matches(&previous)
        {
            if self.undo.len() == 32 {
                self.undo.pop_front();
            }
            self.undo.push_back(previous);
        }
        self.accepted = Some(candidate);
        self.proposal = None;
    }
}

fn membership(
    solution: &LayoutSolution,
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let objects = solution
        .boxes
        .iter()
        .map(|(id, object)| {
            let mut members = object.semantic_ids.clone();
            members.sort();
            (id.clone(), members)
        })
        .collect();
    let routes = solution
        .routes
        .iter()
        .map(|route| {
            let mut members = route.semantic_ids.clone();
            members.sort();
            (route.edge_id.clone(), members)
        })
        .collect();
    (objects, routes)
}

fn fingerprint(solution: &LayoutSolution) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut write = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    write(solution.model_id.as_bytes());
    write(&solution.revision.to_le_bytes());
    write(solution.projection_key.as_bytes());
    write(&solution.version.to_le_bytes());
    for (id, object) in &solution.boxes {
        write(id.as_bytes());
        for coordinate in [
            object.bounds.min.x,
            object.bounds.min.y,
            object.bounds.min.z,
            object.bounds.max.x,
            object.bounds.max.y,
            object.bounds.max.z,
        ] {
            write(&coordinate.to_le_bytes());
        }
    }
    let mut routes: Vec<_> = solution.routes.iter().collect();
    routes.sort_by(|a, b| a.edge_id.cmp(&b.edge_id));
    for route in routes {
        write(route.edge_id.as_bytes());
        for point in &route.points {
            for coordinate in [point.x, point.y, point.z] {
                write(&coordinate.to_le_bytes());
            }
        }
    }
    format!("reorganization-{hash:016x}")
}

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<(), LayoutError> {
        if self.is_cancelled() {
            Err(LayoutError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub fn flag(&self) -> &AtomicBool {
        &self.cancelled
    }
    pub fn report_progress(&self, percent: usize) {
        self.progress.fetch_max(percent.min(100), Ordering::Relaxed);
    }
    pub fn progress(&self) -> usize {
        self.progress.load(Ordering::Relaxed)
    }
}

pub struct LayoutJob {
    token: CancellationToken,
    result: Receiver<Result<LayoutSolution, LayoutError>>,
    finished: bool,
}

impl LayoutJob {
    pub fn spawn(
        work: impl FnOnce(CancellationToken) -> Result<LayoutSolution, LayoutError> + Send + 'static,
    ) -> Self {
        let token = CancellationToken::default();
        let worker_token = token.clone();
        let (sender, result) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let computed = worker_token
                .check()
                .and_then(|()| work(worker_token.clone()));
            let result = worker_token.check().and(computed);
            if result.is_ok() {
                worker_token.report_progress(100);
            }
            let _ = sender.send(result);
        });
        Self {
            token,
            result,
            finished: false,
        }
    }

    pub fn cancel(&self) {
        self.token.cancel();
    }
    pub fn progress(&self) -> usize {
        self.token.progress()
    }

    /// Poll without blocking rendering. A finished or cancelled job emits once.
    pub fn try_result(&mut self) -> Option<Result<LayoutSolution, LayoutError>> {
        if self.finished {
            return None;
        }
        match self.result.try_recv() {
            Ok(result) => {
                self.finished = true;
                Some(self.token.check().and(result))
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Some(Err(LayoutError::Invalid(
                    "layout worker ended without a complete result".into(),
                )))
            }
        }
    }
}

impl Drop for LayoutJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// A bounded LRU of complete accepted interiors. The caller budgets a layout's
/// measured heap footprint, keeping provider-record and scene budgets separate.
pub struct LayoutCache {
    max_bytes: usize,
    bytes: usize,
    tick: u64,
    entries: BTreeMap<String, (u64, usize, LayoutSolution)>,
}

impl LayoutCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            bytes: 0,
            tick: 0,
            entries: BTreeMap::new(),
        }
    }
    pub fn resident_bytes(&self) -> usize {
        self.bytes
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn insert(&mut self, key: String, solution: LayoutSolution, measured_bytes: usize) -> bool {
        if measured_bytes == 0 || measured_bytes > self.max_bytes {
            return false;
        }
        if let Some((_, bytes, _)) = self.entries.remove(&key) {
            self.bytes -= bytes;
        }
        while measured_bytes > self.max_bytes - self.bytes {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (tick, _, _))| *tick)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.bytes -= self.entries.remove(&oldest).unwrap().1;
        }
        self.tick = self.tick.saturating_add(1);
        self.entries
            .insert(key, (self.tick, measured_bytes, solution));
        self.bytes += measured_bytes;
        true
    }
    pub fn get(
        &mut self,
        key: &str,
        model_id: &str,
        revision: u64,
        projection: &str,
        version: u32,
    ) -> Option<&LayoutSolution> {
        let (last_use, _, solution) = self.entries.get_mut(key)?;
        if solution.model_id != model_id
            || solution.revision != revision
            || solution.projection_key != projection
            || solution.version != version
        {
            return None;
        }
        self.tick = self.tick.saturating_add(1);
        *last_use = self.tick;
        Some(solution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, LayoutBox, Metrics, Point};

    fn solution(cost: f64, offset: i64) -> LayoutSolution {
        LayoutSolution {
            model_id: "model".into(),
            revision: 1,
            projection_key: "view".into(),
            version: 1,
            boxes: BTreeMap::from([(
                "a".into(),
                LayoutBox {
                    id: "a".into(),
                    semantic_ids: vec!["a".into()],
                    parent: None,
                    solid: true,
                    bounds: Bounds {
                        min: Point::new(offset, 0, 0),
                        max: Point::new(offset + 10, 10, 10),
                    },
                },
            )]),
            routes: vec![],
            metrics: Metrics {
                structural_cost: cost,
                volume: 1_000.0,
                aspect_ratio: 1.0,
                ..Metrics::default()
            },
            diagnostics: vec![],
        }
    }
    fn session() -> LayoutSession {
        LayoutSession::new("model".into(), 1, "view".into(), 1)
    }

    #[test]
    fn threshold_preview_apply_and_undo_are_explicit() {
        let mut state = session();
        state
            .accept_initial(solution(100.0, 0), |_| Ok(()))
            .unwrap();
        assert!(
            state
                .offer(solution(86.0, 10), |_| Ok(()))
                .unwrap()
                .is_none()
        );
        let proposal = state
            .offer(solution(85.0, 10), |_| Ok(()))
            .unwrap()
            .unwrap();
        assert_eq!(proposal.stats.objects_moved, 1);
        let id = proposal.id.clone();
        assert_eq!(state.accepted().unwrap().metrics.structural_cost, 100.0);
        state.apply(&id, |_| Ok(())).unwrap();
        assert_eq!(state.accepted().unwrap().metrics.structural_cost, 85.0);
        assert!(state.undo(|_| Ok(())).unwrap());
        assert_eq!(state.accepted().unwrap().metrics.structural_cost, 100.0);
    }

    #[test]
    fn stale_or_invalid_results_leave_accepted_scene_untouched() {
        let mut state = session();
        state
            .accept_initial(solution(100.0, 0), |_| Ok(()))
            .unwrap();
        let id = state
            .offer(solution(80.0, 10), |_| Ok(()))
            .unwrap()
            .unwrap()
            .id
            .clone();
        assert!(
            state
                .apply(&id, |_| Err(LayoutError::Unroutable("blocked".into())))
                .is_err()
        );
        state.set_context("model".into(), 2, "view".into(), 1);
        assert!(state.apply(&id, |_| Ok(())).is_err());
        assert!(state.accept_repair(solution(80.0, 10), |_| Ok(())).is_err());
        assert_eq!(state.accepted().unwrap().revision, 1);
        assert!(!state.undo(|_| Ok(())).unwrap());
    }

    #[test]
    fn congestion_guard_rejects_material_worsening_despite_lower_combined_cost() {
        let mut state = session();
        let mut accepted = solution(100.0, 0);
        accepted.metrics.congestion = 1.0;
        state.accept_initial(accepted, |_| Ok(())).unwrap();
        let mut worse = solution(50.0, 10);
        worse.metrics.congestion = 1.21;
        assert!(state.offer(worse, |_| Ok(())).unwrap().is_none());
        let mut small_change = solution(80.0, 10);
        small_change.metrics.congestion = 1.15;
        assert!(state.offer(small_change, |_| Ok(())).unwrap().is_some());
        assert_eq!(state.accepted().unwrap().metrics.congestion, 1.0);

        let mut clean = session();
        clean
            .accept_initial(solution(100.0, 0), |_| Ok(()))
            .unwrap();
        let mut new_junction = solution(50.0, 10);
        new_junction.metrics.congestion = 0.5;
        assert!(clean.offer(new_junction, |_| Ok(())).unwrap().is_none());
        let mut negligible = solution(80.0, 10);
        negligible.metrics.congestion = 0.05;
        assert!(clean.offer(negligible, |_| Ok(())).unwrap().is_some());
        for invalid in [f64::NAN, f64::INFINITY, -1.0] {
            let mut candidate = solution(80.0, 20);
            candidate.metrics.congestion = invalid;
            assert!(clean.offer(candidate, |_| Ok(())).is_err());
        }
        let mut negative_length = solution(80.0, 20);
        negative_length.metrics.shared_lane_length = -1;
        assert!(clean.offer(negative_length, |_| Ok(())).is_err());
    }

    #[test]
    fn dismissed_candidate_and_missing_semantics_are_not_offered_again() {
        let mut state = session();
        state
            .accept_initial(solution(100.0, 0), |_| Ok(()))
            .unwrap();
        let id = state
            .offer(solution(80.0, 10), |_| Ok(()))
            .unwrap()
            .unwrap()
            .id
            .clone();
        assert!(state.dismiss(&id));
        assert!(
            state
                .offer(solution(80.0, 10), |_| Ok(()))
                .unwrap()
                .is_none()
        );
        let mut missing = solution(1.0, 20);
        missing.boxes.clear();
        assert!(state.offer(missing, |_| Ok(())).unwrap().is_none());
        let mut invalid = solution(f64::NAN, 20);
        invalid.metrics.occlusion = 0.0;
        assert!(state.offer(invalid, |_| Ok(())).is_err());
    }

    #[test]
    fn cancellation_never_publishes_a_completed_or_partial_layout() {
        let token = CancellationToken::default();
        token.cancel();
        assert_eq!(token.check(), Err(LayoutError::Cancelled));
        let mut job = LayoutJob::spawn(|token| {
            token.report_progress(50);
            Ok(solution(100.0, 0))
        });
        job.cancel();
        for _ in 0..10_000 {
            if let Some(result) = job.try_result() {
                assert_eq!(result, Err(LayoutError::Cancelled));
                assert!(job.try_result().is_none());
                return;
            }
            thread::yield_now();
        }
        panic!("worker did not finish");
    }

    #[test]
    fn cache_eviction_is_bounded_and_checks_versions() {
        let mut cache = LayoutCache::new(200);
        assert!(cache.insert("a".into(), solution(100.0, 0), 100));
        assert!(cache.insert("b".into(), solution(100.0, 10), 100));
        assert!(cache.get("a", "model", 1, "view", 1).is_some());
        assert!(cache.insert("c".into(), solution(100.0, 20), 100));
        assert!(cache.get("b", "model", 1, "view", 1).is_none());
        assert!(cache.get("a", "model", 2, "view", 1).is_none());
        assert_eq!(cache.resident_bytes(), 200);
        assert!(!cache.insert("huge".into(), solution(100.0, 0), 201));
    }
}
