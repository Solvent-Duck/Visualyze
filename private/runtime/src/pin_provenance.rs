//! Durable display frames are presentation state, never supplied containment.
//! All model lookups run on the layout worker before activating constraints.
use crate::{
    layout_input::{DesktopProjection, LayoutInput},
    native_layout::ComposedConstraints,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicBool, Ordering},
};
use visualyze_layout::{
    LayoutError,
    composed::persistence::{MAX_STATE_BYTES, PinPresentation, PinReference},
    projection::{DisplayGroup, ViewState},
};
use visualyze_model::provider::SemanticLens;

type Registry = BTreeMap<(SemanticLens, String), ComposedConstraints>;
fn canonical_group(group: &DisplayGroup) -> DisplayGroup {
    let mut group = group.clone();
    group.members.sort();
    group
}
fn same_group(a: &DisplayGroup, b: &DisplayGroup) -> bool {
    canonical_group(a) == canonical_group(b)
}
fn invalid(message: impl Into<String>) -> LayoutError {
    LayoutError::Invalid(message.into())
}
fn check(cancel: &AtomicBool) -> Result<(), LayoutError> {
    if cancel.load(Ordering::Relaxed) {
        Err(LayoutError::Cancelled)
    } else {
        Ok(())
    }
}

/// Seed only the effective lens. A retained group can itself identify the lens
/// before its source records are projected, including after recent-group eviction.
pub(crate) fn seed_groups(
    view: &mut ViewState,
    registry: &Registry,
    lens: Option<SemanticLens>,
) -> Result<bool, LayoutError> {
    let lens = view
        .lens
        .or(lens)
        .or_else(|| {
            let focus = view.focus.as_ref()?;
            registry
                .values()
                .find(|set| {
                    set.presentation
                        .values()
                        .any(|pin| pin.groups.iter().any(|group| &group.id == focus))
                })
                .map(|set| set.context.lens)
        })
        .or(view.frame_lens);
    let Some(lens) = lens else {
        return Ok(false);
    };
    let mut groups = BTreeMap::new();
    for set in registry.values().filter(|set| set.context.lens == lens) {
        for pin in set.presentation.values() {
            for group in &pin.groups {
                if groups
                    .get(&group.id)
                    .is_some_and(|old| !same_group(old, group))
                {
                    return Err(invalid(format!(
                        "Pinned visual group {} has conflicting definitions; constraints retained",
                        group.id
                    )));
                }
                groups.insert(group.id.clone(), canonical_group(group));
            }
        }
    }
    if groups.is_empty() {
        return Ok(false);
    }
    let before = view.retained_suggestions().to_vec();
    view.protect_groups(&groups.values().cloned().collect::<Vec<_>>())?;
    Ok(before != view.retained_suggestions())
}

struct Ownership<'a> {
    input: &'a LayoutInput,
    lens: SemanticLens,
    cancelled: &'a AtomicBool,
    parents: BTreeMap<String, Option<Option<String>>>,
    bytes: usize,
}
impl<'a> Ownership<'a> {
    fn path(&mut self, id: &str) -> Result<Vec<String>, LayoutError> {
        let mut path = vec![id.to_owned()];
        loop {
            check(self.cancelled)?;
            let current = path.last().unwrap();
            let parent = if let Some(parent) = self.parents.get(current) {
                parent.clone()
            } else {
                let parent = self
                    .input
                    .supplied_owner(current, self.lens, self.cancelled)?;
                self.bytes = self.bytes.saturating_add(
                    current.len()
                        + parent
                            .as_ref()
                            .and_then(Option::as_ref)
                            .map_or(0, String::len)
                        + 128,
                );
                if self.bytes > MAX_STATE_BYTES {
                    return Err(invalid(
                        "Pin ownership lookup exceeds bounded storage; constraints retained",
                    ));
                }
                self.parents.insert(current.clone(), parent.clone());
                parent
            };
            match parent {
                None => {
                    return Err(invalid(format!(
                        "Pinned component {id} was removed or its supplied ownership changed; constraints retained"
                    )));
                }
                Some(None) => return Ok(path),
                Some(Some(parent)) => {
                    if path.len() >= 65 || path.contains(&parent) {
                        return Err(invalid(format!(
                            "Invalid supplied ownership path for pinned component {id}"
                        )));
                    }
                    path.push(parent);
                }
            }
        }
    }
}

fn display_path(base: &[String], groups: &[DisplayGroup]) -> Result<Vec<String>, LayoutError> {
    let mut result = Vec::new();
    for (index, id) in base.iter().enumerate() {
        result.push(id.clone());
        if let Some(parent) = base.get(index + 1) {
            let mut matches = groups
                .iter()
                .filter(|g| &g.parent == parent && g.members.contains(id));
            if let Some(group) = matches.next() {
                if matches.next().is_some() {
                    return Err(invalid(
                        "Pinned display path belongs to overlapping visual groups",
                    ));
                }
                result.push(group.id.clone());
            }
        }
    }
    if result.len() > 130 || result.iter().collect::<BTreeSet<_>>().len() != result.len() {
        return Err(invalid(
            "Pinned display ancestry is cyclic or exceeds its limit",
        ));
    }
    Ok(result)
}

