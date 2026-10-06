//! Bounded projection of one supplied bundle region. Rich predicates are retained
//! verbatim; asynchronous exchanges never become architecture `requests` edges.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};

use visualyze_model::provider::{
    BundleError, DetailResult, DetailState, EntityKind, EntitySummary, ModelProvider, Page,
    PageCursor, PageRequest, RegionRequest, RelationRecord, SemanticLens,
};

use crate::{
    Edge, LayoutError, Node, NodeKind, Point, Projection, check_cancel,
    noise::{FALLBACK_NOISE, NOISE_THRESHOLD, layer_capacity, level_noise, page_size_in_order},
    projection::{DisplayGroup, LayoutPolicy},
};

#[derive(Debug, Clone)]
pub struct ProviderViewRequest {
    pub focus: Option<String>,
    /// None preserves the complete supplied ownership view for comparison tools.
    pub lens: Option<SemanticLens>,
    pub expected_revision: Option<u64>,
    /// Hard cap on the children of one level shown at once. The actual page
    /// size is chosen from the level's visual noise (ADR 0007) and never
    /// exceeds this.
    pub max_children: usize,
    /// Choose the page size from visual noise: a level whose estimated noise is
    /// within `FALLBACK_NOISE` is shown whole, a noisier one is paged so that
    /// one page is. When false, a level is cut only at `max_children`; producer
    /// advice uses this to measure a level unpaged.
    pub page_by_noise: bool,
    pub child_page: usize,
    pub boundary_after: Option<PageCursor>,
    pub expanded: BTreeSet<String>,
    pub groups: Vec<DisplayGroup>,
    pub retained_groups: Vec<DisplayGroup>,
    pub dismissed_groups: BTreeSet<String>,
    /// Allow suggested fallback groups on a page whose noise is still above
    /// `FALLBACK_NOISE` after paging.
    pub automatic_groups: bool,
    /// The hard object limit; the only limit that is not about readability.
    pub visible_budget: usize,
    pub max_relation_records: usize,
    pub max_references: usize,
    pub children_after: Option<PageCursor>,
    pub incident_after: BTreeMap<String, PageCursor>,
    pub load_focus_detail: bool,
    pub policy: LayoutPolicy,
}

