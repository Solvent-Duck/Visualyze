//! Private decomposition into canonical placement frames, without solving them.
//!
//! A frame's `Projection` is a kernel input, not a semantic view. Expanded
//! assemblies appear as bounded solid proxies in their host frame and as their
//! original non-solid root in their own frame. All other supplied/display IDs,
//! dimensions and memberships are retained. Semantic inspection must continue
//! to use the original projection, never these routing/placement inputs.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};

use crate::{Edge, LayoutError, Node, NodeKind, Point, Projection};

pub const FRAME_SIZING_POLICY_VERSION: u32 = 1;
const MAX_NODES: usize = 1024;
const MAX_EDGES: usize = 4096;
const MAX_IDENTITY_BYTES: usize = 8 * 1024 * 1024;
/// Bound expanded per-relationship topology independently of input edge count.
pub const MAX_FRAME_STEPS: usize = 65_536;

#[derive(Debug, Clone)]
pub struct FramePlan {
    pub root: String,
    pub frames: BTreeMap<String, PlannedFrame>,
    pub edges: BTreeMap<String, PlannedEdge>,
    /// A non-solid endpoint belongs to its own frame. Other objects belong to
    /// their nearest non-solid ancestor, or the top canonical frame.
    pub node_frames: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct PlannedFrame {
    /// Hosting is private composition topology, not a new containment claim.
    pub parent_frame: Option<String>,
    pub child_frames: Vec<String>,
    pub projection: Projection,
    /// Exact kernel-input descriptor, not a complete solved-result cache key.
    /// Callers must also bind algorithm/profile/budget, pins, effective view
    /// context and fixed boundary terminals, then validate any cached result.
    pub cache_key: FrameCacheKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedEdge {
    pub original: Edge,
    pub lca_frame: String,
    pub source: EndpointPlan,
    pub target: EndpointPlan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointPlan {
    pub node_id: String,
    pub frame_id: String,
    /// Actual object or immediate child-frame proxy in the LCA kernel.
    pub representative: String,
    /// Ordered from the endpoint's own frame toward, but excluding, the LCA.
    pub steps: Vec<FrameStep>,
    /// The actual endpoint is a non-solid assembly surface. An external edge
    /// ends at that surface; do not invent an interior fragment in this frame.
    /// If this is the LCA, its real root surface remains the kernel endpoint.
    pub surface_endpoint: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameStep {
    pub frame_id: String,
    pub parent_frame: String,
    /// The instance in `parent_frame` has the same display ID as `frame_id`.
    pub proxy_id: String,
}

/// Allowed dimensions are inputs, not frozen winning dimensions: every kernel
/// may still search permitted rectangle proportions within this same domain.
/// Equality establishes the same local geometry/edge inputs only. It does not
/// establish compatible solve settings, pins, view context or boundary ports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameCacheKey {
    pub sizing_policy_version: u32,
    pub model_id: String,
    pub revision: u64,
    pub frame_id: String,
    pub parent_frame: Option<String>,
    pub nodes: Vec<NodeGeometry>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeGeometry {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub parent: Option<String>,
    pub kind: NodeKind,
    pub band: i32,
    pub solid: bool,
    pub min_size: Point,
    pub max_size: Point,
    pub preferred_size: Point,
}

/// Extract all bounded frame inputs and original-edge routing topology.
///
/// The top canonical frame always exists. In a focused forest its envelope is
/// independent of the object named by `projection.root`: that object may be a
/// solid service with attached interfaces and parentless outside references.
/// Other parentless non-solid objects are hosted by this top frame, preserving
/// `parent: None` on their proxies. Non-root frame roots are detached only in
/// their private kernel input; original ownership remains in the source view.
///
/// Only LCA kernel edges are inserted into frame projections. Boundary fragments
/// belong to the later compositor, not to a fake semantic endpoint or an extra
/// original relationship. No layout, coordinates or routing are produced here.
pub fn plan_frames(
    projection: &Projection,
    cancelled: &AtomicBool,
) -> Result<FramePlan, LayoutError> {
    check_cancel(cancelled)?;
    let nodes = validate_input(projection, cancelled)?;
    let mut topology_budget = TopologyBudget::default();
    let mut node_frames = BTreeMap::new();
    let mut parents = BTreeMap::from([(projection.root.clone(), None)]);
    for node in nodes.values() {
        check_cancel(cancelled)?;
        let owner = if !node.solid {
            node.id.as_str()
        } else {
            containing_frame(node, &nodes, &projection.root)
        };
        topology_budget.charge(&[&node.id, owner])?;
        node_frames.insert(node.id.clone(), owner.to_owned());
        if !node.solid && node.id != projection.root {
            topology_budget
                .charge(&[&node.id, containing_frame(node, &nodes, &projection.root)])?;
            parents.insert(
                node.id.clone(),
                Some(containing_frame(node, &nodes, &projection.root).to_owned()),
            );
        }
    }

    let mut kernels: BTreeMap<_, _> = parents
        .keys()
        .map(|id| {
            let mut root = nodes[id.as_str()].clone();
            root.parent = None;
            (id.clone(), vec![root])
        })
        .collect();
    for node in nodes.values() {
        check_cancel(cancelled)?;
        if node.id == projection.root {
            continue;
        }
        if !node.solid {
            let host = parents[&node.id].as_ref().unwrap();
            let mut proxy = (*node).clone();
            proxy.solid = true;
            kernels.get_mut(host).unwrap().push(proxy);
        } else {
            kernels
                .get_mut(&node_frames[&node.id])
                .unwrap()
                .push((*node).clone());
        }
    }

    let mut local_edges: BTreeMap<String, Vec<Edge>> =
        parents.keys().map(|id| (id.clone(), vec![])).collect();
    let mut edges = BTreeMap::new();
    let mut ordered_edges: Vec<_> = projection.edges.iter().collect();
    ordered_edges.sort_by(|a, b| a.id.cmp(&b.id));
    for edge in ordered_edges {
        check_cancel(cancelled)?;
        let source_frame = &node_frames[&edge.source];
        let target_frame = &node_frames[&edge.target];
        let lca = common_frame(source_frame, target_frame, &parents);
        topology_budget.charge(&[lca])?;
        let source = endpoint_plan(
            nodes[edge.source.as_str()],
            source_frame,
            lca,
            &parents,
            &mut topology_budget,
            cancelled,
        )?;
        let target = endpoint_plan(
            nodes[edge.target.as_str()],
            target_frame,
            lca,
            &parents,
            &mut topology_budget,
            cancelled,
        )?;
        let mut lifted = edge.clone();
        lifted.source.clone_from(&source.representative);
        lifted.target.clone_from(&target.representative);
        // Distinct original endpoints must remain distinct in their true LCA.
        if lifted.source == lifted.target {
            return Err(LayoutError::Invalid(format!(
                "frame extraction collapsed relationship {}",
                edge.id
            )));
        }
        local_edges.get_mut(lca).unwrap().push(lifted);
        edges.insert(
            edge.id.clone(),
            PlannedEdge {
                original: edge.clone(),
                lca_frame: lca.to_owned(),
                source,
                target,
            },
        );
    }

    let mut frames = BTreeMap::new();
    for (id, mut local_nodes) in kernels {
        check_cancel(cancelled)?;
        topology_budget.charge(&[&projection.model_id, &projection.model_id])?;
        local_nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let local_edges = local_edges.remove(&id).unwrap();
        let parent_frame = parents[&id].clone();
        let cache_key = FrameCacheKey::new(
            projection,
            &id,
            parent_frame.clone(),
            &local_nodes,
            &local_edges,
        );
        let kernel = Projection {
            key: cache_key.lookup_key(),
            model_id: projection.model_id.clone(),
            revision: projection.revision,
            root: id.clone(),
            nodes: local_nodes,
            edges: local_edges,
            relation_metadata: BTreeMap::new(),
            internal_relation_ids: vec![],
            semantic_to_display: BTreeMap::new(),
            suggested_groups: vec![],
            references: BTreeMap::new(),
            diagnostics: vec![],
        };
        let child_frames = parents
            .iter()
            .filter(|(_, parent)| parent.as_ref() == Some(&id))
            .map(|(child, _)| child.clone())
            .collect();
        frames.insert(
            id,
            PlannedFrame {
                parent_frame,
                child_frames,
                projection: kernel,
                cache_key,
            },
        );
    }
    check_cancel(cancelled)?;
    Ok(FramePlan {
        root: projection.root.clone(),
        frames,
        edges,
        node_frames,
    })
}

fn validate_input<'a>(
    projection: &'a Projection,
    cancelled: &AtomicBool,
) -> Result<BTreeMap<&'a str, &'a Node>, LayoutError> {
    if projection.nodes.is_empty() || projection.nodes.len() > MAX_NODES {
        return Err(LayoutError::Invalid(
            "frame projection must contain 1–1024 visible objects".into(),
        ));
    }
    if projection.edges.len() > MAX_EDGES {
        return Err(LayoutError::Invalid(
            "frame projection exceeds 4096 visible relationships".into(),
        ));
    }
    let mut identity_bytes: usize = 0;
    let mut charge = |value: &str| -> Result<(), LayoutError> {
        identity_bytes = identity_bytes
            .saturating_add(value.len())
            .saturating_add(std::mem::size_of::<String>());
        if identity_bytes > MAX_IDENTITY_BYTES {
            return Err(LayoutError::Invalid(
                "frame projection identity data exceeds 8 MiB".into(),
            ));
        }
        Ok(())
    };
    charge(&projection.model_id)?;
    charge(&projection.root)?;
    let mut nodes = BTreeMap::new();
    for node in &projection.nodes {
        check_cancel(cancelled)?;
        charge(&node.id)?;
        for id in &node.semantic_ids {
            charge(id)?;
        }
        if let Some(parent) = &node.parent {
            charge(parent)?;
        }
        if node.id.is_empty() || nodes.insert(node.id.as_str(), node).is_some() {
            return Err(LayoutError::Invalid("empty or duplicate display ID".into()));
        }
        validate_membership(&node.id, &node.semantic_ids)?;
        if [node.min_size, node.max_size, node.preferred_size]
            .iter()
            .any(|p| [p.x, p.y, p.z].iter().any(|v| !(1..=10_000).contains(v)))
            || node.preferred_size.min(node.min_size) != node.min_size
            || node.preferred_size.max(node.max_size) != node.max_size
            || !(-64..=64).contains(&node.band)
        {
            return Err(LayoutError::Invalid(format!(
                "invalid frame size range or band: {}",
                node.id
            )));
        }
    }
    let Some(root) = nodes.get(projection.root.as_str()) else {
        return Err(LayoutError::Invalid("missing frame projection root".into()));
    };
    if root.parent.is_some() {
        return Err(LayoutError::Invalid(
            "frame projection root has a parent".into(),
        ));
    }
    let parents_with_children: BTreeSet<_> = nodes
        .values()
        .filter_map(|node| node.parent.as_deref())
        .collect();
    for node in nodes.values() {
        check_cancel(cancelled)?;
        let mut current = *node;
        let mut seen = BTreeSet::from([current.id.as_str()]);
        while let Some(parent) = current.parent.as_deref() {
            check_cancel(cancelled)?;
            if !seen.insert(parent) {
                return Err(LayoutError::Invalid("cyclic display hierarchy".into()));
            }
            let Some(parent) = nodes.get(parent) else {
                return Err(LayoutError::Invalid(format!("missing parent {parent}")));
            };
            if parent.solid
                && (!current.solid
                    || current.kind != NodeKind::Interface
                    || parent.kind == NodeKind::Interface
                    || parents_with_children.contains(current.id.as_str()))
            {
                return Err(LayoutError::Invalid(format!(
                    "unsupported nesting under solid object {}",
                    parent.id
                )));
            }
            current = parent;
        }
    }
    let mut edge_ids = BTreeSet::new();
    for edge in &projection.edges {
        check_cancel(cancelled)?;
        for id in [&edge.id, &edge.source, &edge.target]
            .into_iter()
            .chain(edge.semantic_ids.iter())
        {
            charge(id)?;
        }
        validate_membership(&edge.id, &edge.semantic_ids)?;
        if edge.id.is_empty()
            || !edge_ids.insert(&edge.id)
            || !nodes.contains_key(edge.source.as_str())
            || !nodes.contains_key(edge.target.as_str())
            || edge.source == edge.target
        {
            return Err(LayoutError::Invalid(format!("invalid edge {}", edge.id)));
        }
    }
    Ok(nodes)
}

fn validate_membership(id: &str, members: &[String]) -> Result<(), LayoutError> {
    let mut seen = BTreeSet::new();
    if members.is_empty()
        || members
            .iter()
            .any(|member| member.is_empty() || !seen.insert(member))
    {
        Err(LayoutError::Invalid(format!(
            "empty or duplicate semantic membership: {id}"
        )))
    } else {
        Ok(())
    }
}

fn containing_frame<'a>(
    node: &'a Node,
    nodes: &BTreeMap<&str, &'a Node>,
    root: &'a str,
) -> &'a str {
    let mut parent = node.parent.as_deref();
    while let Some(id) = parent {
        let node = nodes[id];
        if !node.solid {
            return &node.id;
        }
        parent = node.parent.as_deref();
    }
    root
}

fn common_frame<'a>(
    source: &'a str,
    target: &'a str,
    parents: &'a BTreeMap<String, Option<String>>,
) -> &'a str {
    let mut ancestors = BTreeSet::new();
    let mut current = Some(source);
    while let Some(id) = current {
        ancestors.insert(id);
        current = parents[id].as_deref();
    }
    let mut current = target;
    while !ancestors.contains(current) {
        current = parents[current].as_deref().unwrap();
    }
    current
}

