//! Example-only surface-acquisition experiment. This must only be installed in
//! a proof that never requests window screenshots. It does not change product
//! behavior or prevent a first stall before an occlusion message is delivered.
use bevy::{
    ecs::entity::EntityHashMap,
    prelude::*,
    render::{
        Extract, ExtractSchedule, Render, RenderApp, RenderSystems,
        view::window::{ExtractedWindow, ExtractedWindows, create_surfaces},
    },
    window::WindowOccluded,
};
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

/// Only known occlusion suppresses a window. Focus is deliberately not part of
/// this state: an unfocused window may still be visible and useful.
#[derive(Resource, Clone, Default)]
struct WindowOcclusionState {
    live: BTreeSet<Entity>,
    occluded: BTreeSet<Entity>,
}

impl WindowOcclusionState {
    fn observe(
        &mut self,
        live: impl IntoIterator<Item = Entity>,
        events: impl IntoIterator<Item = (Entity, bool)>,
    ) {
        self.live = live.into_iter().collect();
        self.occluded.retain(|window| self.live.contains(window));
        for (window, occluded) in events {
            if !self.live.contains(&window) {
                continue;
            }
            if occluded {
                self.occluded.insert(window);
            } else {
                self.occluded.remove(&window);
            }
        }
    }
}

#[derive(Resource, Default)]
struct ExtractedOcclusion(WindowOcclusionState);

/// Parking lasts for one render invocation. The separate suspended set spans
/// invocations so resuming always reconfigures the retained native surface.
/// Keeping this bookkeeping independent of GPU objects allows lifecycle tests
/// without fabricating native handles.
struct Parking<T> {
    parked: EntityHashMap<T>,
    suspended: BTreeSet<Entity>,
}

impl<T> Default for Parking<T> {
    fn default() -> Self {
        Self {
            parked: default(),
            suspended: default(),
        }
    }
}

#[derive(Default)]
struct Transitions {
    parked: usize,
    newly_suspended: usize,
    resumed: usize,
    suspended: usize,
}

impl<T> Parking<T> {
    fn park(
        &mut self,
        windows: &mut EntityHashMap<T>,
        state: &WindowOcclusionState,
        mut can_suspend: impl FnMut(&T) -> bool,
        mut resume: impl FnMut(&mut T),
    ) -> Transitions {
        assert!(
            self.parked.is_empty(),
            "occlusion probe must restore windows after every render"
        );
        self.suspended.retain(|window| state.live.contains(window));
        let mut transitions = Transitions::default();
        let ids: Vec<_> = windows.keys().copied().collect();
        for id in ids {
            if state.occluded.contains(&id) && can_suspend(&windows[&id]) {
                let window = windows.remove(&id).expect("collected window exists");
                self.parked.insert(id, window);
                transitions.newly_suspended += usize::from(self.suspended.insert(id));
            } else if self.suspended.remove(&id) {
                resume(windows.get_mut(&id).expect("collected window exists"));
                transitions.resumed += 1;
            }
        }
        transitions.parked = self.parked.len();
        transitions.suspended = self.suspended.len();
        transitions
    }

    fn restore(&mut self, windows: &mut EntityHashMap<T>) {
        for (id, window) in self.parked.drain() {
            assert!(
                windows.insert(id, window).is_none(),
                "a parked window must stay absent throughout rendering"
            );
        }
    }
}

#[derive(Resource, Default)]
struct ParkedWindows(Parking<ExtractedWindow>);

#[derive(Default)]
struct ProbeCounts {
    render_invocations: u64,
    completed_render_invocations: u64,
    parked_window_frames: u64,
    suspension_transitions: u64,
    resume_transitions: u64,
    currently_suspended: usize,
    maximum_parked: usize,
}

/// Bounded diagnostic counters shared with the proof's main-world report.
#[derive(Resource, Clone, Default)]
pub struct OcclusionProbeReport(Arc<Mutex<ProbeCounts>>);