impl Default for ProviderViewRequest {
    fn default() -> Self {
        Self {
            focus: None,
            lens: None,
            expected_revision: None,
            max_children: 128,
            page_by_noise: true,
            child_page: 0,
            boundary_after: None,
            expanded: BTreeSet::new(),
            groups: Vec::new(),
            retained_groups: Vec::new(),
            dismissed_groups: BTreeSet::new(),
            automatic_groups: true,
            visible_budget: 250,
            max_relation_records: 4096,
            max_references: 64,
            children_after: None,
            incident_after: BTreeMap::new(),
            load_focus_detail: true,
            policy: LayoutPolicy::Crud,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Continuation {
    Boundary {
        after: PageCursor,
    },
    PendingChildren {
        parent: String,
    },
    Children {
        parent: String,
        after: PageCursor,
    },
    Incident {
        entity: String,
        after: Option<PageCursor>,
    },
}

#[derive(Debug, Clone)]
pub struct BundleProjection {
    pub graph: Projection,
    /// Current normalized view groups, including ancestors outside the focused
    /// graph. These are presentation definitions, not displayed semantic nodes.
    pub group_catalog: Vec<DisplayGroup>,
    pub entities: BTreeMap<String, EntitySummary>,
    pub relations: BTreeMap<String, RelationRecord>,
    pub detail_states: BTreeMap<String, DetailState>,
    pub focus_detail: Option<DetailResult>,
    pub continuations: Vec<Continuation>,
    pub region_diagnostics: BTreeMap<String, String>,
    pub total_children: usize,
    /// Number of supplied summaries represented by one child page.
    pub child_page_size: usize,
    /// Counts include hidden descendants, summed only over examined index pages.
    pub internal_relation_counts: BTreeMap<String, usize>,
    /// Complete for the requested child pages and descendant boundary scan.
    pub complete: bool,
    /// These relations were loaded but exceed the reference-node budget. They
    /// remain inspectable, and complete is false until their endpoints are shown.
    pub unrepresented_relation_ids: Vec<String>,
}

pub fn project_provider(
    provider: &mut impl ModelProvider,
    request: &ProviderViewRequest,
    cancelled: &AtomicBool,
) -> Result<BundleProjection, LayoutError> {
    let provisional = project_provider_once(provider, request, cancelled, None)?;
    let semantic_peers = provisional
        .graph
        .nodes
        .iter()
        .filter(|node| {
            node.id != provisional.graph.root
                && !provisional.graph.references.contains_key(&node.id)
                && (node.parent.as_ref() == Some(&provisional.graph.root) || node.parent.is_none())
        })
        .count();
    // References are immediate peers too. Determine their allowance after
    // coarsening, then repeat the same bounded query only when necessary. Keep
    // the provisional groups exactly: changing the relationship sample must
    // not change the abstraction that determined this allowance.
    // Immediate peers share one layer budget: what one layer may hold at
    // `FALLBACK_NOISE` (ADR 0007). Two reference slots guarantee progress for a
    // claim with two outside endpoints, so a full level may exceed that budget
    // by them; the hard total-object budget remains unchanged.
    let peer_allowance = layer_capacity(FALLBACK_NOISE);
    let reference_budget = request
        .max_references
        .min(peer_allowance.saturating_sub(semantic_peers).max(2));
    let mut result = if provisional.graph.references.len() > reference_budget {
        let reduced = ProviderViewRequest {
            expected_revision: Some(provisional.graph.revision),
            max_references: reference_budget,
            ..request.clone()
        };
        let mut result = project_provider_once(
            provider,
            &reduced,
            cancelled,
            Some(&provisional.graph.suggested_groups),
        )?;
        if result.graph.model_id != provisional.graph.model_id {
            return Err(LayoutError::Invalid(
                "provider changed between bounded projection passes".into(),
            ));
        }
        let note = format!(
            "Immediate-peer budget reserves {reference_budget} reference slots beside {semantic_peers} supplied or grouped peers; deferred claims remain on relationship pages."
        );
        result.graph.diagnostics.push(note.clone());
        result
            .region_diagnostics
            .insert("view:peer-budget".into(), note);
        result
    } else {
        provisional
    };
    // Advisory only, on the final graph; diagnostics are not part of the key.
    result
        .graph
        .diagnostics
        .extend(crate::noise::visual_noise(&result.graph).advisory());
    Ok(result)
}

fn project_provider_once(
    provider: &mut impl ModelProvider,
    request: &ProviderViewRequest,
    cancelled: &AtomicBool,
    fixed_display_groups: Option<&[DisplayGroup]>,
) -> Result<BundleProjection, LayoutError> {
    check_cancel(cancelled)?;
    if request.max_children == 0
        || request.max_children > 128
        || request.max_relation_records == 0
        || request.max_relation_records > 4096
        || request.max_references > 128
        || request.visible_budget == 0
        || request.visible_budget > 250
        || request.retained_groups.len() > 64
        || request.dismissed_groups.len() > 256
    {
        return Err(LayoutError::Invalid(
            "invalid bounded provider projection budget".into(),
        ));
    }
    let manifest = provider.manifest().clone();
    let revision = request.expected_revision.unwrap_or(manifest.revision);
    if revision != manifest.revision {
        return Err(LayoutError::Invalid(format!(
            "stale requested revision {revision}; current revision is {}",
            manifest.revision
        )));
    }
    let mut groups = validate_groups(provider, revision, request.lens, &request.groups, cancelled)?;
    let mut occupied: BTreeSet<_> = groups
        .iter()
        .flat_map(|group| group.members.iter().cloned())
        .collect();
    let mut retained = request.retained_groups.clone();
    retained.sort_by(|a, b| a.id.cmp(&b.id));
    for mut group in retained {
        check_cancel(cancelled)?;
        if groups.len() >= 64 {
            break;
        }
        if request.dismissed_groups.contains(&group.id)
            || !group.suggested
            || !group.id.starts_with("view:suggested-group:")
            || group.id.len() > 256
            || group.members.len() > 128
            || groups.iter().any(|existing| existing.id == group.id)
            || provider
                .summary(revision, &group.id)
                .map_err(provider_error)?
                .is_some()
        {
            continue;
        }
        if let Some(lens) = request.lens
            && !provider
                .in_lens(revision, lens, &group.parent)
                .map_err(provider_error)?
        {
            continue;
        }
        group.members.sort();
        group.members.dedup();
        let mut members = Vec::new();
        for member in group.members {
            check_cancel(cancelled)?;
            if occupied.contains(&member) {
                continue;
            }
            if let Some(lens) = request.lens
                && !provider
                    .in_lens(revision, lens, &member)
                    .map_err(provider_error)?
            {
                continue;
            }
            match owner(provider, revision, request.lens, &member) {
                Ok(Some(parent)) if parent == group.parent => members.push(member),
                Ok(_) | Err(BundleError::NotFound(_)) => {}
                Err(error) => return Err(provider_error(error)),
            }
        }
        group.members = members;
        if group.members.len() >= 2 {
            occupied.extend(group.members.iter().cloned());
            groups.push(group);
        }
    }
    let focused_group = request
        .focus
        .as_ref()
        .and_then(|focus| groups.iter().find(|group| &group.id == focus));
    let root = focused_group
        .map(|group| group.parent.clone())
        .unwrap_or_else(|| {
            request
                .focus
                .clone()
                .unwrap_or_else(|| manifest.root_id.clone())
        });
    let display_root = focused_group
        .map(|group| group.id.clone())
        .unwrap_or_else(|| root.clone());
    let focus = provider
        .summary(revision, &root)
        .map_err(provider_error)?
        .ok_or_else(|| LayoutError::Invalid(format!("unknown focus {root}")))?;
    if focus.id != root {
        return Err(LayoutError::Invalid(
            "provider returned a different focus".into(),
        ));
    }
    if let Some(lens) = request.lens
        && !provider
            .in_lens(revision, lens, &root)
            .map_err(provider_error)?
    {
        return Err(LayoutError::Invalid(format!(
            "The selected focus is outside the {lens:?} view; use Auto or choose its compatible view."
        )));
    }
    let total_children = if let Some(group) = focused_group {
        group.members.len()
    } else {
        child_count(provider, revision, request.lens, &root).map_err(provider_error)?
    };
    let child_page_size = level_page_size(
        provider,
        request,
        revision,
        &root,
        total_children,
        focused_group.map(|group| group.members.as_slice()),
        cancelled,
    )?;
    let child_offset = request
        .child_page
        .checked_mul(child_page_size)
        .filter(|offset| *offset <= total_children)
        .ok_or_else(|| LayoutError::Invalid("child page is outside the supplied region".into()))?;
    let mut entities = BTreeMap::from([(root.clone(), focus)]);
    let mut parents = BTreeMap::new();
    let mut continuations = Vec::new();
    let mut region_diagnostics = BTreeMap::new();
    let mut expanded_regions = BTreeSet::from([root.clone()]);
    let mut pending = BTreeSet::from([root.clone()]);
    while let Some(parent) = pending.pop_first() {
        check_cancel(cancelled)?;
        // Keep room for both ends of an outside dependency so reference pages
        // can always advance even when explicit expansion fills the view budget.
        let reserve = if request.max_references >= 2 && request.visible_budget > 2 {
            2
        } else {
            0
        };
        let remaining = request
            .visible_budget
            .saturating_sub(reserve + entities.len());
        if remaining == 0 {
            continuations.push(Continuation::PendingChildren {
                parent: parent.clone(),
            });
            region_diagnostics.insert(
                parent,
                "Visible-object budget reached; this region remains collapsed.".into(),
            );
            continue;
        }
        let window = if parent == root {
            child_page_size
        } else {
            let count =
                child_count(provider, revision, request.lens, &parent).map_err(provider_error)?;
            level_page_size(provider, request, revision, &parent, count, None, cancelled)?
        };
        let limit = window.min(remaining);
        let mut children = if let (true, Some(group)) = (parent == root, focused_group) {
            if request.children_after.is_some() {
                return Err(LayoutError::Invalid(
                    "virtual groups use bounded child page numbers".into(),
                ));
            }
            let stop = group.members.len().min(child_offset + limit);
            let mut items = Vec::new();
            for id in &group.members[child_offset..stop] {
                check_cancel(cancelled)?;
                items.push(
                    provider
                        .summary(revision, id)
                        .map_err(provider_error)?
                        .ok_or_else(|| {
                            LayoutError::Invalid(format!("missing group member {id}"))
                        })?,
                );
            }
            if stop < group.members.len() {
                continuations.push(Continuation::PendingChildren {
                    parent: display_root.clone(),
                });
                region_diagnostics.insert(
                    display_root.clone(),
                    "More group members are available on the next child page.".into(),
                );
            }
            Ok(Page {
                revision,
                items,
                next: None,
            })
        } else if parent == root && request.children_after.is_none() {
            children_at(
                provider,
                revision,
                request.lens,
                &parent,
                child_offset,
                limit,
            )
        } else {
            child_page(
                provider,
                revision,
                request.lens,
                &parent,
                PageRequest {
                    limit,
                    after: if parent == root {
                        request.children_after.clone()
                    } else {
                        None
                    },
                },
            )
        }
        .map_err(provider_error)?;
        // A storage page may stop at its byte budget before the requested child
        // window ends. Complete that bounded window so integer UI paging cannot
        // skip children whose records happen to carry large details.
        while children.items.len() < limit {
            let Some(after) = children.next.clone() else {
                break;
            };
            check_cancel(cancelled)?;
            let more = child_page(
                provider,
                revision,
                request.lens,
                &parent,
                PageRequest {
                    limit: limit - children.items.len(),
                    after: Some(after.clone()),
                },
            )
            .map_err(provider_error)?;
            if more.revision != revision
                || more.items.is_empty()
                || more.next.as_ref() == Some(&after)
            {
                return Err(LayoutError::Invalid(
                    "child continuation is stale or makes no progress".into(),
                ));
            }
            children.items.extend(more.items);
            children.next = more.next;
        }
        if children.revision != revision {
            return Err(LayoutError::Invalid(
                "provider returned stale children".into(),
            ));
        }
        if children.items.len() > limit
            || children
                .items
                .iter()
                .map(|entity| &entity.id)
                .collect::<BTreeSet<_>>()
                .len()
                != children.items.len()
            || children
                .items
                .iter()
                .any(|entity| entities.contains_key(&entity.id))
        {
            return Err(LayoutError::Invalid(
                "provider returned duplicate, cyclic, or excessive child summaries".into(),
            ));
        }
        if let Some(after) = children.next {
            continuations.push(Continuation::Children {
                parent: parent.clone(),
                after,
            });
            region_diagnostics.insert(
                parent.clone(),
                "More supplied children are available; this region is a bounded page.".into(),
            );
        }
        if !children.items.is_empty() {
            expanded_regions.insert(parent.clone());
        }
        children.items.sort_by(|a, b| a.id.cmp(&b.id));
        for child in children.items {
            parents.insert(child.id.clone(), parent.clone());
            if request.expanded.contains(&child.id) {
                pending.insert(child.id.clone());
            }
            entities.insert(child.id.clone(), child);
        }
    }
    let mut detail_states = BTreeMap::new();
    for id in entities.keys() {
        check_cancel(cancelled)?;
        detail_states.insert(
            id.clone(),
            provider
                .detail_state(revision, id)
                .map_err(provider_error)?,
        );
    }
    let focus_detail = if request.load_focus_detail && focused_group.is_none() {
        check_cancel(cancelled)?;
        let detail = provider.detail(revision, &root).map_err(provider_error)?;
        if detail.revision != revision {
            return Err(LayoutError::Invalid(
                "provider returned stale detail".into(),
            ));
        }
        detail_states.insert(root.clone(), detail.state);
        Some(detail)
    } else {
        None
    };
    // Local collapsed groups can span child pages. Their member IDs become
    // indexed representatives without instantiating or loading their records.
    let mut query_visible: BTreeSet<String> = entities.keys().cloned().collect();
    for group in &groups {
        if entities.contains_key(&group.parent)
            && group.members.iter().any(|id| entities.contains_key(id))
        {
            query_visible.extend(group.members.iter().cloned());
        }
    }
    let mut relations = BTreeMap::new();
    let mut representatives = BTreeMap::new();
    let mut structural_ids = BTreeSet::new();
    let mut internal_relation_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut internal_relation_ids = Vec::new();
    let mut after = request.boundary_after.clone();
    let mut reserved_references = BTreeSet::new();
    let group_frames = groups
        .iter()
        .filter(|group| {
            focused_group.is_none_or(|focus| focus.id != group.id)
                && (request.expanded.contains(&group.id)
                    || request.expanded.iter().any(|id| {
                        if !entities.contains_key(id) {
                            return false;
                        }
                        let mut current = Some(id.as_str());
                        while let Some(id) = current {
                            if group.members.iter().any(|member| member == id) {
                                return true;
                            }
                            current = parents.get(id).map(String::as_str);
                        }
                        false
                    }))
                && entities.contains_key(&group.parent)
                && group.members.iter().any(|id| entities.contains_key(id))
        })
        .count();
    let reference_budget = request.max_references.min(
        request
            .visible_budget
            .saturating_sub(entities.len() + group_frames),
    );
    // Both record residency and index scan work are bounded. A partial scan exposes
    // its cursor; no omitted continuation is presented as a complete architecture.
    for iteration in 0..256 {
        check_cancel(cancelled)?;
        let remaining = request.max_relation_records.saturating_sub(relations.len());
        if remaining == 0 {
            if let Some(after) = after {
                continuations.push(Continuation::Boundary { after });
            }
            break;
        }
        let page = provider
            .region_relations(
                revision,
                RegionRequest {
                    focus: root.clone(),
                    lens: request.lens,
                    visible: query_visible.iter().cloned().collect(),
                    scope_roots: focused_group
                        .map(|group| group.members.clone())
                        .unwrap_or_default(),
                    page: PageRequest {
                        limit: remaining.min(256),
                        after: after.clone(),
                    },
                    scan_budget: 8192,
                    reference_budget,
                    existing_references: reserved_references.iter().cloned().collect(),
                },
                cancelled,
            )
            .map_err(provider_error)?;
        if page.revision != revision
            || page.items.len() > remaining.min(256)
            || (!page.reference_limited
                && page.next.is_some()
                && (page.scanned == 0 || page.next == after))
        {
            return Err(LayoutError::Invalid(
                "provider boundary page is stale, excessive, or makes no progress".into(),
            ));
        }
        for (owner, count) in page.internal_counts {
            *internal_relation_counts.entry(owner).or_default() += count;
        }
        let keep = request
            .max_relation_records
            .saturating_sub(internal_relation_ids.len());
        internal_relation_ids.extend(page.internal_relation_ids.into_iter().take(keep));
        for projected in page.items {
            if projected.structural {
                structural_ids.insert(projected.relation.id.clone());
            }
            for (representative, inside) in [
                (&projected.source_display, projected.source_inside),
                (&projected.target_display, projected.target_inside),
            ] {
                if !query_visible.contains(representative) || (focused_group.is_some() && !inside) {
                    reserved_references.insert(representative.clone());
                }
            }
            let source = if focused_group.is_some()
                && projected.source_inside
                && projected.source_display == root
            {
                display_root.clone()
            } else {
                projected.source_display
            };
            let target = if focused_group.is_some()
                && projected.target_inside
                && projected.target_display == root
            {
                display_root.clone()
            } else {
                projected.target_display
            };
            representatives.insert(projected.relation.id.clone(), (source, target));
            if let Some(existing) =
                relations.insert(projected.relation.id.clone(), projected.relation.clone())
                && existing != projected.relation
            {
                return Err(LayoutError::Invalid(
                    "conflicting relation records within one revision".into(),
                ));
            }
        }
        if page.reference_limited {
            region_diagnostics.insert(root.clone(), if reference_budget >= 2 {
                "Reference-node budget reached; the next relationship page resumes the first deferred claim.".into()
            } else {
                "Reference-node budget is too small for this dependency; increase the object/reference budget.".into()
            });
            if let Some(after) = page.next {
                continuations.push(Continuation::Boundary { after });
            }
            break;
        }
        match page.next {
            Some(cursor) if iteration == 255 => {
                continuations.push(Continuation::Boundary { after: cursor });
                break;
            }
            Some(cursor) => after = Some(cursor),
            None => break,
        }
    }
    let mut nodes: BTreeMap<_, _> = entities
        .values()
        .map(|entity| {
            let mut node = node_for(entity, parents.get(&entity.id).cloned(), request.policy);
            if expanded_regions.contains(&entity.id) {
                let children: Vec<_> = parents
                    .iter()
                    .filter(|(_, parent)| *parent == &entity.id)
                    .map(|(child, _)| &entities[child])
                    .collect();
                // Exposed interfaces remain pads on their component's real face.
                let exposes_only = !children.is_empty()
                    && matches!(
                        entity.kind,
                        EntityKind::Client | EntityKind::Service | EntityKind::Datastore
                    )
                    && children
                        .iter()
                        .all(|child| child.kind == EntityKind::Interface);
                node.solid = children.is_empty() || exposes_only;
                if exposes_only {
                    node.max_size.y = node.max_size.y.max(children.len() as i64 * 18);
                }
            }
            (entity.id.clone(), node)
        })
        .collect();
    // Retained definitions from other pages must not consume this page's group
    // budget. They stay in view state and are reused when their members return.
    let mut display_candidates: Vec<_> = fixed_display_groups.map_or_else(
        || {
            groups
                .iter()
                .filter(|group| {
                    entities.contains_key(&group.parent)
                        && group.members.iter().any(|id| entities.contains_key(id))
                })
                .cloned()
                .collect()
        },
        |groups| groups.to_vec(),
    );
    if request.automatic_groups && fixed_display_groups.is_none() {
        let mut occupied: BTreeSet<_> = groups
            .iter()
            .flat_map(|group| group.members.iter().cloned())
            .collect();
        let parents_to_consider: BTreeSet<_> = nodes
            .values()
            .filter_map(|node| node.parent.clone())
            .collect();
        for parent in parents_to_consider {
            check_cancel(cancelled)?;
            if focused_group.is_some_and(|group| group.parent == parent) {
                continue;
            }
            let peers: BTreeSet<_> = nodes
                .values()
                .filter(|node| {
                    node.parent.as_ref() == Some(&parent) && node.kind != NodeKind::Interface
                })
                .map(|node| node.id.clone())
                .collect();
            // Paging sizes a page from the level's first children, so a page
            // can still be noisier than the estimate when its children sit
            // in fewer layers. Only then does fallback grouping run. The
            // check uses every loaded peer, supplied groups included, so that
            // it does not change as retained suggestions come and go.
            let mut bands = BTreeMap::new();
            for id in &peers {
                *bands.entry(nodes[id].band).or_default() += 1;
            }
            if level_noise(&bands).total <= FALLBACK_NOISE {
                continue;
            }
            // Group so that one layer of groups reads at `NOISE_THRESHOLD`.
            let cap = peers.len().div_ceil(layer_capacity(NOISE_THRESHOLD)).max(2);
            for members in coarsen_region(&peers, &occupied, &relations, &representatives, cap) {
                if display_candidates.len() >= 64 {
                    break;
                }
                let id = suggestion_id(&parent, &members);
                if request.dismissed_groups.contains(&id)
                    || nodes.contains_key(&id)
                    || display_candidates.iter().any(|group| group.id == id)
                {
                    continue;
                }
                occupied.extend(members.iter().cloned());
                display_candidates.push(DisplayGroup {
                    id,
                    parent: parent.clone(),
                    members,
                    suggested: true,
                });
            }
        }
    }
    let (aliases, display_groups) = apply_groups(
        &mut nodes,
        &display_candidates,
        focused_group.map(|group| group.id.as_str()),
        &request.expanded,
        request.visible_budget,
        &mut region_diagnostics,
    )?;
    if focused_group.is_some() {
        nodes.remove(&root);
        if let Some(node) = nodes.get_mut(&display_root) {
            node.parent = None;
        }
        if let Some(count) = internal_relation_counts.remove(&root) {
            *internal_relation_counts
                .entry(display_root.clone())
                .or_default() += count;
        }
    }
    let root = display_root;
    // Fresh automatic groups are discovered after the relationship query. An
    // explicit member expansion can add a frame the earlier reservation did
    // not know about. Fail with an actionable conflict rather than publishing
    // loaded claims whose reference endpoints no longer fit.
    if nodes.len() + reserved_references.len() > request.visible_budget {
        return Err(LayoutError::Invalid(
            "preserving explicit expansions and their visual group frames leaves insufficient room for the relationship page; collapse another region or use a smaller child page".into(),
        ));
    }
    let mut counts = BTreeMap::new();
    for (owner, count) in internal_relation_counts {
        *counts
            .entry(aliases.get(&owner).cloned().unwrap_or(owner))
            .or_insert(0) += count;
    }
    let mut internal_relation_counts = counts;
    let mut references = BTreeMap::new();
    let mut edge_groups: BTreeMap<(String, String, String, Option<String>), Vec<String>> =
        BTreeMap::new();
    let mut unrepresented_relation_ids = Vec::new();
    let mut semantic_to_display: BTreeMap<_, _> = nodes
        .keys()
        .map(|id| (id.clone(), id.clone()))
        .chain(
            aliases
                .iter()
                .map(|(id, display)| (id.clone(), display.clone())),
        )
        .collect();
    for relation in relations
        .values()
        .filter(|relation| !structural_ids.contains(&relation.id))
    {
        check_cancel(cancelled)?;
        let (source, target) = &representatives[&relation.id];
        let missing: BTreeSet<_> = [source, target]
            .into_iter()
            .filter(|id| !semantic_to_display.contains_key(*id))
            .collect();
        if references.len() + missing.len() > request.max_references
            || nodes.len() + missing.len() > request.visible_budget
        {
            unrepresented_relation_ids.push(relation.id.clone());
            continue;
        }
        for semantic in missing {
            let mut id = format!("view:reference:{semantic}");
            while nodes.contains_key(&id) {
                id.push(':');
            }
            nodes.insert(
                id.clone(),
                Node {
                    id: id.clone(),
                    semantic_ids: vec![semantic.clone()],
                    parent: (!nodes[&root].solid).then(|| root.clone()),
                    kind: NodeKind::Group,
                    band: 1,
                    preferred_size: Point::new(24, 12, 20),
                    min_size: Point::new(12, 12, 10),
                    max_size: Point::new(48, 24, 40),
                    solid: true,
                },
            );
            references.insert(id.clone(), semantic.clone());
            semantic_to_display.insert(semantic.clone(), id);
        }
        let source_display = semantic_to_display[source].clone();
        let target_display = semantic_to_display[target].clone();
        semantic_to_display.insert(relation.source.clone(), source_display.clone());
        semantic_to_display.insert(relation.target.clone(), target_display.clone());
        if source_display == target_display {
            internal_relation_ids.push(relation.id.clone());
            *internal_relation_counts
                .entry(source_display.clone())
                .or_default() += 1;
        } else {
            edge_groups
                .entry((
                    source_display,
                    target_display,
                    format!("{:?}", relation.kind),
                    relation.aspect.clone(),
                ))
                .or_default()
                .push(relation.id.clone());
        }
    }
    let mut edges: Vec<Edge> = edge_groups
        .into_iter()
        .map(|((source, target, _, _), mut semantic_ids)| {
            semantic_ids.sort();
            Edge {
                id: semantic_ids[0].clone(),
                source,
                target,
                semantic_ids,
            }
        })
        .collect();
    edges.sort_by(|a, b| a.id.cmp(&b.id));
    let complete = request.child_page == 0
        && request.children_after.is_none()
        && request.boundary_after.is_none()
        && continuations.is_empty()
        && unrepresented_relation_ids.is_empty();
    let internal_count: usize = internal_relation_counts.values().sum();
    let mut diagnostics = vec![format!(
        "Descendant boundary relationships map to visible ancestors; grouped connectors retain original predicate, direction, and membership IDs. {internal_count} relationships in this scanned slice are internal to collapsed regions ({} membership IDs resident; enter a region for records).",
        internal_relation_ids.len()
    )];
    if request.child_page > 0 {
        diagnostics.push(format!(
            "Showing child page {} of {}.",
            request.child_page + 1,
            total_children.div_ceil(child_page_size)
        ));
    }
    for (region, note) in &region_diagnostics {
        diagnostics.push(format!("{region}: {note}"));
    }
    if !complete {
        diagnostics.push(format!("Partial region/page: {} query continuations and {} loaded relationships without displayed endpoints; refine or page before treating this as complete.", continuations.len(), unrepresented_relation_ids.len()));
    }
    if request.policy == LayoutPolicy::Pipeline {
        crate::projection::rank_pipeline(&mut nodes, &edges);
    }
    let nodes: Vec<_> = nodes.into_values().collect();
    let key = format!(
        "{:?}:{}",
        request.lens,
        key(&root, &nodes, &edges, request.policy)
    );
    check_cancel(cancelled)?;
    if provider.manifest().model_id != manifest.model_id || provider.manifest().revision != revision
    {
        return Err(LayoutError::Invalid(
            "provider changed while constructing the projection".into(),
        ));
    }
    let group_catalog = groups
        .iter()
        .chain(&display_groups)
        .map(|group| (group.id.clone(), group.clone()))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    Ok(BundleProjection {
        group_catalog,
        graph: Projection {
            key,
            model_id: manifest.model_id,
            revision,
            root,
            nodes,
            edges,
            relation_metadata: BTreeMap::new(),
            internal_relation_ids,
            semantic_to_display,
            suggested_groups: display_groups,
            references,
            diagnostics,
        },
        entities,
        relations,
        detail_states,
        focus_detail,
        continuations,
        region_diagnostics,
        complete,
        total_children,
        child_page_size,
        internal_relation_counts,
        unrepresented_relation_ids,
    })
}

/// How many children of one level a page shows (ADR 0007). The size depends on
/// the level alone, never on which page is shown, so paging stays consistent.
///
/// A level that one layer could hold at `FALLBACK_NOISE` is shown whole, as is
/// any level whose estimated noise is within it. A noisier level is paged at
/// the largest size whose pages are within `FALLBACK_NOISE` (`noise::
/// page_size_in_order`): the proportional size for the layer mix, lowered until
/// every whole page of the measured children fits. The measured children are
/// the first `max_children` of the level, loaded from offset 0 in paging
/// order; connectors are ignored, being unknown until the relationships are
/// scanned. `max_children` always caps the result. A virtual group is a level
/// too: its members are measured the same way, so it pages rather than
/// refusing. Children beyond the measured ones are not seen, so a later page
/// can still be noisier than the estimate; fallback grouping covers that.
fn level_page_size(
    provider: &mut impl ModelProvider,
    request: &ProviderViewRequest,
    revision: u64,
    parent: &str,
    child_count: usize,
    members: Option<&[String]>,
    cancelled: &AtomicBool,
) -> Result<usize, LayoutError> {
    let cap = request.max_children.max(1);
    let whole = child_count.min(cap).max(1);
    // One layer's worth is the worst case for any mix: noise adds over layers.
    if !request.page_by_noise || child_count <= layer_capacity(FALLBACK_NOISE) {
        return Ok(whole);
    }
    let sample = child_count.min(cap);
    let mut entities = Vec::new();
    if let Some(members) = members {
        for id in &members[..sample.min(members.len())] {
            check_cancel(cancelled)?;
            entities.extend(provider.summary(revision, id).map_err(provider_error)?);
        }
    } else {
        check_cancel(cancelled)?;
        entities = children_at(provider, revision, request.lens, parent, 0, sample)
            .map_err(provider_error)?
            .items;
    }
    let bands: Vec<i32> = entities
        .iter()
        .map(|entity| node_for(entity, None, request.policy).band)
        .collect();
    Ok(page_size_in_order(&bands, FALLBACK_NOISE).clamp(1, cap))
}

pub(crate) fn child_count(
    provider: &impl ModelProvider,
    revision: u64,
    lens: Option<SemanticLens>,
    id: &str,
) -> Result<usize, BundleError> {
    match lens {
        Some(lens) => provider.child_count_in(revision, lens, id),
        None => provider.child_count(revision, id),
    }
}

fn owner(
    provider: &impl ModelProvider,
    revision: u64,
    lens: Option<SemanticLens>,
    id: &str,
) -> Result<Option<String>, BundleError> {
    match lens {
        Some(lens) => provider.owner_id_in(revision, lens, id),
        None => provider.owner_id(revision, id),
    }
}

fn child_page(
    provider: &mut impl ModelProvider,
    revision: u64,
    lens: Option<SemanticLens>,
    id: &str,
    page: PageRequest,
) -> Result<Page<EntitySummary>, BundleError> {
    match lens {
        Some(lens) => provider.children_in(revision, lens, id, page),
        None => provider.children(revision, id, page),
    }
}

pub(crate) fn children_at(
    provider: &mut impl ModelProvider,
    revision: u64,
    lens: Option<SemanticLens>,
    id: &str,
    offset: usize,
    limit: usize,
) -> Result<Page<EntitySummary>, BundleError> {
    match lens {
        Some(lens) => provider.children_at_in(revision, lens, id, offset, limit),
        None => provider.children_at(revision, id, offset, limit),
    }
}

fn suggestion_id(parent: &str, members: &[String]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for text in std::iter::once(parent).chain(members.iter().map(String::as_str)) {
        for byte in (text.len() as u64)
            .to_le_bytes()
            .iter()
            .chain(text.as_bytes())
        {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("view:suggested-group:{hash:016x}")
}

fn coarsen_region(
    peers: &BTreeSet<String>,
    occupied: &BTreeSet<String>,
    relations: &BTreeMap<String, RelationRecord>,
    representatives: &BTreeMap<String, (String, String)>,
    cap: usize,
) -> Vec<Vec<String>> {
    let mut adjacency: BTreeMap<String, BTreeSet<String>> = peers
        .iter()
        .map(|id| (id.clone(), BTreeSet::new()))
        .collect();
    for relation in relations
        .values()
        .filter(|relation| !relation.kind.is_ownership())
    {
        let (source, target) = &representatives[&relation.id];
        if source != target && peers.contains(source) && peers.contains(target) {
            adjacency.get_mut(source).unwrap().insert(target.clone());
            adjacency.get_mut(target).unwrap().insert(source.clone());
        }
    }
    // Keep the degree/attraction reference graph fixed for this supplied page.
    // Partial retention can evict some group definitions, but it must not change
    // the scores used to reconstruct them while neighboring groups survive.
    let mut remaining: BTreeSet<_> = peers.difference(occupied).cloned().collect();
    let mut groups = Vec::new();
    while let Some(seed) = remaining.pop_first() {
        let mut group = vec![seed];
        while group.len() < cap && !remaining.is_empty() {
            // Normalize shared hubs so a ubiquitous dependency does not absorb
            // every peer merely because it has more incident claims.
            let next = remaining
                .iter()
                .max_by(|a, b| {
                    let score = |candidate: &String| {
                        group
                            .iter()
                            .filter(|member| adjacency[candidate].contains(*member))
                            .map(|member| {
                                1_000_000
                                    / (adjacency[candidate].len().max(1)
                                        * adjacency[member].len().max(1))
                            })
                            .sum::<usize>()
                    };
                    score(a).cmp(&score(b)).then_with(|| b.cmp(a))
                })
                .unwrap()
                .clone();
            remaining.remove(&next);
            group.push(next);
        }
        group.sort();
        if group.len() >= 2 {
            groups.push(group);
        }
    }
    groups
}

fn validate_groups(
    provider: &mut impl ModelProvider,
    revision: u64,
    lens: Option<SemanticLens>,
    groups: &[DisplayGroup],
    cancelled: &AtomicBool,
) -> Result<Vec<DisplayGroup>, LayoutError> {
    if groups.len() > 64 {
        return Err(LayoutError::Invalid(
            "at most 64 local groups are supported per view".into(),
        ));
    }
    let mut groups = groups.to_vec();
    groups.sort_by(|a, b| a.id.cmp(&b.id));
    let mut ids = BTreeSet::new();
    let mut members = BTreeSet::new();
    if let Some(lens) = lens {
        let mut active = Vec::new();
        for group in groups {
            if provider
                .in_lens(revision, lens, &group.parent)
                .map_err(provider_error)?
            {
                let mut compatible = true;
                for member in &group.members {
                    compatible &= provider
                        .in_lens(revision, lens, member)
                        .map_err(provider_error)?;
                }
                if compatible {
                    active.push(group);
                }
            }
        }
        groups = active;
    }
    for group in &mut groups {
        check_cancel(cancelled)?;
        if !group.id.starts_with("view:")
            || group.id.len() > 256
            || !group.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b":._-".contains(&byte)
            })
            || !ids.insert(group.id.clone())
            || group.members.len() < 2
            || group.members.len() > 128
            || provider
                .summary(revision, &group.id)
                .map_err(provider_error)?
                .is_some()
        {
            return Err(LayoutError::Invalid(
                "a local group needs a unique view ID and 2–128 supplied members".into(),
            ));
        }
        group.members.sort();
        group.suggested = false;
        for member in &group.members {
            check_cancel(cancelled)?;
            if !members.insert(member.clone())
                || owner(provider, revision, lens, member)
                    .map_err(provider_error)?
                    .as_ref()
                    != Some(&group.parent)
            {
                return Err(LayoutError::Invalid(format!(
                    "group {} must contain distinct, unshared siblings under its supplied parent",
                    group.id
                )));
            }
        }
    }
    Ok(groups)
}

fn apply_groups(
    nodes: &mut BTreeMap<String, Node>,
    groups: &[DisplayGroup],
    focused: Option<&str>,
    expanded: &BTreeSet<String>,
    budget: usize,
    diagnostics: &mut BTreeMap<String, String>,
) -> Result<(BTreeMap<String, String>, Vec<DisplayGroup>), LayoutError> {
    let mut aliases = BTreeMap::new();
    let mut applied = Vec::new();
    for group in groups {
        if !nodes.contains_key(&group.parent) {
            continue;
        }
        let is_focus = focused == Some(group.id.as_str());
        let present: Vec<_> = group
            .members
            .iter()
            .filter(|id| nodes.contains_key(*id))
            .cloned()
            .collect();
        if !is_focus && present.is_empty() {
            continue;
        }
        if present.len() != group.members.len() {
            diagnostics.insert(group.id.clone(), "This visual group includes members on other child pages; enter it to inspect its bounded member pages.".into());
        }
        if present.iter().any(|id| {
            nodes[id].parent.as_ref() != Some(&group.parent)
                || nodes[id].kind == NodeKind::Interface
        }) {
            return Err(LayoutError::Invalid(
                "local groups cannot move ownership boundaries or replace attached interfaces"
                    .into(),
            ));
        }
        let descendants: BTreeSet<_> = nodes
            .keys()
            .filter(|id| {
                let mut current = Some(id.as_str());
                while let Some(id) = current {
                    if present.iter().any(|member| member == id) {
                        return true;
                    }
                    current = nodes.get(id).and_then(|node| node.parent.as_deref());
                }
                false
            })
            .cloned()
            .collect();
        // A visual grouping must never erase an explicit semantic expansion.
        // Open this containing group, keeping its stable ID and membership;
        // only the requested member's descendants were loaded, so neighboring
        // members remain ordinary collapsed boxes. This also applies to local
        // groups and to the frozen groups used by the reference-budget pass.
        let member_expansion = !expanded.is_disjoint(&descendants);
        let expanded = is_focus || expanded.contains(&group.id) || member_expansion;
        if member_expansion && !is_focus {
            diagnostics.entry(group.id.clone()).and_modify(|note| {
                note.push_str(" Opened this visual group to preserve an explicit member expansion; neighboring members keep their own detail level.");
            }).or_insert_with(|| "Opened this visual group to preserve an explicit member expansion; neighboring members keep their own detail level.".into());
        }
        if expanded && nodes.len() + usize::from(!is_focus) > budget {
            return Err(LayoutError::Invalid(format!(
                "opening visual group {} while preserving explicit expansions exceeds the visible-object budget; collapse another region or use a smaller child page",
                group.id
            )));
        }
        let mut semantic_ids: BTreeSet<_> = group.members.iter().cloned().collect();
        let band = present.first().map_or(0, |id| nodes[id].band);
        if expanded {
            for id in &group.members {
                if !present.contains(id) {
                    aliases.insert(id.clone(), group.id.clone());
                }
            }
            for id in &present {
                nodes.get_mut(id).unwrap().parent = Some(group.id.clone());
            }
        } else {
            for id in &group.members {
                aliases.insert(id.clone(), group.id.clone());
            }
            for id in descendants {
                if let Some(node) = nodes.remove(&id) {
                    semantic_ids.extend(node.semantic_ids);
                }
                aliases.insert(id, group.id.clone());
            }
        }
        nodes.insert(
            group.id.clone(),
            Node {
                id: group.id.clone(),
                semantic_ids: semantic_ids.into_iter().collect(),
                parent: Some(group.parent.clone()),
                kind: NodeKind::Group,
                band,
                preferred_size: Point::new(24, 12, 20),
                min_size: Point::new(12, 12, 10),
                max_size: Point::new(48, 24, 40),
                solid: !expanded,
            },
        );
        applied.push(group.clone());
    }
    applied.sort_by(|a, b| a.id.cmp(&b.id));
    Ok((aliases, applied))
}

pub(crate) fn provider_error(error: BundleError) -> LayoutError {
    match error {
        BundleError::Cancelled => LayoutError::Cancelled,
        _ => LayoutError::Invalid(error.to_string()),
    }
}

fn node_for(entity: &EntitySummary, parent: Option<String>, policy: LayoutPolicy) -> Node {
    let kind = match entity.kind {
        EntityKind::Scope | EntityKind::CodeRepository | EntityKind::CodePackage => NodeKind::Scope,
        EntityKind::Client => NodeKind::Client,
        EntityKind::Service => NodeKind::Service,
        EntityKind::Datastore | EntityKind::Table | EntityKind::Column => NodeKind::Datastore,
        EntityKind::Interface => NodeKind::Interface,
        EntityKind::Concern => NodeKind::Concern,
        EntityKind::Deployment | EntityKind::DeploymentInstance => NodeKind::Deployment,
        EntityKind::CodeFile
        | EntityKind::CodeType
        | EntityKind::CodeFunction
        | EntityKind::MessageChannel
        | EntityKind::MessageType => NodeKind::Group,
    };
    let size = if kind == NodeKind::Concern {
        Point::new(28, 4, 24)
    } else {
        Point::new(24, 12, 20)
    };
    let band = match policy {
        LayoutPolicy::Crud => match kind {
            NodeKind::Concern | NodeKind::Client => 2,
            NodeKind::Datastore => 0,
            _ => 1,
        },
        _ => 0,
    };
    Node {
        id: entity.id.clone(),
        semantic_ids: vec![entity.id.clone()],
        parent,
        kind,
        band,
        min_size: Point::new(size.x / 2, size.y, size.z / 2),
        max_size: Point::new(size.x * 2, size.y * 2, size.z * 2),
        preferred_size: size,
        solid: true,
    }
}

fn key(root: &str, nodes: &[Node], edges: &[Edge], policy: LayoutPolicy) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    let mut write = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    };
    write(root.as_bytes());
    write(format!("{policy:?}").as_bytes());
    for node in nodes {
        write(node.id.as_bytes());
        write(node.parent.as_deref().unwrap_or("").as_bytes());
        for id in &node.semantic_ids {
            write(id.as_bytes());
        }
        write(format!("{}:{}:{:?}", node.solid, node.band, node.kind).as_bytes());
    }
    for edge in edges {
        write(edge.id.as_bytes());
        write(edge.source.as_bytes());
        write(edge.target.as_bytes());
        for id in &edge.semantic_ids {
            write(id.as_bytes());
        }
    }
    format!("provider-projection-v1-{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Algorithm, LayoutRequest, QualityProfile, solve, validate};
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use visualyze_model::provider::{
        Boundary, BundleRecord, BundleWriter, Claim, Coverage, EntityDetail, EntityRecord,
        FORMAT_VERSION, Limits, LocalBundle, Manifest, Profile, RelationKind,
    };

    #[test]
    fn partial_group_retention_preserves_the_original_attraction_reference() {
        let peers: BTreeSet<_> = (0..64).map(|i| format!("peer:{i:02}")).collect();
        let mut relations = BTreeMap::new();
        let mut representatives = BTreeMap::new();
        for i in 0..64 {
            for j in [(i * 7 + 17) % 64, (i * 13 + 11) % 64, (i + 1) % 64] {
                if i == j {
                    continue;
                }
                let id = format!("r:{i:02}:{j:02}");
                let source = format!("peer:{i:02}");
                let target = format!("peer:{j:02}");
                representatives.insert(id.clone(), (source.clone(), target.clone()));
                relations.insert(
                    id.clone(),
                    RelationRecord {
                        id,
                        kind: RelationKind::CodeReferences,
                        source,
                        target,
                        claim: Claim::declared(),
                        label: None,
                        aspect: None,
                    },
                );
            }
        }
        let original = coarsen_region(&peers, &BTreeSet::new(), &relations, &representatives, 4);
        let expected: BTreeSet<_> = original.iter().cloned().collect();
        for stride in 2..=5 {
            for start in 0..stride {
                let retained: Vec<_> = original
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| i % stride == start)
                    .map(|(_, group)| group.clone())
                    .collect();
                let occupied = retained.iter().flatten().cloned().collect();
                let reconstructed =
                    coarsen_region(&peers, &occupied, &relations, &representatives, 4);
                let actual: BTreeSet<_> = retained.into_iter().chain(reconstructed).collect();
                assert_eq!(
                    actual, expected,
                    "partial retention changed membership at {start}/{stride}"
                );
            }
        }
    }

    struct Fixture {
        bundle: LocalBundle,
        path: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }

    fn fixture() -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "visualyze-provider-projection-{}-{}.jsonl",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut writer = BundleWriter::create(
            &path,
            Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "test".into(),
                revision: 1,
                root_id: "root".into(),
                profiles: vec![Profile::Architecture, Profile::Data, Profile::Messaging],
                coverage: Coverage::Complete,
                scope_description: "Declared adapter test".into(),
                omissions: vec![],
            },
            Limits::default(),
        )
        .unwrap();
        for (id, kind) in [
            ("db", EntityKind::Datastore),
            ("interface", EntityKind::Interface),
            ("queue", EntityKind::MessageChannel),
            ("root", EntityKind::Scope),
            ("service", EntityKind::Service),
            ("table-a", EntityKind::Table),
            ("table-b", EntityKind::Table),
        ] {
            writer
                .push(&BundleRecord::Entity(EntityRecord {
                    summary: EntitySummary {
                        id: id.into(),
                        kind,
                        label: id.into(),
                        boundary: Boundary::Internal,
                        claim: Claim::declared(),
                    },
                    detail: (id == "db").then(|| EntityDetail {
                        description: "Table ownership".into(),
                        subtype: None,
                        roles: vec![],
                        technology: vec![],
                    }),
                }))
                .unwrap();
        }
        for (id, kind, source, target) in [
            ("r1", RelationKind::Contains, "root", "db"),
            ("r2", RelationKind::Contains, "root", "queue"),
            ("r3", RelationKind::Contains, "root", "service"),
            ("r4", RelationKind::OwnsTable, "db", "table-a"),
            ("r5", RelationKind::OwnsTable, "db", "table-b"),
            ("r6", RelationKind::TableReferences, "table-a", "table-b"),
            ("r7", RelationKind::Publishes, "service", "queue"),
            ("r8", RelationKind::Writes, "service", "db"),
            ("r9", RelationKind::Exposes, "service", "interface"),
        ] {
            writer
                .push(&BundleRecord::Relation(RelationRecord {
                    id: id.into(),
                    kind,
                    source: source.into(),
                    target: target.into(),
                    claim: Claim::declared(),
                    label: None,
                    aspect: None,
                }))
                .unwrap();
        }
        Fixture {
            bundle: writer.finish().unwrap(),
            path,
        }
    }

    #[test]
    fn table_focus_is_bounded_truthful_and_geometrically_valid() {
        let mut fixture = fixture();
        let request = ProviderViewRequest {
            focus: Some("db".into()),
            ..ProviderViewRequest::default()
        };
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        assert!(projection.complete);
        assert_eq!(projection.entities.len(), 3);
        assert_eq!(projection.entities["table-a"].kind, EntityKind::Table);
        assert_eq!(
            projection.relations["r6"].kind,
            RelationKind::TableReferences
        );
        assert!(projection.graph.relation_metadata.is_empty());
        assert_eq!(projection.detail_states["db"], DetailState::Loaded);
        assert_eq!(
            projection.detail_states["table-a"],
            DetailState::NotSupplied
        );
        assert!(
            projection
                .graph
                .references
                .values()
                .any(|id| id == "service")
        );
        let pins = BTreeMap::new();
        let solution = solve(LayoutRequest {
            projection: &projection.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        validate(&projection.graph, &solution, &pins, 2).unwrap();
    }

    #[test]
    fn asynchronous_predicates_and_unloaded_detail_remain_distinct() {
        let mut fixture = fixture();
        let request = ProviderViewRequest {
            load_focus_detail: false,
            ..ProviderViewRequest::default()
        };
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        assert_eq!(projection.relations["r7"].kind, RelationKind::Publishes);
        assert_eq!(projection.detail_states["db"], DetailState::Unloaded);
        assert!(!projection.entities.contains_key("table-a"));
        assert!(
            projection
                .graph
                .diagnostics
                .iter()
                .any(|message| message.contains("Descendant boundary"))
        );
    }

    #[test]
    fn budget_exhaustion_has_explicit_continuations_instead_of_false_completeness() {
        let mut fixture = fixture();
        let request = ProviderViewRequest {
            max_children: 1,
            max_relation_records: 1,
            max_references: 0,
            ..ProviderViewRequest::default()
        };
        let projection =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        assert!(!projection.complete);
        assert!(!projection.continuations.is_empty());
        assert!(projection.graph.nodes.len() <= 2);
        let Continuation::Children { after, .. } = &projection.continuations[0] else {
            panic!("missing child continuation")
        };
        let next = ProviderViewRequest {
            children_after: Some(after.clone()),
            ..request
        };
        let next = project_provider(&mut fixture.bundle, &next, &AtomicBool::new(false)).unwrap();
        assert_ne!(projection.graph.key, next.graph.key);
    }

    #[test]
    fn cancellation_and_stale_revision_publish_nothing() {
        let mut fixture = fixture();
        assert!(matches!(
            project_provider(
                &mut fixture.bundle,
                &ProviderViewRequest::default(),
                &AtomicBool::new(true)
            ),
            Err(LayoutError::Cancelled)
        ));
        let request = ProviderViewRequest {
            expected_revision: Some(2),
            ..ProviderViewRequest::default()
        };
        assert!(project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn mixed_expansion_loads_only_explicit_branches_and_preserves_supplied_parents() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
        let overview = project_provider(
            &mut provider,
            &ProviderViewRequest::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let request = ProviderViewRequest {
            expanded: BTreeSet::from(["datastore:orders".into(), "table:orders".into()]),
            ..ProviderViewRequest::default()
        };
        let mixed = project_provider(&mut provider, &request, &AtomicBool::new(false)).unwrap();
        let nodes: BTreeMap<_, _> = mixed
            .graph
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect();
        assert_eq!(
            nodes["table:orders"].parent.as_deref(),
            Some("datastore:orders")
        );
        assert_eq!(
            nodes["column:customer-id"].parent.as_deref(),
            Some("table:orders")
        );
        assert!(!nodes["datastore:orders"].solid);
        assert!(!nodes["table:orders"].solid);
        assert!(nodes["service:orders"].solid);
        assert!(nodes["code:repository"].solid);
        assert!(!mixed.entities.contains_key("code:package"));
        assert!(!mixed.entities.contains_key("code:file"));
        for id in ["service:orders", "service:worker", "code:repository"] {
            let old = overview
                .graph
                .nodes
                .iter()
                .find(|node| node.id == id)
                .unwrap();
            assert_eq!(nodes[id].preferred_size, old.preferred_size);
            assert_eq!(nodes[id].parent, old.parent);
        }
        assert_eq!(
            mixed.graph.semantic_to_display["table:orders"],
            "table:orders"
        );
        assert_eq!(mixed.relations["rel:writes"].target, "table:orders");
        assert_eq!(
            mixed.relations["rel:publishes"].kind,
            RelationKind::Publishes
        );
        assert!(mixed.complete);
        assert!(mixed.graph.nodes.len() <= request.visible_budget);
        assert!(
            mixed
                .graph
                .edges
                .iter()
                .all(|edge| edge.semantic_ids == [edge.id.clone()])
        );
    }

    #[test]
    fn default_datastore_expansion_succeeds_without_moving_unrelated_boxes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../public/examples/data-messaging.bundle.jsonl");
        let mut provider = LocalBundle::open(path, Limits::default()).unwrap();
        let overview = project_provider(
            &mut provider,
            &ProviderViewRequest::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let request = ProviderViewRequest {
            expanded: BTreeSet::from(["datastore:orders".into()]),
            ..ProviderViewRequest::default()
        };
        let mixed = project_provider(&mut provider, &request, &AtomicBool::new(false)).unwrap();
        let pins = BTreeMap::new();
        let solve_view = |graph: &Projection, previous| {
            solve(LayoutRequest {
                projection: graph,
                algorithm: Algorithm::Floorplan,
                pins: &pins,
                previous,
                quality: QualityProfile::default(),
                work_budget: 32,
                cancelled: &AtomicBool::new(false),
            })
        };
        let accepted = solve_view(&overview.graph, None).unwrap();
        let fresh_mixed = solve_view(&mixed.graph, None).unwrap();
        validate(&mixed.graph, &fresh_mixed, &pins, 2).unwrap();
        let repaired = solve_view(&mixed.graph, Some(&accepted))
            .expect("default datastore expansion must fit into the free lower region");
        validate(&mixed.graph, &repaired, &pins, 2).unwrap();
        for id in [
            "code:repository",
            "client:web",
            "channel:orders",
            "service:orders",
            "service:worker",
        ] {
            assert_eq!(
                repaired.boxes[id].bounds, accepted.boxes[id].bounds,
                "unaffected sibling moved: {id}"
            );
        }
        assert_eq!(
            repaired.boxes["datastore:orders"].bounds.max.y,
            accepted.boxes["datastore:orders"].bounds.max.y
        );
    }

    #[test]
    fn mixed_expansion_budget_exhaustion_names_the_partial_region() {
        let mut fixture = fixture();
        let request = ProviderViewRequest {
            expanded: BTreeSet::from(["db".into()]),
            visible_budget: 5,
            ..ProviderViewRequest::default()
        };
        let mixed =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        assert!(!mixed.complete);
        assert!(mixed.graph.nodes.len() <= 5);
        assert!(mixed.region_diagnostics.contains_key("db"));
        assert!(mixed.continuations.iter().any(|continuation| matches!(continuation, Continuation::Children { parent, .. } | Continuation::PendingChildren { parent } if parent == "db")));
        assert!(mixed.unrepresented_relation_ids.is_empty());
        assert_eq!(mixed.internal_relation_counts["db"], 1);
    }

    #[test]
    fn expanding_exposed_interfaces_keeps_the_solid_owner_and_face_attachment() {
        let mut fixture = fixture();
        let request = ProviderViewRequest {
            expanded: BTreeSet::from(["service".into()]),
            ..ProviderViewRequest::default()
        };
        let mixed =
            project_provider(&mut fixture.bundle, &request, &AtomicBool::new(false)).unwrap();
        let owner = mixed
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "service")
            .unwrap();
        let interface = mixed
            .graph
            .nodes
            .iter()
            .find(|node| node.id == "interface")
            .unwrap();
        assert!(owner.solid);
        assert_eq!(interface.kind, NodeKind::Interface);
        assert_eq!(interface.parent.as_deref(), Some("service"));
        assert_eq!(mixed.relations["r9"].kind, RelationKind::Exposes);
        let pins = BTreeMap::new();
        let solution = solve(LayoutRequest {
            projection: &mixed.graph,
            algorithm: Algorithm::Floorplan,
            pins: &pins,
            previous: None,
            quality: QualityProfile::default(),
            work_budget: 16,
            cancelled: &AtomicBool::new(false),
        })
        .unwrap();
        assert_eq!(
            solution.boxes["service"].bounds.max.z,
            solution.boxes["interface"].bounds.min.z
        );
        validate(&mixed.graph, &solution, &pins, 2).unwrap();
    }
}