fn endpoint_plan(
    node: &Node,
    frame: &str,
    lca: &str,
    parents: &BTreeMap<String, Option<String>>,
    budget: &mut TopologyBudget,
    cancelled: &AtomicBool,
) -> Result<EndpointPlan, LayoutError> {
    let mut steps = Vec::new();
    let mut current = frame;
    while current != lca {
        check_cancel(cancelled)?;
        if budget.steps >= MAX_FRAME_STEPS {
            return Err(LayoutError::Invalid(format!(
                "frame route topology exceeds {MAX_FRAME_STEPS} boundary steps"
            )));
        }
        budget.steps += 1;
        let parent = parents[current].as_deref().unwrap();
        budget.charge(&[current, parent, current])?;
        steps.push(FrameStep {
            frame_id: current.into(),
            parent_frame: parent.into(),
            proxy_id: current.into(),
        });
        current = parent;
    }
    let representative = steps
        .last()
        .map_or(node.id.as_str(), |step| step.proxy_id.as_str());
    budget.charge(&[&node.id, frame, representative])?;
    Ok(EndpointPlan {
        node_id: node.id.clone(),
        frame_id: frame.into(),
        representative: representative.to_owned(),
        steps,
        surface_endpoint: !node.solid,
    })
}

#[derive(Default)]
struct TopologyBudget {
    steps: usize,
    identity_bytes: usize,
}

