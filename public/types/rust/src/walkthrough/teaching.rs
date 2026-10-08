//! Teaching views (ADR 0010): an agent-chosen subset of the model, laid out flat for one
//! explanation. The agent chooses the members; this module only checks the choice against
//! the model and derives the temporary snapshot the viewer lays out. Nothing here is a
//! new fact: entities and relations are copied as supplied, and the only added records
//! are the `contains` relations that flatten members under the system, which say so.
use std::collections::{BTreeSet, HashSet};

use super::{Ownership, ResolvedTeachingView, TeachingView, is_flow, push, reference_code};
use crate::architecture::{
    Architecture, Basis, Coverage, Diagnostic, EntityKind, Relation, RelationKind, Snapshot,
    ValidationError,
};

/// Teaching views larger than this are advised against, never rejected.
pub const ADVISED_MEMBERS: usize = 16;

/// Members plus the interfaces they expose: an interface travels with its owner.
fn shown(model: &Snapshot, members: &[String]) -> BTreeSet<String> {
    let mut shown: BTreeSet<String> = members.iter().cloned().collect();
    for relation in &model.relations {
        if relation.kind == RelationKind::Exposes && members.contains(&relation.source) {
            shown.insert(relation.target.clone());
        }
    }
    shown
}

pub(super) fn resolve(
    index: usize,
    view: &TeachingView,
    model: &Snapshot,
    own: &Ownership,
    errors: &mut Vec<Diagnostic>,
    warnings: &mut Vec<Diagnostic>,
) -> ResolvedTeachingView {
    let at = |rest: &str| format!("/teaching_views/{index}{rest}");
    if view.title.trim().is_empty() {
        push(errors, "blank_text", at("/title"), "title is blank".into());
    }
    let members: HashSet<&str> = view.members.iter().map(String::as_str).collect();
    for (i, id) in view.members.iter().enumerate() {
        let path = at(&format!("/members/{i}"));
        match own.entity(id) {
            None => push(
                errors,
                reference_code(own, id),
                path,
                format!("member {id} is not an entity in this model"),
            ),
            Some(_) if id == own.system => push(
                errors,
                "teaching_member_system",
                path,
                "the system is the whole model; list the objects the explanation is about".into(),
            ),
            Some(EntityKind::Interface) => push(
                errors,
                "teaching_member_interface",
                path,
                format!(
                    "{id} is an interface; list the component that exposes it, and its interfaces come with it"
                ),
            ),
            Some(_) => {
                if let Some(owner) = own.lineage(id)[1..].iter().find(|o| members.contains(*o)) {
                    push(
                        errors,
                        "nested_teaching_member",
                        path,
                        format!(
                            "{id} is inside member {owner}; a teaching view is flat, so list either the container or what it holds"
                        ),
                    );
                }
            }
        }
    }
    if view.members.len() > ADVISED_MEMBERS {
        push(
            warnings,
            "busy_teaching_view",
            at("/members"),
            format!(
                "{} members is a lot for one teaching view; keep it to the objects the explanation visits",
                view.members.len()
            ),
        );
    }
    let entities = shown(model, &view.members);
    let mut relations = BTreeSet::new();
    let mut outside_connections = 0;
    for relation in &model.relations {
        let ends = [&relation.source, &relation.target]
            .iter()
            .filter(|id| entities.contains(id.as_str()))
            .count();
        if ends == 2 && relation.kind != RelationKind::Contains {
            relations.insert(relation.id.clone());
        } else if ends == 1 && is_flow(relation.kind) {
            outside_connections += 1;
        }
    }
    ResolvedTeachingView {
        id: view.id.clone(),
        entities,
        relations,
        outside_connections,
        model_entities: model.entities.len(),
    }
}

/// The temporary snapshot for one teaching view: the same model ID and revision, the
/// members directly inside the system, and every non-containment relation between what is
/// shown. It is for layout only and is never written anywhere.
pub fn teaching_model(
    model: &Architecture,
    view: &TeachingView,
) -> Result<Architecture, ValidationError> {
    let full = model.snapshot();
    let shown = shown(full, &view.members);
    let mut snapshot: Snapshot = full.clone();
    snapshot
        .entities
        .retain(|e| e.id == full.system_id || shown.contains(&e.id));
    snapshot.relations.retain(|r| {
        r.kind != RelationKind::Contains && shown.contains(&r.source) && shown.contains(&r.target)
    });
    let taken: HashSet<String> = full
        .entities
        .iter()
        .map(|e| e.id.clone())
        .chain(full.relations.iter().map(|r| r.id.clone()))
        .chain(full.evidence.iter().map(|e| e.id.clone()))
        .collect();
    // Members the model places inside a scope are flattened under the system. The added
    // relation is a display placement, labelled as one, not a claim about the system.
    for member in &view.members {
        let Some(parent) = full
            .relations
            .iter()
            .find(|r| r.kind == RelationKind::Contains && &r.target == member)
            .map(|r| r.source.as_str())
        else {
            continue;
        };
        let moved = (parent != full.system_id).then_some(parent);
        snapshot
            .relations
            .push(flattened(&taken, &full.system_id, member, moved));
    }
    let cited: HashSet<&str> = snapshot
        .entities
        .iter()
        .flat_map(|e| e.evidence_ids.iter())
        .chain(
            snapshot
                .relations
                .iter()
                .flat_map(|r| r.evidence_ids.iter()),
        )
        .map(String::as_str)
        .collect();
    let evidence = full
        .evidence
        .iter()
        .filter(|e| cited.contains(e.id.as_str()))
        .cloned()
        .collect();
    snapshot.evidence = evidence;
    snapshot.snapshot.coverage = Coverage::Partial;
    snapshot.snapshot.omissions.push(format!(
        "Teaching view {}: {} of {} objects, flattened for explanation",
        view.id,
        view.members.len(),
        full.entities.len()
    ));
    Architecture::from_snapshot(snapshot)
}

fn flattened(
    taken: &HashSet<String>,
    system: &str,
    member: &str,
    parent: Option<&str>,
) -> Relation {
    let base = format!("teaching.contains.{member}");
    let mut id = base.clone();
    let mut n = 1;
    while taken.contains(&id) {
        id = format!("{base}.{n}");
        n += 1;
    }
    Relation {
        id,
        kind: RelationKind::Contains,
        source: system.to_owned(),
        target: member.to_owned(),
        label: None,
        description: None,
        aspect: None,
        condition: None,
        dependency_phase: None,
        necessity: None,
        basis: Basis::Declared,
        evidence_ids: Vec::new(),
        rationale: Some(match parent {
            Some(parent) => format!(
                "Teaching view placement: the model places this inside {parent}; it is shown directly in the system for this explanation"
            ),
            None => "Teaching view placement, as in the model".into(),
        }),
    }
}
