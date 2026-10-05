//! Input isolation for direct-action native proofs, never for normal viewers.
use bevy::{
    input::{
        InputSystems,
        keyboard::{Key, KeyboardInput},
        mouse::{
            AccumulatedMouseMotion, AccumulatedMouseScroll, MouseButtonInput, MouseMotion,
            MouseWheel,
        },
    },
    prelude::*,
};

pub const INPUT_MODE: &str = "Scripted ViewerActions only. Hardware keyboard/mouse messages, button states, accumulated deltas and cursor hover are discarded after Bevy InputSystems and before runtime Update controls. This input isolation does not change window creation, focus or update policy; separate proof options are reported independently. The window may activate. This proof does not verify physical OS input delivery.";

pub fn install(app: &mut App) {
    app.add_systems(PreUpdate, isolate_hardware_input.after(InputSystems));
}

fn isolate_hardware_input(world: &mut World) {
    // InputSystems has already converted this frame's raw events into held and
    // edge states. Clear both representations so controls and camera cannot see
    // either fresh events or keys/buttons held from an earlier frame.
    world.resource_mut::<ButtonInput<KeyCode>>().reset_all();
    world.resource_mut::<ButtonInput<Key>>().reset_all();
    world.resource_mut::<ButtonInput<MouseButton>>().reset_all();
    world.resource_mut::<Messages<KeyboardInput>>().clear();
    world.resource_mut::<Messages<MouseButtonInput>>().clear();
    world.resource_mut::<Messages<MouseMotion>>().clear();
    world.resource_mut::<Messages<MouseWheel>>().clear();
    *world.resource_mut::<AccumulatedMouseMotion>() = default();
    *world.resource_mut::<AccumulatedMouseScroll>() = default();
    for mut window in world.query::<&mut Window>().iter_mut(world) {
        // Only clear the app's reported hover position; do not move the OS cursor
        // or modify focus, window creation, rendering or background update mode.
        window.bypass_change_detection().set_cursor_position(None);
    }
    // ViewerActions is deliberately untouched. Proofs exercise the same action
    // queue, worker, validation, acceptance and rendering path as the viewer.
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::input::{ButtonState, InputPlugin, mouse::MouseScrollUnit};

    #[derive(Resource, Default)]
    struct Observations(usize);

    #[test]
    fn clears_converted_and_raw_input_before_update_without_changing_focus() {
        let mut app = App::new();
        app.add_plugins(InputPlugin).init_resource::<Observations>();
        install(&mut app);
        let window = app.world_mut().spawn(Window::default()).id();
        app.add_systems(Update, |world: &mut World| {
            assert!(
                world
                    .resource::<ButtonInput<KeyCode>>()
                    .get_pressed()
                    .next()
                    .is_none()
            );
            assert!(
                !world
                    .resource::<ButtonInput<KeyCode>>()
                    .just_pressed(KeyCode::KeyV)
            );
            assert!(
                world
                    .resource::<ButtonInput<Key>>()
                    .get_pressed()
                    .next()
                    .is_none()
            );
            assert!(
                world
                    .resource::<ButtonInput<MouseButton>>()
                    .get_pressed()
                    .next()
                    .is_none()
            );
            assert!(world.resource::<Messages<KeyboardInput>>().is_empty());
            assert!(world.resource::<Messages<MouseButtonInput>>().is_empty());
            assert!(world.resource::<Messages<MouseMotion>>().is_empty());
            assert!(world.resource::<Messages<MouseWheel>>().is_empty());
            assert_eq!(world.resource::<AccumulatedMouseMotion>().delta, Vec2::ZERO);
            assert_eq!(world.resource::<AccumulatedMouseScroll>().delta, Vec2::ZERO);
            for window in world.query::<&Window>().iter(world) {
                assert!(window.focused);
                assert_eq!(window.cursor_position(), None);
            }
            world.resource_mut::<Observations>().0 += 1;
        });
        // Include a previously held key, a fresh KeyV and active drag/scroll.
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ArrowLeft);
        app.world_mut()
            .resource_mut::<Messages<KeyboardInput>>()
            .write(KeyboardInput {
                key_code: KeyCode::KeyV,
                logical_key: Key::Character("v".into()),
                state: ButtonState::Pressed,
                text: Some("v".into()),
                repeat: false,
                window,
            });
        app.world_mut()
            .resource_mut::<Messages<MouseButtonInput>>()
            .write(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window,
            });
        app.world_mut()
            .resource_mut::<Messages<MouseMotion>>()
            .write(MouseMotion {
                delta: Vec2::splat(8.0),
            });
        app.world_mut()
            .resource_mut::<Messages<MouseWheel>>()
            .write(MouseWheel {
                unit: MouseScrollUnit::Line,
                x: 0.0,
                y: 9.0,
                window,
            });
        app.world_mut()
            .get_mut::<Window>(window)
            .unwrap()
            .set_cursor_position(Some(Vec2::splat(25.0)));
        app.update();
        app.update();
        assert_eq!(app.world().resource::<Observations>().0, 2);
    }
}
