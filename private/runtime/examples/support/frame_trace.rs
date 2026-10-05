//! Opt-in CPU phase tracing for native proofs. Never changes window/update modes.
use bevy::{
    app::MainScheduleOrder,
    ecs::schedule::ScheduleLabel,
    prelude::*,
    render::{
        Render, RenderApp, RenderSystems, pipelined_rendering::RenderExtractApp,
        view::window::prepare_windows,
    },
    window::{PrimaryWindow, WindowOccluded},
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Instant,
};
use visualyze::LayoutStatus;

const LIMIT: usize = 8192;

#[derive(ScheduleLabel, Debug, Clone, PartialEq, Eq, Hash)]
struct Boundary(usize);

#[derive(Clone)]
struct Span {
    frame: u64,
    phase: String,
    start_ms: f64,
    duration_ms: f64,
    focused: Option<bool>,
    present_mode: Option<String>,
    occluded: Option<bool>,
    loading: bool,
    action_id: Option<u64>,
    capture_pending: bool,
    metadata_at: &'static str,
}

#[derive(Resource)]
pub struct FrameTrace {
    origin: Instant,
    frame: u64,
    active: Option<(Span, Instant)>,
    last_end: Option<Instant>,
    capture_pending: bool,
    spans: VecDeque<Span>,
    dropped: usize,
    surface: SurfaceTrace,
    occlusion: Option<(Entity, bool)>,
}

/// Only bounded diagnostic records are shared with the concurrent render world.
#[derive(Resource, Clone)]
struct SurfaceTrace {
    origin: Instant,
    data: Arc<Mutex<SurfaceData>>,
}
#[derive(Default)]
struct SurfaceData {
    invocation: u64,
    active: Option<Instant>,
    spans: VecDeque<(u64, f64, f64)>,
    dropped: usize,
}
impl SurfaceTrace {
    fn new(origin: Instant) -> Self {
        Self {
            origin,
            data: default(),
        }
    }
    fn begin(&self, now: Instant) {
        let mut data = self.data.lock().unwrap();
        assert!(
            data.active.is_none(),
            "surface timing brackets must not overlap"
        );
        data.invocation += 1;
        data.active = Some(now);
    }
    fn end(&self, now: Instant) {
        let mut data = self.data.lock().unwrap();
        let start = data.active.take().expect("surface timing start");
        if data.spans.len() == LIMIT {
            data.spans.pop_front();
            data.dropped += 1;
        }
        let invocation = data.invocation;
        data.spans.push_back((
            invocation,
            start.duration_since(self.origin).as_secs_f64() * 1000.0,
            now.duration_since(start).as_secs_f64() * 1000.0,
        ));
    }
    fn report(&self) -> serde_json::Value {
        let (spans, dropped) = {
            let data = self.data.lock().unwrap();
            (data.spans.clone(), data.dropped)
        };
        serde_json::json!({
            "scope": "CPU scheduling bracket before/after Bevy prepare_windows in the render schedule, including time to schedule either marker and concurrent render work. This is not exclusive function time or GPU time. It brackets swapchain acquisition and any Outdated recovery performed inside prepare_windows for all extracted windows; a call may reuse an existing texture. Ordinary create_surfaces configuration is not separately attributed and can run before this bracket or overlap it. Render invocation numbers are independent of main frame IDs. Correlate by shared-clock start/duration offsets. Markers add ordering constraints/overhead but leave prepare_windows and window settings unchanged.",
            "capacity":LIMIT,"dropped_spans":dropped,
            "spans":spans.iter().map(|(invocation,start,duration)| serde_json::json!({"invocation":invocation,"start_ms":start,"duration_ms":duration})).collect::<Vec<_>>()
        })
    }
}

fn surface_begin(trace: Res<SurfaceTrace>) {
    trace.begin(Instant::now());
}
fn surface_end(trace: Res<SurfaceTrace>) {
    trace.end(Instant::now());
}

impl FrameTrace {
    fn push(&mut self, span: Span) {
        if self.spans.len() == LIMIT {
            self.spans.pop_front();
            self.dropped += 1;
        }
        self.spans.push_back(span);
    }

    pub fn capture_pending(&mut self, pending: bool) {
        self.capture_pending = pending;
    }

