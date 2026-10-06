//! View-only hierarchy and abstraction. Nothing in this module changes a supplied fact.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use visualyze_model::architecture::{Architecture, EntityKind, Relation, RelationKind};

use crate::{
    Edge, LayoutError, Point,
    noise::{FALLBACK_NOISE, NOISE_THRESHOLD, layer_capacity, level_noise},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NodeKind {
    Scope,
    Client,
    Service,
    Datastore,
    Interface,
    Concern,
    Deployment,
    Group,
}

impl From<EntityKind> for NodeKind {
    fn from(kind: EntityKind) -> Self {
        match kind {
            EntityKind::Scope => Self::Scope,
            EntityKind::Client => Self::Client,
            EntityKind::Service => Self::Service,
            EntityKind::Datastore => Self::Datastore,
            EntityKind::Interface => Self::Interface,
            EntityKind::Concern => Self::Concern,
            EntityKind::Deployment => Self::Deployment,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: String,
    pub semantic_ids: Vec<String>,
    pub parent: Option<String>,
    pub kind: NodeKind,
    pub band: i32,
    pub min_size: Point,
    pub max_size: Point,
    pub preferred_size: Point,
    pub solid: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutPolicy {
    #[default]
    Crud,
    Pipeline,
    Peers,
    Code,
    Deployment,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayGroup {
    pub id: String,
    pub parent: String,
    /// Immediate supplied children, not a new architectural ownership claim.
    pub members: Vec<String>,
    pub suggested: bool,
}

#[derive(Debug, Clone)]
pub struct Projection {
    pub key: String,
    pub model_id: String,
    pub revision: u64,
    pub root: String,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Original records retain direction, qualifiers, evidence, and claim basis.
    pub relation_metadata: BTreeMap<String, Relation>,
    /// Relationships internal to a collapsed representation remain inspectable.
    pub internal_relation_ids: Vec<String>,
    pub semantic_to_display: BTreeMap<String, String>,
    pub suggested_groups: Vec<DisplayGroup>,
    /// Reference display ID -> supplied endpoint ID outside the focused assembly.
    pub references: BTreeMap<String, String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewState {
    pub focus: Option<String>,
    /// None chooses the semantic lens from the supplied model and focus.
    #[serde(default)]
    pub lens: Option<visualyze_model::provider::SemanticLens>,
    /// Effective lens of the current coordinate frame, including Auto mode.
    #[serde(default)]
    pub frame_lens: Option<visualyze_model::provider::SemanticLens>,
    #[serde(default)]
    pub child_page: usize,
    #[serde(default)]
    pub relation_cursor: Option<visualyze_model::provider::PageCursor>,
    #[serde(default)]
    pub relation_history: Vec<Option<visualyze_model::provider::PageCursor>>,
    #[serde(default = "semantic_zoom_default")]
    pub semantic_zoom: bool,
    pub expanded: BTreeSet<String>,
    pub collapsed: BTreeSet<String>,
    pub pins: BTreeMap<String, Point>,
    /// Pin coordinates belong to a focus frame. Keys are `root` or `f:<focus>`;
    /// string keys keep persisted JSON unambiguous and backwards compatible.
    #[serde(default)]
    pub frame_pins: BTreeMap<String, BTreeMap<String, Point>>,
    pub layer_preferences: BTreeMap<String, i32>,
    pub group_overrides: Vec<DisplayGroup>,
    #[serde(default)]
    pub dismissed_groups: BTreeSet<String>,
    pub policy: LayoutPolicy,
    /// Legacy, ignored by projection (ADR 0007): the peer ceiling was replaced
    /// by noise-driven paging and fallback grouping. Kept so that saved views
    /// that carry it still load, and still written so older builds can read
    /// what this one saves.
    #[serde(default = "legacy_peer_ceiling")]
    pub peer_ceiling: usize,
    /// Legacy, ignored by projection (ADR 0007); see `peer_ceiling`.
    #[serde(default = "legacy_group_target")]
    pub group_target: usize,
    pub visible_budget: usize,
    breadcrumbs: Vec<Option<String>>,
    auto_expanded: BTreeSet<String>,
    retained_groups: Vec<DisplayGroup>,
    /// Boxes the detail planner (`crate::detail`) opened on its own working
    /// copy of a view. It is never serialized and a user's view never holds it:
    /// the user's explicit and zoom choices stay exactly as made, and
    /// `collapsed` always takes precedence over it.
    #[serde(skip)]
    planned: BTreeSet<String>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            focus: None,
            lens: None,
            frame_lens: None,
            child_page: 0,
            relation_cursor: None,
            relation_history: vec![],
            semantic_zoom: true,
            expanded: BTreeSet::new(),
            collapsed: BTreeSet::new(),
            pins: BTreeMap::new(),
            frame_pins: BTreeMap::new(),
            layer_preferences: BTreeMap::new(),
            group_overrides: Vec::new(),
            dismissed_groups: BTreeSet::new(),
            policy: LayoutPolicy::Crud,
            peer_ceiling: legacy_peer_ceiling(),
            group_target: legacy_group_target(),
            visible_budget: 250,
            breadcrumbs: Vec::new(),
            auto_expanded: BTreeSet::new(),
            retained_groups: Vec::new(),
            planned: BTreeSet::new(),
        }
    }
}

fn semantic_zoom_default() -> bool {
    true
}

fn legacy_peer_ceiling() -> usize {
    64
}

fn legacy_group_target() -> usize {
    32
}

impl ViewState {
    /// Capture the current frame before entering or leaving a subsystem.
    /// Empty frames need no registry entry; no nonempty override is evicted.
    pub fn save_frame_pins(&mut self) {
        let key = self.frame_key();
        if self.pins.is_empty() {
            self.frame_pins.remove(&key);
        } else {
            self.frame_pins.insert(key, self.pins.clone());
        }
    }

    /// Restore coordinates only for the currently selected focus frame.
    pub fn restore_frame_pins(&mut self) {
        self.pins = self
            .frame_pins
            .get(&self.frame_key())
            .cloned()
            .unwrap_or_default();
    }

    fn frame_key(&self) -> String {
        let frame = self
            .focus
            .as_ref()
            .map_or_else(|| "root".into(), |id| format!("f:{id}"));
        match self.lens.or(self.frame_lens) {
            None => frame,
            Some(visualyze_model::provider::SemanticLens::Logical) => format!("l:logical|{frame}"),
            Some(visualyze_model::provider::SemanticLens::Code) => format!("l:code|{frame}"),
            Some(visualyze_model::provider::SemanticLens::Deployment) => {
                format!("l:deployment|{frame}")
            }
        }
    }

    /// A default view with a different hard visible-object budget, the only
    /// readability-independent limit left (ADR 0007).
    pub fn with_visible_budget(visible_budget: usize) -> Self {
        Self {
            visible_budget,
            ..Self::default()
        }
    }

    pub fn enter(&mut self, id: impl Into<String>) {
        let id = id.into();
        if self.focus.as_ref() != Some(&id) {
            self.breadcrumbs.push(self.focus.take());
            self.focus = Some(id);
            self.child_page = 0;
            self.reset_relation_pages();
        }
    }

    pub fn back(&mut self) -> bool {
        if let Some(focus) = self.breadcrumbs.pop() {
            self.focus = focus;
            self.child_page = 0;
            self.reset_relation_pages();
            true
        } else {
            false
        }
    }

    pub fn breadcrumbs(&self) -> &[Option<String>] {
        &self.breadcrumbs
    }

    pub fn reset_focus(&mut self) {
        self.focus = None;
        self.breadcrumbs.clear();
        self.child_page = 0;
        self.reset_relation_pages();
    }

    /// Call after accepting a projection to avoid regrouping unchanged members.
    pub fn remember_groups(&mut self, projection: &Projection) {
        for group in projection
            .suggested_groups
            .iter()
            .filter(|group| group.suggested)
        {
            if self.dismissed_groups.contains(&group.id) {
                continue;
            }
            self.retained_groups.retain(|old| old.id != group.id);
            self.retained_groups.push(group.clone());
        }
        if self.retained_groups.len() > 64 {
            self.retained_groups
                .drain(..self.retained_groups.len() - 64);
        }
    }

    pub fn retained_suggestions(&self) -> &[DisplayGroup] {
        &self.retained_groups
    }

    /// Refresh reconciled membership without making offscreen groups recent.
    /// Only displayed groups should change recency through `remember_groups`;
    /// refreshing the entire catalog as recent can evict the view just visited.
    pub fn refresh_group_definitions(&mut self, catalog: &[DisplayGroup]) {
        for retained in &mut self.retained_groups {
            if let Some(current) = catalog
                .iter()
                .find(|group| group.id == retained.id && group.suggested)
            {
                retained.clone_from(current);
            }
        }
    }

    /// Seed pin-bearing presentation frames immediately before projecting.
    /// Supplied ancestry is checked separately by the caller; these definitions
    /// remain visual groups, never new semantic parents. Identical manual
    /// overrides are preserved, but removed/dismissed or corrected groups are
    /// conflicts rather than instructions to undo the user's correction.
    ///
    /// Protected suggestions come first in stable ID order. Remaining slots
    /// retain the newest ordinary suggestions in their existing LRU order.
    /// An ordinary suggestion overlapping a protected/manual group is stale and
    /// is discarded whole, so later projection cannot consume protected members.
    /// No state changes on error. Call again after `remember_groups`: protection
    /// is supplied by the durable pin registry, not another persistent LRU here.
    pub fn protect_groups(&mut self, protected: &[DisplayGroup]) -> Result<(), LayoutError> {
        const LIMIT: usize = 64;
        fn canonical(group: &DisplayGroup) -> Result<DisplayGroup, LayoutError> {
            let valid_id = |id: &str| !id.is_empty() && id.len() <= 4096;
            if !valid_id(&group.id)
                || !valid_id(&group.parent)
                || group.id == group.parent
                || !(2..=128).contains(&group.members.len())
                || group
                    .members
                    .iter()
                    .any(|id| !valid_id(id) || id == &group.id || id == &group.parent)
            {
                return Err(LayoutError::Invalid(format!(
                    "invalid protected visual group definition: {}",
                    group.id
                )));
            }
            let mut result = group.clone();
            result.members.sort();
            if result.members.windows(2).any(|pair| pair[0] == pair[1]) {
                return Err(LayoutError::Invalid(format!(
                    "visual group {} contains duplicate members",
                    group.id
                )));
            }
            Ok(result)
        }
        let conflict = |id: &str| {
            LayoutError::Invalid(format!(
                "protected visual group {id} conflicts with an existing group correction; constraints retained"
            ))
        };
        if self.group_overrides.len() > LIMIT || self.retained_groups.len() > LIMIT {
            return Err(LayoutError::Invalid(
                "visual group state exceeds its 64-group bounds".into(),
            ));
        }
        let mut manual = BTreeMap::new();
        for group in &self.group_overrides {
            let group = canonical(group)?;
            if group.suggested || manual.insert(group.id.clone(), group.clone()).is_some() {
                return Err(conflict(&group.id));
            }
        }
        let mut required = BTreeMap::new();
        for group in protected {
            let group = canonical(group)?;
            if required
                .insert(group.id.clone(), group.clone())
                .is_some_and(|old| old != group)
            {
                return Err(conflict(&group.id));
            }
            if required.len() > LIMIT {
                return Err(LayoutError::Invalid(
                    "at most 64 distinct protected visual groups are supported".into(),
                ));
            }
            if self.dismissed_groups.contains(&group.id) {
                return Err(LayoutError::Invalid(format!(
                    "protected visual group {} was explicitly dismissed; constraints retained",
                    group.id
                )));
            }
            match manual.get(&group.id) {
                Some(existing) if existing != &group => return Err(conflict(&group.id)),
                None if !group.suggested => {
                    return Err(LayoutError::Invalid(format!(
                        "protected manual group {} was removed; constraints retained",
                        group.id
                    )));
                }
                _ => {}
            }
        }
        let suggestions: Vec<_> = required.values().filter(|g| g.suggested).cloned().collect();
        if manual.len() + suggestions.len() > LIMIT {
            return Err(LayoutError::Invalid(
                "protected and manual visual groups exceed the shared 64-group budget".into(),
            ));
        }
        let mut occupied = BTreeSet::new();
        for group in manual.values().chain(&suggestions) {
            for member in &group.members {
                if !occupied.insert(member.as_str()) {
                    return Err(LayoutError::Invalid(format!(
                        "protected/manual visual groups overlap at {member}; constraints retained"
                    )));
                }
            }
        }
        let mut ordinary = Vec::new();
        for retained in &self.retained_groups {
            let group = canonical(retained)?;
            if !group.suggested || manual.contains_key(&group.id) {
                return Err(conflict(&group.id));
            }
            if let Some(expected) = required.get(&group.id) {
                if expected != &group {
                    return Err(conflict(&group.id));
                }
                continue;
            }
            if self.dismissed_groups.contains(&group.id)
                || group
                    .members
                    .iter()
                    .any(|id| occupied.contains(id.as_str()))
            {
                continue;
            }
            if let Some(index) = ordinary
                .iter()
                .position(|g: &DisplayGroup| g.id == group.id)
            {
                if ordinary[index] != group {
                    return Err(conflict(&group.id));
                }
                ordinary.remove(index);
            }
            ordinary.push(group);
        }
        let available = LIMIT - manual.len() - suggestions.len();
        ordinary.drain(..ordinary.len().saturating_sub(available));
        let mut merged = suggestions;
        merged.extend(ordinary);
        self.retained_groups = merged;
        Ok(())
    }

    /// Set one object's detail preference; this state has no semantic tree and
    /// does not clear descendant preferences. If raw flags collapse a visual
    /// group while expanding a member, projection keeps that group open and
    /// reports why, independent of flag insertion order. A user-facing group
    /// collapse must clear descendant preferences using the accepted view.
    pub fn set_expanded(&mut self, id: impl Into<String>, expanded: bool) {
        self.reset_relation_pages();
        let id = id.into();
        self.auto_expanded.remove(&id);
        self.planned.remove(&id);
        if expanded {
            self.collapsed.remove(&id);
            self.expanded.insert(id);
        } else {
            self.expanded.remove(&id);
            self.collapsed.insert(id);
        }
    }

    /// Separate thresholds prevent oscillation; explicit choices always win.
    pub fn update_zoom(&mut self, id: &str, projected_pixels: f64) -> bool {
        if !projected_pixels.is_finite()
            || self.expanded.contains(id)
            || self.collapsed.contains(id)
        {
            return false;
        }
        if projected_pixels >= 180.0 {
            self.auto_expanded.insert(id.to_owned())
        } else if projected_pixels <= 120.0 {
            self.auto_expanded.remove(id)
        } else {
            false
        }
    }

    pub fn is_expanded(&self, id: &str) -> bool {
        !self.collapsed.contains(id)
            && (self.expanded.contains(id)
                || self.auto_expanded.contains(id)
                || self.planned.contains(id))
    }

    pub fn expanded_ids(&self) -> BTreeSet<String> {
        self.expanded
            .union(&self.auto_expanded)
            .chain(&self.planned)
            .filter(|id| !self.collapsed.contains(*id))
            .cloned()
            .collect()
    }

    /// Open `id` for the detail planner's working copy only. An explicit
    /// collapse still wins, and the choice is not persisted.
    pub(crate) fn plan_open(&mut self, id: impl Into<String>) {
        self.planned.insert(id.into());
    }

    /// Boxes opened by the planner on this working copy.
    pub fn planned_ids(&self) -> &BTreeSet<String> {
        &self.planned
    }

    pub fn reset_relation_pages(&mut self) {
        self.relation_cursor = None;
        self.relation_history.clear();
    }

    /// Remove only a presentation override and its coordinate-frame state.
    /// Navigation entries resolve to its supplied parent after removal.
    pub fn remove_local_group(&mut self, id: &str) -> bool {
        let Some(index) = self.group_overrides.iter().position(|group| group.id == id) else {
            return false;
        };
        let group = self.group_overrides.remove(index);
        self.forget_group_frame(id, &group.parent);
        true
    }

    pub fn dismiss_suggestion(&mut self, group: &DisplayGroup) {
        self.dismissed_groups.insert(group.id.clone());
        self.retained_groups.retain(|old| old.id != group.id);
        self.forget_group_frame(&group.id, &group.parent);
    }

    fn forget_group_frame(&mut self, id: &str, parent: &str) {
        if self.focus.as_deref() == Some(id) {
            self.focus = Some(parent.to_owned());
        }
        for focus in &mut self.breadcrumbs {
            if focus.as_deref() == Some(id) {
                *focus = Some(parent.to_owned());
            }
        }
        self.breadcrumbs.dedup();
        while self.breadcrumbs.last() == Some(&self.focus) {
            self.breadcrumbs.pop();
        }
        self.pins.remove(id);
        self.frame_pins.remove(&format!("f:{id}"));
        for lens in ["logical", "code", "deployment"] {
            self.frame_pins.remove(&format!("l:{lens}|f:{id}"));
        }
        for pins in self.frame_pins.values_mut() {
            pins.remove(id);
        }
        self.layer_preferences.remove(id);
        self.expanded.remove(id);
        self.collapsed.remove(id);
        self.auto_expanded.remove(id);
        self.planned.remove(id);
        self.child_page = 0;
        self.reset_relation_pages();
    }
}

#[derive(Clone)]
struct Item {
    kind: NodeKind,
    parent: Option<String>,
    semantic_ids: Vec<String>,
}

/// Project a validated snapshot without reinterpreting any relation or evidence.
///
/// A level that is too noisy to read (ADR 0007: estimated noise above
/// `FALLBACK_NOISE`) gets suggested fallback groups; a snapshot has no child
/// paging, so grouping is the only way to bring such a level down.
pub fn project(model: &Architecture, view: &ViewState) -> Result<Projection, LayoutError> {
    project_with(model, view, true)
}

/// The projection exactly as supplied: no fallback grouping, whatever the
/// noise. Producer advice and comparison tools measure the structure the
/// producer gave.
pub fn project_supplied(model: &Architecture, view: &ViewState) -> Result<Projection, LayoutError> {
    project_with(model, view, false)
}

fn project_with(
    model: &Architecture,
    view: &ViewState,
    fallback_groups: bool,
) -> Result<Projection, LayoutError> {
    if view.visible_budget == 0 {
        return Err(LayoutError::Invalid("invalid presentation budgets".into()));
    }
    if view
        .layer_preferences
        .values()
        .any(|band| !(-16..=16).contains(band))
    {
        return Err(LayoutError::Invalid(
            "layer preferences must be between -16 and 16".into(),
        ));
    }
    let snapshot = model.snapshot();
    use visualyze_model::provider::SemanticLens;
    if view.lens == Some(SemanticLens::Code) {
        return Err(LayoutError::Invalid(
            "This snapshot has no code profile; choose Logical or load a code bundle.".into(),
        ));
    }
    let Hierarchy {
        mut items,
        supplied_items,
        structural_ids,
    } = supplied_hierarchy(snapshot, view.lens);
    let mut groups = Vec::new();
    let mut overrides = view.group_overrides.clone();
    overrides.sort_by(|a, b| a.id.cmp(&b.id));
    for mut group in overrides {
        if !items.contains_key(&group.parent) {
            continue;
        }
        group.suggested = false;
        group.members.sort();
        insert_group(&mut items, &group)?;
        groups.push(group);
    }
    for retained in &view.retained_groups {
        if view.dismissed_groups.contains(&retained.id) {
            continue;
        }
        let mut group = retained.clone();
        group.members.retain(|id| {
            items
                .get(id)
                .is_some_and(|item| item.parent.as_ref() == Some(&group.parent))
        });
        if group.members.len() >= 2
            && !items.contains_key(&group.id)
            && items.contains_key(&group.parent)
        {
            insert_group(&mut items, &group)?;
            groups.push(group);
        }
    }
    let relations: BTreeMap<_, _> = snapshot
        .relations
        .iter()
        .filter(|r| match view.lens {
            Some(SemanticLens::Logical) => {
                items.contains_key(&r.source) && items.contains_key(&r.target)
            }
            Some(SemanticLens::Deployment) => r.kind == RelationKind::DeployedIn,
            _ => true,
        })
        .map(|r| (r.id.clone(), r.clone()))
        .collect();
    let mut children = child_index(&items);
    let parents: Vec<_> = children.keys().cloned().collect();
    for parent in parents {
        let peers = children[&parent].clone();
        if !fallback_groups
            || !matches!(items[&parent].kind, NodeKind::Scope | NodeKind::Group)
            || level_noise(&peer_bands(&peers, &items, view)).total <= FALLBACK_NOISE
        {
            continue;
        }
        // Group so that one layer of groups reads at `NOISE_THRESHOLD`.
        let groups_wanted = layer_capacity(NOISE_THRESHOLD);
        for members in coarsen(
            &peers,
            &relations,
            peers.len().div_ceil(groups_wanted).max(2),
        ) {
            let id = fresh_id(&items, &format!("view:group:{}:{}", parent, members[0]));
            let group = DisplayGroup {
                id,
                parent: parent.clone(),
                members,
                suggested: true,
            };
            if view.dismissed_groups.contains(&group.id) {
                continue;
            }
            insert_group(&mut items, &group)?;
            groups.push(group);
        }
    }
    children = child_index(&items);
    let root = view.focus.as_ref().unwrap_or(&snapshot.system_id).clone();
    if !items.contains_key(&root) {
        return Err(LayoutError::Invalid(format!(
            "unknown focus or focus outside the selected semantic view: {root}"
        )));
    }
    let mut nodes = BTreeMap::new();
    let mut semantic_to_display = BTreeMap::new();
    visit(
        &root,
        None,
        &root,
        &items,
        &children,
        view,
        &mut nodes,
        &mut semantic_to_display,
    );
    let mut references: BTreeMap<String, String> = BTreeMap::new();
    let mut edge_groups: BTreeMap<(String, String, String, Option<String>), Vec<String>> =
        BTreeMap::new();
    let mut internal_relation_ids = Vec::new();
    let mut metadata = BTreeMap::new();
    for relation in relations.values() {
        if structural_ids.contains(&relation.id) {
            if semantic_to_display.contains_key(&relation.source)
                || semantic_to_display.contains_key(&relation.target)
            {
                metadata.insert(relation.id.clone(), relation.clone());
            }
            continue;
        }
        let source = semantic_to_display.get(&relation.source).cloned();
        let target = semantic_to_display.get(&relation.target).cloned();
        if source.is_none() && target.is_none() {
            continue;
        }
        metadata.insert(relation.id.clone(), relation.clone());
        let mut endpoint = |mapped: Option<String>, semantic: &str| -> String {
            if let Some(mapped) = mapped {
                return mapped;
            }
            if let Some((id, _)) = references
                .iter()
                .find(|(_, original)| *original == semantic)
            {
                return id.clone();
            }
            let mut id = format!("view:reference:{semantic}");
            while items.contains_key(&id) || nodes.contains_key(&id) {
                id.push(':');
            }
            let item = items.get(semantic).unwrap_or(&supplied_items[semantic]);
            let parent = (!nodes[&root].solid).then(|| root.clone());
            nodes.insert(id.clone(), make_node(&id, item, parent, true, view));
            references.insert(id.clone(), semantic.to_owned());
            id
        };
        let source = endpoint(source, &relation.source);
        let target = endpoint(target, &relation.target);
        if source == target {
            internal_relation_ids.push(relation.id.clone());
        } else {
            // Aggregate parallel relations of the same kind and aspect between
            // the same displayed endpoints into one connector (ADR 0007
            // section 4). Membership stays exact: the edge lists every
            // original relation ID and `metadata` keeps every record.
            edge_groups
                .entry((
                    source,
                    target,
                    format!("{:?}", relation.kind),
                    relation.aspect.clone(),
                ))
                .or_default()
                .push(relation.id.clone());
        }
    }
    let mut edges: Vec<Edge> = edge_groups
        .into_iter()
        .map(|((source, target, kind, aspect), mut semantic_ids)| {
            semantic_ids.sort();
            Edge {
                id: semantic_ids[0].clone(),
                bundle: bundle_key(&nodes, &source, &target, &kind, aspect.as_deref()),
                source,
                target,
                semantic_ids,
            }
        })
        .collect();
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    if view.policy == LayoutPolicy::Pipeline {
        rank_pipeline(&mut nodes, &edges);
    }
    for (id, band) in &view.layer_preferences {
        if let Some(node) = nodes.get_mut(id) {
            node.band = *band;
        }
    }
    let mut diagnostics = Vec::new();
    for group in &groups {
        if group.id != root
            && !view.is_expanded(&group.id)
            && nodes.get(&group.id).is_some_and(|node| !node.solid)
        {
            diagnostics.push(format!(
                "{}: opened this visual group to preserve an explicit member expansion; neighboring members keep their own detail level.",
                group.id
            ));
        }
    }
    if nodes.len() > view.visible_budget {
        diagnostics.push(format!(
            "{} visible objects exceed the {} target; explicit expansion and boundary references were preserved",
            nodes.len(), view.visible_budget
        ));
    }
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    let nodes: Vec<_> = nodes.into_values().collect();
    let key = projection_key(&root, &nodes, &edges, view);
    let mut projection = Projection {
        key,
        model_id: snapshot.model_id.clone(),
        revision: snapshot.revision,
        root,
        nodes,
        edges,
        relation_metadata: metadata,
        internal_relation_ids,
        semantic_to_display,
        suggested_groups: groups,
        references,
        diagnostics,
    };
    // Advisory only; diagnostics are not part of the projection key.
    projection
        .diagnostics
        .extend(crate::noise::advisories(&projection));
    Ok(projection)
}

/// The supplied ownership tree of one semantic view: every entity that belongs
/// to the lens with its supplied parent, before any display grouping.
struct Hierarchy {
    items: BTreeMap<String, Item>,
    /// Every supplied entity, whether or not the lens shows it.
    supplied_items: BTreeMap<String, Item>,
    structural_ids: BTreeSet<String>,
}

fn supplied_hierarchy(
    snapshot: &visualyze_model::architecture::Snapshot,
    lens: Option<visualyze_model::provider::SemanticLens>,
) -> Hierarchy {
    use visualyze_model::provider::SemanticLens;
    let deployment = lens == Some(SemanticLens::Deployment);
    let mut items: BTreeMap<String, Item> = snapshot
        .entities
        .iter()
        .map(|e| {
            (
                e.id.clone(),
                Item {
                    kind: e.kind.into(),
                    parent: None,
                    semantic_ids: vec![e.id.clone()],
                },
            )
        })
        .collect();
    let supplied_items = items.clone();
    items.retain(|id, item| match lens {
        Some(SemanticLens::Logical) => item.kind != NodeKind::Deployment,
        Some(SemanticLens::Deployment) => {
            item.kind == NodeKind::Deployment || id == &snapshot.system_id
        }
        _ => true,
    });
    let mut structural_ids = BTreeSet::new();
    let mut ordered_relations: Vec<_> = snapshot.relations.iter().collect();
    ordered_relations.sort_by(|a, b| a.id.cmp(&b.id));
    for relation in ordered_relations {
        if deployment {
            if relation.kind == RelationKind::DeployedIn
                && items
                    .get(&relation.source)
                    .is_some_and(|item| item.kind == NodeKind::Deployment && item.parent.is_none())
            {
                items.get_mut(&relation.source).unwrap().parent = Some(relation.target.clone());
                structural_ids.insert(relation.id.clone());
            }
        } else if items.contains_key(&relation.source)
            && items.contains_key(&relation.target)
            && matches!(
                relation.kind,
                RelationKind::Contains | RelationKind::Exposes
            )
        {
            items.get_mut(&relation.target).unwrap().parent = Some(relation.source.clone());
            structural_ids.insert(relation.id.clone());
        }
    }
    // Uncontained external objects and deployments share only a display envelope.
    for (id, item) in &mut items {
        if item.parent.is_none() && id != &snapshot.system_id {
            item.parent = Some(snapshot.system_id.clone());
        }
    }
    Hierarchy {
        items,
        supplied_items,
        structural_ids,
    }
}

/// For each supplied box of a semantic view, how many supplied children opening
/// it would reveal as structure. Interfaces on a component are pads on its own
/// face, so a client, service or datastore with only interfaces counts none.
pub(crate) fn hidden_structure_counts(
    model: &Architecture,
    lens: Option<visualyze_model::provider::SemanticLens>,
) -> BTreeMap<String, usize> {
    let hierarchy = supplied_hierarchy(model.snapshot(), lens);
    child_index(&hierarchy.items)
        .into_iter()
        .map(|(parent, children)| {
            let pads_only = matches!(
                hierarchy.items[&parent].kind,
                NodeKind::Client | NodeKind::Service | NodeKind::Datastore
            );
            let count = children
                .iter()
                .filter(|id| !pads_only || hierarchy.items[*id].kind != NodeKind::Interface)
                .count();
            (parent, count)
        })
        .collect()
}

/// Boxes per layer for the peers of one level, before nodes exist. This is an
/// estimate: it uses each peer's own layer under the view's policy and layer
/// preferences, and ignores layers a `Pipeline` ranking assigns from edges
/// later.
fn peer_bands(
    peers: &[String],
    items: &BTreeMap<String, Item>,
    view: &ViewState,
) -> BTreeMap<i32, usize> {
    let mut bands = BTreeMap::new();
    for id in peers {
        let band = view
            .layer_preferences
            .get(id)
            .copied()
            .unwrap_or_else(|| make_node(id, &items[id], None, true, view).band);
        *bands.entry(band).or_default() += 1;
    }
    bands
}

fn child_index(items: &BTreeMap<String, Item>) -> BTreeMap<String, Vec<String>> {
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, item) in items {
        if let Some(parent) = &item.parent {
            children.entry(parent.clone()).or_default().push(id.clone());
        }
    }
    children
}

fn fresh_id(items: &BTreeMap<String, Item>, proposed: &str) -> String {
    let mut id = proposed.to_owned();
    while items.contains_key(&id) {
        id.push(':');
    }
    id
}

fn insert_group(
    items: &mut BTreeMap<String, Item>,
    group: &DisplayGroup,
) -> Result<(), LayoutError> {
    if items.contains_key(&group.id)
        || !items.contains_key(&group.parent)
        || items
            .get(&group.parent)
            .is_some_and(|parent| !matches!(parent.kind, NodeKind::Scope | NodeKind::Group))
        || group.members.len() < 2
        || group.members.iter().collect::<BTreeSet<_>>().len() != group.members.len()
        || group.members.iter().any(|id| {
            items
                .get(id)
                .is_none_or(|item| item.parent.as_ref() != Some(&group.parent))
        })
    {
        return Err(LayoutError::Invalid(format!(
            "display group {} must contain distinct siblings and have a unique ID",
            group.id
        )));
    }
    let mut semantic_ids = Vec::new();
    for id in &group.members {
        let item = items.get_mut(id).unwrap();
        item.parent = Some(group.id.clone());
        semantic_ids.extend(item.semantic_ids.iter().cloned());
    }
    semantic_ids.sort();
    semantic_ids.dedup();
    items.insert(
        group.id.clone(),
        Item {
            kind: NodeKind::Group,
            parent: Some(group.parent.clone()),
            semantic_ids,
        },
    );
    Ok(())
}

fn coarsen(
    peers: &[String],
    relations: &BTreeMap<String, Relation>,
    target: usize,
) -> Vec<Vec<String>> {
    let mut remaining: BTreeSet<_> = peers.iter().cloned().collect();
    let mut degree: BTreeMap<&str, usize> = BTreeMap::new();
    let mut adjacency: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for r in relations.values().filter(|r| {
        !matches!(r.kind, RelationKind::Contains | RelationKind::Exposes)
            && remaining.contains(&r.source)
            && remaining.contains(&r.target)
    }) {
        adjacency.entry(&r.source).or_default().insert(&r.target);
        adjacency.entry(&r.target).or_default().insert(&r.source);
    }
    for (id, neighbors) in &adjacency {
        degree.insert(id, neighbors.len());
    }
    let mut groups = Vec::new();
    while let Some(seed) = remaining.pop_first() {
        let mut members = vec![seed];
        while members.len() < target && !remaining.is_empty() {
            // Inverse degree damps ubiquitous infrastructure's attraction.
            let next = remaining
                .iter()
                .map(|candidate| {
                    let score: u64 = members
                        .iter()
                        .filter(|member| {
                            adjacency
                                .get(member.as_str())
                                .is_some_and(|set| set.contains(candidate.as_str()))
                        })
                        .map(|member| {
                            1_000_000
                                / (degree.get(member.as_str()).copied().unwrap_or(1)
                                    * degree.get(candidate.as_str()).copied().unwrap_or(1))
                                .max(1) as u64
                        })
                        .sum();
                    (score, std::cmp::Reverse(candidate.clone()))
                })
                .max()
                .unwrap()
                .1
                .0;
            remaining.remove(&next);
            members.push(next);
        }
        members.sort();
        groups.push(members);
    }
    // A single leftover still needs an architectural home, but no one-member group.
    if groups.last().is_some_and(|group| group.len() == 1) && groups.len() > 1 {
        let last = groups.pop().unwrap().pop().unwrap();
        let previous = groups.last_mut().unwrap();
        if previous.len() > 2 {
            let moved = previous.pop().unwrap();
            groups.push(vec![last, moved]);
        } else {
            previous.push(last);
        }
    }
    groups
}

#[allow(clippy::too_many_arguments)]
fn visit(
    id: &str,
    display_parent: Option<String>,
    root: &str,
    items: &BTreeMap<String, Item>,
    children: &BTreeMap<String, Vec<String>>,
    view: &ViewState,
    nodes: &mut BTreeMap<String, Node>,
    mapping: &mut BTreeMap<String, String>,
) {
    let item = &items[id];
    let descendants = descendants(id, items, children);
    let expanded = id == root
        || view.is_expanded(id)
        || (item.kind == NodeKind::Group
            && descendants.iter().any(|member| view.is_expanded(member)));
    let mut node = make_node(id, item, display_parent, true, view);
    if matches!(item.kind, NodeKind::Scope | NodeKind::Group) && expanded {
        node.solid = false;
    }
    if expanded && node.solid {
        let interface_count = children.get(id).map_or(0, Vec::len) as i64;
        node.max_size.y = node.max_size.y.max(interface_count * 12);
    }
    if expanded {
        // A visual group has no identity of its own; its children carry the mappings.
        if item.kind != NodeKind::Group {
            mapping.insert(id.to_owned(), id.to_owned());
        }
    } else {
        node.semantic_ids = descendants.clone();
        for semantic in descendants {
            mapping.insert(semantic, id.to_owned());
        }
    }
    nodes.insert(id.to_owned(), node);
    if expanded {
        for child in children.get(id).into_iter().flatten() {
            visit(
                child,
                Some(id.to_owned()),
                root,
                items,
                children,
                view,
                nodes,
                mapping,
            );
        }
    }
}

fn descendants(
    id: &str,
    items: &BTreeMap<String, Item>,
    children: &BTreeMap<String, Vec<String>>,
) -> Vec<String> {
    let mut result = BTreeSet::new();
    let mut stack = vec![id];
    while let Some(id) = stack.pop() {
        result.extend(items[id].semantic_ids.iter().cloned());
        stack.extend(children.get(id).into_iter().flatten().map(String::as_str));
    }
    result.into_iter().collect()
}

fn make_node(id: &str, item: &Item, parent: Option<String>, solid: bool, view: &ViewState) -> Node {
    let preferred_size = match item.kind {
        NodeKind::Concern => Point { x: 28, y: 4, z: 24 },
        NodeKind::Interface => Point { x: 8, y: 4, z: 8 },
        _ => Point {
            x: 24,
            y: 12,
            z: 20,
        },
    };
    let band = match view.policy {
        LayoutPolicy::Crud => match item.kind {
            NodeKind::Concern | NodeKind::Client => 2,
            NodeKind::Datastore => 0,
            _ => 1,
        },
        LayoutPolicy::Deployment => i32::from(item.kind != NodeKind::Deployment),
        _ => 0,
    };
    Node {
        id: id.to_owned(),
        semantic_ids: item.semantic_ids.clone(),
        parent,
        kind: item.kind,
        band,
        min_size: Point {
            x: preferred_size.x / 2,
            y: preferred_size.y,
            z: preferred_size.z / 2,
        },
        max_size: Point {
            x: preferred_size.x * 2,
            y: preferred_size.y * 2,
            z: preferred_size.z * 2,
        },
        preferred_size,
        solid,
    }
}

/// Reachability-equivalent components receive one rank, without reversing edges.
pub(crate) fn rank_pipeline(nodes: &mut BTreeMap<String, Node>, edges: &[Edge]) {
    let ids: Vec<_> = nodes.keys().cloned().collect();
    let mut reach: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        adjacency
            .entry(&edge.source)
            .or_default()
            .push(&edge.target);
    }
    for id in &ids {
        let mut reached = BTreeSet::new();
        let mut stack = vec![id.as_str()];
        while let Some(next) = stack.pop() {
            if reached.insert(next.to_owned()) {
                stack.extend(adjacency.get(next).into_iter().flatten().copied());
            }
        }
        reach.insert(id.clone(), reached);
    }
    let mut components: BTreeMap<String, String> = BTreeMap::new();
    for id in &ids {
        let representative = ids
            .iter()
            .find(|other| reach[id].contains(*other) && reach[*other].contains(id))
            .unwrap();
        components.insert(id.clone(), representative.clone());
    }
    let mut ranks: BTreeMap<String, i32> = components.values().map(|id| (id.clone(), 0)).collect();
    for _ in 0..ranks.len() {
        let mut changed = false;
        for edge in edges {
            let source = &components[&edge.source];
            let target = &components[&edge.target];
            if source != target && ranks[source] <= ranks[target] {
                ranks.insert(source.clone(), ranks[target] + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (id, node) in nodes {
        node.band = ranks[&components[id]].min(16);
    }
}

/// Non-solid display ancestors of `id`, innermost first, starting with `id`
/// itself when it is not solid. This is the router's `enclosure_chain`: solid
/// boxes (including interface owners) are never enclosures, because their
/// exposed pads already carry the attachment.
fn display_enclosures<'a>(nodes: &'a BTreeMap<String, Node>, id: &str) -> Vec<&'a str> {
    let mut chain = Vec::new();
    let mut next = nodes.get(id);
    while let Some(node) = next {
        if !node.solid {
            chain.push(node.id.as_str());
        }
        next = node.parent.as_deref().and_then(|parent| nodes.get(parent));
    }
    chain
}

/// The bundle key of a projected edge (see `Edge::bundle`), or `None` when it
/// crosses no expanded box boundary below the lowest common enclosure.
///
/// `S'` is the enclosure directly below the common one on the source side (or
/// the source itself when it already sits there); `T'` likewise. When the ends
/// share no enclosure, the outermost enclosure on each side is used. The key is
/// the projection's aggregation key with the endpoints replaced by `S'`/`T'`
/// (see `encode_bundle_key` for the exact encoding).
pub(crate) fn bundle_key(
    nodes: &BTreeMap<String, Node>,
    source: &str,
    target: &str,
    kind: &str,
    aspect: Option<&str>,
) -> Option<String> {
    let sources = display_enclosures(nodes, source);
    let targets = display_enclosures(nodes, target);
    let common_id = sources.iter().copied().find(|id| targets.contains(id));
    let below = |chain: &[&str], end: &str| -> String {
        let above = common_id
            .and_then(|id| chain.iter().position(|item| *item == id))
            .unwrap_or(chain.len());
        above
            .checked_sub(1)
            .map_or(end, |index| chain[index])
            .to_owned()
    };
    let outer_source = below(&sources, source);
    let outer_target = below(&targets, target);
    if outer_source == source && outer_target == target {
        return None;
    }
    Some(encode_bundle_key(
        &outer_source,
        &outer_target,
        kind,
        aspect,
    ))
}

/// Encodes `(S', T', kind, aspect)` as `bundle|<len>:<S'>|<len>:<T'>|<len>:<kind>|-`
/// (no aspect) or `...|+<len>:<aspect>`. Every field is length-prefixed, so
/// distinct tuples never collide.
pub(crate) fn encode_bundle_key(
    source: &str,
    target: &str,
    kind: &str,
    aspect: Option<&str>,
) -> String {
    let field = |value: &str| format!("{}:{value}", value.len());
    format!(
        "bundle|{}|{}|{}|{}",
        field(source),
        field(target),
        field(kind),
        aspect.map_or_else(|| "-".to_owned(), |aspect| format!("+{}", field(aspect)))
    )
}

fn projection_key(root: &str, nodes: &[Node], edges: &[Edge], view: &ViewState) -> String {
    // Length-delimited data avoids ambiguous concatenation. FNV is stable across processes.
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
    write(root);
    write(&format!("{:?}:{:?}", view.policy, view.lens));
    for node in nodes {
        write(&node.id);
        write(node.parent.as_deref().unwrap_or(""));
        write(&format!("{}:{}:{:?}", node.band, node.solid, node.kind));
        for semantic in &node.semantic_ids {
            write(semantic);
        }
    }
    for edge in edges {
        write(&edge.id);
        write(&edge.source);
        write(&edge.target);
        write(edge.bundle.as_deref().unwrap_or(""));
    }
    for (id, pin) in &view.pins {
        write(id);
        write(&format!("{},{},{}", pin.x, pin.y, pin.z));
    }
    format!("projection-v1-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Architecture {
        Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json")).unwrap()
    }

    /// The notes example with 25 more services in its scope: 26 peers share one
    /// layer, so the level is noisy enough (above `FALLBACK_NOISE`) for
    /// suggested fallback groups.
    fn wide_notes() -> Architecture {
        Architecture::from_json(&serde_json::to_vec(&wide_notes_json()).unwrap()).unwrap()
    }

    fn wide_notes_json() -> serde_json::Value {
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        for i in 0..25 {
            let id = format!("service:extra:{i:02}");
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
        value
    }

    fn protected_group(index: usize, suggested: bool) -> DisplayGroup {
        DisplayGroup {
            id: format!("view:group:{index:03}"),
            parent: "scope:parent".into(),
            members: vec![format!("member:{index}:a"), format!("member:{index}:b")],
            suggested,
        }
    }

    fn assert_protection_conflict(mut view: ViewState, groups: &[DisplayGroup], message: &str) {
        let before = serde_json::to_value(&view).unwrap();
        let error = view.protect_groups(groups).unwrap_err().to_string();
        assert!(error.contains(message), "{error}");
        assert_eq!(serde_json::to_value(view).unwrap(), before);
    }

    #[test]
    fn catalog_refresh_changes_membership_without_promoting_offscreen_groups() {
        let mut view = ViewState::default();
        let first = protected_group(1, true);
        let mut changed = protected_group(2, true);
        let last = protected_group(3, true);
        view.retained_groups = vec![first.clone(), changed.clone(), last.clone()];
        changed.members.push("added-member".into());
        let unrelated = protected_group(4, true);
        view.refresh_group_definitions(&[last.clone(), unrelated, changed.clone(), first.clone()]);
        assert_eq!(view.retained_groups, vec![first, changed, last]);
    }

    #[test]
    fn protected_groups_reserve_slots_and_reseed_after_ordinary_lru_eviction() {
        let manual = protected_group(100, false);
        let old = protected_group(0, true);
        let evicted = protected_group(200, true);
        let mut view = ViewState {
            group_overrides: vec![manual.clone()],
            retained_groups: (0..64).map(|i| protected_group(i, true)).collect(),
            ..Default::default()
        };
        view.protect_groups(&[evicted.clone(), old.clone()])
            .unwrap();
        assert_eq!(view.group_overrides, std::slice::from_ref(&manual));
        assert_eq!(view.retained_groups.len(), 63);
        assert_eq!(&view.retained_groups[..2], &[old.clone(), evicted.clone()]);
        assert_eq!(view.retained_groups[2], protected_group(3, true));
        assert_eq!(
            view.retained_groups.last(),
            Some(&protected_group(63, true))
        );
        let seeded = serde_json::to_value(&view).unwrap();
        view.protect_groups(&[old.clone(), evicted.clone()])
            .unwrap();
        assert_eq!(serde_json::to_value(&view).unwrap(), seeded);

        let mut accepted = project(&notes(), &ViewState::default()).unwrap();
        accepted.suggested_groups = (300..364).map(|i| protected_group(i, true)).collect();
        view.remember_groups(&accepted);
        assert!(!view.retained_groups.iter().any(|g| g.id == old.id));
        view.protect_groups(&[old.clone(), evicted.clone()])
            .unwrap();
        assert_eq!(&view.retained_groups[..2], &[old, evicted]);
        assert_eq!(view.retained_groups.len() + view.group_overrides.len(), 64);
        assert_eq!(view.retained_groups[2], protected_group(303, true));
        assert_eq!(view.group_overrides, [manual]);
    }

    #[test]
    fn protected_merge_is_order_independent_and_preserves_manual_and_nested_metadata() {
        let mut manual = protected_group(4, false);
        manual.members.reverse();
        let outer = protected_group(1, true);
        let mut nested = protected_group(2, true);
        nested.parent = outer.members[0].clone();
        let mut a = ViewState::default();
        a.group_overrides.push(manual.clone());
        let mut b = a.clone();
        a.protect_groups(&[outer.clone(), manual.clone(), nested.clone()])
            .unwrap();
        let mut reversed = outer.clone();
        reversed.members.reverse();
        b.protect_groups(&[nested.clone(), reversed, manual.clone(), outer.clone()])
            .unwrap();
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert_eq!(a.group_overrides, [manual]);
        assert_eq!(a.retained_suggestions(), &[outer, nested]);
        assert!(a.retained_suggestions().iter().all(|group| group.suggested));
    }

    #[test]
    fn protected_groups_do_not_reverse_explicit_corrections() {
        let manual = protected_group(1, false);
        let suggested = protected_group(2, true);
        assert_protection_conflict(
            ViewState::default(),
            std::slice::from_ref(&manual),
            "was removed",
        );
        let mut view = ViewState::default();
        view.group_overrides.push(manual.clone());
        for changed in [
            DisplayGroup {
                parent: "other-parent".into(),
                ..manual.clone()
            },
            DisplayGroup {
                members: vec!["other:a".into(), "other:b".into()],
                ..manual.clone()
            },
            DisplayGroup {
                suggested: true,
                ..manual.clone()
            },
        ] {
            assert_protection_conflict(view.clone(), &[changed], "correction");
        }
        view.retained_groups.push(suggested.clone());
        let changed = DisplayGroup {
            members: vec!["new:a".into(), "new:b".into()],
            ..suggested.clone()
        };
        assert_protection_conflict(view.clone(), std::slice::from_ref(&changed), "correction");
        assert_protection_conflict(
            ViewState::default(),
            &[suggested.clone(), changed],
            "correction",
        );
        view.dismissed_groups.insert(suggested.id.clone());
        assert_protection_conflict(view, &[suggested], "explicitly dismissed");
    }

    #[test]
    fn protected_overlap_evicts_only_ordinary_suggestions_and_rejects_owned_conflicts() {
        let protected = protected_group(1, true);
        let mut stale = protected_group(2, true);
        stale.members[0] = protected.members[0].clone();
        let unrelated = protected_group(3, true);
        let mut view = ViewState {
            retained_groups: vec![stale.clone(), unrelated.clone()],
            ..Default::default()
        };
        view.protect_groups(std::slice::from_ref(&protected))
            .unwrap();
        assert_eq!(view.retained_suggestions(), &[protected.clone(), unrelated]);
        assert_protection_conflict(
            ViewState::default(),
            &[protected.clone(), stale.clone()],
            "overlap",
        );
        stale.suggested = false;
        let mut view = ViewState::default();
        view.group_overrides.push(stale);
        assert_protection_conflict(view, &[protected], "overlap");
    }

    #[test]
    fn protected_bounds_and_invalid_definitions_are_transactional() {
        let view = ViewState {
            group_overrides: (0..64).map(|i| protected_group(i, false)).collect(),
            ..Default::default()
        };
        assert_protection_conflict(view, &[protected_group(64, true)], "shared 64-group budget");
        let groups: Vec<_> = (0..65).map(|i| protected_group(i, true)).collect();
        assert_protection_conflict(ViewState::default(), &groups, "64 distinct");
        let mut view = ViewState::default();
        view.protect_groups(&groups[..64]).unwrap();
        assert_eq!(view.retained_suggestions().len(), 64);
        // Repeated identical definitions consume one protected slot.
        view.protect_groups(&vec![groups[0].clone(); 65]).unwrap();
        assert_eq!(view.retained_suggestions().len(), 64);
        for members in [
            vec!["only-one".into()],
            vec!["duplicate".into(); 2],
            (0..129).map(|i| format!("member:{i}")).collect(),
        ] {
            let invalid = DisplayGroup {
                members,
                ..protected_group(90, true)
            };
            assert_protection_conflict(view.clone(), &[invalid], "group");
        }
    }

    #[test]
    fn planned_openings_yield_to_explicit_choices_and_are_never_saved() {
        let mut view = ViewState::default();
        view.plan_open("scope:a");
        view.plan_open("scope:b");
        assert!(view.is_expanded("scope:a"));
        assert!(view.expanded_ids().contains("scope:b"));
        // An explicit collapse wins over a planned opening.
        view.set_expanded("scope:a", false);
        assert!(!view.is_expanded("scope:a"));
        assert!(!view.expanded_ids().contains("scope:a"));
        assert!(!view.planned_ids().contains("scope:a"));
        // An explicit expansion takes it over, so the planner no longer owns it.
        view.set_expanded("scope:b", true);
        assert!(view.expanded.contains("scope:b") && view.planned_ids().is_empty());
        view.plan_open("scope:c");
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("planned") && !json.contains("scope:c"));
        let loaded: ViewState = serde_json::from_str(&json).unwrap();
        assert!(loaded.planned_ids().is_empty() && !loaded.is_expanded("scope:c"));
        let mut raw: serde_json::Value = serde_json::from_str(&json).unwrap();
        raw["planned"] = serde_json::json!(["scope:c"]);
        assert!(serde_json::from_value::<ViewState>(raw).is_err());
    }

    #[test]
    fn protected_snapshot_suggestion_keeps_its_supplied_members_after_eviction() {
        let mut view = ViewState::default();
        let first = project(&wide_notes(), &view).unwrap();
        let protected = first.suggested_groups.clone();
        assert!(!protected.is_empty());
        view.retained_groups = (0..64).map(|i| protected_group(i, true)).collect();
        view.protect_groups(&protected).unwrap();
        let next = project(&wide_notes(), &view).unwrap();
        assert_eq!(next.semantic_to_display, first.semantic_to_display);
        let mut expected = first.suggested_groups.clone();
        for group in &mut expected {
            group.members.sort();
        }
        assert_eq!(next.suggested_groups, expected);
        assert_eq!(next.key, first.key);
    }

    #[test]
    fn snapshot_lenses_separate_logical_and_deployment_forests() {
        use visualyze_model::provider::SemanticLens;
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        for id in ["deployment:global", "deployment:region"] {
            value["entities"].as_array_mut().unwrap().push(serde_json::json!({
                "id": id, "kind": "deployment", "label": id, "boundary": "internal", "basis": "declared"
            }));
        }
        for (id, source, target) in [
            ("deploy:region", "deployment:region", "deployment:global"),
            ("deploy:service", "service:notes", "deployment:region"),
        ] {
            value["relations"].as_array_mut().unwrap().push(serde_json::json!({
                "id": id, "source": source, "target": target, "kind": "deployed_in", "basis": "declared"
            }));
        }
        let model = Architecture::from_json(&serde_json::to_vec(&value).unwrap()).unwrap();
        let mut view = ViewState {
            lens: Some(SemanticLens::Logical),
            ..Default::default()
        };
        let logical = project(&model, &view).unwrap();
        assert!(
            logical
                .nodes
                .iter()
                .all(|node| node.kind != NodeKind::Deployment)
        );
        assert!(!logical.relation_metadata.contains_key("deploy:service"));
        view.lens = Some(SemanticLens::Deployment);
        let deployment = project(&model, &view).unwrap();
        assert!(
            deployment
                .nodes
                .iter()
                .any(|node| node.id == "deployment:global")
        );
        assert!(
            !deployment
                .nodes
                .iter()
                .any(|node| node.id == "service:notes")
        );
        assert!(
            deployment
                .references
                .values()
                .any(|id| id == "service:notes")
        );
        assert_eq!(
            deployment.relation_metadata["deploy:service"].source,
            "service:notes"
        );
        view.enter("deployment:global");
        let nested = project(&model, &view).unwrap();
        assert_eq!(
            nested
                .nodes
                .iter()
                .find(|node| node.id == "deployment:region")
                .unwrap()
                .parent
                .as_deref(),
            Some("deployment:global")
        );
        assert!(nested.relation_metadata.contains_key("deploy:region"));
        assert!(nested.edges.iter().all(|edge| edge.id != "deploy:region"));
        view.lens = Some(SemanticLens::Code);
        assert!(project(&model, &view).is_err());
    }

    #[test]
    fn collapsed_interfaces_keep_direction_predicates_and_evidence() {
        let p = project(&notes(), &ViewState::default()).unwrap();
        assert_eq!(p.semantic_to_display["interface:notes"], "service:notes");
        let edge = p
            .edges
            .iter()
            .find(|e| e.id == "rel:request-notes")
            .unwrap();
        assert_eq!(edge.source, "client:web");
        assert_eq!(edge.target, "service:notes");
        assert_eq!(p.relation_metadata[&edge.id].target, "interface:notes");
        assert!(p.edges.iter().any(|e| e.id == "rel:read-database"));
        assert!(p.edges.iter().any(|e| e.id == "rel:write-database"));
    }

    #[test]
    fn focus_preserves_boundary_relationships_and_breadcrumbs() {
        let mut view = ViewState::default();
        view.enter("service:notes");
        let p = project(&notes(), &view).unwrap();
        assert!(p.nodes.iter().any(|n| n.id == "interface:notes"));
        assert!(p.references.values().any(|id| id == "client:web"));
        assert!(p.edges.iter().any(|e| e.id == "rel:request-notes"));
        assert!(view.back());
        assert!(view.focus.is_none());
        assert!(!view.back());
    }

    #[test]
    fn zoom_hysteresis_and_explicit_choices_are_stable() {
        let mut view = ViewState::default();
        assert!(view.update_zoom("a", 190.0));
        assert!(!view.update_zoom("a", 150.0));
        assert!(view.is_expanded("a"));
        assert!(view.update_zoom("a", 110.0));
        view.set_expanded("a", true);
        assert!(!view.update_zoom("a", 10.0));
        assert!(view.is_expanded("a"));
        view.set_expanded("a", false);
        assert!(!view.update_zoom("a", 500.0));
        assert!(!view.is_expanded("a"));
    }

    #[test]
    fn group_overrides_cannot_move_members_across_architectural_boundaries() {
        let mut view = ViewState::default();
        view.group_overrides.push(DisplayGroup {
            id: "visual-only".into(),
            parent: "scope:notes".into(),
            members: vec!["interface:notes".into(), "client:web".into()],
            suggested: false,
        });
        assert!(project(&notes(), &view).is_err());
    }

    /// `sub` holds three services that each read an outside datastore, one of
    /// which also writes to it. An outside concern applies to all three, under
    /// two aspects.
    fn aggregation_model() -> Architecture {
        let entity = |id: &str, kind: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","label":"{id}","boundary":"internal","basis":"declared"{extra}}}"#
            )
        };
        let relation = |id: &str, kind: &str, source: &str, target: &str, extra: &str| {
            format!(
                r#"{{"id":"{id}","kind":"{kind}","source":"{source}","target":"{target}","basis":"declared"{extra}}}"#
            )
        };
        let entities = [
            entity("system", "scope", r#","subtype":"system""#),
            entity("sub", "scope", ""),
            entity("svc0", "service", ""),
            entity("svc1", "service", ""),
            entity("svc2", "service", ""),
            entity("store", "datastore", ""),
            entity("auth", "concern", ""),
        ];
        let relations = [
            relation("own-sub", "contains", "system", "sub", ""),
            relation("own-store", "contains", "system", "store", ""),
            relation("own-svc0", "contains", "sub", "svc0", ""),
            relation("own-svc1", "contains", "sub", "svc1", ""),
            relation("own-svc2", "contains", "sub", "svc2", ""),
            relation("read0", "reads", "svc0", "store", ""),
            relation("read1", "reads", "svc1", "store", ""),
            relation("read2", "reads", "svc2", "store", ""),
            relation("own-auth", "contains", "system", "auth", ""),
            relation(
                "cover0",
                "applies_to",
                "auth",
                "svc0",
                r#","aspect":"sign_in""#,
            ),
            relation(
                "cover1",
                "applies_to",
                "auth",
                "svc1",
                r#","aspect":"sign_in""#,
            ),
            relation(
                "cover2",
                "applies_to",
                "auth",
                "svc2",
                r#","aspect":"audit""#,
            ),
            relation("write0", "writes", "svc0", "store", ""),
            relation("peer", "requests", "svc0", "svc1", ""),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"aggregation","revision":1,"system_id":"system","snapshot":{{"id":"agg-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        Architecture::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn membership(p: &Projection) -> Vec<String> {
        let mut ids: Vec<String> = p
            .edges
            .iter()
            .flat_map(|edge| edge.semantic_ids.iter().cloned())
            .chain(p.internal_relation_ids.iter().cloned())
            .collect();
        ids.sort();
        ids
    }

    #[test]
    fn collapsed_connectors_aggregate_by_endpoints_kind_and_aspect() {
        let model = aggregation_model();
        let collapsed = project(&model, &ViewState::default()).unwrap();
        let edge = |id: &str| {
            collapsed
                .edges
                .iter()
                .find(|edge| edge.id == id)
                .unwrap_or_else(|| panic!("no edge {id}: {:#?}", collapsed.edges))
        };
        // Same endpoints, kind and aspect: one connector, smallest ID, sorted members.
        assert_eq!(edge("read0").semantic_ids, ["read0", "read1", "read2"]);
        assert_eq!(
            (edge("read0").source.as_str(), edge("read0").target.as_str()),
            ("sub", "store")
        );
        // A different kind stays separate, and so does a different aspect.
        assert_eq!(edge("write0").semantic_ids, ["write0"]);
        assert_eq!(edge("cover0").semantic_ids, ["cover0", "cover1"]);
        assert_eq!(edge("cover2").semantic_ids, ["cover2"]);
        assert_eq!(collapsed.edges.len(), 4);
        // `peer` is inside `sub`, so it is an internal record, not a line.
        assert_eq!(collapsed.internal_relation_ids, ["peer"]);
    }

    #[test]
    fn expanding_changes_the_visible_line_count_but_never_semantic_membership() {
        let model = aggregation_model();
        let mut view = ViewState::default();
        let collapsed = project(&model, &view).unwrap();
        view.set_expanded("sub", true);
        let expanded = project(&model, &view).unwrap();
        assert!(expanded.edges.len() > collapsed.edges.len());
        assert_eq!(expanded.edges.len(), 8);
        assert_eq!(expanded.internal_relation_ids.len(), 0);
        let all = [
            "cover0", "cover1", "cover2", "peer", "read0", "read1", "read2", "write0",
        ];
        assert_eq!(membership(&collapsed), all);
        assert_eq!(membership(&expanded), all);
        // Every record is still resident for inspection in both views.
        assert_eq!(
            collapsed.relation_metadata.len(),
            model.snapshot().relations.len()
        );
        assert_eq!(
            expanded.relation_metadata.len(),
            model.snapshot().relations.len()
        );
        // Edge IDs are unique and each lists a relation at most once.
        for p in [&collapsed, &expanded] {
            let ids: BTreeSet<_> = p.edges.iter().map(|edge| &edge.id).collect();
            assert_eq!(ids.len(), p.edges.len());
        }
    }

    #[test]
    fn grouping_and_input_order_do_not_change_semantic_membership() {
        let mut view = ViewState::default();
        let p = project(&wide_notes(), &view).unwrap();
        assert!(!p.suggested_groups.is_empty());
        assert!(p.suggested_groups.iter().all(|g| g.suggested));
        assert_eq!(
            p.semantic_to_display.len(),
            wide_notes().snapshot().entities.len()
        );
        assert_eq!(p.key, project(&wide_notes(), &view).unwrap().key);
        // Connectors aggregate parallel relations, so membership (not the
        // edge count) must account for every supplied non-structural relation.
        let count = p
            .edges
            .iter()
            .map(|edge| edge.semantic_ids.len())
            .sum::<usize>()
            + p.internal_relation_ids.len();
        assert_eq!(
            count,
            p.relation_metadata
                .values()
                .filter(|relation| !matches!(
                    relation.kind,
                    RelationKind::Contains | RelationKind::Exposes
                ))
                .count()
        );
        assert_eq!(
            p.relation_metadata.len(),
            wide_notes().snapshot().relations.len()
        );
        let mut json = wide_notes_json();
        json["entities"].as_array_mut().unwrap().reverse();
        json["relations"].as_array_mut().unwrap().reverse();
        let reordered = Architecture::from_json(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(p.key, project(&reordered, &view).unwrap().key);
        view.remember_groups(&p);
        assert_eq!(
            p.suggested_groups,
            project(&reordered, &view).unwrap().suggested_groups
        );
    }

    #[test]
    fn explicit_port_expansion_preserves_owner_solidity() {
        let mut view = ViewState::default();
        view.set_expanded("service:notes", true);
        let p = project(&notes(), &view).unwrap();
        assert!(
            p.nodes
                .iter()
                .find(|n| n.id == "service:notes")
                .unwrap()
                .solid
        );
        assert_eq!(p.semantic_to_display["interface:notes"], "interface:notes");
        assert_eq!(
            p.nodes
                .iter()
                .find(|n| n.id == "interface:notes")
                .unwrap()
                .parent
                .as_deref(),
            Some("service:notes")
        );
    }

    #[test]
    fn grouped_snapshot_members_preserve_explicit_expansion_without_opening_siblings() {
        let base = ViewState::default();
        let collapsed = project(&wide_notes(), &base).unwrap();
        for mode in 0..3 {
            let mut view = base.clone();
            if mode == 1 {
                view.remember_groups(&collapsed);
            } else if mode == 2 {
                view.group_overrides.push(DisplayGroup {
                    id: "view:local-group:notes".into(),
                    parent: "scope:notes".into(),
                    members: vec!["service:notes".into(), "client:web".into()],
                    suggested: false,
                });
            }
            view.set_expanded("service:notes", true);
            let projection = project(&wide_notes(), &view).unwrap();
            assert_eq!(
                projection.semantic_to_display["interface:notes"],
                "interface:notes"
            );
            let group = projection
                .suggested_groups
                .iter()
                .find(|group| group.members.iter().any(|member| member == "service:notes"))
                .unwrap();
            assert!(
                !projection
                    .nodes
                    .iter()
                    .find(|node| node.id == group.id)
                    .unwrap()
                    .solid
            );
            assert!(
                projection
                    .nodes
                    .iter()
                    .filter(|node| node.kind == NodeKind::Interface)
                    .all(|node| node.parent.as_deref() == Some("service:notes"))
            );
            assert!(projection.diagnostics.iter().any(|note| {
                note.contains(&group.id) && note.contains("explicit member expansion")
            }));
        }
    }

    #[test]
    fn raw_group_collapse_and_member_expansion_flags_have_documented_precedence() {
        let group = DisplayGroup {
            id: "view:local-group:notes".into(),
            parent: "scope:notes".into(),
            members: vec!["service:notes".into(), "client:web".into()],
            suggested: false,
        };
        for collapse_first in [false, true] {
            let mut view = ViewState::default();
            view.group_overrides.push(group.clone());
            if collapse_first {
                view.set_expanded(&group.id, false);
                view.set_expanded("service:notes", true);
            } else {
                view.set_expanded("service:notes", true);
                view.set_expanded(&group.id, false);
            }
            assert!(!view.is_expanded(&group.id));
            assert!(view.is_expanded("service:notes"));
            let projection = project(&notes(), &view).unwrap();
            assert!(
                !projection
                    .nodes
                    .iter()
                    .find(|node| node.id == group.id)
                    .unwrap()
                    .solid
            );
            assert_eq!(
                projection.semantic_to_display["interface:notes"],
                "interface:notes"
            );
            assert!(projection.diagnostics.iter().any(|note| {
                note.contains(&group.id) && note.contains("explicit member expansion")
            }));
            // The interaction layer knows the tree and normalizes descendants
            // before collapsing their group. The raw state setter does not.
            view.set_expanded("service:notes", false);
            let collapsed = project(&notes(), &view).unwrap();
            assert!(
                collapsed
                    .nodes
                    .iter()
                    .find(|node| node.id == group.id)
                    .unwrap()
                    .solid
            );
            assert_eq!(collapsed.semantic_to_display["interface:notes"], group.id);
        }
    }

    #[test]
    fn layer_preferences_are_bounded_and_part_of_projection_identity() {
        let mut view = ViewState::default();
        let original = project(&notes(), &view).unwrap().key;
        view.layer_preferences.insert("client:web".into(), 3);
        assert_ne!(original, project(&notes(), &view).unwrap().key);
        view.layer_preferences.insert("client:web".into(), i32::MIN);
        assert!(project(&notes(), &view).is_err());
    }
    /// System with scope `sa` (a1..a3, a1 exposing interface `ia`), scope `sc`
    /// (c1, c2), outside service `sb` and outside client `sk`.
    fn bundle_model() -> Architecture {
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
            entity("sk", "client", ""),
            entity("ia", "interface", r#","subtype":"api""#),
        ];
        let relations = [
            relation("own-sa", "contains", "system", "sa"),
            relation("own-sc", "contains", "system", "sc"),
            relation("own-sb", "contains", "system", "sb"),
            relation("own-sk", "contains", "system", "sk"),
            relation("own-a1", "contains", "sa", "a1"),
            relation("own-a2", "contains", "sa", "a2"),
            relation("own-a3", "contains", "sa", "a3"),
            relation("own-c1", "contains", "sc", "c1"),
            relation("own-c2", "contains", "sc", "c2"),
            relation("expose", "exposes", "a1", "ia"),
            relation("b1", "requests", "a1", "sb"),
            relation("b2", "requests", "a2", "sb"),
            relation("b3", "depends_on", "a3", "sb"),
            relation("x1", "requests", "a1", "c1"),
            relation("x2", "requests", "a2", "c2"),
            relation("x3", "depends_on", "a3", "c1"),
            relation("pad", "requests", "sk", "ia"),
        ];
        let json = format!(
            r#"{{"schema_version":"0.1.0-draft.1","model_id":"bundles","revision":1,"system_id":"system","snapshot":{{"id":"bundle-001","source_revision":null,"coverage":"partial","scope_description":"test","omissions":[]}},"entities":[{}],"relations":[{}],"evidence":[]}}"#,
            entities.join(","),
            relations.join(",")
        );
        Architecture::from_json(json.as_bytes()).unwrap_or_else(|e| panic!("{e}"))
    }

    fn expanded_view(ids: &[&str]) -> ViewState {
        let mut view = ViewState::default();
        for id in ids {
            view.set_expanded(*id, true);
        }
        view
    }

    /// The `(S', T', kind, aspect)` line each collapsed edge draws, encoded as
    /// a bundle key.
    fn line_keys(p: &Projection) -> BTreeSet<String> {
        p.edges
            .iter()
            .map(|edge| {
                let relation = &p.relation_metadata[&edge.semantic_ids[0]];
                encode_bundle_key(
                    &edge.source,
                    &edge.target,
                    &format!("{:?}", relation.kind),
                    relation.aspect.as_deref(),
                )
            })
            .collect()
    }

    fn bundle_of<'a>(p: &'a Projection, id: &str) -> &'a Option<String> {
        &p.edges
            .iter()
            .find(|edge| edge.semantic_ids.iter().any(|member| member == id))
            .unwrap_or_else(|| panic!("no edge for {id}: {:#?}", p.edges))
            .bundle
    }

    #[test]
    fn expanded_boundary_crossings_bundle_onto_the_collapsed_lines() {
        let model = bundle_model();
        let collapsed = project(&model, &ViewState::default()).unwrap();
        assert!(collapsed.edges.iter().all(|edge| edge.bundle.is_none()));
        let to_b: Vec<_> = collapsed
            .edges
            .iter()
            .filter(|edge| edge.target == "sb")
            .map(|edge| (edge.source.as_str(), edge.semantic_ids.clone()))
            .collect();
        assert_eq!(
            to_b,
            [
                ("sa", vec!["b1".to_owned(), "b2".to_owned()]),
                ("sa", vec!["b3".to_owned()])
            ]
        );

        let expanded = project(&model, &expanded_view(&["sa"])).unwrap();
        assert!(
            expanded.edges.iter().all(|edge| edge.bundle.is_some()),
            "{:#?}",
            expanded.edges
        );
        // Same endpoints and kind share one key; a different kind does not.
        assert_eq!(bundle_of(&expanded, "b1"), bundle_of(&expanded, "b2"));
        assert_ne!(bundle_of(&expanded, "b1"), bundle_of(&expanded, "b3"));
        assert_eq!(
            bundle_of(&expanded, "b1").as_deref(),
            Some("bundle|2:sa|2:sb|8:Requests|-")
        );
        // One distinct key per line the collapsed view draws.
        let keys: BTreeSet<_> = expanded
            .edges
            .iter()
            .map(|edge| edge.bundle.clone().unwrap())
            .collect();
        assert_eq!(keys, line_keys(&collapsed));
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn bundle_keys_are_part_of_the_projection_identity_and_deterministic() {
        let model = bundle_model();
        let view = expanded_view(&["sa"]);
        let first = project(&model, &view).unwrap();
        assert_eq!(first.key, project(&model, &view).unwrap().key);
        let mut changed = first.edges.clone();
        changed[0].bundle = None;
        let nodes = first.nodes.clone();
        assert_ne!(
            projection_key(&first.root, &nodes, &first.edges, &view),
            projection_key(&first.root, &nodes, &changed, &view)
        );
    }

    #[test]
    fn expanding_both_ends_shares_one_trunk_per_kind() {
        let model = bundle_model();
        let collapsed = project(&model, &ViewState::default()).unwrap();
        let both = project(&model, &expanded_view(&["sa", "sc"])).unwrap();
        assert!(both.edges.iter().all(|edge| edge.bundle.is_some()));
        assert_eq!(bundle_of(&both, "x1"), bundle_of(&both, "x2"));
        assert_ne!(bundle_of(&both, "x1"), bundle_of(&both, "x3"));
        assert_eq!(
            bundle_of(&both, "x1").as_deref(),
            Some("bundle|2:sa|2:sc|8:Requests|-")
        );
        assert_eq!(
            bundle_of(&both, "x3").as_deref(),
            Some("bundle|2:sa|2:sc|9:DependsOn|-")
        );
        let keys: BTreeSet<_> = both
            .edges
            .iter()
            .map(|edge| edge.bundle.clone().unwrap())
            .collect();
        assert_eq!(keys, line_keys(&collapsed));
        // Only C expanded: the trunk ends at C's boundary on the target side.
        let one_side = project(&model, &expanded_view(&["sc"])).unwrap();
        assert_eq!(
            bundle_of(&one_side, "x1").as_deref(),
            Some("bundle|2:sa|2:sc|8:Requests|-")
        );
        assert_eq!(bundle_of(&one_side, "b1"), &None);
    }

    #[test]
    fn interface_pads_bundle_through_expanded_scopes_but_not_through_solid_owners() {
        let model = bundle_model();
        // The pad's solid owner a1 is no enclosure; the pad leaves A.
        let view = expanded_view(&["sa", "a1"]);
        let nested = project(&model, &view).unwrap();
        assert_eq!(nested.semantic_to_display["ia"], "ia");
        assert_eq!(
            nested
                .nodes
                .iter()
                .find(|n| n.id == "ia")
                .unwrap()
                .parent
                .as_deref(),
            Some("a1")
        );
        assert_eq!(
            bundle_of(&nested, "pad").as_deref(),
            Some("bundle|2:sk|2:sa|8:Requests|-")
        );
        // The same pad on a solid owner directly in the root is a plain edge.
        let notes = project(&notes(), &expanded_view(&["service:notes"])).unwrap();
        assert!(notes.nodes.iter().any(|n| n.id == "interface:notes"));
        assert!(
            notes
                .edges
                .iter()
                .any(|edge| edge.target == "interface:notes")
        );
        assert!(notes.edges.iter().all(|edge| edge.bundle.is_none()));
    }
}
