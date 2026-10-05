use std::{collections::HashMap, sync::Arc};

use super::{BundleError, Limits, types::*};

#[derive(Debug, Clone, Copy)]
pub(super) struct Span {
    pub offset: u64,
    pub len: usize,
    pub checksum: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum Location {
    Entity(usize),
    Relation(usize),
    Evidence(Span),
}

pub(super) struct EntityIndex {
    pub id: Arc<str>,
    pub kind: EntityKind,
    pub label: Arc<str>,
    pub preorder: usize,
    pub subtree_end: usize,
    pub depth: usize,
    /// Ancestor at depth `depth & (depth - 1)`, or self at the root.
    /// One pointer bounds deep ascents without a model-sized lifting table.
    pub jump: usize,
    pub boundary: Boundary,
    pub span: Span,
    pub has_detail: bool,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub incident: Vec<usize>,
    pub deployment_parent: Option<usize>,
    pub instance_of: Option<usize>,
    pub interface_use: u8,
}

pub(super) struct RelationIndex {
    pub span: Span,
    pub id: Arc<str>,
    pub source: u32,
    pub target: u32,
    pub kind: RelationKind,
}

pub(super) struct Index {
    pub ids: HashMap<Arc<str>, Location>,
    pub entities: Vec<EntityIndex>,
    pub relations: Vec<RelationIndex>,
    pub counts: Counts,
    pub estimated_bytes: usize,
    /// Compact sorted (parent, child) pairs permit bounded lens pages without
    /// allocating a child vector for every entity in every view.
    lens_children: [Vec<(u32, u32)>; 3],
    deployment_positions: HashMap<usize, Position>,
    /// Sorted (ownership preorder, relation) endpoints. Querying a subtree
    /// visits its incident endpoints, independent of unrelated relation IDs.
    region_endpoints: Vec<(u32, u32)>,
    deployment_endpoints: Vec<(u32, u32)>,
    root: usize,
}

#[derive(Clone, Copy)]
pub(super) struct Position {
    pub start: usize,
    pub end: usize,
    pub depth: usize,
    pub jump: usize,
}

/// Final validation does no I/O, but may still traverse millions of indexed
/// entries. Check cancellation after bounded deterministic work, including deep
/// chains and large sibling/lens sorts. The partial index is never published.
const FINALIZE_CHUNK: usize = 1024;
struct FinalizeWork<'a, F> {
    check: &'a mut F,
    remaining: usize,
}
impl<F: FnMut() -> Result<(), BundleError>> FinalizeWork<'_, F> {
    fn checkpoint(&mut self) -> Result<(), BundleError> {
        (self.check)()?;
        self.remaining = FINALIZE_CHUNK;
        Ok(())
    }
    fn advance(&mut self, units: usize) -> Result<(), BundleError> {
        debug_assert!(units <= FINALIZE_CHUNK);
        if units > self.remaining {
            self.checkpoint()?;
        }
        self.remaining -= units;
        Ok(())
    }
    fn step(&mut self) -> Result<(), BundleError> {
        self.advance(1)
    }
}

/// In-place heapsort supplies bounded interruption points without an uncharged
/// model-sized merge buffer. Already ordered indexes take only one linear scan.
fn cancellable_sort<T: Ord>(
    values: &mut [T],
    work: &mut FinalizeWork<'_, impl FnMut() -> Result<(), BundleError>>,
) -> Result<(), BundleError> {
    if values.len() <= FINALIZE_CHUNK {
        work.advance(values.len())?;
        values.sort_unstable();
        return Ok(());
    }
    let mut sorted = true;
    for pair in values.windows(2) {
        work.step()?;
        if pair[0] > pair[1] {
            sorted = false;
            break;
        }
    }
    if sorted {
        return Ok(());
    }
    fn sift<T: Ord>(
        values: &mut [T],
        mut root: usize,
        end: usize,
        work: &mut FinalizeWork<'_, impl FnMut() -> Result<(), BundleError>>,
    ) -> Result<(), BundleError> {
        while root * 2 + 1 < end {
            work.step()?;
            let mut child = root * 2 + 1;
            if child + 1 < end && values[child] < values[child + 1] {
                child += 1;
            }
            if values[root] >= values[child] {
                break;
            }
            values.swap(root, child);
            root = child;
        }
        Ok(())
    }
    let len = values.len();
    for root in (0..len / 2).rev() {
        work.step()?;
        sift(values, root, len, work)?;
    }
    for end in (1..len).rev() {
        work.step()?;
        values.swap(0, end);
        sift(values, 0, end, work)?;
    }
    Ok(())
}

