//! Resolve region-relative overrides without transferring their ownership.
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

use super::{
    CompositionContext, assembly_error, invalid,
    persistence::{
        PersistedConstraintSet, PersistenceError, PinPresentation, validate_constraint_refs,
        validate_constraint_registry,
    },
};
use crate::{
    LayoutError,
    assembly::{AnchorPin, AnchorPinLink, DyadicTransform},
    check_cancel,
};

/// Derive effective constraints across related supplied or protected display frames.
///
/// The original registry remains owned by its source contexts. This result is
/// only an effective view constraint set: **do not persist it as a newly owned
/// context**. The caller must recheck recorded ownership against the current
/// supplied model, then activate only constraints represented in the projection.
/// No current model lookup is performed here; a stored path alone cannot prove
/// that an object has not since been removed or reparented.
///
/// Current-context constraints are retained in full, including dormant pins.
/// Inherited pins require an explicit object-first presentation or supplied path that
/// contains the canonical owner below both the target and source roots. Pins
/// apply when entering a descendant or displaying that region in an ancestor.
/// Protected group definitions and reference-origin evidence are carried with
/// effective pins, without changing architectural ownership. Reference IDs can
/// recur in distinct projections, so reference pins apply only in their original
/// context, never in an ancestor's same-ID reference or the real target's frame.
/// Missing paths do not inherit; IDs never establish identity by spelling.
/// Models and effective lenses are separate;
/// an otherwise applicable policy mismatch is a conflict.
///
/// An inherited anchor crossing the target root is truncated exactly there,
/// preserving all remaining integer coordinates, dyadic transforms, domains,
/// envelopes and semantic members. An anchor below the target remains unchanged:
/// propagating upward never extends the anchor chain or locks the source frame's
/// position in a parent. Conflicting coordinates, ownership, memberships or frame locks
/// are errors rather than last-writer-wins overrides. Local anchor order is
/// retained; inherited anchors follow deterministic source-root/key order.
pub fn resolve_constraints(
    context: &CompositionContext,
    registry: &[PersistedConstraintSet],
    cancelled: &AtomicBool,
) -> Result<PersistedConstraintSet, LayoutError> {
    resolve(context, registry, cancelled, true, true)
}

/// Version 3 files used downward-only inheritance. Keep that interpretation
/// while checking the old file; callers must repair with the current resolver
/// before writing it in the current version.
pub(super) fn resolve_descending_constraints(
    context: &CompositionContext,
    registry: &[PersistedConstraintSet],
    cancelled: &AtomicBool,
) -> Result<PersistedConstraintSet, LayoutError> {
    resolve(context, registry, cancelled, false, false)
}

/// Versions 4/5 used bidirectional supplied ancestry without presentation data.
pub(super) fn resolve_supplied_constraints(
    context: &CompositionContext,
    registry: &[PersistedConstraintSet],
    cancelled: &AtomicBool,
) -> Result<PersistedConstraintSet, LayoutError> {
    resolve(context, registry, cancelled, true, false)
}

