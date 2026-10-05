use bevy::{
    input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel},
    prelude::*,
    window::PrimaryWindow,
};

pub(crate) const FIELD_OF_VIEW: f32 = std::f32::consts::FRAC_PI_4;
const PITCH_LIMIT: f32 = 1.4;

#[derive(Resource, Default)]
pub(crate) struct CameraInputLock(pub bool);

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CameraControls;

#[derive(Component, Debug, PartialEq)]
pub(crate) struct OrbitCamera {
    yaw: f32,
    pitch: f32,
    distance: f32,
    radius: f32,
    bounds: Vec3,
    aspect: f32,
    home_distance: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self::for_scene(Vec3::splat(3.0), 1280.0 / 900.0)
    }
}

impl OrbitCamera {
    pub(crate) fn for_scene(bounds: Vec3, aspect: f32) -> Self {
        let half_vertical = FIELD_OF_VIEW * 0.5;
        let half_horizontal = (half_vertical.tan() * aspect).atan();
        let yaw = 0.28;
        let pitch = 0.18;
        let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0);
        let radius = bounds.length();
        let mut fit = radius + 1.5;
        for x in [-bounds.x, bounds.x] {
            for y in [-bounds.y, bounds.y] {
                for z in [-bounds.z, bounds.z] {
                    let corner = rotation.inverse() * Vec3::new(x, y, z);
                    fit = fit.max(
                        corner.z
                            + (corner.x.abs() / half_horizontal.tan())
                                .max(corner.y.abs() / half_vertical.tan()),
                    );
                }
            }
        }
        let home_distance = fit * 1.12;
        Self {
            yaw,
            pitch,
            distance: home_distance,
            radius,
            bounds,
            aspect,
            home_distance,
        }
    }

    pub(crate) fn transform(&self) -> Transform {
        let rotation = Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0);
        Transform::from_translation(rotation * Vec3::Z * self.distance)
            .looking_at(Vec3::ZERO, Vec3::Y)
    }

    /// Update the home framing without moving an established viewpoint.
    pub(crate) fn update_bounds(&mut self, bounds: Vec3) {
        let fitted = Self::for_scene(bounds, self.aspect);
        self.bounds = bounds;
        self.radius = fitted.radius;
        self.home_distance = fitted.home_distance;
    }

    fn adjust(&mut self, orbit: Vec2, zoom: f32) {
        self.yaw = (self.yaw + orbit.x).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + orbit.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.distance =
            (self.distance * (-zoom).exp()).clamp(self.radius + 1.5, self.home_distance * 3.0);
    }
}

pub(crate) struct OrbitCameraPlugin;

impl Plugin for OrbitCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraInputLock>()
            .add_systems(Update, orbit_camera.in_set(CameraControls));
    }
}

