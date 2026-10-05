//! Finish the last pipelined frame while the main-thread executor is still live.
//!
//! Bevy 0.18's channel destructor waits synchronously for the render world. A
//! render system can itself be waiting for its main-thread work, so entering
//! that destructor before driving the executor can deadlock application exit.
use bevy::{
    app::AppExit,
    ecs::schedule::MainThreadExecutor,
    prelude::*,
    render::pipelined_rendering::{RenderAppChannels, RenderExtractApp},
    tasks::ComputeTaskPool,
};

pub(crate) struct RenderShutdownPlugin;

impl Plugin for RenderShutdownPlugin {
    fn build(&self, app: &mut App) {
        let Some(extract) = app.get_sub_app_mut(RenderExtractApp) else {
            return;
        };
        let Some(mut original) = extract.take_extract() else {
            return;
        };
        let mut drained = false;
        extract.set_extract(move |main, render| {
            if drained {
                return;
            }
            if main
                .get_resource::<Messages<AppExit>>()
                .is_some_and(|exit| !exit.is_empty())
            {
                // Extraction runs after the main schedule. Draining in Last
                // instead would leave the normal extractor waiting a second
                // time for the render world we already consumed.
                finish_render_on_exit(main);
                drained = true;
            } else {
                original(main, render);
            }
        });
    }
}

fn finish_render_on_exit(world: &mut World) {
    if world
        .get_resource::<Messages<AppExit>>()
        .is_none_or(Messages::is_empty)
        || !world.contains_resource::<RenderAppChannels>()
        || !world.contains_resource::<MainThreadExecutor>()
    {
        return;
    }
    world.resource_scope(|world, executor: Mut<MainThreadExecutor>| {
        world.resource_scope(|_world, mut channels: Mut<RenderAppChannels>| {
            // Match the executor pumping used by Bevy's renderer_extract. The
            // returned render world is dropped here, on its owning main thread,
            // after all work finishes. It is not sent out for another frame.
            let rendered = ComputeTaskPool::get()
                .scope_with_executor(true, Some(&*executor.0), |scope| {
                    scope.spawn(async { channels.recv().await });
                })
                .pop();
            drop(rendered);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{
        app::{SubApp, TaskPoolPlugin},
        ecs::schedule::ScheduleLabel,
        render::{RenderApp, pipelined_rendering::PipelinedRenderingPlugin},
    };
    use std::sync::{Arc, Mutex};

    struct DropProbe(Arc<Mutex<Option<std::thread::ThreadId>>>);
    impl Drop for DropProbe {
        fn drop(&mut self) {
            *self.0.lock().unwrap() = Some(std::thread::current().id());
        }
    }

    #[test]
    fn pipelined_exit_drains_once_and_drops_non_send_world_on_owning_thread() {
        let owner = std::thread::current().id();
        let dropped = Arc::new(Mutex::new(None));
        let mut app = App::new();
        let mut render = SubApp::new();
        render.update_schedule = Some(Update.intern());
        render
            .world_mut()
            .insert_non_send_resource(DropProbe(dropped.clone()));
        render.add_systems(Update, |_: NonSend<DropProbe>| {});
        app.insert_sub_app(RenderApp, render);
        app.add_plugins((
            TaskPoolPlugin::default(),
            PipelinedRenderingPlugin,
            RenderShutdownPlugin,
        ));
        app.finish();
        app.cleanup();
        app.update();
        assert!(dropped.lock().unwrap().is_none());
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert_eq!(*dropped.lock().unwrap(), Some(owner));
        app.update(); // No second receive or normal extraction after draining.
        drop(app);
    }

    #[test]
    fn headless_exit_remains_valid_without_renderer_resources() {
        let mut app = App::new();
        app.add_plugins(RenderShutdownPlugin);
        app.world_mut().write_message(AppExit::Success);
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::Success));
    }
}