    /// Screenshot decoding/checking/encoding is a subset of its enclosing
    /// schedule, not an additional exclusive phase to add to frame duration.
    pub fn capture_cpu(&mut self, started: Instant) {
        self.push(Span {
            frame: self.frame,
            phase: "screenshot_cpu_nested".into(),
            start_ms: started.duration_since(self.origin).as_secs_f64() * 1000.0,
            duration_ms: started.elapsed().as_secs_f64() * 1000.0,
            focused: None,
            present_mode: None,
            occluded: None,
            loading: false,
            action_id: None,
            capture_pending: true,
            metadata_at: "unavailable",
        });
    }

    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({
            "scope": "Opt-in CPU schedule boundaries and the unchanged pipelined renderer extract callback. render_join_and_extract includes waiting for the render world, main-thread render tasks, extraction and dispatch; it is not GPU presentation timing. between_updates and before_render_extract are residuals including event-loop/OS/other-subapp work, not proven sleep time. Residual metadata is sampled at the end; other phase metadata at the start. between_updates belongs to the frame it precedes. window_occlusion_changed is a zero-duration notification observed in PreUpdate, not the OS event timestamp. occluded is the last primary-window notification, independent of focus; null means none has been observed for that window. Screenshot CPU spans are nested, not additive, and have no focus/occlusion/loading/action sample. All offsets share the proof start. Startup, current unfinished schedule and later shutdown are excluded. Boundary instrumentation adds overhead; window focus and update settings are unchanged.",
            "capacity": LIMIT,
            "dropped_spans": self.dropped,
            "surface_acquisition": self.surface.report(),
            "spans": self.spans.iter().map(|span| serde_json::json!({
                "frame":span.frame,"phase":span.phase,"start_ms":span.start_ms,
                "duration_ms":span.duration_ms,"focused":span.focused,"occluded":span.occluded,
                "requested_present_mode":span.present_mode,
                "loading":span.loading,"last_action_id":span.action_id,
                "capture_pending":span.capture_pending,"metadata_at":span.metadata_at
            })).collect::<Vec<_>>()
        })
    }
}

fn sample(world: &mut World, phase: String, now: Instant) -> Span {
    let window = world
        .query_filtered::<(Entity, &Window), With<PrimaryWindow>>()
        .iter(world)
        .next()
        .map(|(entity, window)| (entity, window.focused, window.present_mode));
    let status = world.get_resource::<LayoutStatus>();
    let trace = world.resource::<FrameTrace>();
    Span {
        frame: trace.frame,
        phase,
        start_ms: now.duration_since(trace.origin).as_secs_f64() * 1000.0,
        duration_ms: 0.0,
        focused: window.map(|(_, focused, _)| focused),
        present_mode: window.map(|(_, _, mode)| format!("{mode:?}")),
        occluded: window.and_then(|(entity, _, _)| {
            trace
                .occlusion
                .filter(|(id, _)| *id == entity)
                .map(|(_, occluded)| occluded)
        }),
        loading: status.is_some_and(|s| s.loading),
        action_id: status.map(|s| s.last_action_id),
        capture_pending: trace.capture_pending,
        metadata_at: "start",
    }
}

fn observe_occlusion(
    mut events: MessageReader<WindowOccluded>,
    window: Query<(Entity, &Window), With<PrimaryWindow>>,
    status: Option<Res<LayoutStatus>>,
    mut trace: ResMut<FrameTrace>,
) {
    let Ok((primary, window)) = window.single() else {
        return;
    };
    for event in events.read().filter(|event| event.window == primary) {
        trace.occlusion = Some((primary, event.occluded));
        let span = Span {
            frame: trace.frame,
            phase: "window_occlusion_changed".into(),
            start_ms: trace.origin.elapsed().as_secs_f64() * 1000.0,
            duration_ms: 0.0,
            focused: Some(window.focused),
            present_mode: Some(format!("{:?}", window.present_mode)),
            occluded: Some(event.occluded),
            loading: status.as_ref().is_some_and(|s| s.loading),
            action_id: status.as_ref().map(|s| s.last_action_id),
            capture_pending: trace.capture_pending,
            metadata_at: "event_observed",
        };
        trace.push(span);
    }
}

fn boundary(world: &mut World, phase: Option<String>, first: bool) {
    let now = Instant::now();
    let mut next = sample(world, phase.unwrap_or_default(), now);
    let mut trace = world.resource_mut::<FrameTrace>();
    if let Some((mut span, start)) = trace.active.take() {
        span.duration_ms = now.duration_since(start).as_secs_f64() * 1000.0;
        trace.push(span);
    }
    if first {
        trace.frame += 1;
        next.frame = trace.frame;
        if let Some(last) = trace.last_end {
            let mut gap = next.clone();
            gap.phase = "between_updates".into();
            gap.start_ms = last.duration_since(trace.origin).as_secs_f64() * 1000.0;
            gap.duration_ms = now.duration_since(last).as_secs_f64() * 1000.0;
            gap.metadata_at = "end";
            trace.push(gap);
        }
    }
    trace.last_end = Some(now);
    if !next.phase.is_empty() {
        trace.active = Some((next, now));
    }
}

