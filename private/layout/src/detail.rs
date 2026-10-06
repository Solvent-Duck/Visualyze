//! Automatic level of detail (ADR 0007 section 3): open the supplied boxes of
//! the focused view whose own interiors are readable.
//!
//! Every expanded box is its own region with its own noise budget
//! (`noise::region_noise`): what a box holds never counts toward the view
//! around it. So whether a box opens depends only on that box's interior, not on
//! its siblings and not on how dense the focused view already is. The planner
//! opens one level: the direct children of the focused root. Opening a box
//! shows its children collapsed; the user expands or enters them, and entering
//! one plans its own level.
//!
//! Only supplied structure is ever opened. Display groups (suggested or manual)
//! are never candidates, and a box whose opening makes the projector invent a
//! suggested group inside it is left closed. A user's explicit choices are never
//! touched: the planner works on a clone of the view, `collapsed` wins over what
//! it opens, and explicit expansions stay as they are.
//!
//! The planner is a pure, deterministic function of the view and the source,
//! so cached projections stay valid for the same view.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::atomic::AtomicBool;

use visualyze_model::architecture::Architecture;
use visualyze_model::provider::{EntityKind, ModelProvider, SemanticLens};

use crate::{
    LayoutError, Projection,
    noise::{BOX_KNEE, NOISE_THRESHOLD, VisualNoise, region_noise},
    projection::{self, DisplayGroup, ViewState},
    provider_projection::{
        BundleProjection, Continuation, ProviderViewRequest, child_count, children_at,
        project_provider, provider_error,
    },
};

/// Projections for one snapshot view. A snapshot projects in well under a
/// millisecond. Boxes open in batches (all candidates at once, then once more
/// with the ones that fit), so a view needs two or three; a batch that hits the
/// object limit is halved, which costs one more per cut, so 32 leaves room for
/// a long run of cuts.
pub const SNAPSHOT_MAX_TRIALS: usize = 32;
/// Projections for one bundle view. Each is a bounded provider projection that
/// loads children and scans relationship pages, which costs far more than a
/// snapshot projection (tens to hundreds of milliseconds on a production
/// bundle). A view needs two or three; eight keeps the first view's planning
/// within the same order as one projection and still allows a few cuts when a
/// batch exceeds the object limit.
pub const BUNDLE_MAX_TRIALS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanBudget {
    /// Projections the planner may run beyond the base view. Each focused
    /// view is planned separately with its own budget.
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
    /// Whether `trial`, projected from `view`, ran into the hard visible-object
    /// limit where `before` did not: a region left collapsed or the limit
    /// exceeded. The planner then tries fewer boxes.
    fn hit_visible_budget(view: &ViewState, before: &Self::Output, trial: &Self::Output) -> bool;
    /// How many supplied children opening the displayed box `id` would reveal
    /// as structure; zero for a leaf, or for a component whose children are only
    /// interfaces (pads on its own face).
    fn hidden_structure(&mut self, output: &Self::Output, id: &str) -> Result<usize, LayoutError>;
}