impl Index {
    pub fn new() -> Self {
        Self {
            ids: HashMap::new(),
            entities: Vec::new(),
            relations: Vec::new(),
            counts: Counts::default(),
            estimated_bytes: 0,
            lens_children: std::array::from_fn(|_| Vec::new()),
            deployment_positions: HashMap::new(),
            region_endpoints: Vec::new(),
            deployment_endpoints: Vec::new(),
            root: 0,
        }
    }

    pub fn entity_index(&self, id: &str) -> Option<usize> {
        match self.ids.get(id) {
            Some(Location::Entity(i)) => Some(*i),
            _ => None,
        }
    }

    pub fn add(
        &mut self,
        record: &BundleRecord,
        span: Span,
        manifest: &Manifest,
        limits: &Limits,
    ) -> Result<(), BundleError> {
        let (id, _) = record
            .identity()
            .ok_or_else(|| invalid("expected an evidence, entity, or relation record"))?;
        if self.ids.contains_key(id) {
            return Err(invalid(format!("duplicate identity: {id}")));
        }
        validate_record(record, manifest)?;
        let id: Arc<str> = Arc::from(id);
        let location = match record {
            BundleRecord::Evidence(_) => {
                self.counts.evidence += 1;
                if self.counts.evidence > limits.max_evidence {
                    return Err(BundleError::Limit("evidence count"));
                }
                self.charge(192 + id.len(), limits)?;
                Location::Evidence(span)
            }
            BundleRecord::Entity(entity) => {
                self.claim(&entity.summary.claim)?;
                self.counts.entities += 1;
                if self.counts.entities > limits.max_entities {
                    return Err(BundleError::Limit("entity count"));
                }
                // Conservative accounting includes hash/vector capacity slack and allocation
                // headers. Actual allocator RSS is measured separately by the benchmark.
                self.charge(592 + id.len() + entity.summary.label.len(), limits)?;
                let i = self.entities.len();
                self.entities.push(EntityIndex {
                    id: Arc::clone(&id),
                    kind: entity.summary.kind,
                    label: Arc::from(entity.summary.label.as_str()),
                    preorder: 0,
                    subtree_end: 0,
                    depth: 0,
                    jump: 0,
                    boundary: entity.summary.boundary,
                    span,
                    has_detail: entity.detail.is_some(),
                    parent: None,
                    children: Vec::new(),
                    incident: Vec::new(),
                    deployment_parent: None,
                    instance_of: None,
                    interface_use: 0,
                });
                Location::Entity(i)
            }
            BundleRecord::Relation(relation) => {
                self.claim(&relation.claim)?;
                self.counts.relations += 1;
                if self.counts.relations > limits.max_relations {
                    return Err(BundleError::Limit("relation count"));
                }
                self.charge(320 + id.len(), limits)?;
                let source = self
                    .entity_index(&relation.source)
                    .ok_or_else(|| invalid(format!("unknown source: {}", relation.source)))?;
                let target = self
                    .entity_index(&relation.target)
                    .ok_or_else(|| invalid(format!("unknown target: {}", relation.target)))?;
                self.validate_endpoints(relation, source, target)?;
                if self.entities[target].kind == EntityKind::Interface {
                    self.entities[target].interface_use |= match relation.kind {
                        RelationKind::Requests => 1,
                        RelationKind::Reads | RelationKind::Writes => 2,
                        _ => 0,
                    };
                }
                if relation.kind.is_ownership() {
                    if self.entities[target].parent.replace(source).is_some() {
                        return Err(invalid(format!("multiple owners: {}", relation.target)));
                    }
                    if relation.kind != RelationKind::Contains
                        && self.entities[source].boundary != self.entities[target].boundary
                    {
                        return Err(invalid("an owned object must share its owner's boundary"));
                    }
                    self.entities[source].children.push(target);
                } else if relation.kind == RelationKind::DeployedIn
                    && matches!(
                        self.entities[source].kind,
                        EntityKind::Deployment | EntityKind::DeploymentInstance
                    )
                {
                    if self.entities[source]
                        .deployment_parent
                        .replace(target)
                        .is_some()
                    {
                        return Err(invalid("deployment context or instance has multiple homes"));
                    }
                } else if relation.kind == RelationKind::InstanceOf
                    && self.entities[source].instance_of.replace(target).is_some()
                {
                    return Err(invalid("an instance represents exactly one component"));
                }
                let i = self.relations.len();
                self.relations.push(RelationIndex {
                    span,
                    id: Arc::clone(&id),
                    source: u32::try_from(source)
                        .map_err(|_| BundleError::Limit("entity index"))?,
                    target: u32::try_from(target)
                        .map_err(|_| BundleError::Limit("entity index"))?,
                    kind: relation.kind,
                });
                self.entities[source].incident.push(i);
                self.entities[target].incident.push(i);
                Location::Relation(i)
            }
            _ => unreachable!(),
        };
        self.ids.insert(id, location);
        Ok(())
    }