impl OcclusionProbeReport {
    pub fn report(&self) -> serde_json::Value {
        let counts = self.0.lock().unwrap();
        serde_json::json!({
            "scope": "Example-only event-gated acquisition suspension. After initial presentation, known-occluded windows are absent from ExtractedWindows during the render schedule and restored after Cleanup. Initial presentation, unknown and unfocused-only windows are permitted. Resume forces surface reconfiguration. This cannot prevent a stall before a late occlusion notification. Window screenshots are prohibited while this probe is installed.",
            "render_invocations": counts.render_invocations,
            "completed_render_invocations": counts.completed_render_invocations,
            "parked_window_frames": counts.parked_window_frames,
            "suspension_transitions": counts.suspension_transitions,
            "resume_transitions": counts.resume_transitions,
            "currently_suspended": counts.currently_suspended,
            "maximum_parked": counts.maximum_parked,
        })
    }
}

fn observe_occlusion(
    windows: Query<Entity, With<Window>>,
    mut events: MessageReader<WindowOccluded>,
    mut state: ResMut<WindowOcclusionState>,
) {
    state.observe(
        windows.iter(),
        events.read().map(|event| (event.window, event.occluded)),
    );
}

fn extract_occlusion(
    source: Extract<Res<WindowOcclusionState>>,
    mut destination: ResMut<ExtractedOcclusion>,
) {
    destination.0.clone_from(&source);
}

fn park_windows(
    state: Res<ExtractedOcclusion>,
    mut windows: ResMut<ExtractedWindows>,
    mut parked: ResMut<ParkedWindows>,
    report: Res<OcclusionProbeReport>,
) {
    let transitions = parked.0.park(
        &mut windows.windows,
        &state.0,
        // Startup occlusion can precede the first present. Suspending then can
        // prevent a window from ever showing its initial content. Bevy clears
        // this flag after presentation, so use its actual lifecycle state.
        |window| !window.needs_initial_present,
        |window| {
            // Stock extraction has tracked the latest size/mode even while
            // hidden, but create_surfaces has not run for that window. Force
            // old view targets to be dropped and the surface reconfigured.
            window.size_changed = true;
        },
    );
    let mut counts = report.0.lock().unwrap();
    counts.render_invocations += 1;
    counts.parked_window_frames += transitions.parked as u64;
    counts.suspension_transitions += transitions.newly_suspended as u64;
    counts.resume_transitions += transitions.resumed as u64;
    counts.currently_suspended = transitions.suspended;
    counts.maximum_parked = counts.maximum_parked.max(transitions.parked);
}

fn restore_windows(
    mut windows: ResMut<ExtractedWindows>,
    mut parked: ResMut<ParkedWindows>,
    report: Res<OcclusionProbeReport>,
) {
    parked.0.restore(&mut windows.windows);
    report.0.lock().unwrap().completed_render_invocations += 1;
}

