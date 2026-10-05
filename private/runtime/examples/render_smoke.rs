//! Load the real contract, render every object, and inspect a separate screen region
//! for each semantic root. This is a GPU visibility proof, not a visual regression suite.
use bevy::{
    app::AppExit,
    prelude::*,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    window::PrimaryWindow,
};
use std::collections::BTreeSet;
use visualyze::RenderedObject;

#[derive(Component)]
struct Regions(Vec<(String, Vec2, Vec2)>);

fn main() -> AppExit {
    let options = visualyze::input::parse(std::env::args_os().skip(1))
        .expect("valid arguments")
        .expect("use the regular executable for help");
    let model = visualyze::input::load(&options.source).expect("valid architecture input");
    assert_eq!(
        model.snapshot().entities.len(),
        7,
        "smoke test expects the seven-object fixture"
    );
    assert_eq!(
        model
            .snapshot()
            .entities
            .iter()
            .map(|e| e.kind)
            .collect::<BTreeSet<_>>()
            .len(),
        7
    );
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
    objects: Query<(&RenderedObject, &GlobalTransform)>,
    mut exit: MessageWriter<AppExit>,
) {
    if !*requested && time.elapsed_secs() >= 5.0 {
        *requested = true;
        let (camera, transform) = *camera;
        let mut regions = Vec::new();
        for (object, position) in &objects {
            let mut min = Vec2::splat(f32::INFINITY);
            let mut max = Vec2::splat(f32::NEG_INFINITY);
            for x in [-1.5, 1.5] {
                for y in [-1.1, 1.1] {
                    for z in [-1.0, 1.0] {
                        let point = camera
                            .world_to_viewport(
                                transform,
                                position.translation() + Vec3::new(x, y, z),
                            )
                            .expect("object in front of camera");
                        min = min.min(point);
                        max = max.max(point);
                    }
                }
            }
            assert!(
                min.x > 0.0
                    && min.y > 100.0
                    && max.x < window.width()
                    && max.y < window.height() - 50.0,
                "{} must fit inside the viewport",
                object.id
            );
            regions.push((
                object.id.clone(),
                min * window.scale_factor(),
                max * window.scale_factor(),
            ));
        }
        assert_eq!(regions.len(), 7);
        commands
            .spawn((Screenshot::primary_window(), Regions(regions)))
            .observe(verify);
    }
    if time.elapsed_secs() >= 30.0 {
        error!("Timed out waiting for the GPU screenshot");
        exit.write(AppExit::error());
    }
}

fn verify(
    capture: On<ScreenshotCaptured>,
    regions: Query<&Regions>,
    mut exit: MessageWriter<AppExit>,
) {
    let image = capture
        .image
        .clone()
        .try_into_dynamic()
        .expect("GPU image")
        .to_rgb8();
    let (width, height) = image.dimensions();
    let mut all_visible = true;
    for (id, min, max) in &regions.get(capture.entity).expect("capture regions").0 {
        let left = (min.x as u32).min(width);
        let right = (max.x as u32).min(width);
        let top = (min.y as u32).min(height);
        let bottom = (max.y as u32).min(height);
        let mut colored = 0;
        for y in top..bottom {
            for x in left..right {
                let pixel = image.get_pixel(x, y);
                let brightest = *pixel.0.iter().max().unwrap();
                let darkest = *pixel.0.iter().min().unwrap();
                if brightest > 55 && brightest - darkest > 16 {
                    colored += 1;
                }
            }
        }
        let visible = colored > ((right - left) * (bottom - top) / 100).max(20);
        info!("{id}: {colored} visible object-colored pixels; passed={visible}");
        all_visible &= visible;
    }
    let path = std::env::temp_dir().join("visualyze-objects.png");
    image
        .save(&path)
        .expect("save screenshot outside the repository");
    info!("Object render proof: {}", path.display());
    exit.write(if all_visible {
        AppExit::Success
    } else {
        AppExit::error()
    });
}