    fn charge(&mut self, bytes: usize, limits: &Limits) -> Result<(), BundleError> {
        self.estimated_bytes = self
            .estimated_bytes
            .checked_add(bytes)
            .ok_or(BundleError::Limit("index bytes"))?;
        if self.estimated_bytes > limits.max_index_bytes {
            return Err(BundleError::Limit("index bytes"));
        }
        Ok(())
    }

    fn claim(&self, claim: &Claim) -> Result<(), BundleError> {
        for id in &claim.evidence_ids {
            if !matches!(self.ids.get(id.as_str()), Some(Location::Evidence(_))) {
                return Err(invalid(format!("unresolved evidence: {id}")));
            }
        }
        Ok(())
    }

    fn validate_endpoints(
        &self,
        relation: &RelationRecord,
        source: usize,
        target: usize,
    ) -> Result<(), BundleError> {
        use EntityKind as E;
        use RelationKind as R;
        if source == target {
            return Err(invalid("self-relations are not supported in bundle v1"));
        }
        let s = self.entities[source].kind;
        let t = self.entities[target].kind;
        // Interface ownership may occur later in canonical relation order. Interface
        // request/read target-owner compatibility is checked in finish().
        let allowed = match relation.kind {
            R::Contains => {
                s == E::Scope
                    && matches!(
                        t,
                        E::Scope
                            | E::Client
                            | E::Service
                            | E::Datastore
                            | E::Concern
                            | E::CodeRepository
                            | E::MessageChannel
                            | E::MessageType
                    )
            }
            R::Exposes => s.component() && t == E::Interface,
            R::Requests => s.caller() && (t.caller() || t == E::Interface),
            R::Reads | R::Writes => {
                s.caller() && matches!(t, E::Datastore | E::Interface | E::Table)
            }
            R::DependsOn => s.component() && (t.component() || t == E::Interface),
            R::AppliesTo | R::ImplementedBy => {
                s == E::Concern && (t.component() || t == E::Interface)
            }
            R::DeployedIn => {
                (s.component() || matches!(s, E::Deployment | E::DeploymentInstance))
                    && t == E::Deployment
            }
            R::OwnsTable => s == E::Datastore && t == E::Table,
            R::OwnsColumn => s == E::Table && t == E::Column,
            R::OwnsCode => matches!(
                (s, t),
                (E::CodeRepository, E::CodePackage | E::CodeFile)
                    | (E::CodePackage, E::CodePackage | E::CodeFile)
                    | (E::CodeFile, E::CodeType | E::CodeFunction)
                    | (E::CodeType, E::CodeType | E::CodeFunction)
            ),
            R::TableReferences => {
                matches!((s, t), (E::Table, E::Table) | (E::Column, E::Column))
            }
            R::CodeReferences => s.code() && t.code(),
            R::Publishes | R::Subscribes => s.caller() && t == E::MessageChannel,
            R::Carries => s == E::MessageChannel && t == E::MessageType,
            R::InstanceOf => s == E::DeploymentInstance && t.component(),
        };
        if !allowed {
            return Err(invalid(format!("invalid endpoints on {}", relation.id)));
        }
        Ok(())
    }