/// Install before `app.run()` while RenderApp is still on the main thread.
/// The caller must prohibit screenshots for the entire run. Surface resources
/// and native handles remain owned by Bevy; no raw handles or platform FFI are
/// accessed here. This intentionally leaves the first late-event race intact.
pub fn install(app: &mut App) {
    assert!(
        !app.world().contains_resource::<OcclusionProbeReport>(),
        "occlusion probe is already installed"
    );
    let report = OcclusionProbeReport::default();
    app.init_resource::<WindowOcclusionState>()
        .insert_resource(report.clone());
    if !app.world().contains_resource::<Messages<WindowOccluded>>() {
        app.add_message::<WindowOccluded>();
    }
    app.add_systems(PreUpdate, observe_occlusion);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render
            .init_resource::<ExtractedOcclusion>()
            .init_resource::<ParkedWindows>()
            .insert_resource(report)
            .add_systems(ExtractSchedule, extract_occlusion)
            .add_systems(
                Render,
                (
                    park_windows
                        .after(RenderSystems::ExtractCommands)
                        .before(RenderSystems::ManageViews)
                        .before(create_surfaces),
                    restore_windows.after(RenderSystems::Cleanup),
                ),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::{app::SubApp, render::view::window::prepare_windows, window::PresentMode};

    #[derive(Resource, Default)]
    struct ScheduleChecks(Mutex<BTreeSet<&'static str>>);

    fn skip_surface_creation(
        report: Res<OcclusionProbeReport>,
        checks: Res<ScheduleChecks>,
    ) -> bool {
        let counts = report.0.lock().unwrap();
        assert_eq!(counts.render_invocations, 1);
        assert_eq!(counts.completed_render_invocations, 0);
        checks.0.lock().unwrap().insert("configuration condition");
        false
    }

    fn check_manage_views(report: Res<OcclusionProbeReport>, checks: Res<ScheduleChecks>) {
        let counts = report.0.lock().unwrap();
        assert_eq!(counts.render_invocations, 1);
        assert_eq!(counts.completed_render_invocations, 0);
        checks.0.lock().unwrap().insert("manage views");
    }

    fn check_cleanup(report: Res<OcclusionProbeReport>, checks: Res<ScheduleChecks>) {
        let counts = report.0.lock().unwrap();
        assert_eq!(counts.render_invocations, 1);
        assert_eq!(counts.completed_render_invocations, 0);
        checks.0.lock().unwrap().insert("cleanup");
    }

    #[test]
    fn installed_probe_parks_before_surface_condition_and_views_and_restores_after_cleanup() {
        let mut app = App::new();
        let mut render = SubApp::new();
        render
            .add_schedule(Render::base_schedule())
            .init_resource::<ExtractedWindows>()
            .init_resource::<ScheduleChecks>();
        app.insert_sub_app(RenderApp, render);
        install(&mut app);
        let render = app.get_sub_app_mut(RenderApp).unwrap();
        render.add_systems(
            Render,
            (
                // The real public preparation system is registered, but its
                // condition must observe parking and skip all GPU parameters.
                create_surfaces
                    .run_if(skip_surface_creation)
                    .before(prepare_windows),
                prepare_windows
                    .run_if(|| false)
                    .in_set(RenderSystems::ManageViews),
                check_manage_views.in_set(RenderSystems::ManageViews),
                check_cleanup.in_set(RenderSystems::Cleanup),
            ),
        );
        render.world_mut().run_schedule(Render);
        assert_eq!(
            *render
                .world()
                .resource::<ScheduleChecks>()
                .0
                .lock()
                .unwrap(),
            BTreeSet::from(["configuration condition", "manage views", "cleanup"])
        );
        assert!(
            render
                .world()
                .resource::<ParkedWindows>()
                .0
                .parked
                .is_empty()
        );
        let report = app.world().resource::<OcclusionProbeReport>();
        let counts = report.0.lock().unwrap();
        assert_eq!(counts.render_invocations, 1);
        assert_eq!(counts.completed_render_invocations, 1);
    }

    #[derive(Debug, PartialEq, Eq)]
    struct WindowRecord {
        width: u32,
        mode: PresentMode,
        size_changed: bool,
        needs_initial_present: bool,
    }

    fn record() -> WindowRecord {
        WindowRecord {
            width: 100,
            mode: PresentMode::Fifo,
            size_changed: false,
            needs_initial_present: false,
        }
    }

    fn park(
        parking: &mut Parking<WindowRecord>,
        windows: &mut EntityHashMap<WindowRecord>,
        state: &WindowOcclusionState,
    ) -> Transitions {
        parking.park(
            windows,
            state,
            |window| !window.needs_initial_present,
            |window| window.size_changed = true,
        )
    }

    #[test]
    fn known_occluded_window_presents_initial_content_before_suspension() {
        let mut world = World::new();
        let id = world.spawn_empty().id();
        let mut state = WindowOcclusionState::default();
        state.observe([id], [(id, true)]);
        let mut initial = record();
        initial.needs_initial_present = true;
        let mut windows = EntityHashMap::from_iter([(id, initial)]);
        let mut parking = Parking::default();
        let first = park(&mut parking, &mut windows, &state);
        assert_eq!(first.parked, 0);
        assert_eq!(first.suspended, 0);
        assert!(windows[&id].needs_initial_present);
        parking.restore(&mut windows);

        // Renderer presentation, rather than event delivery or elapsed time,
        // unlocks suspension. No new occlusion event is needed afterward.
        windows.get_mut(&id).unwrap().needs_initial_present = false;
        let next = park(&mut parking, &mut windows, &state);
        assert_eq!(next.parked, 1);
        assert_eq!(next.newly_suspended, 1);
        assert!(!windows.contains_key(&id));
        parking.restore(&mut windows);
        assert!(!windows[&id].needs_initial_present);
    }

    #[test]
    fn unknown_startup_and_unfocused_windows_remain_active() {
        let mut app = App::new();
        let id = app
            .world_mut()
            .spawn(Window {
                focused: false,
                ..default()
            })
            .id();
        install(&mut app);
        app.update();
        let state = app.world().resource::<WindowOcclusionState>();
        assert!(state.live.contains(&id));
        assert!(state.occluded.is_empty());
        let mut windows = EntityHashMap::from_iter([(id, record())]);
        let mut parking = Parking::default();
        let transitions = park(&mut parking, &mut windows, state);
        assert_eq!(transitions.parked, 0);
        assert_eq!(transitions.resumed, 0);
        assert_eq!(windows[&id], record());
    }

    #[test]
    fn multiple_windows_park_independently_and_resume_reconfigures_latest_geometry() {
        let mut world = World::new();
        let a = world.spawn_empty().id();
        let b = world.spawn_empty().id();
        let mut state = WindowOcclusionState::default();
        state.observe([a, b], [(a, true)]);
        let mut windows = EntityHashMap::from_iter([(a, record()), (b, record())]);
        let mut parking = Parking::default();
        let first = park(&mut parking, &mut windows, &state);
        assert_eq!(first.parked, 1);
        assert_eq!(first.newly_suspended, 1);
        assert!(!windows.contains_key(&a));
        assert_eq!(windows[&b], record());
        // All render consumers see A absent until cleanup has completed.
        parking.restore(&mut windows);
        assert_eq!(windows[&a], record());

        // Simulate stock extraction receiving a hidden resize and mode change.
        windows.get_mut(&a).unwrap().width = 200;
        windows.get_mut(&a).unwrap().mode = PresentMode::Immediate;
        windows.get_mut(&a).unwrap().size_changed = true;
        let hidden = park(&mut parking, &mut windows, &state);
        assert_eq!(hidden.parked, 1);
        assert_eq!(hidden.newly_suspended, 0);
        parking.restore(&mut windows);
        // The next stock extraction clears its transient size-change flag.
        windows.get_mut(&a).unwrap().size_changed = false;
        state.observe([a, b], [(a, false), (b, true)]);
        let resumed = park(&mut parking, &mut windows, &state);
        assert_eq!(resumed.resumed, 1);
        assert_eq!(resumed.newly_suspended, 1);
        assert_eq!(resumed.suspended, 1);
        assert_eq!(
            windows[&a],
            WindowRecord {
                width: 200,
                mode: PresentMode::Immediate,
                size_changed: true,
                needs_initial_present: false,
            }
        );
        assert!(!windows.contains_key(&b));
        parking.restore(&mut windows);
        windows.get_mut(&a).unwrap().size_changed = false;
        let repeated = park(&mut parking, &mut windows, &state);
        assert_eq!(repeated.resumed, 0);
        assert!(!windows[&a].size_changed);
        parking.restore(&mut windows);
    }

    #[test]
    fn closing_a_suspended_window_clears_state_and_stale_messages() {
        let mut world = World::new();
        let id = world.spawn_empty().id();
        let mut state = WindowOcclusionState::default();
        let mut windows = EntityHashMap::from_iter([(id, record())]);
        let mut parking = Parking::default();
        state.observe([id], [(id, true)]);
        park(&mut parking, &mut windows, &state);
        parking.restore(&mut windows);
        windows.remove(&id); // Stock extraction handles the closed native window.
        state.observe([], [(id, true)]);
        let transitions = park(&mut parking, &mut windows, &state);
        assert_eq!(transitions.suspended, 0);
        assert!(state.occluded.is_empty());
        parking.restore(&mut windows);
        assert!(windows.is_empty());
    }

    #[test]
    fn message_delivery_keeps_last_event_and_cleans_up_despawned_windows() {
        let mut app = App::new();
        let id = app.world_mut().spawn(Window::default()).id();
        install(&mut app);
        for occluded in [true, false, true] {
            app.world_mut().write_message(WindowOccluded {
                window: id,
                occluded,
            });
        }
        app.update();
        assert!(
            app.world()
                .resource::<WindowOcclusionState>()
                .occluded
                .contains(&id)
        );
        app.world_mut().despawn(id);
        app.update();
        let state = app.world().resource::<WindowOcclusionState>();
        assert!(state.live.is_empty());
        assert!(state.occluded.is_empty());
    }
}
