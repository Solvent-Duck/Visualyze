//! View-only hierarchy and abstraction. Nothing in this module changes a supplied fact.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use visualyze_model::architecture::{Architecture, EntityKind, Relation, RelationKind};

use crate::{Edge, LayoutError, Point};

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
    pub peer_ceiling: usize,
    pub group_target: usize,
    pub visible_budget: usize,
    breadcrumbs: Vec<Option<String>>,
    auto_expanded: BTreeSet<String>,
    retained_groups: Vec<DisplayGroup>,
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
            peer_ceiling: 64,
            group_target: 32,
            visible_budget: 250,
            breadcrumbs: Vec::new(),
            auto_expanded: BTreeSet::new(),
            retained_groups: Vec::new(),
        }
    }
}

fn semantic_zoom_default() -> bool {
    true
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

    pub fn with_budgets(peer_ceiling: usize, group_target: usize, visible_budget: usize) -> Self {
        Self {
            peer_ceiling,
            group_target,
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
            && (self.expanded.contains(id) || self.auto_expanded.contains(id))
    }

    pub fn expanded_ids(&self) -> BTreeSet<String> {
        self.expanded
            .union(&self.auto_expanded)
            .filter(|id| !self.collapsed.contains(*id))
            .cloned()
            .collect()
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
pub fn project(model: &Architecture, view: &ViewState) -> Result<Projection, LayoutError> {
    if view.group_target < 2 || view.peer_ceiling < view.group_target || view.visible_budget == 0 {
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
    let deployment = view.lens == Some(SemanticLens::Deployment);
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
    items.retain(|id, item| match view.lens {
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
        if peers.len() <= view.peer_ceiling
            || !matches!(items[&parent].kind, NodeKind::Scope | NodeKind::Group)
        {
            continue;
        }
        for members in coarsen(&peers, &relations, view.group_target) {
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
    let mut edges = Vec::new();
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
            // Keep parallel predicates independent; membership IDs permit future explicit bundles.
            edges.push(Edge {
                id: relation.id.clone(),
                source,
                target,
                semantic_ids: vec![relation.id.clone()],
            });
        }
    }
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
        .extend(crate::noise::visual_noise(&projection).advisory());
    Ok(projection)
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
    fn protected_snapshot_suggestion_keeps_its_supplied_members_after_eviction() {
        let mut view = ViewState::with_budgets(3, 2, 250);
        let first = project(&notes(), &view).unwrap();
        let protected = first.suggested_groups.clone();
        assert!(!protected.is_empty());
        view.retained_groups = (0..64).map(|i| protected_group(i, true)).collect();
        view.protect_groups(&protected).unwrap();
        let next = project(&notes(), &view).unwrap();
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

    #[test]
    fn grouping_and_input_order_do_not_change_semantic_membership() {
        let mut view = ViewState::with_budgets(3, 2, 250);
        let p = project(&notes(), &view).unwrap();
        assert!(!p.suggested_groups.is_empty());
        assert!(p.suggested_groups.iter().all(|g| g.suggested));
        assert_eq!(
            p.semantic_to_display.len(),
            notes().snapshot().entities.len()
        );
        assert_eq!(p.key, project(&notes(), &view).unwrap().key);
        let count = p.edges.len() + p.internal_relation_ids.len();
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
            notes().snapshot().relations.len()
        );
        let mut json: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
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
        let base = ViewState::with_budgets(3, 2, 250);
        let collapsed = project(&notes(), &base).unwrap();
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
            let projection = project(&notes(), &view).unwrap();
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
}