    pub fn finish(
        &mut self,
        manifest: &Manifest,
        limits: &Limits,
        mut check: impl FnMut() -> Result<(), BundleError>,
    ) -> Result<(), BundleError> {
        let mut work = FinalizeWork {
            check: &mut check,
            remaining: FINALIZE_CHUNK,
        };
        work.checkpoint()?;
        let root = self
            .entity_index(&manifest.root_id)
            .ok_or_else(|| invalid("root_id does not resolve to an entity"))?;
        let root_entity = &self.entities[root];
        if root_entity.kind != EntityKind::Scope
            || root_entity.boundary != Boundary::Internal
            || root_entity.parent.is_some()
        {
            return Err(invalid("root must be an unowned internal scope"));
        }
        self.root = root;
        self.validate_forest(false, &mut work)?;
        self.validate_forest(true, &mut work)?;
        let mut reachable = vec![false; self.entities.len()];
        let mut pending = vec![root];
        while let Some(i) = pending.pop() {
            work.step()?;
            reachable[i] = true;
            for child in &self.entities[i].children {
                work.step()?;
                pending.push(*child);
            }
        }
        for (i, entity) in self.entities.iter().enumerate() {
            work.step()?;
            use EntityKind as E;
            if matches!(
                entity.kind,
                E::Interface
                    | E::Table
                    | E::Column
                    | E::CodePackage
                    | E::CodeFile
                    | E::CodeType
                    | E::CodeFunction
            ) && entity.parent.is_none()
            {
                return Err(invalid(format!("missing owner: {}", entity.id)));
            }
            if entity.kind == E::DeploymentInstance
                && (entity.instance_of.is_none() || entity.deployment_parent.is_none())
            {
                return Err(invalid(format!(
                    "instance requires component and location: {}",
                    entity.id
                )));
            }
            if entity.kind == E::Interface {
                let owner = &self.entities[entity.parent.expect("checked interface owner")];
                if (entity.interface_use & 1 != 0 && !owner.kind.caller())
                    || (entity.interface_use & 2 != 0 && owner.kind != E::Datastore)
                {
                    return Err(invalid(format!(
                        "interface owner disagrees with request/read/write use: {}",
                        entity.id
                    )));
                }
            }
            if i != root
                && entity.boundary == Boundary::Internal
                && !matches!(entity.kind, E::Deployment | E::DeploymentInstance)
                && !reachable[i]
            {
                return Err(invalid(format!(
                    "internal entity is outside root: {}",
                    entity.id
                )));
            }
        }
        // Input relations are sorted by identity, but children should be sorted by
        // child identity, independent of how structural relation IDs were named.
        for entity in &mut self.entities {
            work.step()?;
            cancellable_sort(&mut entity.children, &mut work)?;
        }
        // Euler intervals support descendant tests without loading or walking records.
        // Include external ownership trees as well as the main supplied scope.
        let mut clock = 0;
        let mut ancestors = Vec::new();
        for root in 0..self.entities.len() {
            work.step()?;
            if self.entities[root].parent.is_some() {
                continue;
            }
            let mut stack = vec![(root, false, 0)];
            while let Some((i, exiting, depth)) = stack.pop() {
                work.step()?;
                if exiting {
                    self.entities[i].subtree_end = clock;
                    ancestors.pop();
                } else {
                    self.entities[i].jump = if depth == 0 {
                        i
                    } else {
                        ancestors[depth & (depth - 1)]
                    };
                    ancestors.push(i);
                    self.entities[i].preorder = clock;
                    self.entities[i].depth = depth;
                    clock += 1;
                    stack.push((i, true, depth));
                    for child in self.entities[i].children.iter().rev() {
                        work.step()?;
                        stack.push((*child, false, depth + 1));
                    }
                }
            }
        }
        self.build_lens_indexes(limits, &mut work)?;
        self.build_region_indexes(limits, &mut work)?;
        work.checkpoint()?;
        Ok(())
    }

    fn build_lens_indexes(
        &mut self,
        limits: &Limits,
        work: &mut FinalizeWork<'_, impl FnMut() -> Result<(), BundleError>>,
    ) -> Result<(), BundleError> {
        work.checkpoint()?;
        for i in 0..self.entities.len() {
            work.step()?;
            for lens in [
                SemanticLens::Logical,
                SemanticLens::Code,
                SemanticLens::Deployment,
            ] {
                work.step()?;
                if !self.in_lens(lens, i) {
                    continue;
                }
                if let Some(parent) = self.parent_in(Some(lens), i) {
                    self.charge(16, limits)?;
                    self.lens_children[lens_slot(lens)].push((parent as u32, i as u32));
                }
            }
        }
        for children in &mut self.lens_children {
            cancellable_sort(children, work)?;
        }
        let mut clock = 0;
        let mut ancestors = Vec::new();
        let mut stack = vec![(self.root, false, 0)];
        while let Some((i, exiting, depth)) = stack.pop() {
            work.step()?;
            if exiting {
                self.deployment_positions
                    .get_mut(&i)
                    .expect("entered node")
                    .end = clock;
                ancestors.pop();
            } else {
                self.charge(128, limits)?;
                self.deployment_positions.insert(
                    i,
                    Position {
                        start: clock,
                        end: 0,
                        depth,
                        jump: if depth == 0 {
                            i
                        } else {
                            ancestors[depth & (depth - 1)]
                        },
                    },
                );
                ancestors.push(i);
                clock += 1;
                stack.push((i, true, depth));
                for (_, child) in self.children_in(SemanticLens::Deployment, i).iter().rev() {
                    work.step()?;
                    stack.push((*child as usize, false, depth + 1));
                }
            }
        }
        Ok(())
    }

