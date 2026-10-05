//! Native proof of the experimental asynchronous placement path.
use bevy::{
    app::AppExit,
    input::InputSystems,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
};
use std::collections::BTreeMap;
use visualyze::{LayoutEngine, LayoutStatus, PlacedObject, PlacedRelation, ViewMode};

#[derive(Resource, Default)]
struct ExpansionProof {
    enabled: bool,
    phase: usize,
    tabs: usize,
    before: BTreeMap<String, Vec3>,
    camera: Option<Transform>,
}
fn main() -> AppExit {
    let options = visualyze::input::parse(std::env::args_os().skip(1))
        .expect("arguments")
        .expect("options");
    let engine = if options.layout == LayoutEngine::Legacy {
        LayoutEngine::Floorplan
    } else {
        options.layout
    };
    let mut app = if let Some(path) = options.bundle {
        visualyze::app_with_bundle(path, engine)
    } else {
        let model = visualyze::input::load(&options.source).expect("model");
        visualyze::app_with_layout(model, ViewMode::Architecture, engine)
    };
    app.insert_resource(visualyze::LayoutStorage(None))
        .insert_resource(ExpansionProof {
            enabled: std::env::var_os("VISUALYZE_SMOKE_EXPAND").is_some(),
            ..default()
        })
        .add_systems(PreUpdate, expand_database.after(InputSystems))
        .add_systems(Update, capture)
        .run()
}

// Exercise the actual keyboard/worker/render path in the declared bundle fixture.
// Headless geometry tests independently check all retained box bounds and routes.
fn expand_database(
    mut proof: ResMut<ExpansionProof>,
    status: Res<LayoutStatus>,
    objects: Query<(&PlacedObject, &Transform)>,
    camera: Single<&Transform, With<Camera3d>>,
    mut window: Single<&mut Window>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
) {
    if !proof.enabled || proof.phase == 5 {
        return;
    }
    window.focused = true;
    keys.reset_all();
    match proof.phase {
        0 => {
            // A reorganization request cannot supersede the initial refinement.
            keys.press(KeyCode::KeyO);
            proof.phase = 1;
        }
        1 if status.ready => {
            proof.before = objects
                .iter()
                .map(|(o, t)| (o.id.clone(), t.translation))
                .collect();
            let ids: Vec<_> = proof
                .before
                .keys()
                .filter(|id| id.as_str() != "scope:orders")
                .collect();
            proof.tabs = ids
                .iter()
                .position(|id| id.as_str() == "datastore:orders")
                .expect("use data-messaging bundle")
                + 1;
            proof.camera = Some(**camera);
            proof.phase = 2;
        }
        2 if proof.tabs > 0 => {
            keys.press(KeyCode::Tab);
            proof.tabs -= 1;
        }
        2 => {
            keys.press(KeyCode::KeyE);
            proof.phase = 3;
        }
        3 if objects.iter().any(|(o, _)| o.id == "table:orders") => {
            for (object, transform) in &objects {
                if object.id != "scope:orders"
                    && object.id != "datastore:orders"
                    && let Some(before) = proof.before.get(&object.id)
                {
                    assert_eq!(
                        *before, transform.translation,
                        "expansion moved {}",
                        object.id
                    );
                }
            }
            assert_eq!(
                proof.camera,
                Some(**camera),
                "expansion changed the established camera"
            );
            proof.phase = 4;
            println!("Database expansion retained neighboring scene positions and camera.");
        }
        4 => {
            proof.phase = 5;
        }
        _ => {}
    }
}
#[allow(clippy::too_many_arguments)]
fn capture(
    mut commands: Commands,
    time: Res<Time<Real>>,
    status: Res<LayoutStatus>,
    objects: Query<&PlacedObject>,
    relations: Query<&PlacedRelation>,
    proof: Res<ExpansionProof>,
    mut requested: Local<bool>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Some(error) = &status.error {
        panic!("layout failed: {error}");
    }
    if status.ready
        && (!proof.enabled || proof.phase == 5)
        && !*requested
        && time.elapsed_secs() > 5.0
    {
        assert_eq!(objects.iter().count(), status.object_count);
        assert_eq!(relations.iter().count(), status.relation_count);
        assert!(relations.iter().all(|r| r.points.len() >= 2));
        *requested = true;
        commands.spawn(Screenshot::primary_window()).observe(
            |capture: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
                let image = capture.image.clone().try_into_dynamic().unwrap();
                let rgb = image.to_rgb8();
                let colored = rgb
                    .pixels()
                    .filter(|p| {
                        let c = p.0;
                        *c.iter().max().unwrap() > 60
                            && c.iter().max().unwrap() - c.iter().min().unwrap() > 20
                    })
                    .count();
                assert!(
                    colored > 1500,
                    "scene must contain visible colored geometry"
                );
                let path = std::env::temp_dir().join("visualyze-placement.png");
                image.save(&path).unwrap();
                println!("Placement screenshot: {}", path.display());
                exit.write(AppExit::Success);
            },
        );
    }
    if time.elapsed_secs() > 30.0 {
        exit.write(AppExit::error());
    }
}
