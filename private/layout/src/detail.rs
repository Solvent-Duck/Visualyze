//! Automatic level of detail (ADR 0007 section 3): among the views the
//! projector could show, pick the one that opens the most supplied structure
//! while its visual noise stays at or under `NOISE_THRESHOLD`.
//!
//! Only supplied structure is ever opened. Display groups (suggested or manual)
//! are never candidates, and a trial that makes the projector invent a new
//! suggested group is rejected. A user's explicit choices are never touched:
//! the planner works on a clone of the view, `collapsed` wins over what it
//! opens, explicit expansions stay as they are, and a view that is already over
//! the threshold is returned as it is.
//!
//! The planner is a pure, deterministic function of the view and the source,
//! so cached projections stay valid for the same view.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::AtomicBool;

use visualyze_model::architecture::Architecture;
use visualyze_model::provider::{EntityKind, ModelProvider, SemanticLens};

use crate::{
    LayoutError, Projection,
    noise::{BOX_KNEE, NOISE_THRESHOLD, visual_noise},
    projection::{self, ViewState},
    provider_projection::{
        BundleProjection, ProviderViewRequest, child_count, children_at, project_provider,
        provider_error,
    },
};

/// Trials for one snapshot view. A snapshot projects in well under a
/// millisecond, and a readable view holds about 27 boxes, so 32 trials can test
/// every container a view could open and a few more revealed by earlier ones.
pub const SNAPSHOT_MAX_TRIALS: usize = 32;
/// Trials for one bundle view. Each trial is a bounded provider projection that
/// loads children and scans relationship pages, which costs far more than a
/// snapshot projection (tens to hundreds of milliseconds on a production
/// bundle), and the focused interior it opens is small. Eight keeps the first
/// view's planning within the same order as one projection.
pub const BUNDLE_MAX_TRIALS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanBudget {
    /// Projections the planner may run beyond the base view. Each focused
    /// interior is planned separately with its own budget.
    pub max_trials: usize,
}

impl PlanBudget {
    pub const SNAPSHOT: Self = Self {
        max_trials: SNAPSHOT_MAX_TRIALS,
    };
    pub const BUNDLE: Self = Self {
        max_trials: BUNDLE_MAX_TRIALS,
    };
}

/// A planned projection and the boxes the planner opened to reach it.
#[derive(Debug, Clone)]
pub struct Planned<T> {
    pub result: T,
    /// Boxes opened automatically, none of which the user opened or collapsed.
    pub opened: BTreeSet<String>,
}

/// What the planner needs from a snapshot or a bundle.
pub trait DetailSource {
    type Output;
    /// Project a view. The view's expansion already includes whatever the
    /// planner has tentatively opened.
    fn project(&mut self, view: &ViewState) -> Result<Self::Output, LayoutError>;
    fn graph(output: &Self::Output) -> &Projection;
    /// How many supplied children opening the displayed box `id` would reveal
    /// as structure; zero for a leaf, or for a component whose children are only
    /// interfaces (pads on its own face).
    fn hidden_structure(&mut self, output: &Self::Output, id: &str) -> Result<usize, LayoutError>;
}