    fn build_region_indexes(
        &mut self,
        limits: &Limits,
        work: &mut FinalizeWork<'_, impl FnMut() -> Result<(), BundleError>>,
    ) -> Result<(), BundleError> {
        // This index has an exact cardinality. Reserve once rather than doubling
        // a growing vector near the large-model memory limit. Charge capacity,
        // including any allocator-reported excess, before publishing the index.
        let entries = self
            .relations
            .len()
            .checked_mul(2)
            .ok_or(BundleError::Limit("index bytes"))?;
        self.charge(
            entries
                .checked_mul(8)
                .and_then(|n| n.checked_add(48))
                .ok_or(BundleError::Limit("index bytes"))?,
            limits,
        )?;
        self.region_endpoints
            .try_reserve_exact(entries)
            .map_err(|_| BundleError::Limit("index allocation"))?;
        self.charge((self.region_endpoints.capacity() - entries) * 8, limits)?;
        // Walk the already validated preorder and each entity's canonical
        // incident list. This produces sorted endpoints in O(N + R), avoiding
        // a second model-sized sort during import.
        let mut stack = Vec::new();
        for root in 0..self.entities.len() {
            work.step()?;
            if self.entities[root].parent.is_some() {
                continue;
            }
            stack.push(root);
            while let Some(entity) = stack.pop() {
                work.step()?;
                for &relation in &self.entities[entity].incident {
                    work.step()?;
                    self.region_endpoints
                        .push((self.entities[entity].preorder as u32, relation as u32));
                }
                for &child in self.entities[entity].children.iter().rev() {
                    work.step()?;
                    stack.push(child);
                }
            }
        }
        stack.push(self.root);
        while let Some(entity) = stack.pop() {
            work.step()?;
            let position = self.deployment_positions[&entity];
            for r in 0..self.entities[entity].incident.len() {
                work.step()?;
                let relation = self.entities[entity].incident[r];
                if self.relation_in(Some(SemanticLens::Deployment), &self.relations[relation]) {
                    // Deployment membership is sparse and discovered here;
                    // include growth capacity slack in its separate accounting.
                    self.charge(16, limits)?;
                    self.deployment_endpoints
                        .push((position.start as u32, relation as u32));
                }
            }
            for (_, child) in self
                .children_in(SemanticLens::Deployment, entity)
                .iter()
                .rev()
            {
                work.step()?;
                stack.push(*child as usize);
            }
        }
        Ok(())
    }

    pub fn region_endpoints(&self, lens: Option<SemanticLens>) -> &[(u32, u32)] {
        if lens == Some(SemanticLens::Deployment) {
            &self.deployment_endpoints
        } else {
            &self.region_endpoints
        }
    }

    pub fn in_lens(&self, lens: SemanticLens, i: usize) -> bool {
        let kind = self.entities[i].kind;
        match lens {
            SemanticLens::Logical => {
                !kind.code()
                    && !matches!(
                        kind,
                        EntityKind::Deployment | EntityKind::DeploymentInstance
                    )
            }
            SemanticLens::Code => kind == EntityKind::Scope || kind.code(),
            SemanticLens::Deployment => {
                i == self.root
                    || matches!(
                        kind,
                        EntityKind::Deployment | EntityKind::DeploymentInstance
                    )
            }
        }
    }

    pub fn parent_in(&self, lens: Option<SemanticLens>, i: usize) -> Option<usize> {
        if let Some(lens) = lens {
            if !self.in_lens(lens, i) {
                return None;
            }
            if lens == SemanticLens::Deployment {
                return (i != self.root)
                    .then(|| self.entities[i].deployment_parent.unwrap_or(self.root));
            }
        }
        self.entities[i].parent
    }

    pub fn children_in(&self, lens: SemanticLens, parent: usize) -> &[(u32, u32)] {
        let pairs = &self.lens_children[lens_slot(lens)];
        let start = pairs.partition_point(|(p, _)| (*p as usize) < parent);
        let end = pairs.partition_point(|(p, _)| (*p as usize) <= parent);
        &pairs[start..end]
    }

    pub fn position_in(&self, lens: Option<SemanticLens>, i: usize) -> Option<Position> {
        if let Some(lens) = lens {
            if !self.in_lens(lens, i) {
                return None;
            }
            if lens == SemanticLens::Deployment {
                return self.deployment_positions.get(&i).copied();
            }
        }
        let node = &self.entities[i];
        Some(Position {
            start: node.preorder,
            end: node.subtree_end,
            depth: node.depth,
            jump: node.jump,
        })
    }

