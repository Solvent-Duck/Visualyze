//! Deliberate presentation experiments for production_smoke only.
//! The viewer's production defaults and window update policy are unchanged.
use bevy::{
    prelude::*,
    window::{PresentMode, PrimaryWindow, WindowOccluded},
};
use std::time::Instant;

/// Optional native exercise. The app keeps updating while an operator covers
/// and restores its window; only the scripted architecture actions wait.
#[derive(Resource)]
pub struct OcclusionCycle {
    enabled: bool,
    hidden_ms: Option<f64>,
    restored_ms: Option<f64>,
    origin: Instant,
}

impl OcclusionCycle {
    pub fn complete(&self) -> bool {
        !self.enabled || self.restored_ms.is_some()
    }

    fn observe(&mut self, occluded: bool, elapsed_ms: f64) {
        if self.enabled && self.restored_ms.is_none() {
            if occluded {
                self.hidden_ms.get_or_insert(elapsed_ms);
            } else if self.hidden_ms.is_some() {
                self.restored_ms = Some(elapsed_ms);
            }
        }
    }

    pub fn report(&self) -> serde_json::Value {
        serde_json::json!({"enabled":self.enabled,"hidden_observed_ms":self.hidden_ms,
            "restored_observed_ms":self.restored_ms,"completed":self.complete(),
            "scope":"When enabled, only scripted action dispatch waits for a primary-window occluded-then-visible notification cycle. Rendering, layout work and event processing continue. Offsets are event observation times relative to proof start, not OS event or display timestamps; the operator controls the window through the desktop UI."})
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationProbe {
    #[default]
    Default,
    Immediate,
    BackgroundImmediate,
}

impl PresentationProbe {
    pub fn with_argument(self, arg: &str) -> Self {
        assert_eq!(
            self,
            Self::Default,
            "only one presentation probe is allowed"
        );
        match arg {
            "--present-immediate" => Self::Immediate,
            "--present-background-immediate" => Self::BackgroundImmediate,
            _ => panic!("unknown presentation probe"),
        }
    }

    fn apply(self, window: &mut Window) {
        match self {
            Self::Default => (),
            Self::Immediate => window.present_mode = PresentMode::Immediate,
            Self::BackgroundImmediate => {
                window.present_mode = if window.focused {
                    PresentMode::Fifo
                } else {
                    PresentMode::Immediate
                };
            }
        }
    }

    pub fn report(self, window: &Window) -> serde_json::Value {
        serde_json::json!({
            "requested":format!("{self:?}"),
            "last_window_present_mode":format!("{:?}",window.present_mode),
            "last_window_focused":window.focused,
            "desired_maximum_frame_latency":window.desired_maximum_frame_latency.map(|n| n.get()),
            "scope":"Example-only experiment. Default leaves all window settings unchanged. Immediate requests unsynchronized presentation; BackgroundImmediate requests Fifo while focused and Immediate while unfocused. Window update policy is unchanged. These are requested Bevy settings, not a query of the actual GPU swapchain mode; the pinned Metal backend supports both explicit modes. CPU traces do not measure GPU presentation, power or visual tearing."
        })
    }
}

pub fn install(app: &mut App, probe: PresentationProbe, await_cycle: bool, origin: Instant) {
    app.insert_resource(OcclusionCycle {
        enabled: await_cycle,
        hidden_ms: None,
        restored_ms: None,
        origin,
    })
    .insert_resource(probe)
    .add_systems(
        PreUpdate,
        |probe: Res<PresentationProbe>, mut windows: Query<&mut Window>| {
            if *probe != PresentationProbe::Default {
                for mut window in &mut windows {
                    probe.apply(&mut window);
                }
            }
        },
    )
    .add_systems(PreUpdate, observe_cycle);
}

fn observe_cycle(
    mut cycle: ResMut<OcclusionCycle>,
    mut events: MessageReader<WindowOccluded>,
    windows: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(primary) = windows.single() else {
        return;
    };
    for event in events.read().filter(|event| event.window == primary) {
        let elapsed = cycle.origin.elapsed().as_secs_f64() * 1000.0;
        cycle.observe(event.occluded, elapsed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preserves_custom_settings_and_background_probe_restores_fifo() {
        let mut window = Window {
            present_mode: PresentMode::Mailbox,
            desired_maximum_frame_latency: std::num::NonZero::new(1),
            ..default()
        };
        PresentationProbe::Default.apply(&mut window);
        assert_eq!(window.present_mode, PresentMode::Mailbox);
        for (focus, mode) in [
            (true, PresentMode::Fifo),
            (false, PresentMode::Immediate),
            (true, PresentMode::Fifo),
        ] {
            window.focused = focus;
            PresentationProbe::BackgroundImmediate.apply(&mut window);
            assert_eq!(window.present_mode, mode);
            assert_eq!(window.desired_maximum_frame_latency.unwrap().get(), 1);
            assert!(window.visible);
        }
        PresentationProbe::Immediate.apply(&mut window);
        assert_eq!(window.present_mode, PresentMode::Immediate);
    }

    #[test]
    #[should_panic(expected = "only one presentation probe")]
    fn conflicting_probes_are_rejected() {
        PresentationProbe::Default
            .with_argument("--present-immediate")
            .with_argument("--present-background-immediate");
    }

    #[test]
    fn exercise_requires_hidden_then_visible_and_keeps_first_cycle() {
        let mut cycle = OcclusionCycle {
            enabled: true,
            hidden_ms: None,
            restored_ms: None,
            origin: Instant::now(),
        };
        cycle.observe(false, 1.0);
        assert!(!cycle.complete());
        cycle.observe(true, 2.0);
        cycle.observe(true, 3.0);
        assert!(!cycle.complete());
        cycle.observe(false, 4.0);
        cycle.observe(true, 5.0);
        assert!(cycle.complete());
        assert_eq!(cycle.hidden_ms, Some(2.0));
        assert_eq!(cycle.restored_ms, Some(4.0));
    }
}