/// Open supplied boxes on a clone of `view` while each one's own interior stays
/// within `NOISE_THRESHOLD`, or `budget` trials are spent.
///
/// 1. Project the view as given. Its own noise does not matter: it is the
///    focused region, and a box opened beside others never changes what they
///    hold.
/// 2. Candidates are the focused root's direct children that are displayed,
///    collapsed boxes that are not a reference, a display group, collapsed by
///    the user or already open, and that hide supplied structure. Their order,
///    which every cut below keeps, is fewest hidden children first, then by ID.
/// 3. A batch is projected with all of its boxes open. A box passes when the
///    projection reveals it, its own region scores within the threshold, and
///    it adds no suggested group inside it. Boxes that pass are projected again
///    on their own, which adopts them if they all still pass.
/// 4. A batch that fails to project, or that hits the visible-object limit, is
///    cut into two halves in candidate order; a single box that does is left
///    closed. Explicit choices are never changed, and stay as they are.
///
/// Boxes are independent, so a sibling's state never decides whether a box
/// opens. The one exception is the visible-object limit (`visible_budget`, 250
/// by default): what the view already shows counts against it, so when it is
/// reached later candidates may not fit.
pub fn plan_detail<S: DetailSource>(
    source: &mut S,
    view: &ViewState,
    budget: PlanBudget,
) -> Result<Planned<S::Output>, LayoutError> {
    let mut current = source.project(view)?;
    let mut opened = BTreeSet::new();
    let mut working = view.clone();
    let candidates = candidates(source, &current, view, max_boxes_within(NOISE_THRESHOLD))?;
    let mut batches: VecDeque<Vec<String>> = VecDeque::new();
    if !candidates.is_empty() {
        batches.push_back(candidates);
    }
    let mut trials = 0;
    while let Some(batch) = batches.pop_front() {
        if trials >= budget.max_trials {
            break;
        }
        trials += 1;
        let mut trial_view = working.clone();
        for id in &batch {
            trial_view.plan_open(id.clone());
        }
        let cut = |batch: Vec<String>, batches: &mut VecDeque<Vec<String>>| {
            if batch.len() > 1 {
                let second = batch[batch.len() / 2..].to_vec();
                let mut first = batch;
                first.truncate(first.len() - second.len());
                batches.push_front(second);
                batches.push_front(first);
            }
        };
        let trial = match source.project(&trial_view) {
            Ok(trial) => trial,
            // Automatic detail never makes a view that renders fail.
            Err(LayoutError::Cancelled) => return Err(LayoutError::Cancelled),
            Err(_) => {
                cut(batch, &mut batches);
                continue;
            }
        };
        if S::hit_visible_budget(&trial_view, &current, &trial) {
            cut(batch, &mut batches);
            continue;
        }
        let interiors = region_noise(S::graph(&trial));
        let fits: Vec<String> = batch
            .iter()
            .filter(|id| fits(S::graph(&current), S::graph(&trial), &interiors, id))
            .cloned()
            .collect();
        if fits.len() == batch.len() {
            for id in batch {
                working.plan_open(id.clone());
                opened.insert(id);
            }
            current = trial;
        } else if !fits.is_empty() {
            batches.push_front(fits);
        }
    }
    Ok(Planned {
        result: current,
        opened,
    })
}

/// Most boxes a region can hold at or under `limit` noise, whatever their layers.
fn max_boxes_within(limit: f64) -> usize {
    (BOX_KNEE * (1.0 + (limit / 100.0).sqrt())).floor() as usize
}

/// A box fits when the trial revealed it, its own region is within the
/// threshold, and opening it invented no suggested group inside it.
fn fits(
    before: &Projection,
    trial: &Projection,
    interiors: &BTreeMap<String, VisualNoise>,
    id: &str,
) -> bool {
    let revealed = trial
        .nodes
        .iter()
        .any(|node| node.parent.as_deref() == Some(id));
    // The projector lists the suggestions it could make for a collapsed box
    // too, so a group counts only when the trial draws it.
    let drawn = |projection: &Projection, group: &DisplayGroup| {
        projection.nodes.iter().any(|node| node.id == group.id)
    };
    let invented = trial.suggested_groups.iter().any(|group| {
        group.suggested && group.parent == id && drawn(trial, group) && !drawn(before, group)
    });
    revealed
        && !invented
        && interiors
            .get(id)
            .is_some_and(|noise| noise.total <= NOISE_THRESHOLD)
}

fn candidates<S: DetailSource>(
    source: &mut S,
    current: &S::Output,
    view: &ViewState,
    max_boxes: usize,
) -> Result<Vec<String>, LayoutError> {
    let graph = S::graph(current);
    let groups: BTreeSet<&str> = graph
        .suggested_groups
        .iter()
        .map(|group| group.id.as_str())
        .collect();
    let mut ranked = Vec::new();
    for node in &graph.nodes {
        if node.parent.as_deref() != Some(graph.root.as_str())
            || node.id == graph.root
            || !node.solid
            || graph.references.contains_key(&node.id)
            || groups.contains(node.id.as_str())
            || view.collapsed.contains(&node.id)
            || view.is_expanded(&node.id)
        {
            continue;
        }
        let hidden = source.hidden_structure(current, &node.id)?;
        // More children than any readable region can hold would be rejected.
        if hidden == 0 || hidden > max_boxes {
            continue;
        }
        ranked.push((hidden, node.id.clone()));
    }
    ranked.sort();
    Ok(ranked.into_iter().map(|(_, id)| id).collect())
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
    fn hit_visible_budget(view: &ViewState, before: &Projection, trial: &Projection) -> bool {
        trial.nodes.len() > view.visible_budget && before.nodes.len() <= view.visible_budget
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
    fn hit_visible_budget(
        _: &ViewState,
        before: &BundleProjection,
        trial: &BundleProjection,
    ) -> bool {
        // A region the object limit left collapsed is reported as pending.
        let pending = |output: &BundleProjection| {
            output
                .continuations
                .iter()
                .filter_map(|continuation| match continuation {
                    Continuation::PendingChildren { parent } => Some(parent.clone()),
                    _ => None,
                })
                .collect::<BTreeSet<String>>()
        };
        !pending(trial).is_subset(&pending(before))
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