    pub fn contains_in(
        &self,
        lens: Option<SemanticLens>,
        ancestor: usize,
        descendant: usize,
    ) -> bool {
        match (
            self.position_in(lens, ancestor),
            self.position_in(lens, descendant),
        ) {
            (Some(a), Some(d)) => a.start <= d.start && d.start < a.end,
            _ => false,
        }
    }

    /// Return the external branch immediately below the common ancestor, or
    /// the root of a separate ownership tree. The binary-depth jump clears a
    /// depth bit; when it crosses the common ancestor we take one parent step.
    /// This takes at most O(log²(depth)) steps with one stored pointer per node.
    pub fn external_representative(
        &self,
        lens: Option<SemanticLens>,
        entity: usize,
        focus: usize,
        mut checkpoint: impl FnMut() -> Result<(), BundleError>,
    ) -> Result<usize, BundleError> {
        let mut current = entity;
        while let Some(parent) = self.parent_in(lens, current) {
            checkpoint()?;
            if self.contains_in(lens, parent, focus) {
                break;
            }
            let jump = self
                .position_in(lens, current)
                .expect("parent has a lens position")
                .jump;
            current = if jump != current && !self.contains_in(lens, jump, focus) {
                jump
            } else {
                parent
            };
        }
        Ok(current)
    }

    pub fn relation_in(&self, lens: Option<SemanticLens>, relation: &RelationIndex) -> bool {
        let Some(lens) = lens else {
            return true;
        };
        match lens {
            SemanticLens::Logical => {
                !matches!(
                    relation.kind,
                    RelationKind::DeployedIn
                        | RelationKind::InstanceOf
                        | RelationKind::OwnsCode
                        | RelationKind::CodeReferences
                ) && self.in_lens(lens, relation.source as usize)
                    && self.in_lens(lens, relation.target as usize)
            }
            SemanticLens::Code => {
                matches!(
                    relation.kind,
                    RelationKind::Contains | RelationKind::OwnsCode | RelationKind::CodeReferences
                ) && self.in_lens(lens, relation.source as usize)
                    && self.in_lens(lens, relation.target as usize)
            }
            SemanticLens::Deployment => matches!(
                relation.kind,
                RelationKind::DeployedIn | RelationKind::InstanceOf
            ),
        }
    }

    pub fn structural_in(&self, lens: Option<SemanticLens>, relation: &RelationIndex) -> bool {
        relation.kind.is_ownership()
            || (lens == Some(SemanticLens::Deployment)
                && relation.kind == RelationKind::DeployedIn
                && matches!(
                    self.entities[relation.source as usize].kind,
                    EntityKind::Deployment | EntityKind::DeploymentInstance
                ))
    }

    fn validate_forest(
        &self,
        deployment: bool,
        work: &mut FinalizeWork<'_, impl FnMut() -> Result<(), BundleError>>,
    ) -> Result<(), BundleError> {
        work.checkpoint()?;
        let mut states = vec![0u8; self.entities.len()];
        let parent = |i: usize| {
            if deployment {
                self.entities[i].deployment_parent
            } else {
                self.entities[i].parent
            }
        };
        // Iterative three-color traversal is linear even for million-node chains.
        for start in 0..self.entities.len() {
            work.step()?;
            if states[start] != 0 {
                continue;
            }
            let mut current = Some(start);
            while let Some(i) = current {
                work.step()?;
                match states[i] {
                    0 => states[i] = 1,
                    1 => return Err(invalid("ownership/deployment cycle")),
                    _ => break,
                }
                current = parent(i);
            }
            current = Some(start);
            while let Some(i) = current {
                work.step()?;
                if states[i] != 1 {
                    break;
                }
                states[i] = 2;
                current = parent(i);
            }
        }
        Ok(())
    }
}

fn lens_slot(lens: SemanticLens) -> usize {
    match lens {
        SemanticLens::Logical => 0,
        SemanticLens::Code => 1,
        SemanticLens::Deployment => 2,
    }
}

pub(super) fn validate_manifest(manifest: &Manifest) -> Result<(), BundleError> {
    if manifest.format != FORMAT_VERSION || manifest.revision == 0 {
        return Err(invalid("unsupported bundle format or zero revision"));
    }
    valid_id(&manifest.model_id)?;
    valid_id(&manifest.root_id)?;
    valid_text(&manifest.scope_description, 4096, true)?;
    text_list(&manifest.omissions, 32, 4096)?;
    if !manifest.profiles.contains(&Profile::Architecture)
        || manifest.profiles.len() > 5
        || manifest
            .profiles
            .iter()
            .enumerate()
            .any(|(i, p)| manifest.profiles[..i].contains(p))
    {
        return Err(invalid(
            "profiles must include architecture.v1 without duplicates",
        ));
    }
    Ok(())
}