pub fn install(app: &mut App, origin: Instant) {
    assert!(!app.world().contains_resource::<FrameTrace>());
    let surface = SurfaceTrace::new(origin);
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(surface.clone()).add_systems(
            Render,
            (
                surface_begin
                    .in_set(RenderSystems::ManageViews)
                    .before(prepare_windows),
                surface_end
                    .in_set(RenderSystems::ManageViews)
                    .after(prepare_windows),
            ),
        );
    }
    app.insert_resource(FrameTrace {
        origin,
        frame: 0,
        active: None,
        last_end: None,
        capture_pending: false,
        spans: VecDeque::new(),
        dropped: 0,
        surface,
        occlusion: None,
    });
    if !app.world().contains_resource::<Messages<WindowOccluded>>() {
        app.add_message::<WindowOccluded>();
    }
    app.add_systems(PreUpdate, observe_occlusion);
    let labels = app.world().resource::<MainScheduleOrder>().labels.clone();
    let mut wrapped = Vec::with_capacity(labels.len() * 2 + 1);
    for (index, label) in labels.iter().enumerate() {
        let name = format!("{label:?}");
        app.add_systems(Boundary(index), move |world: &mut World| {
            boundary(world, Some(name.clone()), index == 0);
        });
        wrapped.extend([Boundary(index).intern(), *label]);
    }
    let end = Boundary(labels.len());
    app.add_systems(end.clone(), |world: &mut World| {
        boundary(world, None, false)
    });
    wrapped.push(end.intern());
    app.world_mut().resource_mut::<MainScheduleOrder>().labels = wrapped;
    if let Some(extract) = app.get_sub_app_mut(RenderExtractApp)
        && let Some(mut original) = extract.take_extract()
    {
        extract.set_extract(move |main, render| {
            let started = Instant::now();
            let mut span = sample(main, "render_join_and_extract".into(), started);
            {
                let mut trace = main.resource_mut::<FrameTrace>();
                if let Some(last) = trace.last_end {
                    let mut gap = span.clone();
                    gap.phase = "before_render_extract".into();
                    gap.start_ms = last.duration_since(trace.origin).as_secs_f64() * 1000.0;
                    gap.duration_ms = started.duration_since(last).as_secs_f64() * 1000.0;
                    gap.metadata_at = "end";
                    trace.push(gap);
                }
            }
            original(main, render);
            let ended = Instant::now();
            span.duration_ms = ended.duration_since(started).as_secs_f64() * 1000.0;
            let mut trace = main.resource_mut::<FrameTrace>();
            trace.push(span);
            trace.last_end = Some(ended);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::app::SubApp;

    #[derive(Resource, Default)]
    struct Calls(Vec<&'static str>);

    #[test]
    fn traces_schedules_and_original_extract_without_changing_order_or_focus() {
        let mut app = App::new();
        app.init_resource::<Calls>();
        app.add_systems(First, |mut calls: ResMut<Calls>| calls.0.push("first"));
        app.add_systems(Update, |mut calls: ResMut<Calls>| calls.0.push("update"));
        app.add_systems(Last, |mut calls: ResMut<Calls>| calls.0.push("last"));
        let mut sub = SubApp::new();
        sub.set_extract(|main, _| main.resource_mut::<Calls>().0.push("extract"));
        app.insert_sub_app(RenderExtractApp, sub);
        app.world_mut().spawn((
            Window {
                focused: false,
                ..default()
            },
            PrimaryWindow,
        ));
        install(&mut app, Instant::now());
        app.update();
        app.update();
        assert_eq!(
            app.world().resource::<Calls>().0,
            [
                "first", "update", "last", "extract", "first", "update", "last", "extract"
            ]
        );
        let trace = app.world().resource::<FrameTrace>();
        let phases: Vec<_> = trace.spans.iter().map(|s| s.phase.as_str()).collect();
        assert_eq!(
            &phases[..9],
            [
                "First",
                "PreUpdate",
                "RunFixedMainLoop",
                "Update",
                "SpawnScene",
                "PostUpdate",
                "Last",
                "before_render_extract",
                "render_join_and_extract"
            ]
        );
        assert_eq!(phases[9], "between_updates");
        assert_eq!(trace.spans[9].frame, 2);
        assert_eq!(trace.spans[9].metadata_at, "end");
        for adjacent in trace.spans.iter().zip(trace.spans.iter().skip(1)) {
            let end = adjacent.0.start_ms + adjacent.0.duration_ms;
            assert!((end - adjacent.1.start_ms).abs() < 0.000001);
        }
        assert!(trace.spans.iter().all(|s| s.focused == Some(false)));
        assert!(
            trace
                .spans
                .iter()
                .all(|s| s.present_mode.as_deref() == Some("Fifo"))
        );
        assert!(trace.spans.iter().all(|s| s.duration_ms >= 0.0));
        assert_eq!(
            trace
                .spans
                .iter()
                .filter(|s| s.phase == "render_join_and_extract")
                .count(),
            2
        );
    }

    #[test]
    fn trace_memory_is_bounded_and_reports_dropped_samples() {
        let mut app = App::new();
        install(&mut app, Instant::now());
        for _ in 0..1100 {
            app.update();
        }
        let trace = app.world().resource::<FrameTrace>();
        assert_eq!(trace.spans.len(), LIMIT);
        assert!(trace.dropped > 0);
        assert_eq!(trace.report()["dropped_spans"], trace.dropped);
    }

    #[test]
    fn capture_cpu_is_nested_and_pending_state_does_not_leak_to_next_phase() {
        let mut app = App::new();
        install(&mut app, Instant::now());
        app.add_systems(Update, |mut trace: ResMut<FrameTrace>| {
            trace.capture_pending(true);
            let started = Instant::now();
            trace.capture_cpu(started);
            trace.capture_pending(false);
        });
        app.update();
        let trace = app.world().resource::<FrameTrace>();
        let capture = trace
            .spans
            .iter()
            .find(|s| s.phase == "screenshot_cpu_nested")
            .unwrap();
        let update = trace.spans.iter().find(|s| s.phase == "Update").unwrap();
        assert!(capture.start_ms >= update.start_ms);
        assert!(capture.start_ms + capture.duration_ms <= update.start_ms + update.duration_ms);
        assert!(capture.capture_pending);
        assert_eq!(capture.metadata_at, "unavailable");
        assert!(
            trace
                .spans
                .iter()
                .filter(|s| s.phase != "screenshot_cpu_nested")
                .all(|s| !s.capture_pending)
        );
    }

    #[test]
    fn render_brackets_share_the_clock_and_bound_memory() {
        let origin = Instant::now();
        let trace = SurfaceTrace::new(origin);
        let peer = trace.clone();
        for invocation in 0..LIMIT + 3 {
            let start = origin + std::time::Duration::from_millis(invocation as u64 * 10);
            trace.begin(start);
            peer.end(start + std::time::Duration::from_millis(2));
        }
        let report = trace.report();
        assert_eq!(report["dropped_spans"], 3);
        assert_eq!(report["spans"].as_array().unwrap().len(), LIMIT);
        assert_eq!(
            report["spans"][0],
            serde_json::json!({"invocation":4,"start_ms":30.0,"duration_ms":2.0})
        );
        assert!(trace.data.lock().unwrap().active.is_none());
    }

    #[test]
    fn occlusion_is_observed_independently_of_focus_without_consuming_other_readers() {
        let mut app = App::new();
        install(&mut app, Instant::now());
        let primary = app
            .world_mut()
            .spawn((
                Window {
                    focused: false,
                    ..default()
                },
                PrimaryWindow,
            ))
            .id();
        let other = app.world_mut().spawn(Window::default()).id();
        app.update();
        assert!(app.world().resource::<FrameTrace>().occlusion.is_none());
        let mut cursor = bevy::ecs::message::MessageCursor::<WindowOccluded>::default();
        app.world_mut().write_message(WindowOccluded {
            window: primary,
            occluded: true,
        });
        app.world_mut().write_message(WindowOccluded {
            window: other,
            occluded: false,
        });
        app.update();
        assert_eq!(
            app.world().resource::<FrameTrace>().occlusion,
            Some((primary, true))
        );
        assert_eq!(
            cursor
                .read(app.world().resource::<Messages<WindowOccluded>>())
                .count(),
            2
        );
        app.world_mut().write_message(WindowOccluded {
            window: primary,
            occluded: false,
        });
        app.update();
        let trace = app.world().resource::<FrameTrace>();
        assert_eq!(trace.occlusion, Some((primary, false)));
        assert_eq!(
            trace
                .spans
                .iter()
                .filter(|s| s.phase == "window_occlusion_changed")
                .count(),
            2
        );
    }
}