/// Open supplied structure on a clone of `view` until the next opening would
/// push noise over `NOISE_THRESHOLD`, or `budget` trials are spent.
///
/// 1. Project the view as given. Over the threshold: return it, opening nothing.
/// 2. Candidates are displayed, collapsed boxes that are not the root, a
///    reference, a display group, collapsed by the user or already open, and
///    that hide supplied structure. They are tried shallowest first, then with
///    the fewest hidden children (more containers open within the budget), then
///    by ID.
/// 3. A trial opens one candidate and is kept when the projection reveals it,
///    adds no suggested group, and stays within the threshold.
/// 4. After a pass that kept anything, containers it revealed become
///    candidates; rejected ones are not retried. Stop when a pass keeps nothing.
pub fn plan_detail<S: DetailSource>(
    source: &mut S,
    view: &ViewState,
    budget: PlanBudget,
) -> Result<Planned<S::Output>, LayoutError> {
    let mut current = source.project(view)?;
    let mut opened = BTreeSet::new();
    if visual_noise(S::graph(&current)).total > NOISE_THRESHOLD {
        return Ok(Planned {
            result: current,
            opened,
        });
    }
    let mut working = view.clone();
    let mut declined: BTreeSet<String> = BTreeSet::new();
    let mut trials = 0;
    let max_boxes = max_boxes_within(NOISE_THRESHOLD);
    loop {
        let candidates = candidates(source, &current, view, &declined, max_boxes)?;
        let mut kept_any = false;
        for id in candidates {
            if trials >= budget.max_trials {
                return Ok(Planned {
                    result: current,
                    opened,
                });
            }
            trials += 1;
            working.plan_open(id.clone());
            let kept = match source.project(&working) {
                Ok(trial) if keeps(S::graph(&current), S::graph(&trial), &id) => Some(trial),
                Ok(_) => None,
                // Automatic detail never makes a view that renders fail.
                Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
                Err(_) => None,
            };
            match kept {
                Some(trial) => {
                    current = trial;
                    opened.insert(id);
                    kept_any = true;
                }
                None => {
                    working.plan_close(&id);
                    declined.insert(id);
                }
            }
        }
        if !kept_any {
            return Ok(Planned {
                result: current,
                opened,
            });
        }
    }
}

/// Most boxes a view can hold at or under `limit` noise, whatever their layers.
fn max_boxes_within(limit: f64) -> usize {
    (BOX_KNEE * (1.0 + (limit / 100.0).sqrt())).floor() as usize
}

/// A trial is kept when it opened the box, invented no suggested group, and is
/// still within the threshold.
fn keeps(before: &Projection, trial: &Projection, id: &str) -> bool {
    let revealed = trial
        .nodes
        .iter()
        .any(|node| node.parent.as_deref() == Some(id));
    let invented = trial.suggested_groups.iter().any(|group| {
        group.suggested
            && !before
                .suggested_groups
                .iter()
                .any(|known| known.id == group.id)
    });
    revealed && !invented && visual_noise(trial).total <= NOISE_THRESHOLD
}

fn candidates<S: DetailSource>(
    source: &mut S,
    current: &S::Output,
    view: &ViewState,
    declined: &BTreeSet<String>,
    max_boxes: usize,
) -> Result<Vec<String>, LayoutError> {
    let graph = S::graph(current);
    let depth = depths(graph);
    let groups: BTreeSet<&str> = graph
        .suggested_groups
        .iter()
        .map(|group| group.id.as_str())
        .collect();
    let mut ranked = Vec::new();
    for node in &graph.nodes {
        if node.id == graph.root
            || !node.solid
            || graph.references.contains_key(&node.id)
            || groups.contains(node.id.as_str())
            || declined.contains(&node.id)
            || view.collapsed.contains(&node.id)
            || view.is_expanded(&node.id)
        {
            continue;
        }
        let hidden = source.hidden_structure(current, &node.id)?;
        // More children than any readable view can hold would be rejected.
        if hidden == 0 || hidden > max_boxes {
            continue;
        }
        ranked.push((
            depth.get(node.id.as_str()).copied().unwrap_or(usize::MAX),
            hidden,
            node.id.clone(),
        ));
    }
    ranked.sort();
    Ok(ranked.into_iter().map(|(_, _, id)| id).collect())
}

/// Distance from the root through displayed parents.
fn depths(graph: &Projection) -> BTreeMap<&str, usize> {
    let parents: BTreeMap<&str, Option<&str>> = graph
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.parent.as_deref()))
        .collect();
    let mut depth = BTreeMap::new();
    for node in &graph.nodes {
        let mut steps = 0;
        let mut at = node.id.as_str();
        while at != graph.root && steps <= graph.nodes.len() {
            match parents.get(at).copied().flatten() {
                Some(parent) => at = parent,
                None => break,
            }
            steps += 1;
        }
        depth.insert(node.id.as_str(), steps);
    }
    depth
}

