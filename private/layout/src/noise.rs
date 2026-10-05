//! Visual noise: how much a projected view would show before layout (ADR 0007
//! section 3). It is a pure function of the projection, so it can be computed
//! without solving and never changes placement. Each term is free up to a soft
//! knee and then grows quadratically; no view is ever refused on this score.
//!
//! The knees are private tuning constants, not public contract.
use crate::{Node, NodeKind, Projection};
use serde::Serialize;
use std::collections::BTreeMap;

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

/// Box counts shared by every way of scoring a projection: total boxes and
/// boxes per layer. Counted as boxes: solid boxes, frames (`FRAME_WEIGHT` each),
/// and, when `include_references`, external references. Not counted: the focused
/// root, which is the view itself, and interface pads attached to a solid owner,
/// which belong to the owner.
fn count_boxes(projection: &Projection, include_references: bool) -> (f64, BTreeMap<i32, f64>) {
    let by_id: BTreeMap<&str, &Node> = projection
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node))
        .collect();
    let mut boxes = 0.0;
    let mut layers: BTreeMap<i32, f64> = BTreeMap::new();
    for node in &projection.nodes {
        let reference = projection.references.contains_key(&node.id);
        if reference {
            if !include_references {
                continue;
            }
        } else {
            if node.id == projection.root {
                continue;
            }
            let pad = node.kind == NodeKind::Interface
                && node
                    .parent
                    .as_deref()
                    .and_then(|parent| by_id.get(parent))
                    .is_some_and(|owner| owner.solid);
            if pad {
                continue;
            }
        }
        let weight = if node.solid { 1.0 } else { FRAME_WEIGHT };
        boxes += weight;
        *layers.entry(node.band).or_default() += weight;
    }
    (boxes, layers)
}

/// Score what a projection would draw: its boxes, including external
/// references, and every projected edge as one connector however many relations
/// it carries.
pub fn visual_noise(projection: &Projection) -> VisualNoise {
    let (boxes, layers) = count_boxes(projection, true);
    noise_from_counts(boxes, &layers, projection.edges.len())
}

/// Score only the interior of the projection's root: its direct children and
/// the connectors among them. External references, and any edge that touches a
/// reference or the root itself, belong to the surroundings and are left out.
/// The box rules are those of `visual_noise`.
pub fn interior_noise(projection: &Projection) -> VisualNoise {
    let (boxes, layers) = count_boxes(projection, false);
    let outside = |id: &String| *id == projection.root || projection.references.contains_key(id);
    let connectors = projection
        .edges
        .iter()
        .filter(|edge| !outside(&edge.source) && !outside(&edge.target))
        .count();
    noise_from_counts(boxes, &layers, connectors)
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
        let n = visual_noise(&projection(nodes, 0));
        assert_eq!(n.boxes, 2.0 * 1.0 + FRAME_WEIGHT);
        assert_eq!(n.layers, BTreeMap::from([(0, 1.0), (1, 2.0)]));
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
    fn interior_noise_excludes_references_and_edges_touching_them_or_the_root() {
        let nodes = vec![
            node("root", NodeKind::Scope, None, 1, false),
            node("a", NodeKind::Service, Some("root"), 1, true),
            node("b", NodeKind::Datastore, Some("root"), 0, true),
            node("view:reference:x", NodeKind::Group, Some("root"), 1, true),
        ];
        let mut p = projection(nodes, 0);
        p.references = BTreeMap::from([("view:reference:x".into(), "x".into())]);
        let edge = |id: &str, source: &str, target: &str| Edge {
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
        assert_eq!(inside.connectors, 1);
        let whole = visual_noise(&p);
        assert_eq!(whole.boxes, 3.0);
        assert_eq!(whole.connectors, 4);
    }

    #[test]
    fn every_drawn_edge_is_one_connector_however_many_relations() {
        let mut p = projection(vec![node("root", NodeKind::Scope, None, 1, false)], 1);
        p.edges[0].semantic_ids = (0..50).map(|i| format!("r{i}")).collect();
        assert_eq!(visual_noise(&p).connectors, 1);
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
