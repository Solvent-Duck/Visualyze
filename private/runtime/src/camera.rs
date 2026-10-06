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
    /// The orbit centre. Home framing orbits the scene origin.
    target: Vec3,
    /// Closest allowed distance; a framed shot may come nearer than home allows.
    near: f32,
}

/// A camera pose an explanation can move to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Shot {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

/// An eased move between shots. Manual camera input ends it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tween {
    from: Shot,
    to: Shot,
    elapsed: f32,
    duration: f32,
}

/// Scripted camera moves. `interrupted` records that the person took the camera
/// back, so an explanation can pause rather than fight them (spec §10).
#[derive(Resource, Debug, Default)]
pub(crate) struct CameraDirector {
    tween: Option<Tween>,
    pub interrupted: bool,
    /// Cut instead of moving.
    pub reduced_motion: bool,
}

impl CameraDirector {
    /// Move to a shot; the duration grows with how far the camera travels.
    pub(crate) fn move_to(&mut self, orbit: &OrbitCamera, to: Shot) {
        let from = orbit.shot();
        let travel = (to.target - from.target).length() / from.distance.max(1.0)
            + angle_between(from.yaw, to.yaw).abs()
            + (to.pitch - from.pitch).abs()
            + (to.distance / from.distance.max(0.01)).ln().abs();
        let duration = if self.reduced_motion {
            0.0
        } else {
            (0.6 + travel * 0.7).clamp(0.6, 2.0)
        };
        self.tween = Some(Tween {
            from,
            to,
            elapsed: 0.0,
            duration,
        });
    }

    pub(crate) fn moving(&self) -> bool {
        self.tween.is_some()
    }

    fn advance(&mut self, orbit: &mut OrbitCamera, dt: f32) {
        let Some(tween) = &mut self.tween else {
            return;
        };
        tween.elapsed += dt;
        let t = if tween.duration <= 0.0 {
            1.0
        } else {
            (tween.elapsed / tween.duration).min(1.0)
        };
        // Cubic ease in-out: no snap at either end.
        let e = if t < 0.5 {
            4.0 * t * t * t
        } else {
            1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
        };
        let (from, to) = (tween.from, tween.to);
        orbit.target = from.target.lerp(to.target, e);
        orbit.yaw =
            (from.yaw + angle_between(from.yaw, to.yaw) * e).rem_euclid(std::f32::consts::TAU);
        orbit.pitch = from.pitch + (to.pitch - from.pitch) * e;
        // Interpolate distance geometrically so zooming feels even.
        orbit.distance = from.distance * (to.distance / from.distance.max(0.01)).powf(e);
        orbit.near = orbit.near.min(to.distance * 0.5);
        if t >= 1.0 {
            self.tween = None;
        }
    }
}

/// The signed shortest turn from `a` to `b`.
fn angle_between(a: f32, b: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    (b - a + PI).rem_euclid(TAU) - PI
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self::for_scene(Vec3::splat(3.0), 1280.0 / 900.0)
    }
}

impl OrbitCamera {
    pub(crate) fn for_scene(bounds: Vec3, aspect: f32) -> Self {
        let yaw = 0.28;
        let pitch = 0.18;
        let radius = bounds.length();
        let home_distance = fit_distance(bounds, yaw, pitch, aspect).max(radius + 1.5) * 1.12;
        Self {
            yaw,
            pitch,
            distance: home_distance,
            radius,
            bounds,
            aspect,
            home_distance,
            target: Vec3::ZERO,
            near: radius + 1.5,
        }
    }

    pub(crate) fn shot(&self) -> Shot {
        Shot {
            target: self.target,
            yaw: self.yaw,
            pitch: self.pitch,
            distance: self.distance,
        }
    }

    /// A shot from `yaw` and `pitch` that fits the box from `min` to `max`, with
    /// room left at the bottom of the frame for captions.
    pub(crate) fn frame(&self, min: Vec3, max: Vec3, yaw: f32, pitch: f32) -> Shot {
        let half = ((max - min) * 0.5).max(Vec3::splat(0.5));
        let centre = (min + max) * 0.5;
        // Generous margins: captions take the bottom of the frame and the
        // subject should breathe, as in a composed shot.
        let distance = (fit_distance(half, yaw, pitch, self.aspect) * 1.65)
            .clamp(half.length() + 1.0, self.home_distance * 3.0);
        // Lower the aim slightly so the subject sits above the caption band.
        let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0);
        let down = rotation * Vec3::Y;
        Shot {
            target: centre - down * distance * 0.04,
            yaw,
            pitch,
            distance,
        }
    }

    pub(crate) fn yaw(&self) -> f32 {
        self.yaw
    }

    pub(crate) fn transform(&self) -> Transform {
        let rotation = Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0);
        Transform::from_translation(self.target + rotation * Vec3::Z * self.distance)
            .looking_at(self.target, Vec3::Y)
    }

    /// Update the home framing without moving an established viewpoint.
    pub(crate) fn update_bounds(&mut self, bounds: Vec3) {
        let fitted = Self::for_scene(bounds, self.aspect);
        self.bounds = bounds;
        self.radius = fitted.radius;
        self.home_distance = fitted.home_distance;
        self.near = self.near.min(fitted.near);
    }

    fn adjust(&mut self, orbit: Vec2, zoom: f32) {
        self.yaw = (self.yaw + orbit.x).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + orbit.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        self.distance = (self.distance * (-zoom).exp())
            .clamp(self.near.min(self.radius + 1.5), self.home_distance * 3.0);
    }
}