fn resolve(
    context: &CompositionContext,
    registry: &[PersistedConstraintSet],
    cancelled: &AtomicBool,
    include_descendant_origins: bool,
    include_presentation: bool,
) -> Result<PersistedConstraintSet, LayoutError> {
    check_cancel(cancelled)?;
    if registry.len() > 64 {
        return Err(invalid("too many stored constraint contexts"));
    }
    let mut result = PersistedConstraintSet {
        context: context.clone(),
        pins: BTreeMap::new(),
        anchors: vec![],
        ownership: BTreeMap::new(),
        presentation: BTreeMap::new(),
    };
    validate_constraint_registry(&context.model_id, std::slice::from_ref(&result), cancelled)
        .map_err(registry_error)?;
    // Validate shared limits before cloning retained data. Collecting references
    // is bounded by the 64-context check, including foreign model contexts.
    let mut matching = Vec::new();
    for set in registry {
        check_cancel(cancelled)?;
        if set.context.model_id == context.model_id {
            matching.push(set);
        }
    }
    validate_constraint_refs(&context.model_id, &matching, cancelled).map_err(registry_error)?;
    matching.sort_by(|a, b| a.context.root.cmp(&b.context.root));
    if let Some(local) = matching
        .iter()
        .find(|set| set.context.lens == context.lens && set.context.root == context.root)
    {
        if local.context.policy != context.policy {
            return Err(invalid(
                "local pin layout policy conflicts with the target context",
            ));
        }
        result = (**local).clone();
    }
    for metadata in result.presentation.values_mut() {
        canonicalize_presentation(metadata);
    }
    let mut owners: BTreeMap<_, _> = result
        .pins
        .iter()
        .flat_map(|(owner, pins)| pins.keys().map(|id| (id.clone(), owner.clone())))
        .collect();
    let mut anchor_indices: BTreeMap<_, _> = result
        .anchors
        .iter()
        .enumerate()
        .map(|(index, pin)| (anchor_key(pin), index))
        .collect();
    for set in &matching {
        if set.context.lens != context.lens || set.context.root == context.root {
            continue;
        }
        let mut anchored: BTreeMap<_, Vec<_>> = BTreeMap::new();
        for pin in &set.anchors {
            check_cancel(cancelled)?;
            anchored
                .entry((pin.owner(), pin.object_id()))
                .or_default()
                .push(pin);
        }
        for anchors in anchored.values_mut() {
            anchors.sort_by_key(|pin| pin.anchor());
        }
        for (owner, pins) in &set.pins {
            for (id, point) in pins {
                check_cancel(cancelled)?;
                let metadata = include_presentation
                    .then(|| set.presentation.get(id))
                    .flatten();
                let Some(path) = metadata.map(|p| &p.path).or_else(|| set.ownership.get(id)) else {
                    continue;
                };
                if metadata.is_some_and(|metadata| metadata.reference.is_some()) {
                    continue;
                }
                let Some(target_index) = inherited_owner_index(
                    path,
                    owner,
                    &context.root,
                    &set.context.root,
                    include_descendant_origins,
                ) else {
                    continue;
                };
                if set.context.policy != context.policy {
                    return Err(invalid(format!(
                        "inherited pin {id} layout policy conflicts with the target context"
                    )));
                }
                if owners
                    .get(id)
                    .is_some_and(|other_owner| other_owner != owner)
                {
                    return Err(invalid(format!(
                        "pin {id} has conflicting canonical owners"
                    )));
                }
                owners.insert(id.clone(), owner.clone());
                let local = result.pins.entry(owner.clone()).or_default();
                if local.get(id).is_some_and(|old| old != point) {
                    return Err(invalid(format!(
                        "pin {id} has conflicting canonical coordinates"
                    )));
                }
                local.insert(id.clone(), *point);
                if let Some(ownership) = set.ownership.get(id) {
                    if result.ownership.get(id).is_some_and(|old| old != ownership) {
                        return Err(invalid(format!(
                            "pin {id} has conflicting supplied ownership"
                        )));
                    }
                    result.ownership.insert(id.clone(), ownership.clone());
                }
                if let Some(metadata) = metadata {
                    let mut metadata = metadata.clone();
                    canonicalize_presentation(&mut metadata);
                    if result
                        .presentation
                        .get(id)
                        .is_some_and(|old| old != &metadata)
                    {
                        return Err(invalid(format!(
                            "pin {id} has conflicting presentation ancestry"
                        )));
                    }
                    result.presentation.insert(id.clone(), metadata);
                }
                for pin in anchored
                    .get(&(owner.as_str(), id.as_str()))
                    .into_iter()
                    .flatten()
                {
                    let pin = truncate_anchor(pin, &context.root, path, target_index)?;
                    let key = anchor_key(&pin);
                    if let Some(index) = anchor_indices.get(&key) {
                        let old = &result.anchors[*index];
                        if old != &pin {
                            return Err(invalid(format!("pin {id} has conflicting anchor locks")));
                        }
                    } else {
                        anchor_indices.insert(key, result.anchors.len());
                        result.anchors.push(pin);
                    }
                }
            }
        }
    }
    check_merged_anchors(&result, cancelled)?;
    validate_constraint_registry(&context.model_id, std::slice::from_ref(&result), cancelled)
        .map_err(registry_error)?;
    check_cancel(cancelled)?;
    Ok(result)
}

