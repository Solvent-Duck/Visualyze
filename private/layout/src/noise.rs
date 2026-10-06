//! Visual noise: how much a projected view would show before layout (ADR 0007
//! section 3). It is a pure function of the projection, so it can be computed
//! without solving and never changes placement. Each term is free up to a soft
//! knee and then grows quadratically; no view is ever refused on this score.
//!
//! The knees are private tuning constants, not public contract.
use crate::projection::bundle_ends;
use crate::{Node, NodeKind, Projection};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Visible boxes in a whole view that cost nothing.
pub const BOX_KNEE: f64 = 27.0;
/// Visible boxes in one layer (`Node.band`) that cost nothing.
pub const LAYER_KNEE: f64 = 12.0;
/// Drawn connectors, after aggregation, that cost nothing.
pub const CONNECTOR_KNEE: f64 = 36.0;
/// A view with total noise above this is "noisy"; it is advised on, never refused.
pub const NOISE_THRESHOLD: f64 = 10.0;
/// How many boxes an expanded frame (group or scope drawn as an outline) counts as.
pub const FRAME_WEIGHT: f64 = 1.0;
/// Noise to assume for a view whose real score is unknown or unbounded: "far
/// over the threshold". Step 4 of ADR 0007 uses it so that fallback grouping
/// only runs on views that are far over budget.
pub const FALLBACK_NOISE: f64 = 100.0;