fn metadata_path(
    id: &str,
    metadata: &PinPresentation,
    ownership: Option<&Vec<String>>,
) -> Result<Vec<String>, LayoutError> {
    let supplied = |id: &str| {
        metadata.supplied.get(id).ok_or_else(|| {
            invalid(format!(
                "Missing supplied evidence for pinned display frame {id}"
            ))
        })
    };
    let base = if let Some(reference) = &metadata.reference {
        let origin = if let Some(group) = metadata.groups.iter().find(|g| g.id == reference.origin)
        {
            let mut path = vec![group.id.clone()];
            path.extend(supplied(&group.parent)?.clone());
            path
        } else {
            supplied(&reference.origin)?.clone()
        };
        let mut path = vec![id.to_owned()];
        path.extend(origin);
        path
    } else if let Some(group) = metadata.groups.iter().find(|g| g.id == id) {
        let mut path = vec![id.to_owned()];
        path.extend(supplied(&group.parent)?.clone());
        path
    } else {
        ownership.cloned().ok_or_else(|| {
            invalid(format!(
                "Missing supplied ancestry for pinned component {id}"
            ))
        })?
    };
    display_path(&base, &metadata.groups)
}

/// The focused projection intentionally stops at its root. Use the retained
/// projector-normalized catalog plus fresh supplied ancestry to recover enclosing
/// group frames, including when a pin is created after entering a descendant.
/// Only groups on that path become protected; cached neighbors are not evidence.
fn capture_presentation(
    id: &str,
    p: &DesktopProjection,
    source: &mut Ownership<'_>,
) -> Result<Option<PinPresentation>, LayoutError> {
    let catalog = &p.group_catalog;
    let reference = p.references.get(id).map(|target| PinReference {
        origin: p.root.clone(),
        target: target.clone(),
    });
    let origin = reference
        .as_ref()
        .map_or(id, |binding| binding.origin.as_str());
    let direct = catalog.iter().find(|group| group.id == origin);
    let base = if let Some(group) = direct {
        let mut base = vec![origin.to_owned()];
        base.extend(source.path(&group.parent)?);
        base
    } else {
        source.path(origin)?
    };
    let mut groups = BTreeMap::new();
    for group in catalog {
        check(source.cancelled)?;
        if group.id == origin
            || base
                .windows(2)
                .any(|pair| group.members.contains(&pair[0]) && group.parent == pair[1])
        {
            let group = canonical_group(group);
            if groups
                .insert(group.id.clone(), group.clone())
                .is_some_and(|old| old != group)
            {
                return Err(invalid(format!(
                    "Pinned visual group {} has conflicting definitions; constraints retained",
                    group.id
                )));
            }
        }
    }
    Ok(
        (!groups.is_empty() || reference.is_some()).then(|| PinPresentation {
            path: vec![],
            groups: groups.into_values().collect(),
            reference,
            supplied: BTreeMap::new(),
        }),
    )
}