/// The distance at which a box of half extents `half`, centred on the orbit
/// target, fits the view from `yaw` and `pitch`.
fn fit_distance(half: Vec3, yaw: f32, pitch: f32, aspect: f32) -> f32 {
    let half_vertical = FIELD_OF_VIEW * 0.5;
    let half_horizontal = (half_vertical.tan() * aspect).atan();
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -pitch, 0.0);
    let mut fit: f32 = 0.0;
    for x in [-half.x, half.x] {
        for y in [-half.y, half.y] {
            for z in [-half.z, half.z] {
                let corner = rotation.inverse() * Vec3::new(x, y, z);
                fit = fit.max(
                    corner.z
                        + (corner.x.abs() / half_horizontal.tan())
                            .max(corner.y.abs() / half_vertical.tan()),
                );
            }
        }
    }
    fit
}

pub(crate) struct OrbitCameraPlugin;

impl Plugin for OrbitCameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraInputLock>()
            .init_resource::<CameraDirector>()
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
    director: Option<ResMut<CameraDirector>>,
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
    // Scripted moves run while the person is in another app, e.g. reading the
    // agent's explanation in a terminal.
    let mut director = director;
    if let Some(director) = director.as_deref_mut() {
        director.advance(orbit, time.delta_secs());
    }
    // macOS may resize a new window before it receives focus. Framing must
    // follow the actual viewport even when keyboard/mouse input is inactive.
    if !window.focused || input_lock.is_some_and(|lock| lock.0) {
        **transform = orbit.transform();
        return;
    }
    let take_over = |director: &mut Option<ResMut<CameraDirector>>| {
        if let Some(director) = director.as_deref_mut() {
            director.tween = None;
            director.interrupted = true;
        }
    };
    if keys.just_pressed(KeyCode::KeyR) || keys.just_pressed(KeyCode::Home) {
        take_over(&mut director);
        **orbit = OrbitCamera::for_scene(orbit.bounds, aspect);
    } else if keys.just_pressed(KeyCode::KeyT) {
        take_over(&mut director);
        orbit.yaw = 0.0;
        orbit.pitch = PITCH_LIMIT;
    } else if keys.just_pressed(KeyCode::KeyF) {
        take_over(&mut director);
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
            take_over(&mut director);
            orbit.adjust(delta, zoom);
        }
    }
    **transform = orbit.transform();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_framed_shot_fits_the_box_and_a_tween_eases_into_it() {
        let orbit = OrbitCamera::for_scene(Vec3::splat(20.0), 1.5);
        let shot = orbit.frame(Vec3::new(4.0, 0.0, 4.0), Vec3::new(6.0, 2.0, 6.0), 0.5, 0.4);
        assert!(shot.distance < orbit.home_distance);
        assert!((shot.target - Vec3::new(5.0, 1.0, 5.0)).length() < shot.distance * 0.1);

        let mut orbit = orbit;
        let mut director = CameraDirector::default();
        director.move_to(&orbit, shot);
        director.advance(&mut orbit, 0.05);
        let early = orbit.shot();
        assert!(director.moving());
        // Eased: the first twentieth of a second covers little of the way.
        assert!((early.target - shot.target).length() > (shot.target.length() * 0.8));
        for _ in 0..100 {
            director.advance(&mut orbit, 0.05);
        }
        assert!(!director.moving());
        assert!((orbit.shot().target - shot.target).length() < 1e-4);
        assert!((orbit.shot().distance - shot.distance).abs() < 1e-3);
        let eye = orbit.transform();
        assert!((eye.translation - shot.target).length() - shot.distance < 1e-3);
    }

    #[test]
    fn yaw_turns_the_short_way_round() {
        assert!((angle_between(6.2, 0.1) - (0.1 + std::f32::consts::TAU - 6.2)).abs() < 1e-5);
        assert!((angle_between(0.1, 6.2) + (0.1 + std::f32::consts::TAU - 6.2)).abs() < 1e-5);
    }

    #[test]
    fn reduced_motion_cuts() {
        let mut orbit = OrbitCamera::default();
        let mut director = CameraDirector {
            reduced_motion: true,
            ..default()
        };
        let shot = orbit.frame(Vec3::ONE, Vec3::splat(2.0), 1.0, 0.3);
        director.move_to(&orbit, shot);
        director.advance(&mut orbit, 0.0);
        assert!(!director.moving());
        assert_eq!(orbit.shot(), shot);
    }

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