/// `100 * (max(0, x - knee) / knee)^2`: zero up to the knee, then quadratic.
pub fn overshoot(x: f64, knee: f64) -> f64 {
    let over = (x - knee).max(0.0) / knee;
    100.0 * over * over
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VisualNoise {
    /// Total visible boxes (frames weighted by `FRAME_WEIGHT`).
    pub boxes: f64,
    /// Boxes per layer, keyed by `Node.band`.
    pub layers: BTreeMap<i32, f64>,
    /// Drawn connectors after aggregation.
    pub connectors: usize,
    pub box_term: f64,
    pub layer_term: f64,
    pub connector_term: f64,
    pub total: f64,
}

impl VisualNoise {
    pub fn over_threshold(&self) -> bool {
        self.total > NOISE_THRESHOLD
    }

    pub fn widest_layer(&self) -> f64 {
        self.layers.values().copied().fold(0.0, f64::max)
    }

    /// For example `visual noise 5.2 (30 boxes, widest layer 14, 40 connectors)`.
    pub fn summary(&self) -> String {
        format!("visual noise {:.1} ({})", self.total, self.counts_summary())
    }

    /// A diagnostic to attach to a projection, only when the view is over the
    /// threshold. Expansion that causes it is still allowed.
    pub fn advisory(&self) -> Option<String> {
        self.over_threshold().then(|| {
            format!(
                "Visual noise {:.1} is over the {} readability threshold ({}); explicit expansion is still allowed.",
                self.total,
                NOISE_THRESHOLD,
                self.counts_summary()
            )
        })
    }

    /// The advisory for an expanded box's interior, naming the region.
    pub fn region_advisory(&self, region: &str) -> Option<String> {
        self.over_threshold().then(|| {
            format!(
                "{region}: visual noise {:.1} inside this box is over the {} readability threshold ({}); explicit expansion is still allowed.",
                self.total,
                NOISE_THRESHOLD,
                self.counts_summary()
            )
        })
    }

    fn counts_summary(&self) -> String {
        format!(
            "{} boxes, widest layer {}, {} connectors",
            count(self.boxes),
            count(self.widest_layer()),
            self.connectors
        )
    }
}

fn count(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// Score precomputed counts. `visual_noise` delegates here.
pub fn noise_from_counts(
    boxes: f64,
    layers: &BTreeMap<i32, f64>,
    connectors: usize,
) -> VisualNoise {
    let box_term = overshoot(boxes, BOX_KNEE);
    let layer_term = layers.values().map(|n| overshoot(*n, LAYER_KNEE)).sum();
    let connector_term = overshoot(connectors as f64, CONNECTOR_KNEE);
    VisualNoise {
        boxes,
        layers: layers.clone(),
        connectors,
        box_term,
        layer_term,
        connector_term,
        total: box_term + layer_term + connector_term,
    }
}

/// Estimate the noise of one level (the direct children of one region) from how
/// many boxes it has per layer. Boxes are the sum of the counts. Connectors are
/// unknown before routing, so the estimate ignores them (it counts zero) and is
/// a lower bound on what the drawn level will score.
pub fn level_noise(band_counts: &BTreeMap<i32, usize>) -> VisualNoise {
    let layers: BTreeMap<i32, f64> = band_counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(band, n)| (*band, *n as f64))
        .collect();
    noise_from_counts(layers.values().sum(), &layers, 0)
}

/// The largest page of a level, in boxes, whose noise stays at or under `limit`.
///
/// The level's layer distribution is scaled proportionally to `p` boxes (each
/// layer rounds up, so the result is deterministic) and scored with
/// `level_noise`; the largest such `p` between 2 and the level's total wins. A
/// level that already fits returns its whole total. A level of at most two
/// boxes is returned whole.
pub fn page_size_within(band_counts: &BTreeMap<i32, usize>, limit: f64) -> usize {
    let counts: Vec<usize> = band_counts.values().copied().filter(|n| *n > 0).collect();
    let total: usize = counts.iter().sum();
    if total <= 2 {
        return total;
    }
    (2..=total)
        .rev()
        .find(|page| {
            let scaled: BTreeMap<i32, usize> = counts
                .iter()
                .enumerate()
                .map(|(band, n)| (band as i32, (n * page).div_ceil(total)))
                .collect();
            level_noise(&scaled).total <= limit
        })
        .unwrap_or(2)
}

/// Like `page_size_within`, for a level whose children are known in the order
/// they will be paged, each given by its layer.
///
/// It starts from the proportional size for the level's layer mix, then lowers
/// it until every whole page of that size, cut from the front of `bands` as
/// paging cuts it, is within `limit`. A level sorted into runs of one layer
/// therefore pages smaller than an evenly mixed one: every page that is
/// measured fits, not only the average one.
pub fn page_size_in_order(bands: &[i32], limit: f64) -> usize {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for band in bands {
        *counts.entry(*band).or_default() += 1;
    }
    let start = page_size_within(&counts, limit);
    let fits = |page: usize| {
        bands.chunks(page).all(|chunk| {
            let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
            for band in chunk {
                *counts.entry(*band).or_default() += 1;
            }
            level_noise(&counts).total <= limit
        })
    };
    (2..=start)
        .rev()
        .find(|page| fits(*page))
        .unwrap_or(start.min(2))
}

/// The most boxes one layer can hold, alone in a view, within `limit` noise
/// (at least 2). With `FALLBACK_NOISE` this is the single-layer page size; with
/// `NOISE_THRESHOLD` it is how many fallback groups fit one readable level.
pub fn layer_capacity(limit: f64) -> usize {
    let alone = |n: usize| {
        let boxes = n as f64;
        overshoot(boxes, BOX_KNEE) + overshoot(boxes, LAYER_KNEE)
    };
    (3..=4096)
        .take_while(|n| alone(*n) <= limit)
        .last()
        .unwrap_or(2)
}

/// What one region draws, before scoring: its boxes, boxes per layer, and
/// connectors.
#[derive(Default)]
struct RegionCounts {
    boxes: f64,
    layers: BTreeMap<i32, f64>,
    connectors: usize,
}

/// Counts for every region of a projection (see `region_noise`). The one
/// counting routine that `region_noise`, `visual_noise` and `interior_noise`
/// share, so they cannot drift.
///
/// Counted as boxes, in the region they are parented to: solid boxes, frames
/// (`FRAME_WEIGHT` each, whatever they hold), and, when `include_references`,
/// external references. Not counted: the focused root, which is the view
/// itself, and interface pads attached to a solid owner, which belong to the
/// owner. With `include_references` false, references are neither boxes nor
/// ends of a counted connector.
fn region_counts<'p>(
    projection: &'p Projection,
    include_references: bool,
) -> BTreeMap<String, RegionCounts> {
    let by_id: BTreeMap<&str, &Node> = projection
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let is_reference = |id: &str| projection.references.contains_key(id);
    // Interface pad id -> its solid owner, which the pad is drawn as part of.
    let pad_of: BTreeMap<&str, &str> = projection
        .nodes
        .iter()
        .filter_map(|node| {
            let owner = node.parent.as_deref()?;
            (node.kind == NodeKind::Interface && by_id.get(owner).is_some_and(|o| o.solid))
                .then_some((node.id.as_str(), owner))
        })
        .collect();
    let counted = |node: &Node| {
        node.id != projection.root
            && if is_reference(&node.id) {
                include_references
            } else {
                !pad_of.contains_key(node.id.as_str())
            }
    };
    // A region is the root, or any node that shows a counted child.
    let mut regions: BTreeSet<&str> = BTreeSet::from([projection.root.as_str()]);
    for node in projection.nodes.iter().filter(|node| counted(node)) {
        if let Some(parent) = node.parent.as_deref().filter(|p| by_id.contains_key(p)) {
            regions.insert(parent);
        }
    }
    let mut counts: BTreeMap<String, RegionCounts> = regions
        .iter()
        .map(|id| ((*id).to_owned(), RegionCounts::default()))
        .collect();
    for node in projection.nodes.iter().filter(|node| counted(node)) {
        let region = node
            .parent
            .as_deref()
            .filter(|parent| regions.contains(parent))
            .unwrap_or(&projection.root);
        let weight = if node.solid { 1.0 } else { FRAME_WEIGHT };
        let entry = counts.get_mut(region).expect("every region has counts");
        entry.boxes += weight;
        *entry.layers.entry(node.band).or_default() += weight;
    }
    // The regions an endpoint sits in, innermost first. An endpoint that is a
    // region starts at itself, any other (a pad counts as its owner) at its
    // parent; the root always ends the chain.
    let root = projection.root.as_str();
    let chain = |id: &str| {
        let id = pad_of.get(id).copied().unwrap_or(id);
        let mut chain = Vec::new();
        let mut at = by_id.get_key_value(id).map(|(key, node)| {
            if regions.contains(key) {
                *key
            } else {
                node.parent.as_deref().unwrap_or(root)
            }
        });
        while let Some(id) = at {
            if chain.len() > by_id.len() {
                break;
            }
            if regions.contains(id) {
                chain.push(id);
            }
            at = by_id.get(id).and_then(|node| node.parent.as_deref());
        }
        if chain.last() != Some(&root) {
            chain.push(root);
        }
        chain
    };
    // The lowest region holding both ends.
    let lowest_common = |source: &str, target: &str| -> &str {
        let target = chain(target);
        chain(source)
            .into_iter()
            .find(|region| target.contains(region))
            .unwrap_or(root)
    };
    // Bundled connectors are counted as they are drawn (see `region_noise`):
    // one trunk per bundle key, one branch per distinct (side, outer box,
    // inner endpoint, bundle key).
    let mut trunks: BTreeSet<&str> = BTreeSet::new();
    let mut branches: BTreeSet<(bool, String, &str, &str)> = BTreeSet::new();
    let node_of = |id: &str| by_id.get(id).copied();
    let owner = |id: &'p str| -> &'p str { pad_of.get(id).copied().unwrap_or(id) };
    for edge in &projection.edges {
        if !include_references && (is_reference(&edge.source) || is_reference(&edge.target)) {
            continue;
        }
        let ends = edge
            .bundle
            .as_deref()
            .and_then(|key| bundle_ends(&node_of, &edge.source, &edge.target).zip(Some(key)));
        let Some(((outer_source, outer_target), key)) = ends else {
            counts
                .get_mut(lowest_common(&edge.source, &edge.target))
                .expect("every chain holds regions")
                .connectors += 1;
            continue;
        };
        if trunks.insert(key) {
            counts
                .get_mut(lowest_common(&outer_source, &outer_target))
                .expect("every chain holds regions")
                .connectors += 1;
        }
        for (source_side, outer, inner) in [
            (true, &outer_source, owner(&edge.source)),
            (false, &outer_target, owner(&edge.target)),
        ] {
            if inner != outer.as_str() && branches.insert((source_side, outer.clone(), inner, key))
            {
                let region = chain(outer)[0];
                counts
                    .get_mut(region)
                    .expect("every chain holds regions")
                    .connectors += 1;
            }
        }
    }
    counts
}

