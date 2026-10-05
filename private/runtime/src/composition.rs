//! Small, deterministic architecture composition. All positions are renderer-owned.
use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use visualyze_model::architecture::{Architecture, EntityKind as Kind, RelationKind as Link};

use crate::primitives;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Resource)]
pub enum ViewMode {
    #[default]
    Auto,
    Gallery,
    Architecture,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Placement {
    pub position: Vec3,
    /// World-space half dimensions, shared by geometry and connection routing.
    pub half_size: Vec3,
    pub label: Vec3,
    pub parent: Option<String>,
}

impl Placement {
    fn new(kind: Kind, position: Vec3) -> Self {
        Self {
            position,
            half_size: primitives::half_size(kind),
            label: position + Vec3::new(0.0, -1.45, 0.0),
            parent: None,
        }
    }
}

pub(crate) struct Composition {
    pub connected: bool,
    pub objects: BTreeMap<String, Placement>,
}

pub(crate) fn compose(model: &Architecture, mode: ViewMode) -> Composition {
    let connected = match mode {
        ViewMode::Auto => model
            .snapshot()
            .relations
            .iter()
            .any(|r| !matches!(r.kind, Link::Contains | Link::Exposes)),
        ViewMode::Gallery => false,
        ViewMode::Architecture => true,
    };
    let mut entities: Vec<_> = model.snapshot().entities.iter().collect();
    entities.sort_by_key(|e| (e.kind, &e.id));
    let mut objects = BTreeMap::new();
    if !connected {
        let columns = ((entities.len() as f32 * 1.3).sqrt().ceil() as usize).min(entities.len());
        let rows = entities.len().div_ceil(columns);
        for (i, entity) in entities.iter().enumerate() {
            let row = i / columns;
            let row_count = (entities.len() - row * columns).min(columns);
            let x = (i % columns) as f32 - (row_count - 1) as f32 * 0.5;
            let y = (rows - 1) as f32 * 0.5 - row as f32;
            objects.insert(
                entity.id.clone(),
                Placement::new(entity.kind, Vec3::new(x * 4.25, y * 4.3, 0.0)),
            );
        }
        return Composition { connected, objects };
    }

    let owners: BTreeMap<_, _> = model
        .snapshot()
        .relations
        .iter()
        .filter(|r| r.kind == Link::Exposes)
        .map(|r| (r.target.as_str(), r.source.as_str()))
        .collect();
    let compute: Vec<_> = entities
        .iter()
        .filter(|e| matches!(e.kind, Kind::Client | Kind::Service))
        .collect();
    for (i, entity) in compute.iter().enumerate() {
        let x = (i as f32 - (compute.len().saturating_sub(1)) as f32 * 0.5) * 5.8;
        objects.insert(
            entity.id.clone(),
            Placement::new(
                entity.kind,
                Vec3::new(
                    x,
                    1.3,
                    if entity.kind == Kind::Client {
                        0.5
                    } else {
                        0.0
                    },
                ),
            ),
        );
    }
    for (i, entity) in entities
        .iter()
        .filter(|e| e.kind == Kind::Datastore)
        .enumerate()
    {
        let users: BTreeSet<_> = model
            .snapshot()
            .relations
            .iter()
            .filter(|r| {
                matches!(r.kind, Link::Reads | Link::Writes | Link::DependsOn)
                    && (r.target == entity.id
                        || owners.get(r.target.as_str()) == Some(&entity.id.as_str()))
            })
            .map(|r| r.source.as_str())
            .collect();
        let positions: Vec<_> = users
            .iter()
            .filter_map(|id| objects.get(*id).map(|p| p.position.x))
            .collect();
        let x = if positions.is_empty() {
            i as f32 * 4.5
        } else {
            positions.iter().sum::<f32>() / positions.len() as f32
        };
        // Separate shared stores instead of claiming that any one service owns them.
        let mut position = Vec3::new(x, -3.2, -0.2);
        while objects
            .values()
            .any(|p| (p.position - position).length() < 3.5)
        {
            position.x += 4.5;
        }
        objects.insert(entity.id.clone(), Placement::new(entity.kind, position));
    }
    for (i, entity) in entities
        .iter()
        .filter(|e| e.kind == Kind::Deployment)
        .enumerate()
    {
        objects.insert(
            entity.id.clone(),
            Placement::new(entity.kind, Vec3::new(-4.0 - i as f32 * 4.2, -3.2, -0.2)),
        );
    }
    for entity in entities.iter().filter(|e| e.kind == Kind::Interface) {
        let owner = owners[entity.id.as_str()];
        let siblings: Vec<_> = owners
            .iter()
            .filter(|(_, candidate)| **candidate == owner)
            .map(|(id, _)| *id)
            .collect();
        let index = siblings.iter().position(|id| *id == entity.id).unwrap();
        let owner_placement = &objects[owner];
        let port_half = primitives::half_size(entity.kind) * 0.26;
        let mut p = Placement::new(
            entity.kind,
            owner_placement.position
                + Vec3::new(
                    -0.65,
                    (siblings.len() as f32 - 1.0) * 0.35 - index as f32 * 0.7,
                    owner_placement.half_size.z + port_half.z,
                ),
        );
        p.half_size = port_half;
        p.label = p.position + Vec3::new(0.75, 0.1, 0.25);
        p.parent = Some(owner.to_owned());
        objects.insert(entity.id.clone(), p);
    }
    let mut spanning = 0;
    let mut local = BTreeMap::<String, usize>::new();
    for entity in entities.iter().filter(|e| e.kind == Kind::Concern) {
        let targets: BTreeSet<_> = model
            .snapshot()
            .relations
            .iter()
            .filter(|r| r.kind == Link::AppliesTo && r.source == entity.id)
            .map(|r| r.target.as_str())
            .collect();
        let components: BTreeSet<_> = targets
            .iter()
            .map(|target| *owners.get(target).unwrap_or(target))
            .collect();
        let mut p;
        if components.len() > 1 {
            let min = targets
                .iter()
                .map(|id| objects[*id].position.x - 0.65)
                .fold(f32::INFINITY, f32::min);
            let max = targets
                .iter()
                .map(|id| objects[*id].position.x + 0.65)
                .fold(f32::NEG_INFINITY, f32::max);
            p = Placement::new(
                entity.kind,
                Vec3::new((min + max) * 0.5, 4.25 + spanning as f32 * 1.5, 0.0),
            );
            p.half_size = Vec3::new((max - min) * 0.5, p.half_size.y * 0.25, 0.3);
            p.label = p.position + Vec3::new(0.0, 1.45, 0.0);
            spanning += 1;
        } else if let Some(owner) = components.first() {
            let index = local.entry((*owner).to_owned()).or_default();
            p = Placement::new(
                entity.kind,
                objects[*owner].position + Vec3::new(2.4 + *index as f32, 0.25, -0.25),
            );
            p.half_size *= Vec3::new(0.13, 1.1, 2.0);
            p.label = p.position + Vec3::new(0.25, 2.35, 0.0);
            *index += 1;
        } else {
            p = Placement::new(
                entity.kind,
                Vec3::new(-4.0, 4.25 + spanning as f32 * 2.8, 0.0),
            );
            spanning += 1;
        }
        objects.insert(entity.id.clone(), p);
    }

    let children: BTreeMap<String, Vec<String>> = model
        .snapshot()
        .relations
        .iter()
        .filter(|r| r.kind == Link::Contains || r.kind == Link::Exposes)
        .fold(BTreeMap::new(), |mut m, r| {
            m.entry(r.source.clone())
                .or_default()
                .push(r.target.clone());
            m
        });
    for entity in entities.iter().filter(|e| e.kind == Kind::Scope) {
        scope(entity.id.as_str(), &children, &mut objects);
    }
    for r in model
        .snapshot()
        .relations
        .iter()
        .filter(|r| r.kind == Link::Contains)
    {
        objects.get_mut(&r.target).unwrap().parent = Some(r.source.clone());
    }
    // Normalize the complete assembly around the camera's stable home target.
    let min = objects
        .values()
        .map(|p| (p.position - p.half_size).min(p.label - Vec3::Y * 0.5))
        .fold(Vec3::splat(f32::INFINITY), Vec3::min);
    let max = objects
        .values()
        .map(|p| (p.position + p.half_size).max(p.label + Vec3::Y * 0.5))
        .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
    let center = (min + max) * 0.5;
    for p in objects.values_mut() {
        p.position -= center;
        p.label -= center;
    }
    Composition { connected, objects }
}

fn scope(
    id: &str,
    children: &BTreeMap<String, Vec<String>>,
    objects: &mut BTreeMap<String, Placement>,
) {
    if objects.contains_key(id) {
        return;
    }
    let mut descendants = BTreeSet::new();
    let mut pending = children.get(id).cloned().unwrap_or_default();
    while let Some(child) = pending.pop() {
        if !objects.contains_key(&child) {
            scope(&child, children, objects);
        }
        if descendants.insert(child.clone()) {
            pending.extend(children.get(&child).into_iter().flatten().cloned());
        }
    }
    let mut p = Placement::new(Kind::Scope, Vec3::ZERO);
    if !descendants.is_empty() {
        let min = descendants
            .iter()
            .map(|id| objects[id].position - objects[id].half_size)
            .fold(Vec3::splat(f32::INFINITY), Vec3::min);
        let max = descendants
            .iter()
            .map(|id| objects[id].position + objects[id].half_size)
            .fold(Vec3::splat(f32::NEG_INFINITY), Vec3::max);
        p.position = (min + max) * 0.5;
        p.half_size = (max - min) * 0.5 + Vec3::new(0.85, 0.85, 0.6);
    }
    p.label = p.position + Vec3::new(-p.half_size.x + 1.4, p.half_size.y + 1.25, -p.half_size.z);
    objects.insert(id.to_owned(), p);
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn notes() -> Architecture {
        Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json")).unwrap()
    }

    #[test]
    fn connected_layout_attaches_ports_and_encloses_children_without_auth_over_database() {
        let scene = compose(&notes(), ViewMode::Auto);
        assert!(scene.connected);
        assert_eq!(scene.objects.len(), 8);
        let service = &scene.objects["service:notes"];
        for id in ["interface:notes", "interface:sessions"] {
            let port = &scene.objects[id];
            assert_eq!(port.parent.as_deref(), Some("service:notes"));
            assert!((port.position - service.position).length() < 1.3);
            assert!(
                (port.position.z - port.half_size.z - (service.position.z + service.half_size.z))
                    .abs()
                    < 0.00001
            );
        }
        let scope = &scene.objects["scope:notes"];
        for (id, p) in &scene.objects {
            if id != "scope:notes" {
                assert!(
                    ((p.position - scope.position).abs() + p.half_size)
                        .cmplt(scope.half_size)
                        .all()
                );
            }
        }
        let db = &scene.objects["store:notes"];
        for id in ["concern:authentication", "concern:authorization"] {
            let p = &scene.objects[id];
            assert!(p.position.y - p.half_size.y > db.position.y + db.half_size.y);
        }
    }

    #[test]
    fn layout_is_stable_under_json_reordering_and_gallery_is_available() {
        let original = notes();
        let mut value: serde_json::Value =
            serde_json::from_slice(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        value["entities"].as_array_mut().unwrap().reverse();
        value["relations"].as_array_mut().unwrap().reverse();
        let reordered = Architecture::from_json(value.to_string().as_bytes()).unwrap();
        assert_eq!(
            compose(&original, ViewMode::Auto).objects,
            compose(&reordered, ViewMode::Auto).objects
        );
        let gallery = compose(&original, ViewMode::Gallery);
        assert!(!gallery.connected);
        assert!(gallery.objects.values().all(|p| p.parent.is_none()));
    }

    #[test]
    fn all_existing_architecture_fixtures_can_be_composed() {
        for input in [
            include_bytes!("../../../public/examples/architecture.json").as_slice(),
            include_bytes!("../../../public/examples/architecture-minimal.json"),
            include_bytes!("../../../public/examples/objects.json"),
        ] {
            let model = Architecture::from_json(input).unwrap();
            let scene = compose(&model, ViewMode::Architecture);
            assert_eq!(scene.objects.len(), model.snapshot().entities.len());
            assert!(
                scene
                    .objects
                    .values()
                    .all(|p| p.position.is_finite() && p.half_size.cmpgt(Vec3::ZERO).all())
            );
        }
    }
}