pub(super) fn validate_record(
    record: &BundleRecord,
    manifest: &Manifest,
) -> Result<(), BundleError> {
    match record {
        BundleRecord::Evidence(e) => {
            valid_id(&e.id)?;
            valid_text(&e.description, 8192, true)?;
            optional_text(&e.reference, 4096)?;
        }
        BundleRecord::Entity(e) => {
            valid_id(&e.summary.id)?;
            valid_text(&e.summary.label, 512, true)?;
            valid_claim(&e.summary.claim)?;
            require_profile(e.summary.kind.profile(), manifest)?;
            if let Some(detail) = &e.detail {
                valid_text(&detail.description, 32768, false)?;
                optional_text(&detail.subtype, 128)?;
                text_list(&detail.roles, 32, 128)?;
                text_list(&detail.technology, 32, 128)?;
            }
        }
        BundleRecord::Relation(r) => {
            valid_id(&r.id)?;
            valid_id(&r.source)?;
            valid_id(&r.target)?;
            valid_claim(&r.claim)?;
            require_profile(r.kind.profile(), manifest)?;
            optional_text(&r.label, 512)?;
            optional_text(&r.aspect, 512)?;
            if (r.kind == RelationKind::AppliesTo) != r.aspect.is_some() {
                return Err(invalid("aspect is required only for applies_to"));
            }
        }
        _ => return Err(invalid("expected a semantic record")),
    }
    Ok(())
}

fn require_profile(profile: Profile, manifest: &Manifest) -> Result<(), BundleError> {
    if manifest.profiles.contains(&profile) {
        Ok(())
    } else {
        Err(invalid(format!(
            "record requires undeclared {profile:?} profile"
        )))
    }
}

fn valid_claim(claim: &Claim) -> Result<(), BundleError> {
    if claim.evidence_ids.len() > 32 {
        return Err(BundleError::Limit("evidence references per claim"));
    }
    for (i, id) in claim.evidence_ids.iter().enumerate() {
        valid_id(id)?;
        if claim.evidence_ids[..i].contains(id) {
            return Err(invalid("duplicate claim evidence reference"));
        }
    }
    optional_text(&claim.rationale, 4096)?;
    if claim.basis == Basis::Supported && claim.evidence_ids.is_empty() {
        return Err(invalid("supported claim requires evidence"));
    }
    if claim.basis == Basis::Inferred && claim.rationale.is_none() {
        return Err(invalid("inferred claim requires a rationale"));
    }
    Ok(())
}

pub(super) fn valid_id(id: &str) -> Result<(), BundleError> {
    if id.is_empty()
        || id.len() > 128
        || !id.as_bytes()[0].is_ascii_lowercase()
        || !id
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"._:-".contains(&c))
    {
        return Err(invalid(
            "IDs require 1–128 lowercase ASCII ID characters, starting with a letter",
        ));
    }
    Ok(())
}

fn valid_text(value: &str, max: usize, required: bool) -> Result<(), BundleError> {
    if value.len() > max || value.contains('\0') || (required && value.trim().is_empty()) {
        return Err(invalid(
            "text exceeds its byte limit, is blank, or contains NUL",
        ));
    }
    Ok(())
}

fn optional_text(value: &Option<String>, max: usize) -> Result<(), BundleError> {
    if let Some(value) = value {
        valid_text(value, max, true)?;
    }
    Ok(())
}

fn text_list(values: &[String], count: usize, bytes: usize) -> Result<(), BundleError> {
    if values.len() > count {
        return Err(BundleError::Limit("text list length"));
    }
    for value in values {
        valid_text(value, bytes, true)?;
    }
    Ok(())
}