fn score(counts: &RegionCounts) -> VisualNoise {
    noise_from_counts(counts.boxes, &counts.layers, counts.connectors)
}

/// Visual noise of every region of a projection, keyed by region id; the
/// focused root is always present. Boxes inside an expanded box are their own
/// setup, so they never count toward the box or layer totals of the region
/// around it.
///
/// A region is the focused root and every displayed node that shows displayed
/// children: an expanded scope or group frame, or an expanded component with
/// children other than its interface pads.
///
/// * In its parent region an expanded box counts as exactly one box
///   (`FRAME_WEIGHT`); its interior counts only in its own region.
/// * A connector counts once, in the region where it is drawn: the lowest
///   region that contains both ends. A region itself is an end that sits in it,
///   so a connector from a frame to one of its own children counts in that
///   frame, and one from a child to a neighbour of the frame counts in the
///   region around the frame.
/// * Edges that cross expanded boxes carry a bundle key (`Edge::bundle`) and are
///   drawn as one trunk between the outer boxes `S'` and `T'` plus a branch
///   inside each expanded outer box to every distinct inner end. The trunk
///   counts once per bundle key, in the lowest region holding `S'` and `T'`
///   (where the collapsed line would count). Each branch counts once per
///   distinct (side, outer box, inner end, bundle key), in the region of its
///   outer box, because it ends on that box's boundary however deep the inner
///   end sits. Opening a box therefore never changes its parent region's
///   connector count; the branches appear in the box's own region.
/// * External references are boxes in whichever region they are parented to.
///   Interface pads on solid owners belong to the owner and are not counted.
pub fn region_noise(projection: &Projection) -> BTreeMap<String, VisualNoise> {
    region_counts(projection, true)
        .iter()
        .map(|(id, counts)| (id.clone(), score(counts)))
        .collect()
}

/// Score the focused view: its root region, that is, what the focused root
/// shows at its own level, including external references. A box opened inside
/// it counts once there, and its interior is scored by `region_noise`.
pub fn visual_noise(projection: &Projection) -> VisualNoise {
    let mut regions = region_counts(projection, true);
    score(&regions.remove(&projection.root).unwrap_or_default())
}

/// Score only the interior of the projection's root: the root region without
/// external references, and without any connector that touches one. These
/// belong to the surroundings. The box rules are those of `region_noise`, and
/// for a view focused on a scope the result is that scope's region.
pub fn interior_noise(projection: &Projection) -> VisualNoise {
    let mut regions = region_counts(projection, false);
    score(&regions.remove(&projection.root).unwrap_or_default())
}

/// The densest region of the projection (highest total) and its noise. The
/// focused root wins a tie, then the smaller id.
pub fn densest_region(projection: &Projection) -> (String, VisualNoise) {
    let mut regions = region_noise(projection);
    let root = regions
        .remove(&projection.root)
        .unwrap_or_else(|| score(&RegionCounts::default()));
    match densest_of(regions) {
        Some((id, noise)) if noise.total > root.total => (id, noise),
        _ => (projection.root.clone(), root),
    }
}

/// The densest region other than the focused root, if there is one: the
/// interior of an expanded box, where a view shows its busiest setup. The
/// smaller id wins a tie.
pub fn densest_interior(projection: &Projection) -> Option<(String, VisualNoise)> {
    let mut regions = region_noise(projection);
    regions.remove(&projection.root);
    densest_of(regions)
}

fn densest_of(regions: BTreeMap<String, VisualNoise>) -> Option<(String, VisualNoise)> {
    regions.into_iter().reduce(|best, next| {
        if next.1.total > best.1.total {
            next
        } else {
            best
        }
    })
}

