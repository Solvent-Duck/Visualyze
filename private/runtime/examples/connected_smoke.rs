//! File -> validation -> composition -> native GPU proof for the notes model.
use bevy::{
    app::AppExit,
    ecs::system::SystemParam,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::PrimaryWindow,
};
use visualyze::{RenderedObject, RenderedRelation};
use visualyze_model::architecture::RelationKind;

#[derive(Component)]
struct Samples {
    points: Vec<(String, Vec<Vec2>, Option<[u8; 3]>)>,
    fits: bool,
}

#[derive(SystemParam)]
struct SceneGeometry<'w, 's> {
    objects: Query<'w, 's, (Entity, &'static RenderedObject)>,
    geometry: Query<'w, 's, (&'static ChildOf, &'static GlobalTransform), With<Mesh3d>>,
    relations: Query<'w, 's, &'static RenderedRelation>,
}

fn main() -> AppExit {
    let options = visualyze::input::parse(std::env::args_os().skip(1))
        .expect("valid arguments")
        .expect("use the regular executable for help");
    let model = visualyze::input::load(&options.source).expect("valid architecture input");
    assert_eq!(model.snapshot().model_id, "example-notes-crud");
    visualyze::app_with_model(model)
        .add_systems(Update, capture_after_warmup)
        .run()
}

fn capture_after_warmup(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut requested: Local<bool>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    window: Single<&Window, With<PrimaryWindow>>,
    scene: SceneGeometry,
    mut exit: MessageWriter<AppExit>,
) {
    let SceneGeometry {
        objects,
        geometry,
        relations,
    } = scene;
    if !*requested && time.elapsed_secs() >= 5.0 {
        *requested = true;
        assert_eq!(objects.iter().count(), 8);
        assert_eq!(relations.iter().count(), 16);
        let (camera, transform) = *camera;
        let project = |point: Vec3| {
            let p = camera
                .world_to_viewport(transform, point)
                .expect("in front of camera");
            p * window.scale_factor()
        };
        println!(
            "Window: {} x {} @ {}",
            window.width(),
            window.height(),
            window.scale_factor()
        );
        let mut samples = Vec::new();
        for (id, object) in &objects {
            let points: Vec<_> = geometry
                .iter()
                .filter(|(parent, _)| parent.parent() == id)
                .map(|(_, t)| project(t.translation()))
                .collect();
            samples.push((object.id.clone(), points, None));
        }
        for relation in &relations {
            if relation.points.is_empty() {
                continue;
            }
            let color = match relation.kind {
                RelationKind::Requests => [83, 159, 219],
                RelationKind::Reads => [66, 180, 172],
                RelationKind::Writes => [209, 162, 97],
                RelationKind::AppliesTo => [183, 139, 212],
                _ => [125, 144, 164],
            };
            let points: Vec<_> = relation
                .points
                .windows(2)
                .flat_map(|p| (1..20).map(move |i| project(p[0].lerp(p[1], i as f32 / 20.0))))
                .collect();
            samples.push((relation.id.clone(), points, Some(color)));
        }
        assert_eq!(samples.len(), 17);
        let fits = samples.iter().all(|(_, points, _)| {
            points.iter().all(|p| {
                let p = *p / window.scale_factor();
                p.x > 10.0
                    && p.x < window.width() - 10.0
                    && p.y > 110.0
                    && p.y < window.height() - 50.0
            })
        });
        println!("Geometry fits viewport: {fits}");
        commands
            .spawn((
                Screenshot::primary_window(),
                Samples {
                    points: samples,
                    fits,
                },
            ))
            .observe(verify);
    }
    if time.elapsed_secs() >= 30.0 {
        exit.write(AppExit::error());
    }
}

fn verify(
    capture: On<ScreenshotCaptured>,
    samples: Query<&Samples>,
    mut exit: MessageWriter<AppExit>,
) {
    let image = capture
        .image
        .clone()
        .try_into_dynamic()
        .expect("GPU image")
        .to_rgb8();
    let samples = samples.get(capture.entity).expect("samples");
    let mut all_visible = samples.fits;
    for (id, points, color) in &samples.points {
        let mut visible = 0;
        for point in points {
            let matches = ((point.y as u32).saturating_sub(5)..=point.y as u32 + 5).any(|y| {
                ((point.x as u32).saturating_sub(5)..=point.x as u32 + 5).any(|x| {
                    if x >= image.width() || y >= image.height() {
                        return false;
                    }
                    let pixel = image.get_pixel(x, y).0;
                    if let Some(color) = color {
                        pixel.iter().zip(color).all(|(a, b)| a.abs_diff(*b) < 24)
                    } else {
                        let min = *pixel.iter().min().unwrap();
                        let max = *pixel.iter().max().unwrap();
                        max > 55 && max - min > 16
                    }
                })
            });
            if matches {
                visible += 1;
            }
        }
        let passed = visible >= if color.is_some() { 3 } else { 1 };
        println!(
            "{id}: {visible}/{} visible samples; passed={passed}",
            points.len()
        );
        all_visible &= passed;
    }
    let path = std::env::temp_dir().join("visualyze-notes-crud.png");
    image
        .save(&path)
        .expect("save screenshot outside repository");
    println!("Connected architecture proof: {}", path.display());
    exit.write(if all_visible {
        AppExit::Success
    } else {
        AppExit::error()
    });
}
