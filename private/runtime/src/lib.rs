mod camera;
mod composition;
mod connections;
pub mod input;
mod layout_geometry;
mod layout_input;
mod layout_scene;
mod native_layout;
mod pin_provenance;
mod primitives;
mod scene;
mod shutdown;

use bevy::{prelude::*, window::WindowResolution};
use visualyze_model::architecture::{Architecture, OBJECTS_FIXTURE};

pub use composition::ViewMode;
pub use connections::RenderedRelation;
pub use layout_scene::{
    LayoutEngine, LayoutStatus, LayoutStorage, LayoutTiming, PlacedObject, PlacedRelation,
    ViewerAction, ViewerActions, ViewerSearchResult,
};
pub use scene::RenderedObject;

/// Build the object simulator from the embedded, validated contract specimen.
pub fn app() -> Result<App, Box<dyn std::error::Error>> {
    Ok(app_with_model(Architecture::from_json(
        OBJECTS_FIXTURE.as_bytes(),
    )?))
}

/// Raw or partially decoded data cannot bypass the public validation boundary.
pub fn app_with_model(model: Architecture) -> App {
    app_with_view(model, ViewMode::Auto)
}

/// View preferences belong to the renderer, never to the semantic payload.
pub fn app_with_view(model: Architecture, view: ViewMode) -> App {
    app_with_layout(model, view, LayoutEngine::Legacy)
}

/// The new placement engines remain explicit choices until comprehension gates pass.
pub fn app_with_layout(model: Architecture, view: ViewMode, engine: LayoutEngine) -> App {
    app_with_snapshot_source(model, None, view, engine)
}

/// Preserve the explicitly supplied snapshot path for transactional F5 reloads.
pub fn app_with_snapshot_file(
    model: Architecture,
    path: std::path::PathBuf,
    view: ViewMode,
    engine: LayoutEngine,
) -> App {
    app_with_snapshot_source(model, Some(path), view, engine)
}

fn app_with_snapshot_source(
    model: Architecture,
    path: Option<std::path::PathBuf>,
    view: ViewMode,
    engine: LayoutEngine,
) -> App {
    let mut app = base_app();
    app.insert_resource(view);
    if engine == LayoutEngine::Legacy {
        app.insert_resource(scene::ArchitectureModel(model))
            .add_plugins(scene::ObjectScenePlugin);
    } else {
        let input = match path {
            Some(path) => layout_input::LayoutInput::snapshot_file(model, path),
            None => layout_input::LayoutInput::snapshot(model),
        };
        app.insert_resource(input)
            .insert_resource(engine)
            .add_plugins(layout_scene::LayoutScenePlugin);
    }
    app
}

/// Index supplied local bundle data in the background; no source paths are opened.
pub fn app_with_bundle(path: std::path::PathBuf, engine: LayoutEngine) -> App {
    let mut app = base_app();
    app.insert_resource(layout_input::LayoutInput::bundle(path))
        .insert_resource(if engine == LayoutEngine::Legacy {
            LayoutEngine::Floorplan
        } else {
            engine
        })
        .add_plugins(layout_scene::LayoutScenePlugin);
    app
}

fn base_app() -> App {
    let mut app = App::new();
    app.insert_resource(ClearColor(Color::srgb_u8(18, 22, 29)))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Visualyze — Architecture simulator".into(),
                resolution: WindowResolution::new(1280, 900),
                resize_constraints: bevy::window::WindowResizeConstraints {
                    min_width: 640.0,
                    min_height: 480.0,
                    ..default()
                },
                ..default()
            }),
            ..default()
        }))
        .add_plugins((camera::OrbitCameraPlugin, shutdown::RenderShutdownPlugin));
    app
}