fn canonicalize_presentation(metadata: &mut PinPresentation) {
    metadata.groups.sort_by(|a, b| a.id.cmp(&b.id));
    for group in &mut metadata.groups {
        group.members.sort();
    }
}

fn anchor_key(pin: &AnchorPin) -> (String, String, String) {
    (
        pin.owner().into(),
        pin.object_id().into(),
        pin.anchor().into(),
    )
}

fn inherited_owner_index(
    path: &[String],
    owner: &str,
    target: &str,
    source: &str,
    include_descendant_origins: bool,
) -> Option<usize> {
    let owner = path.iter().position(|id| id == owner)?;
    let target = path.iter().position(|id| id == target)?;
    let source = path.iter().position(|id| id == source)?;
    (owner <= target.min(source)
        && (target < source || include_descendant_origins && source < target))
        .then_some(target)
}

fn truncate_anchor(
    pin: &AnchorPin,
    target: &str,
    path: &[String],
    target_index: usize,
) -> Result<AnchorPin, LayoutError> {
    let mut previous = path.iter().position(|id| id == pin.owner()).unwrap();
    for link in pin.links() {
        if let Some(next) = path.iter().position(|id| id == &link.parent) {
            if next <= previous {
                return Err(invalid(format!(
                    "pin {} anchor chain conflicts with supplied ownership",
                    pin.object_id()
                )));
            }
            previous = next;
        }
    }
    let keep = if pin.owner() == target {
        Some(0)
    } else {
        pin.links()
            .iter()
            .position(|link| link.parent == target)
            .map(|index| index + 1)
    };
    if let Some(keep) = keep {
        let mut record = pin.to_record();
        record.anchor = target.into();
        record.links.truncate(keep);
        let transform = record
            .links
            .iter()
            .try_fold(DyadicTransform::identity(), |inner, link| {
                link.transform.compose(inner)
            })
            .map_err(assembly_error)?;
        record.anchored_point = transform
            .map_point(record.local_point)
            .map_err(assembly_error)?;
        return AnchorPin::from_record(record).map_err(assembly_error);
    }
    if path
        .iter()
        .position(|id| id == pin.anchor())
        .is_some_and(|anchor| anchor < target_index)
    {
        return Ok(pin.clone());
    }
    Err(invalid(format!(
        "pin {} anchor chain does not establish the target inspection frame",
        pin.object_id()
    )))
}

fn check_merged_anchors(
    set: &PersistedConstraintSet,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    let mut membership = BTreeMap::new();
    let mut locks: BTreeMap<&str, &AnchorPinLink> = BTreeMap::new();
    for pin in &set.anchors {
        check_cancel(cancelled)?;
        if membership
            .insert(pin.object_id(), pin.semantic_ids())
            .is_some_and(|old| old != pin.semantic_ids())
        {
            return Err(invalid(format!(
                "pin {} has conflicting semantic membership",
                pin.object_id()
            )));
        }
        for link in pin.links() {
            if locks
                .insert(&link.frame, link)
                .is_some_and(|old| old != link)
            {
                return Err(invalid(format!(
                    "anchor pins have conflicting frame locks for {}",
                    link.frame
                )));
            }
            if set
                .pins
                .get(&link.parent)
                .and_then(|pins| pins.get(&link.frame))
                .is_some_and(|point| *point != link.envelope.center())
            {
                return Err(invalid(format!(
                    "canonical pin {} conflicts with an anchor envelope",
                    link.frame
                )));
            }
        }
    }
    Ok(())
}

fn registry_error(error: PersistenceError) -> LayoutError {
    match error {
        PersistenceError::Layout(error) => error,
        other => invalid(other.to_string()),
    }
}