/// A snapshot to plan: its projection is `projection::project`, and a box
/// hides structure when its supplied children, in the view's lens, include more
/// than interface pads.
pub struct SnapshotDetail<'a> {
    model: &'a Architecture,
    hidden: BTreeMap<String, usize>,
}

impl<'a> SnapshotDetail<'a> {
    pub fn new(model: &'a Architecture, lens: Option<SemanticLens>) -> Self {
        Self {
            model,
            hidden: projection::hidden_structure_counts(model, lens),
        }
    }
}

impl DetailSource for SnapshotDetail<'_> {
    type Output = Projection;
    fn project(&mut self, view: &ViewState) -> Result<Projection, LayoutError> {
        projection::project(self.model, view)
    }
    fn graph(output: &Projection) -> &Projection {
        output
    }
    fn hidden_structure(&mut self, _: &Projection, id: &str) -> Result<usize, LayoutError> {
        Ok(self.hidden.get(id).copied().unwrap_or(0))
    }
}

/// Plan a snapshot view. `view.lens` is the lens whose hierarchy is opened.
pub fn plan_snapshot(
    model: &Architecture,
    view: &ViewState,
    budget: PlanBudget,
) -> Result<Planned<Projection>, LayoutError> {
    plan_detail(&mut SnapshotDetail::new(model, view.lens), view, budget)
}

/// A bundle region to plan. `request` turns a view into the bounded provider
/// request that projects it, with `ViewState::expanded_ids()` as its expansion.
pub struct ProviderDetail<'a, P: ModelProvider, F: Fn(&ViewState) -> ProviderViewRequest> {
    provider: &'a mut P,
    request: F,
    lens: Option<SemanticLens>,
    cancelled: &'a AtomicBool,
}

impl<P: ModelProvider, F: Fn(&ViewState) -> ProviderViewRequest> DetailSource
    for ProviderDetail<'_, P, F>
{
    type Output = BundleProjection;
    fn project(&mut self, view: &ViewState) -> Result<BundleProjection, LayoutError> {
        project_provider(self.provider, &(self.request)(view), self.cancelled)
    }
    fn graph(output: &BundleProjection) -> &Projection {
        &output.graph
    }
    fn hidden_structure(
        &mut self,
        output: &BundleProjection,
        id: &str,
    ) -> Result<usize, LayoutError> {
        let revision = output.graph.revision;
        let lens = self.lens;
        let count = child_count(&*self.provider, revision, lens, id).map_err(provider_error)?;
        if count == 0 {
            return Ok(0);
        }
        let component = self
            .provider
            .summary(revision, id)
            .map_err(provider_error)?
            .is_some_and(|summary| {
                matches!(
                    summary.kind,
                    EntityKind::Client | EntityKind::Service | EntityKind::Datastore
                )
            });
        if !component {
            return Ok(count);
        }
        // Exposed interfaces stay pads on the component's face.
        let children = children_at(self.provider, revision, lens, id, 0, count.min(128))
            .map_err(provider_error)?;
        let structure = children
            .items
            .iter()
            .filter(|child| child.kind != EntityKind::Interface)
            .count();
        Ok(if children.items.len() < count {
            // A longer list than one page: the interfaces cannot be all of it.
            count
        } else {
            structure
        })
    }
}

/// Plan one bundle region. `request` must give the same lens for every view.
pub fn plan_provider<P: ModelProvider>(
    provider: &mut P,
    view: &ViewState,
    request: impl Fn(&ViewState) -> ProviderViewRequest,
    budget: PlanBudget,
    cancelled: &AtomicBool,
) -> Result<Planned<BundleProjection>, LayoutError> {
    let lens = request(view).lens;
    let mut source = ProviderDetail {
        provider,
        request,
        lens,
        cancelled,
    };
    plan_detail(&mut source, view, budget)
}