pub(super) fn invalid(message: impl Into<String>) -> BundleError {
    BundleError::Invalid(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_external_branches_match_parent_walk_with_bounded_cancellable_work() {
        const DEPTH: usize = 32_768;
        for (deployment, branch_parent) in [
            (false, 0),
            (false, DEPTH / 2 + 1),
            (true, 0),
            (true, DEPTH / 2 + 1),
        ] {
            let manifest = Manifest {
                format: FORMAT_VERSION.into(),
                model_id: "test:deep".into(),
                revision: 1,
                root_id: "e:00000".into(),
                profiles: vec![Profile::Architecture],
                coverage: Coverage::Complete,
                scope_description: "Deep ownership fixture".into(),
                omissions: vec![],
            };
            let limits = Limits::default();
            let span = Span {
                offset: 0,
                len: 1,
                checksum: 0,
            };
            let mut index = Index::new();
            for i in 0..=DEPTH + 3 {
                index
                    .add(
                        &BundleRecord::Entity(EntityRecord {
                            summary: EntitySummary {
                                id: format!("e:{i:05}"),
                                kind: if deployment && i > 0 {
                                    EntityKind::Deployment
                                } else {
                                    EntityKind::Scope
                                },
                                label: format!("Entity {i}"),
                                boundary: if i > DEPTH + 1 {
                                    Boundary::External
                                } else {
                                    Boundary::Internal
                                },
                                claim: Claim::declared(),
                            },
                            detail: None,
                        }),
                        span,
                        &manifest,
                        &limits,
                    )
                    .unwrap();
            }
            for i in 1..=DEPTH + 3 {
                if i == DEPTH + 2 {
                    continue;
                } // separate supplied external tree
                let parent = if i == DEPTH + 1 { branch_parent } else { i - 1 };
                if deployment && parent == 0 {
                    continue;
                } // virtual deployment root
                let (source, target, kind) = if deployment {
                    (i, parent, RelationKind::DeployedIn)
                } else {
                    (parent, i, RelationKind::Contains)
                };
                index
                    .add(
                        &BundleRecord::Relation(RelationRecord {
                            id: format!("r:{i:05}"),
                            kind,
                            source: format!("e:{source:05}"),
                            target: format!("e:{target:05}"),
                            claim: Claim::declared(),
                            label: None,
                            aspect: None,
                        }),
                        span,
                        &manifest,
                        &limits,
                    )
                    .unwrap();
            }
            index.finish(&manifest, &limits, || Ok(())).unwrap();
            let lens = deployment.then_some(SemanticLens::Deployment);
            for focus in [
                0,
                1,
                2,
                31,
                1024,
                DEPTH / 2 + 3,
                DEPTH - 1,
                DEPTH + 1,
                DEPTH + 2,
            ] {
                for entity in [1, 17, 1023, DEPTH - 1, DEPTH, DEPTH + 3] {
                    // Independent unaccelerated reference, including ancestors,
                    // same-branch descendants, an intermediate fork, and an
                    // independent supplied external ownership tree.
                    let mut expected = entity;
                    while let Some(parent) = index.parent_in(lens, expected) {
                        if index.contains_in(lens, parent, focus) {
                            break;
                        }
                        expected = parent;
                    }
                    let mut visits = 0;
                    let actual = index
                        .external_representative(lens, entity, focus, || {
                            visits += 1;
                            Ok(())
                        })
                        .unwrap();
                    assert_eq!(actual, expected, "{deployment} {entity} {focus}");
                    assert!(
                        visits <= 256,
                        "deep ancestry must not walk linearly: {visits}"
                    );
                }
            }
            let mut visits = 0;
            let cancelled = index.external_representative(lens, DEPTH - 1, DEPTH + 1, || {
                visits += 1;
                if visits == 3 {
                    Err(BundleError::Cancelled)
                } else {
                    Ok(())
                }
            });
            assert!(matches!(cancelled, Err(BundleError::Cancelled)));
            assert_eq!(visits, 3);
        }
    }

    #[test]
    fn cooperative_sort_matches_canonical_order_for_wide_unsorted_indexes() {
        for len in [0, 1, 1024, 1025, 8192] {
            for order in 0..3 {
                let mut values: Vec<_> = (0..len)
                    .map(|i| match order {
                        0 => (i / 7, i % 7),
                        1 => (len - i, i % 5),
                        _ => ((i * 7919 + 17) % 257, i % 31),
                    })
                    .collect();
                let mut expected = values.clone();
                expected.sort_unstable();
                let mut calls = 0;
                let mut check = || {
                    calls += 1;
                    Ok(())
                };
                let mut work = FinalizeWork {
                    check: &mut check,
                    remaining: FINALIZE_CHUNK,
                };
                cancellable_sort(&mut values, &mut work).unwrap();
                assert_eq!(values, expected);
                if len > FINALIZE_CHUNK + 1 {
                    assert!(calls > 0);
                }
            }
        }
    }

    #[test]
    fn wide_sort_cancels_before_completing_the_remaining_work() {
        let mut values: Vec<_> = (0..8192).rev().collect();
        let mut calls = 0;
        let mut check = || {
            calls += 1;
            Err(BundleError::Cancelled)
        };
        let mut work = FinalizeWork {
            check: &mut check,
            remaining: FINALIZE_CHUNK,
        };
        assert!(matches!(
            cancellable_sort(&mut values, &mut work),
            Err(BundleError::Cancelled)
        ));
        assert_eq!(calls, 1);
        assert!(values.windows(2).any(|pair| pair[0] > pair[1]));
    }
}