/// Advisory diagnostics for a projection: one for the focused view when it is
/// over the threshold, and one for each expanded box whose interior is, each
/// naming its region. Expansion that causes them is still allowed.
pub fn advisories(projection: &Projection) -> Vec<String> {
    region_noise(projection)
        .into_iter()
        .filter_map(|(id, noise)| {
            if id == projection.root {
                noise.advisory()
            } else {
                noise.region_advisory(&id)
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edge, Point};

    fn node(id: &str, kind: NodeKind, parent: Option<&str>, band: i32, solid: bool) -> Node {
        Node {
            id: id.into(),
            semantic_ids: vec![id.into()],
            parent: parent.map(Into::into),
            kind,
            band,
            min_size: Point::new(12, 12, 10),
            max_size: Point::new(48, 24, 40),
            preferred_size: Point::new(24, 12, 20),
            solid,
        }
    }

    fn projection(nodes: Vec<Node>, edges: usize) -> Projection {
        Projection {
            key: "k".into(),
            model_id: "m".into(),
            revision: 1,
            root: "root".into(),
            nodes,
            edges: (0..edges)
                .map(|i| Edge {
                    bundle: None,
                    id: format!("e{i}"),
                    source: "a".into(),
                    target: "b".into(),
                    semantic_ids: vec![format!("e{i}")],
                })
                .collect(),
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        }
    }

    /// `layers` lists the box count for each band.
    fn counts(boxes: f64, layers: &[(i32, f64)], connectors: usize) -> VisualNoise {
        noise_from_counts(boxes, &layers.iter().copied().collect(), connectors)
    }

    fn near(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 0.1,
            "{actual} is not within 0.1 of {expected}"
        );
    }

    #[test]
    fn adr_row_within_knees_is_zero() {
        let n = counts(27.0, &[(0, 12.0), (1, 12.0), (2, 3.0)], 36);
        assert_eq!(n.total, 0.0);
        assert!(!n.over_threshold());
        assert!(n.advisory().is_none());
    }

    #[test]
    fn adr_row_30_boxes_layer_14_40_connectors() {
        let n = counts(30.0, &[(0, 14.0), (1, 12.0), (2, 4.0)], 40);
        near(n.box_term, 1.2);
        near(n.layer_term, 2.8);
        near(n.connector_term, 1.2);
        near(n.total, 5.2);
        assert!(!n.over_threshold());
    }

    #[test]
    fn adr_row_35_boxes_layer_16_48_connectors() {
        let n = counts(35.0, &[(0, 16.0), (1, 12.0), (2, 7.0)], 48);
        near(n.box_term, 8.8);
        near(n.layer_term, 11.1);
        near(n.connector_term, 11.1);
        near(n.total, 31.0);
        assert!(n.over_threshold());
    }

    #[test]
    fn adr_row_45_boxes_layer_20_70_connectors() {
        let n = counts(45.0, &[(0, 20.0), (1, 12.0), (2, 12.0), (3, 1.0)], 70);
        near(n.box_term, 44.4);
        near(n.layer_term, 44.4);
        near(n.connector_term, 89.2);
        near(n.total, 178.0);
    }

    #[test]
    fn summary_and_advisory_name_the_counts() {
        let n = counts(30.0, &[(0, 14.0), (1, 16.0)], 40);
        assert_eq!(
            n.summary(),
            format!(
                "visual noise {:.1} (30 boxes, widest layer 16, 40 connectors)",
                n.total
            )
        );
        let n = counts(35.0, &[(0, 16.0), (1, 19.0)], 48);
        let advice = n.advisory().unwrap();
        assert!(
            advice.contains("over the 10 readability threshold"),
            "{advice}"
        );
        assert!(
            advice.contains("35 boxes, widest layer 19, 48 connectors"),
            "{advice}"
        );
    }

    #[test]
    fn root_envelope_is_not_a_box() {
        let nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("a", NodeKind::Service, Some("root"), 1, true),
            node("b", NodeKind::Datastore, Some("root"), 0, true),
        ];
        let n = visual_noise(&projection(nodes, 1));
        assert_eq!(n.boxes, 2.0);
        assert_eq!(n.layers, BTreeMap::from([(0, 1.0), (1, 1.0)]));
        assert_eq!(n.connectors, 1);
    }

    #[test]
    fn solid_root_is_also_excluded() {
        let nodes = vec![
            node("root", NodeKind::Service, None, 1, true),
            node("pad", NodeKind::Interface, Some("root"), 1, true),
        ];
        let n = visual_noise(&projection(nodes, 0));
        assert_eq!(n.boxes, 0.0);
    }

    #[test]
    fn interface_pads_belong_to_solid_owners_only() {
        let nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("svc", NodeKind::Service, Some("root"), 1, true),
            node("pad", NodeKind::Interface, Some("svc"), 1, true),
            // An interface whose parent is a frame stands alone: no owner box.
            node("loose", NodeKind::Interface, Some("root"), 1, true),
        ];
        let n = visual_noise(&projection(nodes, 0));
        assert_eq!(n.boxes, 2.0, "svc and the loose interface");
    }

    #[test]
    fn frames_count_by_weight_and_collapsed_groups_as_solid_boxes() {
        let nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("open", NodeKind::Group, Some("root"), 1, false),
            node("inner", NodeKind::Service, Some("open"), 1, true),
            node("closed", NodeKind::Group, Some("root"), 0, true),
        ];
        let p = projection(nodes, 0);
        // The open frame is one box in the root; its member is in its own region.
        let n = visual_noise(&p);
        assert_eq!(n.boxes, 1.0 + FRAME_WEIGHT);
        assert_eq!(n.layers, BTreeMap::from([(0, 1.0), (1, FRAME_WEIGHT)]));
        let regions = region_noise(&p);
        assert_eq!(regions["open"].boxes, 1.0);
        assert_eq!(regions["open"].layers, BTreeMap::from([(1, 1.0)]));
    }

    #[test]
    fn external_references_are_boxes() {
        let mut nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("a", NodeKind::Service, Some("root"), 1, true),
            // A reference to an interface is still a box, not a pad.
            node(
                "view:reference:x",
                NodeKind::Interface,
                Some("root"),
                1,
                true,
            ),
            node("view:reference:y", NodeKind::Group, Some("root"), 1, true),
        ];
        let mut p = projection(std::mem::take(&mut nodes), 2);
        p.references = BTreeMap::from([
            ("view:reference:x".into(), "x".into()),
            ("view:reference:y".into(), "y".into()),
        ]);
        let n = visual_noise(&p);
        assert_eq!(n.boxes, 3.0);
    }

    #[test]
    fn interior_noise_excludes_references_and_edges_touching_them() {
        let nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("a", NodeKind::Service, Some("root"), 1, true),
            node("b", NodeKind::Datastore, Some("root"), 0, true),
            node("view:reference:x", NodeKind::Group, Some("root"), 1, true),
        ];
        let mut p = projection(nodes, 0);
        p.references = BTreeMap::from([("view:reference:x".into(), "x".into())]);
        let edge = |id: &str, source: &str, target: &str| Edge {
            bundle: None,
            id: id.into(),
            source: source.into(),
            target: target.into(),
            semantic_ids: vec![id.into()],
        };
        p.edges = vec![
            edge("inner", "a", "b"),
            edge("outer", "a", "view:reference:x"),
            edge("incoming", "view:reference:x", "b"),
            edge("rooted", "root", "a"),
        ];
        let inside = interior_noise(&p);
        assert_eq!(inside.boxes, 2.0);
        assert_eq!(inside.layers, BTreeMap::from([(0, 1.0), (1, 1.0)]));
        // The root frame draws a connector to its own child inside itself.
        assert_eq!(inside.connectors, 2);
        let whole = visual_noise(&p);
        assert_eq!(whole.boxes, 3.0);
        assert_eq!(whole.connectors, 4);
    }

    fn edges(p: &mut Projection, ends: &[(&str, &str)]) {
        p.edges = ends
            .iter()
            .enumerate()
            .map(|(i, (source, target))| Edge {
                bundle: None,
                id: format!("e{i}"),
                source: (*source).into(),
                target: (*target).into(),
                semantic_ids: vec![format!("e{i}")],
            })
            .collect();
    }

    /// root > (open frame `a` > `members` services, solid `b`)
    fn nested(members: usize) -> Projection {
        let mut nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("a", NodeKind::Scope, Some("root"), 1, false),
            node("b", NodeKind::Service, Some("root"), 1, true),
        ];
        for i in 0..members {
            nodes.push(node(
                &format!("a{i}"),
                NodeKind::Service,
                Some("a"),
                1,
                true,
            ));
        }
        projection(nodes, 0)
    }

    #[test]
    fn an_expanded_interior_counts_as_one_box_in_its_parent_region() {
        for members in [3, 20, 60] {
            let p = nested(members);
            let regions = region_noise(&p);
            assert_eq!(regions.len(), 2, "the root and the one expanded frame");
            let root = &regions["root"];
            assert_eq!(root.boxes, 1.0 + FRAME_WEIGHT, "the frame and b only");
            assert_eq!(root.layers, BTreeMap::from([(1, 2.0)]));
            assert_eq!(root.total, 0.0);
            assert_eq!(regions["a"].boxes, members as f64);
            assert_eq!(regions["a"].layers, BTreeMap::from([(1, members as f64)]));
            // The view is the focused root region.
            assert_eq!(visual_noise(&p), *root);
        }
        let dense = nested(60);
        assert!(!visual_noise(&dense).over_threshold());
        assert!(region_noise(&dense)["a"].over_threshold());
    }

    #[test]
    fn a_nested_frame_is_a_region_of_its_own_and_counts_once_in_its_parent() {
        // root > a > (inner > 4 services, 2 services), b
        let mut p = nested(2);
        p.nodes
            .push(node("inner", NodeKind::Scope, Some("a"), 1, false));
        for i in 0..4 {
            p.nodes.push(node(
                &format!("i{i}"),
                NodeKind::Service,
                Some("inner"),
                1,
                true,
            ));
        }
        let regions = region_noise(&p);
        assert_eq!(
            regions.keys().map(String::as_str).collect::<Vec<_>>(),
            ["a", "inner", "root"]
        );
        assert_eq!(regions["root"].boxes, 2.0);
        assert_eq!(regions["a"].boxes, 2.0 + FRAME_WEIGHT);
        assert_eq!(regions["inner"].boxes, 4.0);
    }

    #[test]
    fn a_connector_counts_in_the_lowest_region_holding_both_ends() {
        let mut p = nested(3);
        p.nodes
            .push(node("c", NodeKind::Scope, Some("root"), 1, false));
        p.nodes
            .push(node("c0", NodeKind::Service, Some("c"), 1, true));
        edges(
            &mut p,
            &[
                // Both inside a.
                ("a0", "a1"),
                // Across two sibling frames: drawn in the root.
                ("a0", "c0"),
                // From inside a frame to a solid sibling of the frame.
                ("a2", "b"),
                // Between solid siblings in the root.
                ("b", "b"),
                // Frame to a sibling frame.
                ("a", "c"),
            ],
        );
        let regions = region_noise(&p);
        assert_eq!(regions["a"].connectors, 1);
        assert_eq!(regions["c"].connectors, 0);
        assert_eq!(regions["root"].connectors, 4);
        assert_eq!(
            regions.values().map(|n| n.connectors).sum::<usize>(),
            p.edges.len(),
            "every connector counts exactly once"
        );
    }

    #[test]
    fn a_connector_from_a_frame_to_its_own_child_counts_in_that_frame() {
        let mut p = nested(2);
        edges(&mut p, &[("a", "a0"), ("a1", "a")]);
        let regions = region_noise(&p);
        assert_eq!(regions["a"].connectors, 2);
        assert_eq!(regions["root"].connectors, 0);
        // The root frame's own connector to a direct child lies in the root.
        edges(&mut p, &[("root", "a"), ("root", "b")]);
        assert_eq!(region_noise(&p)["root"].connectors, 2);
        // A connector from a child of the frame to a sibling of the frame is the
        // root's, not the frame's.
        edges(&mut p, &[("a0", "b")]);
        let regions = region_noise(&p);
        assert_eq!(
            (regions["a"].connectors, regions["root"].connectors),
            (0, 1)
        );
    }

    fn bundle_edge(id: &str, source: &str, target: &str, key: Option<&str>) -> Edge {
        Edge {
            bundle: key.map(Into::into),
            id: id.into(),
            source: source.into(),
            target: target.into(),
            semantic_ids: vec![id.into()],
        }
    }

    /// root > (frame `a` > a1..a3, frame `c` > c1, c2, solid `b`), edges bundled
    /// as the projection would key them.
    fn bundled() -> Projection {
        let mut p = nested(3);
        p.nodes
            .push(node("c", NodeKind::Scope, Some("root"), 1, false));
        p.nodes
            .push(node("c1", NodeKind::Service, Some("c"), 1, true));
        p.nodes
            .push(node("c2", NodeKind::Service, Some("c"), 1, true));
        p.edges = vec![
            bundle_edge("b1", "a0", "b", Some("a>b req")),
            bundle_edge("b2", "a1", "b", Some("a>b req")),
            bundle_edge("b3", "a2", "b", Some("a>b dep")),
            bundle_edge("x1", "a0", "c1", Some("a>c req")),
            bundle_edge("x2", "a1", "c2", Some("a>c req")),
            bundle_edge("x3", "a2", "c1", Some("a>c dep")),
        ];
        p
    }

    #[test]
    fn a_bundle_counts_one_trunk_and_one_branch_per_inner_end() {
        let regions = region_noise(&bundled());
        // Trunks, one per key, in the root: a>b req, a>b dep, a>c req, a>c dep.
        assert_eq!(regions["root"].connectors, 4);
        // Source-side branches in a: a0 and a1 for each request bundle, a2 for
        // each dependency bundle.
        assert_eq!(regions["a"].connectors, 2 + 1 + 2 + 1);
        // Target-side branches in c: c1 and c2 for the request bundle, c1 for
        // the dependency bundle. The edge to solid `b` has none.
        assert_eq!(regions["c"].connectors, 2 + 1);
    }

    #[test]
    fn a_branch_from_deeper_inside_the_outer_box_still_counts_in_it() {
        // a > (inner > i0), and i0 leaves a through the bundle.
        let mut p = nested(1);
        p.nodes
            .push(node("inner", NodeKind::Scope, Some("a"), 1, false));
        p.nodes
            .push(node("i0", NodeKind::Service, Some("inner"), 1, true));
        p.edges = vec![bundle_edge("e", "i0", "b", Some("a>b"))];
        let regions = region_noise(&p);
        assert_eq!(regions["root"].connectors, 1, "the trunk");
        assert_eq!(
            regions["a"].connectors, 1,
            "the branch ends on a's boundary"
        );
        assert_eq!(regions["inner"].connectors, 0);
    }

    #[test]
    fn a_pad_counts_as_its_owner_when_deduplicating_branches() {
        // a > a0 (solid, owns pad `p0`); both leave through the same bundle.
        let mut p = nested(1);
        p.nodes
            .push(node("p0", NodeKind::Interface, Some("a0"), 1, true));
        p.edges = vec![
            bundle_edge("e1", "a0", "b", Some("a>b")),
            bundle_edge("e2", "p0", "b", Some("a>b")),
        ];
        let regions = region_noise(&p);
        assert_eq!(regions["root"].connectors, 1);
        assert_eq!(regions["a"].connectors, 1, "a0 and its pad are one branch");
    }

    #[test]
    fn a_bundled_edge_from_the_outer_box_itself_has_no_branch_on_that_side() {
        // `a` is open; `b` reaches into it, `a` is the outer target.
        let mut p = nested(2);
        p.edges = vec![bundle_edge("e", "b", "a0", Some("b>a"))];
        let regions = region_noise(&p);
        assert_eq!(regions["root"].connectors, 1, "the trunk");
        assert_eq!(regions["a"].connectors, 1, "the target-side branch only");
    }

    #[test]
    fn opening_a_box_never_changes_its_parent_regions_connector_count() {
        use crate::projection::{ViewState, project};
        use visualyze_model::architecture::Architecture;
        let entity = |id: &str, kind: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","label":"{id}","boundary":"internal","basis":"declared"{extra}}}"#
            )
        };
        let relation = |id: &str, kind: &str, source: &str, target: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"}}"#
            )
        };
        let entities = [
            entity("system", "scope", r#","subtype":"system""#),
            entity("sa", "scope", ""),
            entity("sc", "scope", ""),
            entity("a1", "service", ""),
            entity("a2", "service", ""),
            entity("a3", "service", ""),
            entity("c1", "service", ""),
            entity("c2", "service", ""),
            entity("sb", "service", ""),
        ];
        let relations = [
            relation("own-sa", "contains", "system", "sa"),
            relation("own-sc", "contains", "system", "sc"),
            relation("own-sb", "contains", "system", "sb"),
            relation("own-a1", "contains", "sa", "a1"),
            relation("own-a2", "contains", "sa", "a2"),
            relation("own-a3", "contains", "sa", "a3"),
            relation("own-c1", "contains", "sc", "c1"),
            relation("own-c2", "contains", "sc", "c2"),
            relation("b1", "requests", "a1", "sb"),
            relation("b2", "requests", "a2", "sb"),
            relation("b3", "depends_on", "a3", "sb"),
            relation("x1", "requests", "a1", "c1"),
            relation("x2", "requests", "a2", "c2"),
            relation("x3", "depends_on", "a3", "c1"),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"bundles","revision":1,"system_id":"system","snapshot":{{"id":"bundle-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        let model = Architecture::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"));
        let view = |ids: &[&str]| {
            let mut view = ViewState::default();
            for id in ids {
                view.set_expanded(*id, true);
            }
            project(&model, &view).unwrap()
        };

        let collapsed = region_noise(&view(&[]));
        let opened = region_noise(&view(&["sa"]));
        let both = region_noise(&view(&["sa", "sc"]));
        let system = |regions: &BTreeMap<String, VisualNoise>| regions["system"].connectors;
        // Four lines: sa>sb and sa>sc, each as requests and depends_on.
        assert_eq!(system(&collapsed), 4);
        assert_eq!(system(&opened), 4, "opening sa leaves the parent unchanged");
        assert_eq!(system(&both), 4, "so does opening sc too");
        // Opening adds the branches to the box's own region: sa fans out to a1
        // and a2 (requests) and a3 (depends_on) toward sb, and again toward sc.
        assert_eq!(opened["sa"].connectors, 6);
        assert_eq!(both["sa"].connectors, 6);
        // sc fans in to c1 and c2 (requests) and c1 (depends_on).
        assert_eq!(both["sc"].connectors, 3);
        // Interior advice is the root region without references.
        let p = view(&["sa", "sc"]);
        assert_eq!(interior_noise(&p).connectors, 4);
        assert_eq!(interior_noise(&p), visual_noise(&p));
    }

    #[test]
    fn interface_pads_stay_with_their_owner_in_every_region() {
        // `s` is an expanded solid component with a pad and one real child.
        let mut p = projection(
            vec![
                node("root", NodeKind::Scope, None, 1, false),
                node("s", NodeKind::Service, Some("root"), 1, true),
                node("pad", NodeKind::Interface, Some("s"), 1, true),
                node("inside", NodeKind::Service, Some("s"), 1, true),
                node("peer", NodeKind::Service, Some("root"), 1, true),
            ],
            0,
        );
        edges(
            &mut p,
            &[("pad", "peer"), ("pad", "inside"), ("peer", "pad")],
        );
        let regions = region_noise(&p);
        assert_eq!(
            regions["root"].boxes, 2.0,
            "s and peer; the pad is not a box"
        );
        assert_eq!(regions["s"].boxes, 1.0, "inside only");
        // A pad is the owner: to a peer it is drawn in the root, to the
        // owner's own child inside the owner.
        assert_eq!(regions["root"].connectors, 2);
        assert_eq!(regions["s"].connectors, 1);
        // A component that holds only pads is not a region.
        let mut flat = p.clone();
        flat.nodes.retain(|node| node.id != "inside");
        edges(&mut flat, &[("pad", "peer")]);
        let regions = region_noise(&flat);
        assert_eq!(regions.len(), 1);
        assert_eq!(regions["root"].connectors, 1);
    }

    #[test]
    fn references_are_boxes_in_the_region_they_hang_on() {
        let mut p = nested(2);
        p.nodes.push(node(
            "view:reference:x",
            NodeKind::Group,
            Some("root"),
            1,
            true,
        ));
        p.nodes.push(node(
            "view:reference:y",
            NodeKind::Group,
            Some("a"),
            1,
            true,
        ));
        p.references = BTreeMap::from([
            ("view:reference:x".into(), "x".into()),
            ("view:reference:y".into(), "y".into()),
        ]);
        edges(
            &mut p,
            &[
                ("a0", "view:reference:x"),
                ("a1", "view:reference:y"),
                ("b", "view:reference:x"),
            ],
        );
        let regions = region_noise(&p);
        assert_eq!(regions["root"].boxes, 1.0 + FRAME_WEIGHT + 1.0);
        assert_eq!(regions["a"].boxes, 2.0 + 1.0);
        // a0 inside the frame to a reference outside it: the root's connector.
        assert_eq!(regions["root"].connectors, 2);
        assert_eq!(regions["a"].connectors, 1);
        // Left out, the references and the connectors that touch them vanish.
        let inside = interior_noise(&p);
        assert_eq!(inside.boxes, 1.0 + FRAME_WEIGHT);
        assert_eq!(inside.connectors, 0);
    }

    #[test]
    fn interior_noise_is_the_root_region_without_references() {
        // A view focused on a scope that holds an open frame, with references.
        let mut p = nested(40);
        p.nodes.push(node(
            "view:reference:x",
            NodeKind::Group,
            Some("root"),
            0,
            true,
        ));
        p.references = BTreeMap::from([("view:reference:x".into(), "x".into())]);
        edges(
            &mut p,
            &[("b", "a"), ("a5", "view:reference:x"), ("a0", "a1")],
        );
        let inside = interior_noise(&p);
        assert_eq!(
            inside.boxes, 2.0,
            "the frame counts once, its members never"
        );
        assert_eq!(inside.connectors, 1);
        // The same projection without the references scores identically as a region.
        let mut bare = p.clone();
        bare.nodes.retain(|node| node.id != "view:reference:x");
        bare.references.clear();
        edges(&mut bare, &[("b", "a"), ("a0", "a1")]);
        assert_eq!(region_noise(&bare)["root"], inside);
        assert_eq!(visual_noise(&bare), inside);
        assert_eq!(visual_noise(&p).boxes, 3.0, "the view adds its reference");
    }

    #[test]
    fn the_densest_region_and_the_advisories_name_where_the_noise_is() {
        let quiet = nested(3);
        assert_eq!(densest_region(&quiet).0, "root");
        assert_eq!(densest_interior(&quiet).unwrap().0, "a");
        assert!(advisories(&quiet).is_empty());
        let flat = projection(vec![node("root", NodeKind::Scope, None, 1, false)], 0);
        assert!(densest_interior(&flat).is_none());

        let dense = nested(40);
        let (id, noise) = densest_region(&dense);
        assert_eq!(id, "a");
        assert!(noise.over_threshold());
        assert_eq!(densest_interior(&dense).unwrap().0, "a");
        let advice = advisories(&dense);
        assert_eq!(advice.len(), 1, "{advice:?}");
        assert!(advice[0].starts_with("a: visual noise "), "{}", advice[0]);
        assert!(advice[0].contains("40 boxes"), "{}", advice[0]);
    }

    #[test]
    fn every_drawn_edge_is_one_connector_however_many_relations() {
        let mut p = projection(vec![node("root", NodeKind::Scope, None, 1, false)], 1);
        p.edges[0].semantic_ids = (0..50).map(|i| format!("r{i}")).collect();
        assert_eq!(visual_noise(&p).connectors, 1);
    }

    fn level(counts: &[(i32, usize)]) -> BTreeMap<i32, usize> {
        counts.iter().copied().collect()
    }

    #[test]
    fn level_noise_scores_layer_counts_and_ignores_connectors() {
        let n = level_noise(&level(&[(0, 14), (1, 16)]));
        assert_eq!(n.boxes, 30.0);
        assert_eq!(n.connectors, 0);
        assert_eq!(n.layers, BTreeMap::from([(0, 14.0), (1, 16.0)]));
        near(n.total, 1.2 + 2.8 + 11.1);
        // Empty layers carry no boxes.
        assert_eq!(level_noise(&level(&[(0, 0)])).total, 0.0);
    }

    #[test]
    fn single_layer_page_sizes_match_the_formula() {
        assert_eq!(layer_capacity(FALLBACK_NOISE), 24);
        assert_eq!(layer_capacity(NOISE_THRESHOLD), 15);
        let wide = level(&[(1, 500)]);
        assert_eq!(page_size_within(&wide, FALLBACK_NOISE), 24);
        assert_eq!(page_size_within(&wide, NOISE_THRESHOLD), 15);
        assert_eq!(level_noise(&level(&[(1, 24)])).total, 100.0);
        near(level_noise(&level(&[(1, 15)])).total, 6.25);
        assert!(level_noise(&level(&[(1, 16)])).total > NOISE_THRESHOLD);
    }

    #[test]
    fn a_level_that_fits_is_paged_whole() {
        assert_eq!(page_size_within(&level(&[(1, 24)]), FALLBACK_NOISE), 24);
        assert_eq!(
            page_size_within(&level(&[(0, 15), (1, 15)]), FALLBACK_NOISE),
            30
        );
        assert_eq!(page_size_within(&level(&[(1, 2)]), 0.0), 2);
        assert_eq!(page_size_within(&level(&[(1, 1)]), 0.0), 1);
        assert_eq!(page_size_within(&BTreeMap::new(), 0.0), 0);
    }

    #[test]
    fn ordered_pages_fit_even_when_the_level_is_sorted_into_runs() {
        // Evenly interleaved: every page matches the mix, so the size is the
        // proportional one.
        let mixed: Vec<i32> = (0..100).map(|i| i % 2).collect();
        let even = page_size_in_order(&mixed, FALLBACK_NOISE);
        assert_eq!(
            even,
            page_size_within(&level(&[(0, 50), (1, 50)]), FALLBACK_NOISE)
        );
        // The same mix sorted into two runs: a page may hold only one layer.
        let sorted: Vec<i32> = (0..100).map(|i| i32::from(i >= 50)).collect();
        let runs = page_size_in_order(&sorted, FALLBACK_NOISE);
        assert_eq!(runs, 24);
        for page in sorted.chunks(runs) {
            let mut counts = BTreeMap::new();
            for band in page {
                *counts.entry(*band).or_insert(0usize) += 1;
            }
            assert!(level_noise(&counts).total <= FALLBACK_NOISE);
        }
        // Small levels are returned whole.
        assert_eq!(page_size_in_order(&[1, 1, 1], FALLBACK_NOISE), 3);
        assert_eq!(page_size_in_order(&[], FALLBACK_NOISE), 0);
    }

    #[test]
    fn mixed_layers_scale_proportionally_and_round_up() {
        let mixed = level(&[(0, 40), (1, 60)]);
        let page = page_size_within(&mixed, FALLBACK_NOISE);
        assert!(page > 24 && page < 100, "{page}");
        let scaled = |p: usize| level(&[(0, (40 * p).div_ceil(100)), (1, (60 * p).div_ceil(100))]);
        assert!(level_noise(&scaled(page)).total <= FALLBACK_NOISE);
        assert!(level_noise(&scaled(page + 1)).total > FALLBACK_NOISE);
        // Deterministic.
        assert_eq!(page, page_size_within(&mixed, FALLBACK_NOISE));
        // A tighter limit never gives a bigger page.
        assert!(page_size_within(&mixed, NOISE_THRESHOLD) <= page);
    }

    #[test]
    fn projection_noise_matches_counts() {
        let mut nodes = vec![node("root", NodeKind::Scope, None, 1, false)];
        for i in 0..30 {
            nodes.push(node(
                &format!("n{i}"),
                NodeKind::Service,
                Some("root"),
                i32::from(i >= 14),
                true,
            ));
        }
        let n = visual_noise(&projection(nodes, 40));
        assert_eq!(n, counts(30.0, &[(0, 14.0), (1, 16.0)], 40));
    }
}
