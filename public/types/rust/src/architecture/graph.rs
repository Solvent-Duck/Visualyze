use std::collections::{HashMap, HashSet, VecDeque};

use super::*;

#[derive(Default)]
struct Diagnostics(Vec<Diagnostic>);

impl Diagnostics {
    fn push(&mut self, code: &'static str, path: impl Into<String>, message: impl Into<String>) {
        if self.0.len() < MAX_DIAGNOSTICS {
            self.0.push(Diagnostic::new(code, path, message));
        }
    }

    fn finish(self) -> Result<(), ValidationError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ValidationError {
                diagnostics: self.0,
            })
        }
    }
}

pub(super) fn validate(model: &Snapshot) -> Result<Vec<Diagnostic>, ValidationError> {
    let mut errors = Diagnostics::default();
    let mut warnings = Diagnostics::default();
    let mut ids = HashSet::new();
    for (collection, records) in [
        (
            "entities",
            model.entities.iter().map(|e| &e.id).collect::<Vec<_>>(),
        ),
        ("relations", model.relations.iter().map(|e| &e.id).collect()),
        ("evidence", model.evidence.iter().map(|e| &e.id).collect()),
    ] {
        for (i, id) in records.into_iter().enumerate() {
            if !ids.insert(id) {
                errors.push(
                    "duplicate_id",
                    format!("/{collection}/{i}/id"),
                    "IDs must be unique across entities, relations, and evidence",
                );
            }
        }
    }
    let entities: HashMap<_, _> = model.entities.iter().map(|e| (e.id.as_str(), e)).collect();
    let evidence: HashSet<_> = model.evidence.iter().map(|e| e.id.as_str()).collect();
    if !entities.contains_key(model.system_id.as_str()) {
        errors.push(
            "missing_reference",
            "/system_id",
            "system_id must reference an entity",
        );
    }
    for (i, relation) in model.relations.iter().enumerate() {
        for (field, id) in [("source", &relation.source), ("target", &relation.target)] {
            if !entities.contains_key(id.as_str()) {
                errors.push(
                    "missing_reference",
                    format!("/relations/{i}/{field}"),
                    "endpoint must reference an entity",
                );
            }
        }
    }
    for (collection, claims) in [
        (
            "entities",
            model
                .entities
                .iter()
                .map(|e| &e.evidence_ids)
                .collect::<Vec<_>>(),
        ),
        (
            "relations",
            model.relations.iter().map(|e| &e.evidence_ids).collect(),
        ),
    ] {
        for (i, refs) in claims.into_iter().enumerate() {
            for (j, id) in refs.iter().enumerate() {
                if !evidence.contains(id.as_str()) {
                    errors.push(
                        "missing_evidence",
                        format!("/{collection}/{i}/evidence_ids/{j}"),
                        "evidence ID must resolve to an evidence record",
                    );
                }
            }
        }
    }
    // Stop before indexing an ambiguous graph or dereferencing absent records.
    errors.finish()?;
    let mut errors = Diagnostics::default();
    let root = entities[model.system_id.as_str()];
    if root.kind != EntityKind::Scope
        || root.subtype.as_deref() != Some("system")
        || root.boundary != Boundary::Internal
    {
        errors.push(
            "invalid_system",
            "/system_id",
            "root must be an internal scope with subtype system",
        );
    }

    let mut parents = HashMap::new();
    let mut owners = HashMap::new();
    for (i, relation) in model.relations.iter().enumerate() {
        let source = entities[relation.source.as_str()];
        let target = entities[relation.target.as_str()];
        let path = format!("/relations/{i}");
        if source.id == target.id {
            errors.push(
                "self_relation",
                &path,
                "direct self-relations are not supported in this draft",
            );
        }
        match relation.kind {
            RelationKind::Contains => {
                if source.kind != EntityKind::Scope || !target.kind.is_logical() {
                    errors.push(
                        "invalid_relation_endpoint",
                        &path,
                        "contains requires a scope and a logical scope/component/concern",
                    );
                }
                if parents
                    .insert(target.id.as_str(), source.id.as_str())
                    .is_some()
                {
                    errors.push(
                        "multiple_parents",
                        &path,
                        "an entity can have only one incoming contains relation",
                    );
                }
            }
            RelationKind::Exposes => {
                if !source.kind.is_component() || target.kind != EntityKind::Interface {
                    errors.push(
                        "invalid_relation_endpoint",
                        &path,
                        "exposes requires a component and an interface",
                    );
                }
                if owners
                    .insert(target.id.as_str(), source.id.as_str())
                    .is_some()
                {
                    errors.push(
                        "multiple_interface_owners",
                        &path,
                        "an interface must have exactly one owner",
                    );
                }
                if source.boundary != target.boundary {
                    errors.push(
                        "interface_boundary_mismatch",
                        &path,
                        "interface boundary must match its owner",
                    );
                }
            }
            RelationKind::AppliesTo | RelationKind::ImplementedBy => {
                if source.kind != EntityKind::Concern
                    || !(target.kind.is_component() || target.kind == EntityKind::Interface)
                {
                    errors.push(
                        "invalid_relation_endpoint",
                        &path,
                        "concern relations require a concern and a component/interface",
                    );
                }
            }
            RelationKind::DeployedIn => {
                if !(source.kind.is_component() || source.kind == EntityKind::Deployment)
                    || target.kind != EntityKind::Deployment
                {
                    errors.push(
                        "invalid_relation_endpoint",
                        &path,
                        "deployed_in requires a component/deployment and a deployment",
                    );
                }
            }
            RelationKind::DependsOn => {
                if !source.kind.is_component()
                    || !(target.kind.is_component() || target.kind == EntityKind::Interface)
                {
                    errors.push(
                        "invalid_relation_endpoint",
                        &path,
                        "depends_on requires a component and a component/interface",
                    );
                }
            }
            RelationKind::Requests
            | RelationKind::Reads
            | RelationKind::Writes
            | RelationKind::Publishes
            | RelationKind::Subscribes => {}
        }
    }
    for (i, relation) in model.relations.iter().enumerate() {
        if matches!(
            relation.kind,
            RelationKind::Requests
                | RelationKind::Reads
                | RelationKind::Writes
                | RelationKind::Publishes
                | RelationKind::Subscribes
        ) {
            let source = entities[relation.source.as_str()];
            let target = entities[relation.target.as_str()];
            let effective_target = if target.kind == EntityKind::Interface {
                owners.get(target.id.as_str()).map(|owner| entities[owner])
            } else {
                Some(target)
            };
            let target_valid = effective_target.is_some_and(|e| match relation.kind {
                RelationKind::Requests => e.kind.is_caller(),
                RelationKind::Publishes | RelationKind::Subscribes => e.kind == EntityKind::Service,
                _ => e.kind == EntityKind::Datastore,
            });
            let source_valid = match relation.kind {
                RelationKind::Subscribes => source.kind.is_component(),
                _ => source.kind.is_caller(),
            };
            if !source_valid || !target_valid {
                errors.push("invalid_relation_endpoint", format!("/relations/{i}"), "requests target a caller; reads/writes target storage (directly or through its interface); publishes/subscribes target a message broker service (directly or through a topic interface); sources are clients/services, except that any component may subscribe");
            }
        }
    }
    if parents.contains_key(model.system_id.as_str()) {
        errors.push(
            "contained_system",
            "/system_id",
            "root system cannot have a logical parent",
        );
    }
    let containment_cycle = has_cycle(model, RelationKind::Contains);
    if containment_cycle {
        errors.push(
            "containment_cycle",
            "/relations",
            "logical containment must be acyclic",
        );
    }
    if has_cycle(model, RelationKind::DeployedIn) {
        errors.push(
            "deployment_cycle",
            "/relations",
            "deployment nesting must be acyclic",
        );
    }
    for (i, entity) in model.entities.iter().enumerate() {
        if entity.kind == EntityKind::Interface && !owners.contains_key(entity.id.as_str()) {
            errors.push(
                "interface_owner_missing",
                format!("/entities/{i}"),
                "interface requires one incoming exposes relation",
            );
        }
        if !containment_cycle && entity.kind.is_logical() && entity.boundary == Boundary::Internal {
            let mut ancestor = entity.id.as_str();
            while let Some(parent) = parents.get(ancestor) {
                ancestor = parent;
            }
            if ancestor != model.system_id {
                errors.push(
                    "uncontained_internal_entity",
                    format!("/entities/{i}"),
                    "internal logical objects require a contains path from system_id",
                );
            }
        }
        if let Some(subtype) = &entity.subtype
            && !known_subtypes(entity.kind).contains(&subtype.as_str())
        {
            warnings.push(
                "generic_subtype",
                format!("/entities/{i}/subtype"),
                "unrecognized subtype; use the generic form for this kind",
            );
        }
        for (j, role) in entity.roles.iter().enumerate() {
            if ![
                "gateway",
                "worker",
                "scheduler",
                "shared_substrate",
                "cache",
            ]
            .contains(&role.as_str())
            {
                warnings.push(
                    "descriptive_role",
                    format!("/entities/{i}/roles/{j}"),
                    "unrecognized role retained as descriptive text",
                );
            }
        }
    }
    // In a model that draws flows, a box that takes part in none is usually a modelling
    // slip: a path that was never connected. Structure and coverage do not count. A
    // structure-only model (the object gallery) is a deliberate choice and is exempt.
    let mut connected = HashSet::new();
    for relation in &model.relations {
        if matches!(
            relation.kind,
            RelationKind::Requests
                | RelationKind::Reads
                | RelationKind::Writes
                | RelationKind::DependsOn
                | RelationKind::Publishes
                | RelationKind::Subscribes
        ) {
            connected.insert(relation.source.as_str());
            connected.insert(relation.target.as_str());
        }
    }
    for (i, entity) in model.entities.iter().enumerate() {
        if matches!(
            entity.kind,
            EntityKind::Client
                | EntityKind::Service
                | EntityKind::Datastore
                | EntityKind::Interface
        ) && !connected.is_empty()
            && !connected.contains(entity.id.as_str())
        {
            warnings.push(
                "unconnected_entity",
                format!("/entities/{i}"),
                "nothing requests, reads, writes, depends on, publishes to, or subscribes to this object; connect it with evidence, or leave it out and list it in omissions",
            );
        }
    }
    for (i, record) in model.evidence.iter().enumerate() {
        if let Some(Locator::Source {
            path,
            start_line,
            end_line,
        }) = &record.locator
        {
            let windows_absolute = path.as_bytes().get(1) == Some(&b':');
            if windows_absolute
                || path.contains(['\\', '\0'])
                || path
                    .split('/')
                    .any(|segment| matches!(segment, "" | "." | ".."))
            {
                errors.push("invalid_source_path", format!("/evidence/{i}/locator/path"), "source paths must be relative, with no empty, dot, parent, backslash, or NUL segments");
            }
            if let (Some(start), Some(end)) = (start_line, end_line)
                && end < start
            {
                errors.push(
                    "invalid_line_range",
                    format!("/evidence/{i}/locator/end_line"),
                    "end_line must be at least start_line",
                );
            }
        }
    }
    // A supported claim should point at something checkable: a line range in a source file.
    let cited: HashSet<&str> = model
        .entities
        .iter()
        .filter(|e| e.basis == Basis::Supported)
        .flat_map(|e| &e.evidence_ids)
        .chain(
            model
                .relations
                .iter()
                .filter(|r| r.basis == Basis::Supported)
                .flat_map(|r| &r.evidence_ids),
        )
        .map(String::as_str)
        .collect();
    for (i, record) in model.evidence.iter().enumerate() {
        let located = matches!(
            record.locator,
            Some(Locator::Source {
                start_line: Some(_),
                ..
            })
        );
        if cited.contains(record.id.as_str()) && !located {
            warnings.push(
                "coarse_evidence",
                format!("/evidence/{i}"),
                "supports a supported claim but has no source line range; cite path, start_line and end_line",
            );
        }
    }
    let mut claims = HashSet::new();
    for (i, r) in model.relations.iter().enumerate() {
        let key = (
            r.kind,
            &r.source,
            &r.target,
            &r.aspect,
            &r.condition,
            r.dependency_phase.unwrap_or(DependencyPhase::Unknown),
            r.necessity.unwrap_or(Necessity::Unknown),
        );
        if !claims.insert(key) {
            warnings.push(
                "duplicate_claim",
                format!("/relations/{i}"),
                "another relation already expresses this predicate, endpoints, and qualifiers",
            );
        }
    }
    errors.finish()?;
    Ok(warnings.0)
}

