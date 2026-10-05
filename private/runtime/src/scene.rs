use std::collections::BTreeMap;

use bevy::{
    camera::CameraUpdateSystems, core_pipeline::tonemapping::Tonemapping, prelude::*,
    transform::TransformSystems,
};
use visualyze_model::architecture::{Architecture, Basis, EntityKind};

use crate::{
    camera::{FIELD_OF_VIEW, OrbitCamera},
    composition::{ViewMode, compose},
    connections, primitives,
};

#[derive(Resource)]
pub(crate) struct ArchitectureModel(pub Architecture);

/// One semantic root per input object. All its cuboid fragments are children.
#[derive(Component, Debug)]
pub struct RenderedObject {
    pub id: String,
    pub kind: EntityKind,
    pub basis: Basis,
}

#[derive(Component)]
struct WorldLabel {
    position: Vec3,
    width: f32,
}

pub(crate) struct ObjectScenePlugin;

impl Plugin for ObjectScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewMode>()
            .add_systems(Startup, setup)
            .add_systems(
                PostUpdate,
                position_labels
                    .after(TransformSystems::Propagate)
                    .after(CameraUpdateSystems),
            );
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    model: Res<ArchitectureModel>,
    view: Res<ViewMode>,
) {
    let scene = compose(&model.0, *view);
    let routes = connections::routes(&model.0, &scene);
    let mut bounds = scene
        .objects
        .values()
        .map(|p| (p.position.abs() + p.half_size).max(p.label.abs() + Vec3::new(1.65, 0.9, 0.5)))
        .fold(Vec3::ZERO, Vec3::max);
    for route in &routes {
        if route.label.is_some() {
            bounds = bounds.max(route.label_position.abs() + Vec3::new(1.1, 0.4, 0.3));
        }
        for p in &route.relation.points {
            bounds = bounds.max(p.abs() + Vec3::splat(0.5));
        }
    }
    let roots: BTreeMap<_, _> = model
        .0
        .snapshot()
        .entities
        .iter()
        .map(|e| (e.id.as_str(), commands.spawn_empty().id()))
        .collect();
    for entity in &model.0.snapshot().entities {
        let p = &scene.objects[&entity.id];
        let root = roots[entity.id.as_str()];
        let parent = p
            .parent
            .as_ref()
            .map(|id| (roots[id.as_str()], scene.objects[id].position));
        commands.entity(root).insert((
            Name::new(entity.label.clone()),
            RenderedObject {
                id: entity.id.clone(),
                kind: entity.kind,
                basis: entity.basis,
            },
            Transform::from_translation(
                p.position - parent.map_or(Vec3::ZERO, |(_, position)| position),
            ),
            Visibility::default(),
        ));
        if let Some((parent, _)) = parent {
            commands.entity(root).insert(ChildOf(parent));
        }
        let material = materials.add(StandardMaterial {
            base_color: primitives::color(entity.kind),
            perceptual_roughness: 0.6,
            ..default()
        });
        let parts = if scene.connected && entity.kind == EntityKind::Scope {
            primitives::frame(p.half_size)
        } else {
            vec![primitives::block(p.half_size)]
        };
        for part in parts {
            commands.spawn((
                Mesh3d(meshes.add(Cuboid::from_size(part.size))),
                MeshMaterial3d(material.clone()),
                Transform::from_translation(part.position),
                ChildOf(root),
            ));
        }
        let port = scene.connected && entity.kind == EntityKind::Interface;
        let title = if port {
            entity.label.clone()
        } else {
            format!("{}\n{}", entity.kind.as_str().to_uppercase(), entity.label)
        };
        let label = commands
            .spawn((
                WorldLabel {
                    position: p.label,
                    width: if port { 140.0 } else { 210.0 },
                },
                Text::new(title),
                TextFont {
                    font_size: if port { 11.0 } else { 14.0 },
                    ..default()
                },
                TextColor(Color::srgb_u8(187, 200, 216)),
                TextLayout::new_with_justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    width: px(if port { 140 } else { 210 }),
                    ..default()
                },
            ))
            .id();
        // Compact port labels still surface exceptions to the normal declared/internal example.
        if !port
            || entity.basis != Basis::Declared
            || entity.boundary != visualyze_model::architecture::Boundary::Internal
        {
            commands.spawn((
                TextSpan::new(format!("\n{:?} / {:?}", entity.basis, entity.boundary)),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb_u8(136, 155, 175)),
                ChildOf(label),
            ));
        }
    }
    for route in &routes {
        connections::draw(&mut commands, &mut meshes, &mut materials, route);
        if let Some(label) = &route.label {
            commands.spawn((
                WorldLabel {
                    position: route.label_position,
                    width: 150.0,
                },
                Text::new(label.clone()),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(connections::color(route.relation.kind)),
                TextLayout::new_with_justify(Justify::Center),
                Node {
                    position_type: PositionType::Absolute,
                    width: px(150),
                    ..default()
                },
            ));
        }
    }
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.75, 0.85, 1.0),
        brightness: 200.0,
        ..default()
    });
    commands.spawn((
        DirectionalLight {
            illuminance: 4_000.0,
            ..default()
        },
        Transform::from_xyz(-3.0, 7.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    let orbit = OrbitCamera::for_scene(bounds, 1280.0 / 900.0);
    commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: FIELD_OF_VIEW,
            ..default()
        }),
        Tonemapping::None,
        orbit.transform(),
        orbit,
    ));
    commands.spawn((
        Text::new(if scene.connected {
            "Visualyze  /  Connected architecture"
        } else {
            "Visualyze  /  Object gallery"
        }),
        TextFont {
            font_size: 22.0,
            ..default()
        },
        TextColor(Color::srgb_u8(225, 231, 238)),
        Node {
            position_type: PositionType::Absolute,
            left: px(28),
            top: px(24),
            ..default()
        },
    ));
    let snapshot = model.0.snapshot();
    let description = if scene.connected {
        "Blue arrows: requests    Teal: reads    Amber: writes\nViolet dashes: coverage    Gray dashes: implementation / other logical links\nFrames: containment    Attached ports: interfaces    Dashed arrows: uncertain claims"
    } else {
        "Standalone specimens / relationships hidden"
    };
    commands.spawn((
        Text::new(format!(
            "{} objects  /  {} relations  /  revision {}  /  {:?} coverage  /  {} warnings\n{}",
            snapshot.entities.len(),
            snapshot.relations.len(),
            snapshot.revision,
            snapshot.snapshot.coverage,
            model.0.warnings().len(),
            description
        )),
        TextFont {
            font_size: 14.0,
            ..default()
        },
        TextColor(Color::srgb_u8(145, 165, 181)),
        Node {
            position_type: PositionType::Absolute,
            left: px(28),
            top: px(60),
            right: px(28),
            ..default()
        },
    ));
    commands.spawn((
        Text::new("Drag or arrows to orbit  |  Scroll or +/- to zoom  |  R to fit all objects"),
        TextFont {
            font_size: 13.0,
            ..default()
        },
        TextColor(Color::srgb_u8(145, 165, 181)),
        Node {
            position_type: PositionType::Absolute,
            left: px(28),
            right: px(28),
            bottom: px(24),
            ..default()
        },
    ));
}