#[allow(clippy::too_many_arguments)]
fn orbit_camera(
    mut camera: Single<(&mut OrbitCamera, &mut Transform)>,
    window: Single<&Window, With<PrimaryWindow>>,
    mouse: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    motion: Res<AccumulatedMouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    time: Res<Time>,
    input_lock: Option<Res<CameraInputLock>>,
) {
    let mut scroll = 0.0;
    for event in wheel.read() {
        scroll += event.y
            * match event.unit {
                MouseScrollUnit::Line => 0.12,
                MouseScrollUnit::Pixel => 0.002,
            };
    }
    let (orbit, transform) = &mut *camera;
    let aspect = window.width() / window.height().max(1.0);
    if (aspect - orbit.aspect).abs() > 0.001 {
        let fitted = OrbitCamera::for_scene(orbit.bounds, aspect);
        orbit.distance *= fitted.home_distance / orbit.home_distance;
        orbit.aspect = aspect;
        orbit.home_distance = fitted.home_distance;
        orbit.adjust(Vec2::ZERO, 0.0);
    }
    // macOS may resize a new window before it receives focus. Framing must
    // follow the actual viewport even when keyboard/mouse input is inactive.
    if !window.focused || input_lock.is_some_and(|lock| lock.0) {
        **transform = orbit.transform();
        return;
    }
    if keys.just_pressed(KeyCode::KeyR) || keys.just_pressed(KeyCode::Home) {
        **orbit = OrbitCamera::for_scene(orbit.bounds, aspect);
    } else if keys.just_pressed(KeyCode::KeyT) {
        orbit.yaw = 0.0;
        orbit.pitch = PITCH_LIMIT;
    } else if keys.just_pressed(KeyCode::KeyF) {
        orbit.yaw = std::f32::consts::FRAC_PI_2;
        orbit.pitch = 0.0;
    } else {
        let pressed = |key| if keys.pressed(key) { 1.0 } else { 0.0 };
        let dt = time.delta_secs();
        let mut delta = Vec2::new(
            pressed(KeyCode::ArrowRight) - pressed(KeyCode::ArrowLeft),
            pressed(KeyCode::ArrowUp) - pressed(KeyCode::ArrowDown),
        ) * dt;
        if mouse.pressed(MouseButton::Left) {
            delta += Vec2::new(-motion.delta.x, motion.delta.y) * 0.006;
        }
        let zoom = scroll
            + dt * (pressed(KeyCode::Equal) + pressed(KeyCode::NumpadAdd)
                - pressed(KeyCode::Minus)
                - pressed(KeyCode::NumpadSubtract));
        if delta != Vec2::ZERO || zoom != 0.0 {
            orbit.adjust(delta, zoom);
        }
    }
    **transform = orbit.transform();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changed_scene_bounds_preserve_the_established_viewpoint() {
        let mut orbit = OrbitCamera::default();
        orbit.adjust(Vec2::new(0.9, 0.5), 0.2);
        let before = orbit.transform();
        orbit.update_bounds(Vec3::new(4.0, 5.0, 2.0));
        assert_eq!(before, orbit.transform());
        assert_eq!(orbit.bounds, Vec3::new(4.0, 5.0, 2.0));
    }

    #[test]
    fn an_unfocused_window_still_updates_camera_framing() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<AccumulatedMouseMotion>()
            .add_message::<MouseWheel>()
            .add_systems(Update, orbit_camera);
        app.world_mut().spawn((
            Window {
                focused: false,
                resolution: bevy::window::WindowResolution::new(600, 1000),
                ..default()
            },
            PrimaryWindow,
        ));
        let orbit = OrbitCamera::default();
        let initial_distance = orbit.distance;
        let entity = app.world_mut().spawn((orbit.transform(), orbit)).id();
        app.update();
        let orbit = app.world().get::<OrbitCamera>(entity).unwrap();
        assert!((orbit.aspect - 0.6).abs() < 0.001);
        assert!(orbit.distance > initial_distance);
        assert_eq!(
            *app.world().get::<Transform>(entity).unwrap(),
            orbit.transform()
        );
    }

    #[test]
    fn search_palette_lock_prevents_camera_shortcuts_and_motion() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(CameraInputLock(true))
            .insert_resource(AccumulatedMouseMotion {
                delta: Vec2::splat(20.0),
            })
            .add_message::<MouseWheel>()
            .add_systems(Update, orbit_camera);
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: bevy::window::WindowResolution::new(1280, 900),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyR);
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        let mut orbit = OrbitCamera::for_scene(Vec3::splat(3.0), 1280.0 / 900.0);
        orbit.adjust(Vec2::new(0.9, 0.2), 0.1);
        let expected = orbit.transform();
        let entity = app.world_mut().spawn((expected, orbit)).id();
        app.update();
        assert_eq!(*app.world().get::<Transform>(entity).unwrap(), expected);
    }

    #[test]
    fn background_layout_work_does_not_lock_camera_motion() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(CameraInputLock(false))
            .insert_resource(crate::layout_scene::LayoutStatus {
                loading: true,
                ..default()
            })
            .insert_resource(AccumulatedMouseMotion {
                delta: Vec2::new(20.0, 10.0),
            })
            .add_message::<MouseWheel>()
            .add_systems(Update, orbit_camera);
        app.world_mut().spawn((
            Window {
                focused: true,
                resolution: bevy::window::WindowResolution::new(1280, 900),
                ..default()
            },
            PrimaryWindow,
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(MouseButton::Left);
        let orbit = OrbitCamera::for_scene(Vec3::splat(3.0), 1280.0 / 900.0);
        let before = orbit.transform();
        let entity = app.world_mut().spawn((before, orbit)).id();
        app.update();
        assert_ne!(*app.world().get::<Transform>(entity).unwrap(), before);
        assert!(
            app.world()
                .resource::<crate::layout_scene::LayoutStatus>()
                .loading
        );
    }

    #[test]
    fn orbit_always_faces_the_scene() {
        let mut orbit = OrbitCamera::default();
        for delta in [Vec2::ZERO, Vec2::new(2.0, 0.5), Vec2::new(-4.0, -1.0)] {
            orbit.adjust(delta, 0.0);
            let transform = orbit.transform();
            assert!((transform.translation.length() - orbit.distance).abs() < 0.0001);
            assert!(transform.forward().dot(-transform.translation.normalize()) > 0.9999);
        }
    }

    #[test]
    fn extreme_input_cannot_flip_camera_or_enter_scene() {
        let mut orbit = OrbitCamera::default();
        orbit.adjust(Vec2::new(100_000.0, 100_000.0), 100_000.0);
        assert_eq!(orbit.pitch, PITCH_LIMIT);
        assert_eq!(orbit.distance, orbit.radius + 1.5);
        assert!(orbit.transform().is_finite());
        orbit.adjust(Vec2::new(-100_000.0, -100_000.0), -100_000.0);
        assert_eq!(orbit.pitch, -PITCH_LIMIT);
        assert_eq!(orbit.distance, orbit.home_distance * 3.0);
        assert!(orbit.transform().is_finite());
    }

    #[test]
    fn fit_accounts_for_scene_size_and_narrow_windows() {
        let bounds = Vec3::new(8.0, 4.2, 1.2);
        let wide = OrbitCamera::for_scene(bounds, 1.5);
        let narrow = OrbitCamera::for_scene(bounds, 0.7);
        assert!(narrow.distance > wide.distance);
        assert!(OrbitCamera::for_scene(bounds * 2.0, 1.5).distance > wide.distance);
        assert!(wide.distance > wide.radius + 1.5);
        for camera in [wide, narrow] {
            let inverse = camera.transform().to_matrix().inverse();
            for x in [-bounds.x, bounds.x] {
                for y in [-bounds.y, bounds.y] {
                    for z in [-bounds.z, bounds.z] {
                        let corner = inverse.transform_point3(Vec3::new(x, y, z));
                        let vertical = -corner.z * (FIELD_OF_VIEW * 0.5).tan();
                        assert!(corner.y.abs() < vertical);
                        assert!(corner.x.abs() < vertical * camera.aspect);
                    }
                }
            }
        }
    }
}