impl TopologyBudget {
    fn charge(&mut self, values: &[&str]) -> Result<(), LayoutError> {
        for value in values {
            self.identity_bytes = self
                .identity_bytes
                .saturating_add(value.len())
                .saturating_add(std::mem::size_of::<String>());
        }
        if self.identity_bytes > MAX_IDENTITY_BYTES {
            Err(LayoutError::Invalid(
                "expanded frame identity data exceeds 8 MiB".into(),
            ))
        } else {
            Ok(())
        }
    }
}

impl FrameCacheKey {
    fn new(
        original: &Projection,
        frame: &str,
        parent_frame: Option<String>,
        nodes: &[Node],
        edges: &[Edge],
    ) -> Self {
        let nodes = nodes
            .iter()
            .map(|node| {
                let mut semantic_ids = node.semantic_ids.clone();
                semantic_ids.sort();
                NodeGeometry {
                    id: node.id.clone(),
                    semantic_ids,
                    parent: node.parent.clone(),
                    kind: node.kind,
                    band: node.band,
                    solid: node.solid,
                    min_size: node.min_size,
                    max_size: node.max_size,
                    preferred_size: node.preferred_size,
                }
            })
            .collect();
        let edges = edges
            .iter()
            .map(|edge| {
                let mut edge = edge.clone();
                edge.semantic_ids.sort();
                edge
            })
            .collect();
        Self {
            sizing_policy_version: FRAME_SIZING_POLICY_VERSION,
            model_id: original.model_id.clone(),
            revision: original.revision,
            frame_id: frame.into(),
            parent_frame,
            nodes,
            edges,
        }
    }