/// A missing display representation is safe to keep dormant only after the
/// recorded source identities and ancestry have been checked against this model.
pub(crate) fn verify(
    input: &LayoutInput,
    p: &DesktopProjection,
    constraints: &mut ComposedConstraints,
    cancelled: &AtomicBool,
) -> Result<(), LayoutError> {
    check(cancelled)?;
    let ids: BTreeSet<_> = constraints
        .pins
        .values()
        .flat_map(|pins| pins.keys().cloned())
        .collect();
    if ids.len() > visualyze_layout::assembly::MAX_TOTAL_BOXES {
        return Err(invalid("Too many durable pinned components"));
    }
    let mut checked = constraints.clone();
    checked.ownership.retain(|id, _| ids.contains(id));
    checked.presentation.retain(|id, _| ids.contains(id));
    let mut source = Ownership {
        input,
        lens: constraints.context.lens,
        cancelled,
        parents: BTreeMap::new(),
        bytes: 0,
    };
    let mut evidence_bytes = 0usize;
    for id in ids {
        check(cancelled)?;
        let existing = checked.presentation.get(&id).cloned();
        let mut metadata = if let Some(existing) = existing.clone() {
            Some(existing)
        } else if constraints.context.root == p.root {
            capture_presentation(&id, p, &mut source)?
        } else {
            None
        };
        let is_display = metadata
            .as_ref()
            .is_some_and(|m| m.reference.is_some() || m.groups.iter().any(|g| g.id == id));
        if !is_display {
            // Old same-context group/reference records can be captured above.
            // A foreign legacy display pin has no source proof and cannot inherit.
            let path = source.path(&id)?;
            if checked.ownership.get(&id).is_some_and(|old| *old != path) {
                return Err(invalid(format!(
                    "Pinned component {id} was removed or its supplied ownership changed; constraints retained"
                )));
            }
            evidence_bytes =
                evidence_bytes.saturating_add(path.iter().map(|s| s.len() + 32).sum::<usize>());
            checked.ownership.insert(id.clone(), path);
        } else if checked.ownership.contains_key(&id) {
            return Err(invalid(format!(
                "Pinned display {id} conflicts with supplied-object provenance"
            )));
        }
        if let Some(metadata) = metadata.as_mut() {
            let mut required = BTreeSet::new();
            for group in &metadata.groups {
                check(cancelled)?;
                if p.suggested_groups
                    .iter()
                    .any(|current| current.id == group.id && !same_group(current, group))
                {
                    return Err(invalid(format!(
                        "Pinned visual group {} changed membership; constraints retained",
                        group.id
                    )));
                }
                required.insert(group.parent.clone());
                required.extend(group.members.iter().cloned());
            }
            if let Some(reference) = &metadata.reference {
                required.insert(reference.target.clone());
                if !metadata.groups.iter().any(|g| g.id == reference.origin) {
                    required.insert(reference.origin.clone());
                }
                if p.root == reference.origin
                    && (p
                        .references
                        .get(&id)
                        .is_some_and(|target| target != &reference.target)
                        || (p.nodes.iter().any(|node| node.id == id)
                            && !p.references.contains_key(&id)))
                {
                    return Err(invalid(format!(
                        "Pinned reference {id} changed its supplied target; constraints retained"
                    )));
                }
            }
            if existing.is_some()
                && metadata.supplied.keys().cloned().collect::<BTreeSet<_>>() != required
            {
                return Err(invalid(format!(
                    "Pinned display {id} has incomplete or unrelated supplied evidence"
                )));
            }
            for supplied_id in required {
                let path = source.path(&supplied_id)?;
                if metadata
                    .supplied
                    .get(&supplied_id)
                    .is_some_and(|old| *old != path)
                {
                    return Err(invalid(format!(
                        "Pinned display {id} has changed supplied ownership at {supplied_id}; constraints retained"
                    )));
                }
                evidence_bytes =
                    evidence_bytes.saturating_add(path.iter().map(|s| s.len() + 32).sum::<usize>());
                if evidence_bytes > MAX_STATE_BYTES {
                    return Err(invalid(
                        "Pin evidence exceeds bounded storage; constraints retained",
                    ));
                }
                metadata.supplied.insert(supplied_id, path);
            }
            for group in &metadata.groups {
                for member in &group.members {
                    let path = &metadata.supplied[member];
                    if path.get(1) != Some(&group.parent)
                        || path[1..] != metadata.supplied[&group.parent]
                    {
                        return Err(invalid(format!(
                            "Pinned visual group {} no longer contains supplied siblings; constraints retained",
                            group.id
                        )));
                    }
                }
            }
            let path = metadata_path(&id, metadata, checked.ownership.get(&id))?;
            if existing.is_some() && metadata.path != path {
                return Err(invalid(format!(
                    "Pinned display {id} changed its coordinate ancestry; constraints retained"
                )));
            }
            metadata.path = path;
            metadata.groups.sort_by(|a, b| a.id.cmp(&b.id));
            checked.presentation.insert(id.clone(), metadata.clone());
        }
        if evidence_bytes > MAX_STATE_BYTES {
            return Err(invalid(
                "Pin ownership paths exceed bounded storage; constraints retained",
            ));
        }
    }
    visualyze_layout::composed::persistence::validate_constraint_registry(
        &checked.context.model_id,
        &[checked.persisted()],
        cancelled,
    )
    .map_err(|error| invalid(error.to_string()))?;
    check(cancelled)?;
    *constraints = checked;
    Ok(())
}

pub(crate) fn context_ancestors(
    input: &LayoutInput,
    root: &str,
    lens: SemanticLens,
    registry: &Registry,
    p: &DesktopProjection,
    cancelled: &AtomicBool,
) -> Result<BTreeSet<String>, LayoutError> {
    let group = p
        .suggested_groups
        .iter()
        .find(|g| g.id == root)
        .or_else(|| {
            registry
                .values()
                .filter(|s| s.context.lens == lens)
                .flat_map(|s| s.presentation.values())
                .flat_map(|m| m.groups.iter())
                .find(|g| g.id == root)
        });
    let mut source = Ownership {
        input,
        lens,
        cancelled,
        parents: BTreeMap::new(),
        bytes: 0,
    };
    if let Some(group) = group {
        let mut result: BTreeSet<_> = source.path(&group.parent)?.into_iter().collect();
        result.insert(root.into());
        Ok(result)
    } else {
        // Preserve legacy classification for an absent source root; the caller
        // checks recorded formerly-related pin paths before accepting changes.
        match input.supplied_owner(root, lens, cancelled)? {
            None => Ok(BTreeSet::from([root.into()])),
            Some(_) => Ok(source.path(root)?.into_iter().collect()),
        }
    }
}