fn position_labels(
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    mut labels: Query<(&WorldLabel, &mut Node)>,
) {
    let (camera, camera_transform) = *camera;
    for (label, mut node) in &mut labels {
        match camera.world_to_viewport(camera_transform, label.position) {
            Ok(position) => {
                node.display = Display::Flex;
                node.left = px(position.x - label.width * 0.5);
                node.top = px(position.y);
            }
            Err(_) => node.display = Display::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::mesh::VertexAttributeValues;
    use std::collections::BTreeSet;
    use visualyze_model::architecture::OBJECTS_FIXTURE;

    fn model() -> Architecture {
        Architecture::from_json(OBJECTS_FIXTURE.as_bytes()).unwrap()
    }

    // Check real mesh vertices after parenting, not just the layout's own values.
    fn assert_box_geometry(world: &mut World, scene: &crate::composition::Composition) {
        let objects: Vec<_> = world
            .query::<(Entity, &RenderedObject)>()
            .iter(world)
            .map(|(root, object)| (root, object.id.clone(), object.kind))
            .collect();
        let parts: Vec<_> = world
            .query::<(
                &Mesh3d,
                &MeshMaterial3d<StandardMaterial>,
                &ChildOf,
                &GlobalTransform,
            )>()
            .iter(world)
            .map(|(mesh, material, parent, transform)| {
                (
                    mesh.0.clone(),
                    material.0.clone(),
                    parent.parent(),
                    *transform,
                )
            })
            .collect();
        let meshes = world.resource::<Assets<Mesh>>();
        let materials = world.resource::<Assets<StandardMaterial>>();
        for (root, id, kind) in objects {
            let geometry: Vec<_> = parts
                .iter()
                .filter(|(_, _, parent, _)| *parent == root)
                .collect();
            let outlined = scene.connected && kind == EntityKind::Scope;
            assert_eq!(geometry.len(), if outlined { 12 } else { 1 }, "{id}");
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for (mesh, material, _, transform) in geometry {
                assert_eq!(
                    materials.get(material).unwrap().base_color,
                    primitives::color(kind)
                );
                let VertexAttributeValues::Float32x3(vertices) = meshes
                    .get(mesh)
                    .unwrap()
                    .attribute(Mesh::ATTRIBUTE_POSITION)
                    .unwrap()
                else {
                    panic!("expected cuboid positions for {id}")
                };
                for vertex in vertices {
                    let point = transform.transform_point(Vec3::from_array(*vertex));
                    min = min.min(point);
                    max = max.max(point);
                }
            }
            let placement = &scene.objects[&id];
            assert!(
                min.abs_diff_eq(placement.position - placement.half_size, 0.00001),
                "{id}"
            );
            assert!(
                max.abs_diff_eq(placement.position + placement.half_size, 0.00001),
                "{id}"
            );
        }
    }

    #[test]
    fn full_ingestion_spawns_every_kind_with_owned_geometry_and_no_connections() {
        let model = model();
        let scene = compose(&model, ViewMode::Auto);
        let expected: BTreeSet<_> = model
            .snapshot()
            .entities
            .iter()
            .map(|e| e.id.clone())
            .collect();
        let mut app = App::new();
        app.add_plugins((TransformPlugin, bevy::camera::visibility::VisibilityPlugin))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(ArchitectureModel(model))
            .add_plugins(ObjectScenePlugin);
        app.update();
        let world = app.world_mut();
        let objects: Vec<_> = world
            .query::<(Entity, &RenderedObject)>()
            .iter(world)
            .map(|(id, object)| (id, object.id.clone(), object.kind))
            .collect();
        assert_eq!(objects.len(), 7);
        assert_eq!(
            objects
                .iter()
                .map(|(_, id, _)| id.clone())
                .collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            objects
                .iter()
                .map(|(_, _, kind)| *kind)
                .collect::<BTreeSet<_>>()
                .len(),
            7
        );
        let roots: BTreeSet<_> = objects.iter().map(|(id, _, _)| *id).collect();
        let parts: Vec<_> = world
            .query::<(&Mesh3d, &ChildOf, &Visibility)>()
            .iter(world)
            .map(|(_, parent, visibility)| (parent.parent(), *visibility))
            .collect();
        assert_eq!(parts.len(), 7);
        assert!(parts.iter().all(
            |(parent, visibility)| roots.contains(parent) && *visibility != Visibility::Hidden
        ));
        for root in roots {
            assert!(parts.iter().any(|(parent, _)| *parent == root));
        }
        // No loose geometry: every mesh belongs to one semantic specimen, not an edge.
        assert_eq!(world.query::<&Mesh3d>().iter(world).count(), parts.len());
        assert_eq!(world.query::<&WorldLabel>().iter(world).count(), 7);
        assert_eq!(world.query::<&Camera3d>().iter(world).count(), 1);
        assert_box_geometry(world, &scene);
    }

    #[test]
    fn layout_depends_on_identity_and_kind_not_json_order() {
        let original = model();
        let mut value: serde_json::Value = serde_json::from_str(OBJECTS_FIXTURE).unwrap();
        value["entities"].as_array_mut().unwrap().reverse();
        value["relations"].as_array_mut().unwrap().reverse();
        let reordered = Architecture::from_json(value.to_string().as_bytes()).unwrap();
        let positions = |model: &Architecture| compose(model, ViewMode::Auto).objects;
        assert_eq!(positions(&original), positions(&reordered));
    }
    #[test]
    fn connected_ingestion_creates_hierarchy_and_owned_connection_geometry() {
        let model =
            Architecture::from_json(include_bytes!("../../../public/examples/notes-crud.json"))
                .unwrap();
        let scene = compose(&model, ViewMode::Auto);
        let mut app = App::new();
        app.add_plugins((TransformPlugin, bevy::camera::visibility::VisibilityPlugin))
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .insert_resource(ArchitectureModel(model))
            .add_plugins(ObjectScenePlugin);
        app.update();
        let world = app.world_mut();
        let roots: BTreeMap<_, _> = world
            .query::<(Entity, &RenderedObject)>()
            .iter(world)
            .map(|(id, o)| (o.id.clone(), id))
            .collect();
        assert_eq!(roots.len(), 8);
        for port in ["interface:notes", "interface:sessions"] {
            assert_eq!(
                world.get::<ChildOf>(roots[port]).unwrap().parent(),
                roots["service:notes"]
            );
        }
        let edges: Vec<_> = world
            .query::<(Entity, &connections::RenderedRelation)>()
            .iter(world)
            .map(|(id, r)| (id, r.points.is_empty()))
            .collect();
        assert_eq!(edges.len(), 16);
        assert_eq!(edges.iter().filter(|(_, empty)| !empty).count(), 9);
        let owners: BTreeSet<_> = roots
            .values()
            .copied()
            .chain(edges.iter().map(|(id, _)| *id))
            .collect();
        let parts: Vec<_> = world
            .query::<(&Mesh3d, &ChildOf)>()
            .iter(world)
            .map(|(_, p)| p.parent())
            .collect();
        assert!(parts.iter().all(|p| owners.contains(p)));
        for (edge, structural) in edges {
            assert_eq!(parts.contains(&edge), !structural);
        }
        let service = world
            .get::<GlobalTransform>(roots["service:notes"])
            .unwrap()
            .translation();
        let port = world
            .get::<GlobalTransform>(roots["interface:notes"])
            .unwrap()
            .translation();
        assert!((port - service).length() < 1.3);
        assert_box_geometry(world, &scene);
    }
}