    /// Stable length-delimited hashing of every descriptor field. This identity
    /// is private to frame kernels; the source projection key is not changed.
    pub fn lookup_key(&self) -> String {
        let mut hash = 0xcbf29ce484222325_u64;
        let mut write = |value: &str| {
            for byte in (value.len() as u64)
                .to_le_bytes()
                .iter()
                .chain(value.as_bytes())
            {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        };
        write(&self.sizing_policy_version.to_string());
        write(&self.model_id);
        write(&self.revision.to_string());
        write(&self.frame_id);
        write(self.parent_frame.as_deref().unwrap_or(""));
        write(&self.nodes.len().to_string());
        for node in &self.nodes {
            write(&node.id);
            write(node.parent.as_deref().unwrap_or(""));
            write(&format!("{:?}", node.kind));
            write(&node.band.to_string());
            write(if node.solid { "solid" } else { "frame" });
            for p in [node.min_size, node.max_size, node.preferred_size] {
                for coordinate in [p.x, p.y, p.z] {
                    write(&coordinate.to_string());
                }
            }
            write(&node.semantic_ids.len().to_string());
            for id in &node.semantic_ids {
                write(id);
            }
        }
        write(&self.edges.len().to_string());
        for edge in &self.edges {
            write(&edge.id);
            write(&edge.source);
            write(&edge.target);
            write(&edge.semantic_ids.len().to_string());
            for id in &edge.semantic_ids {
                write(id);
            }
        }
        format!("frame-kernel-v{FRAME_SIZING_POLICY_VERSION}-{hash:016x}")
    }
}

fn check_cancel(cancelled: &AtomicBool) -> Result<(), LayoutError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}