fn has_cycle(model: &Snapshot, kind: RelationKind) -> bool {
    let mut incoming: HashMap<&str, usize> =
        model.entities.iter().map(|e| (e.id.as_str(), 0)).collect();
    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in model.relations.iter().filter(|r| r.kind == kind) {
        *incoming
            .get_mut(r.target.as_str())
            .expect("resolved target") += 1;
        outgoing.entry(&r.source).or_default().push(&r.target);
    }
    let mut ready: VecDeque<_> = incoming
        .iter()
        .filter_map(|(&id, &count)| (count == 0).then_some(id))
        .collect();
    let mut visited = 0;
    while let Some(id) = ready.pop_front() {
        visited += 1;
        for target in outgoing.get(id).into_iter().flatten() {
            let count = incoming.get_mut(target).expect("resolved target");
            *count -= 1;
            if *count == 0 {
                ready.push_back(*target);
            }
        }
    }
    visited != model.entities.len()
}

fn known_subtypes(kind: EntityKind) -> &'static [&'static str] {
    match kind {
        EntityKind::Scope => &["system", "subsystem", "domain"],
        EntityKind::Client => &["web", "mobile", "desktop", "cli"],
        EntityKind::Service => &["application", "gateway", "worker", "scheduler", "broker"],
        EntityKind::Datastore => &[
            "relational",
            "graph",
            "document",
            "key_value",
            "object",
            "columnar",
            "time_series",
            "search",
            "log",
        ],
        EntityKind::Interface => &["api", "storage", "user_interface", "topic", "queue"],
        EntityKind::Concern => &[
            "authentication",
            "authorization",
            "service_identity",
            "data_access_control",
            "billing_entitlement",
            "observability",
        ],
        EntityKind::Deployment => &["environment", "host", "container", "process", "browser"],
    }
}
