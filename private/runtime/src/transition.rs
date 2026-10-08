//! Scene transitions: how one accepted scene gives way to the next.
//!
//! A transition is data. A [`Choreography`] says, for each role an object can play
//! between two scenes, where it moves, whether it fades, and when:
//!
//! - **leaving**: drawn only in the outgoing scene;
//! - **travelling**: drawn in both, matched by semantic identity, moving from its old
//!   place to its new one;
//! - **arriving**: drawn only in the incoming scene;
//!
//! plus a highlight for travellers and windows for connectors and labels. The scene rebuild assigns the roles and
//! attaches one [`Track`] per entity; [`play`] plays every track. A new animation is a
//! new choreography (or a new channel on [`Track`]), never a new system.
use bevy::prelude::*;

use crate::style::GlassMaterial;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ease {
    Linear,
    /// Starts slow: things falling away.
    In,
    /// Ends slow: things settling into place.
    Out,
    InOut,
}

impl Ease {
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::In => t * t * t,
            Self::Out => 1.0 - (1.0 - t).powi(3),
            Self::InOut if t < 0.5 => 4.0 * t * t * t,
            Self::InOut => 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0,
        }
    }
}

/// When something happens, in seconds from the start of the transition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    pub start: f32,
    pub duration: f32,
    pub ease: Ease,
}

impl Window {
    pub const fn new(start: f32, duration: f32, ease: Ease) -> Self {
        Self {
            start,
            duration,
            ease,
        }
    }

    pub fn end(&self) -> f32 {
        self.start + self.duration
    }

    /// Eased progress in [0, 1] at `elapsed` seconds.
    pub fn progress(&self, elapsed: f32) -> f32 {
        if self.duration <= 0.0 {
            return if elapsed >= self.start { 1.0 } else { 0.0 };
        }
        self.ease.apply((elapsed - self.start) / self.duration)
    }
}

/// How an object leaves or arrives: the displacement, scale, and opacity it has
/// while away from its place in the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Motion {
    pub window: Window,
    pub offset: Vec3,
    pub scale: f32,
    pub fade: bool,
}

/// A rise and fall of emphasis: the object glows in its own colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Glow {
    pub rise: Window,
    pub fall: Window,
    /// Added emission at the peak, as a fraction of the object's colour.
    pub strength: f32,
}

impl Glow {
    pub fn level(&self, elapsed: f32) -> f32 {
        self.strength * self.rise.progress(elapsed) * (1.0 - self.fall.progress(elapsed))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Choreography {
    pub name: &'static str,
    pub leave: Motion,
    pub travel: Window,
    /// Travelling objects light up while the rest of the change happens.
    pub highlight: Option<Glow>,
    pub arrive: Motion,
    pub connectors_out: Window,
    pub connectors_in: Window,
    /// Incoming labels appear at this time, once their objects have settled.
    pub labels_in: f32,
}

impl Choreography {
    /// Lifting part of the scene out: the part lights up where it stands while the
    /// rest falls away, then the part flies to its own layout.
    pub fn pull_out() -> Self {
        Self {
            name: "pull_out",
            leave: Motion {
                window: Window::new(0.0, 1.8, Ease::In),
                offset: Vec3::new(0.0, -8.0, 0.0),
                scale: 0.85,
                fade: true,
            },
            travel: Window::new(1.5, 1.5, Ease::InOut),
            highlight: Some(Glow {
                rise: Window::new(0.0, 0.6, Ease::Out),
                fall: Window::new(2.7, 0.6, Ease::InOut),
                strength: 0.6,
            }),
            arrive: Motion {
                window: Window::new(2.7, 0.5, Ease::Out),
                offset: Vec3::ZERO,
                scale: 1.0,
                fade: true,
            },
            connectors_out: Window::new(0.0, 0.5, Ease::Linear),
            connectors_in: Window::new(3.05, 0.4, Ease::Linear),
            labels_in: 3.3,
        }
    }

    /// The reverse: the part flies home, then the rest rises back around it.
    pub fn put_back() -> Self {
        Self {
            name: "put_back",
            leave: Motion {
                window: Window::new(0.0, 0.3, Ease::Linear),
                offset: Vec3::ZERO,
                scale: 1.0,
                fade: true,
            },
            travel: Window::new(0.1, 1.0, Ease::InOut),
            highlight: None,
            arrive: Motion {
                window: Window::new(0.8, 0.75, Ease::Out),
                offset: Vec3::new(0.0, -8.0, 0.0),
                scale: 0.85,
                fade: true,
            },
            connectors_out: Window::new(0.0, 0.25, Ease::Linear),
            connectors_in: Window::new(1.25, 0.35, Ease::Linear),
            labels_in: 1.45,
        }
    }

    /// A general change of layout: what stays moves, the rest cross-fades.
    pub fn morph() -> Self {
        Self {
            name: "morph",
            leave: Motion {
                window: Window::new(0.0, 0.35, Ease::Linear),
                offset: Vec3::ZERO,
                scale: 1.0,
                fade: true,
            },
            travel: Window::new(0.15, 0.8, Ease::InOut),
            highlight: None,
            arrive: Motion {
                window: Window::new(0.6, 0.35, Ease::Out),
                offset: Vec3::ZERO,
                scale: 1.0,
                fade: true,
            },
            connectors_out: Window::new(0.0, 0.25, Ease::Linear),
            connectors_in: Window::new(0.85, 0.3, Ease::Linear),
            labels_in: 1.0,
        }
    }

    /// Ends whatever is playing at once; used when a scene changes without one.
    const fn cut() -> Self {
        let instant = Window::new(0.0, 0.0, Ease::Linear);
        let motion = Motion {
            window: instant,
            offset: Vec3::ZERO,
            scale: 1.0,
            fade: false,
        };
        Self {
            name: "cut",
            leave: motion,
            travel: instant,
            highlight: None,
            arrive: motion,
            connectors_out: instant,
            connectors_in: instant,
            labels_in: 0.0,
        }
    }

    pub fn duration(&self) -> f32 {
        [
            self.leave.window.end(),
            self.travel.end(),
            self.highlight.map_or(0.0, |glow| glow.fall.end()),
            self.arrive.window.end(),
            self.connectors_out.end(),
            self.connectors_in.end(),
            self.labels_in,
        ]
        .into_iter()
        .fold(0.0, f32::max)
    }
}

/// One entity's animation: position, scale, and opacity over a window. The entity
/// rests at `to`, scale 1, and its own opacity once the track ends, unless it is
/// removed.
#[derive(Component, Clone, Debug, PartialEq)]
pub(crate) struct Track {
    pub generation: u64,
    pub window: Window,
    pub from: Vec3,
    pub to: Vec3,
    /// Per axis, relative to the size the entity was drawn at.
    pub scale: (Vec3, Vec3),
    pub opacity: (f32, f32),
    /// Emphasis over the whole transition, on its own clock.
    pub glow: Option<Glow>,
    /// Leaving entities are removed when the transition ends.
    pub despawn: bool,
}

impl Track {
    pub fn travel(generation: u64, window: Window, from: Vec3, to: Vec3) -> Self {
        Self {
            generation,
            window,
            from,
            to,
            scale: (Vec3::ONE, Vec3::ONE),
            opacity: (1.0, 1.0),
            glow: None,
            despawn: false,
        }
    }

    /// Adds emphasis on the transition's clock.
    pub fn glowing(self, glow: Option<Glow>) -> Self {
        Self { glow, ..self }
    }

    /// Grows or shrinks from the size an object had in the outgoing scene to the
    /// size it is drawn at, alongside the move. Both are half extents.
    pub fn resizing(self, from_half: Vec3, to_half: Vec3) -> Self {
        let ratio = |from: f32, to: f32| if to > 1e-4 { from / to } else { 1.0 };
        Self {
            scale: (
                Vec3::new(
                    ratio(from_half.x, to_half.x),
                    ratio(from_half.y, to_half.y),
                    ratio(from_half.z, to_half.z),
                ),
                Vec3::ONE,
            ),
            ..self
        }
    }

    pub fn leave(generation: u64, motion: Motion, at: Vec3) -> Self {
        Self {
            generation,
            window: motion.window,
            from: at,
            to: at + motion.offset,
            scale: (Vec3::ONE, Vec3::splat(motion.scale)),
            opacity: (1.0, if motion.fade { 0.0 } else { 1.0 }),
            glow: None,
            despawn: true,
        }
    }

    pub fn arrive(generation: u64, motion: Motion, at: Vec3) -> Self {
        Self {
            generation,
            window: motion.window,
            from: at + motion.offset,
            to: at,
            scale: (Vec3::splat(motion.scale), Vec3::ONE),
            opacity: (if motion.fade { 0.0 } else { 1.0 }, 1.0),
            glow: None,
            despawn: false,
        }
    }

    pub fn fade(generation: u64, window: Window, at: Vec3, opacity: (f32, f32)) -> Self {
        Self {
            generation,
            window,
            from: at,
            to: at,
            scale: (Vec3::ONE, Vec3::ONE),
            opacity,
            glow: None,
            despawn: opacity.1 <= 0.0,
        }
    }

    /// Where the entity stands, its scale, and its opacity at `progress`.
    pub fn sample(&self, progress: f32) -> (Vec3, Vec3, f32) {
        (
            self.from.lerp(self.to, progress),
            self.scale.0.lerp(self.scale.1, progress),
            self.opacity.0 + (self.opacity.1 - self.opacity.0) * progress,
        )
    }
}

/// A world-anchored label that waits until `at` before it may show.
#[derive(Component, Clone, Copy, Debug)]
pub(crate) struct Held {
    pub generation: u64,
    pub at: f32,
}

/// The materials a track fades, and their own values, recorded on first use.
#[derive(Component, Default)]
pub(crate) struct Faded {
    standard: Vec<(AssetId<StandardMaterial>, Color, LinearRgba, AlphaMode)>,
    glass: Vec<(AssetId<GlassMaterial>, Vec4)>,
}

struct Playing {
    choreography: Choreography,
    generation: u64,
    started: Option<f32>,
}

/// The transition being played, if any. A new one ends the previous at once.
#[derive(Resource, Default)]
pub(crate) struct SceneTransition {
    playing: Option<Playing>,
    generation: u64,
}

impl SceneTransition {
    /// Starts `choreography` and returns the generation its tracks must carry.
    pub fn begin(&mut self, choreography: Choreography) -> u64 {
        self.generation += 1;
        self.playing = Some(Playing {
            choreography,
            generation: self.generation,
            started: None,
        });
        self.generation
    }

    /// A scene changed without a transition: settle anything still playing.
    pub fn cut(&mut self) {
        if self.playing.is_some() {
            self.begin(Choreography::cut());
        }
    }

    pub fn active(&self) -> bool {
        self.playing.is_some()
    }

    /// Whether objects have started to travel: true once the playing transition's
    /// travel window has opened, and false before it starts or when none plays.
    pub fn travelling(&self, now: f32) -> bool {
        self.playing.as_ref().is_some_and(|playing| {
            playing
                .started
                .is_some_and(|started| now - started >= playing.choreography.travel.start)
        })
    }

    pub fn name(&self) -> Option<&'static str> {
        self.playing.as_ref().map(|p| p.choreography.name)
    }
}

pub(crate) struct TransitionPlugin;

impl Plugin for TransitionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneTransition>().add_systems(
            PostUpdate,
            play.before(bevy::transform::TransformSystems::Propagate),
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn play(
    mut commands: Commands,
    time: Res<Time>,
    mut transition: ResMut<SceneTransition>,
    mut tracks: Query<(Entity, &Track, &mut Transform, Option<&Faded>)>,
    mut labels: Query<(Entity, &Held, &mut Node)>,
    children: Query<&Children>,
    standard: Query<&MeshMaterial3d<StandardMaterial>>,
    glass: Query<&MeshMaterial3d<GlassMaterial>>,
    mut standard_assets: ResMut<Assets<StandardMaterial>>,
    mut glass_assets: ResMut<Assets<GlassMaterial>>,
) {
    let Some(playing) = transition.playing.as_mut() else {
        return;
    };
    let now = time.elapsed_secs();
    let elapsed = now - *playing.started.get_or_insert(now);
    let generation = playing.generation;
    let finished = elapsed >= playing.choreography.duration();
    for (entity, track, mut transform, faded) in &mut tracks {
        // Tracks from an earlier transition end where they were going.
        let progress = if track.generation == generation {
            track.window.progress(elapsed)
        } else {
            1.0
        };
        let (at, scale, opacity) = track.sample(progress);
        let glow = match track.glow {
            Some(glow) if track.generation == generation => glow.level(elapsed),
            _ => 0.0,
        };
        let done = progress >= 1.0 && (finished || track.generation != generation);
        if done && track.despawn {
            commands.entity(entity).despawn();
            continue;
        }
        transform.translation = at;
        transform.scale = scale;
        match faded {
            Some(faded) => appear(
                faded,
                opacity,
                glow,
                &mut standard_assets,
                &mut glass_assets,
            ),
            None => {
                let recorded = record(
                    entity,
                    &children,
                    &standard,
                    &glass,
                    &standard_assets,
                    &glass_assets,
                );
                appear(
                    &recorded,
                    opacity,
                    glow,
                    &mut standard_assets,
                    &mut glass_assets,
                );
                if !done {
                    commands.entity(entity).insert(recorded);
                }
            }
        }
        if done {
            commands.entity(entity).remove::<(Track, Faded)>();
        }
    }
    for (entity, held, mut node) in &mut labels {
        if held.generation != generation || elapsed >= held.at {
            commands.entity(entity).remove::<Held>();
        } else {
            node.display = Display::None;
        }
    }
    if finished {
        transition.playing = None;
    }
}

fn record(
    root: Entity,
    children: &Query<&Children>,
    standard: &Query<&MeshMaterial3d<StandardMaterial>>,
    glass: &Query<&MeshMaterial3d<GlassMaterial>>,
    standard_assets: &Assets<StandardMaterial>,
    glass_assets: &Assets<GlassMaterial>,
) -> Faded {
    let mut faded = Faded::default();
    for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
        if let Ok(handle) = standard.get(entity)
            && let Some(material) = standard_assets.get(&handle.0)
            && !faded.standard.iter().any(|(id, ..)| *id == handle.0.id())
        {
            faded.standard.push((
                handle.0.id(),
                material.base_color,
                material.emissive,
                material.alpha_mode,
            ));
        }
        if let Ok(handle) = glass.get(entity)
            && let Some(material) = glass_assets.get(&handle.0)
            && !faded.glass.iter().any(|(id, _)| *id == handle.0.id())
        {
            faded.glass.push((handle.0.id(), material.settings.shape));
        }
    }
    faded
}

/// Sets every recorded material to `opacity` of its own, plus `glow` emphasis.
/// At full opacity and no glow, the materials are exactly as recorded.
fn appear(
    faded: &Faded,
    opacity: f32,
    glow: f32,
    standard: &mut Assets<StandardMaterial>,
    glass: &mut Assets<GlassMaterial>,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    let glow = glow.max(0.0);
    for (id, color, emissive, mode) in &faded.standard {
        let Some(material) = standard.get_mut(*id) else {
            continue;
        };
        let tint = color.to_linear();
        if opacity >= 1.0 {
            material.base_color = *color;
            material.alpha_mode = *mode;
        } else {
            material.base_color = color.with_alpha(color.alpha() * opacity);
            material.alpha_mode = AlphaMode::Blend;
        }
        material.emissive = LinearRgba::new(
            emissive.red * opacity + tint.red * glow,
            emissive.green * opacity + tint.green * glow,
            emissive.blue * opacity + tint.blue * glow,
            emissive.alpha,
        );
    }
    for (id, shape) in &faded.glass {
        if let Some(material) = glass.get_mut(*id) {
            let level = opacity * (1.0 + glow);
            material.settings.shape = Vec4::new(shape.x * level, shape.y * level, shape.z, shape.w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases_start_and_end_where_they_should() {
        for ease in [Ease::Linear, Ease::In, Ease::Out, Ease::InOut] {
            assert_eq!(ease.apply(0.0), 0.0, "{ease:?}");
            assert!((ease.apply(1.0) - 1.0).abs() < 1e-6, "{ease:?}");
            assert_eq!(ease.apply(-1.0), 0.0, "{ease:?}");
            assert_eq!(ease.apply(2.0), 1.0, "{ease:?}");
        }
        assert!(Ease::In.apply(0.5) < 0.5 && Ease::Out.apply(0.5) > 0.5);
    }

    #[test]
    fn windows_hold_before_they_start_and_after_they_end() {
        let window = Window::new(1.0, 2.0, Ease::Linear);
        assert_eq!(window.progress(0.0), 0.0);
        assert_eq!(window.progress(2.0), 0.5);
        assert_eq!(window.progress(9.0), 1.0);
        assert_eq!(Window::new(1.0, 0.0, Ease::Linear).progress(0.5), 0.0);
        assert_eq!(Window::new(1.0, 0.0, Ease::Linear).progress(1.0), 1.0);
    }

    /// The old prelude (morph 1.15 s, then a 0.6 s hold) and pull-out (1.7 s), as one.
    const PULL_OUT_SECONDS: f32 = 1.15 + 0.6 + 1.7;

    #[test]
    fn pulling_out_lights_the_part_as_the_rest_falls_then_lands_it() {
        let c = Choreography::pull_out();
        assert!(
            (c.duration() - PULL_OUT_SECONDS).abs() < 1e-4,
            "{}",
            c.duration()
        );
        // Lighting up and falling away start together.
        let glow = c.highlight.expect("the part lights up");
        assert_eq!(glow.rise.start, c.leave.window.start);
        assert!(glow.rise.end() < c.leave.window.end());
        // The part leaves once the rest is mostly gone, and stays lit until it lands.
        assert!(c.leave.window.progress(c.travel.start) > 0.5);
        assert!(c.leave.window.end() < c.travel.end());
        assert!(glow.fall.start >= c.travel.end() - 0.3);
        // Connectors go first and return last, with the labels.
        assert!(c.connectors_out.end() <= c.travel.start);
        assert!(c.connectors_in.start >= c.travel.end());
        assert!(c.labels_in >= c.travel.end());
        // Putting back mirrors it: the part moves home before the rest returns.
        let back = Choreography::put_back();
        assert!(back.travel.start < back.arrive.window.start);
        assert!(back.arrive.offset.y < 0.0 && c.leave.offset.y < 0.0);
    }

    #[test]
    fn a_glow_rises_holds_and_falls_away_completely() {
        let glow = Choreography::pull_out().highlight.unwrap();
        assert_eq!(glow.level(0.0), 0.0);
        assert!((glow.level(glow.rise.end()) - glow.strength).abs() < 1e-6);
        assert!((glow.level(glow.fall.start) - glow.strength).abs() < 1e-6);
        assert_eq!(glow.level(glow.fall.end()), 0.0);
        assert!(Choreography::morph().highlight.is_none());
    }

    #[test]
    fn objects_count_as_travelling_only_once_their_window_opens() {
        let mut transition = SceneTransition::default();
        assert!(!transition.travelling(10.0));
        transition.begin(Choreography::pull_out());
        assert!(
            !transition.travelling(10.0),
            "not started until first played"
        );
        transition.playing.as_mut().unwrap().started = Some(10.0);
        let travel = Choreography::pull_out().travel.start;
        assert!(!transition.travelling(10.0 + travel - 0.1));
        assert!(transition.travelling(10.0 + travel));
    }

    #[test]
    fn tracks_leave_arrive_and_travel_between_their_ends() {
        let pull = Choreography::pull_out();
        let at = Vec3::new(1.0, 2.0, 3.0);
        let leave = Track::leave(1, pull.leave, at);
        assert_eq!(leave.sample(0.0), (at, Vec3::ONE, 1.0));
        assert_eq!(
            leave.sample(1.0),
            (at + pull.leave.offset, Vec3::splat(0.85), 0.0)
        );
        assert!(leave.despawn);
        let arrive = Track::arrive(1, Choreography::put_back().arrive, at);
        assert_eq!(arrive.sample(1.0), (at, Vec3::ONE, 1.0));
        assert_eq!(arrive.sample(0.0).2, 0.0);
        assert!(!arrive.despawn);
        let travel = Track::travel(1, pull.travel, Vec3::ZERO, at);
        assert_eq!(travel.sample(0.5).0, at * 0.5);
        assert_eq!(travel.sample(0.5).1, Vec3::ONE, "no resize unless asked");
        // A box twice as wide and half as tall in its new place starts at its old
        // size and grows into the new one along the way.
        let resized = travel
            .clone()
            .resizing(Vec3::new(1.0, 2.0, 3.0), Vec3::new(2.0, 1.0, 3.0));
        assert_eq!(resized.sample(0.0).1, Vec3::new(0.5, 2.0, 1.0));
        assert_eq!(resized.sample(0.5).1, Vec3::new(0.75, 1.5, 1.0));
        assert_eq!(resized.sample(1.0).1, Vec3::ONE);
        // A flat new extent never divides by zero.
        let flat = travel.resizing(Vec3::ONE, Vec3::new(1.0, 0.0, 1.0));
        assert_eq!(flat.sample(0.0).1, Vec3::ONE);
        assert!(Track::fade(1, pull.connectors_out, at, (1.0, 0.0)).despawn);
        assert!(!Track::fade(1, pull.connectors_in, at, (0.0, 1.0)).despawn);
    }

    #[test]
    fn a_new_transition_supersedes_the_last_and_a_cut_settles_it() {
        let mut transition = SceneTransition::default();
        transition.cut();
        assert!(
            !transition.active(),
            "a cut with nothing playing does nothing"
        );
        let first = transition.begin(Choreography::pull_out());
        let second = transition.begin(Choreography::morph());
        assert!(second > first);
        assert_eq!(transition.name(), Some("morph"));
        transition.cut();
        assert_eq!(transition.name(), Some("cut"));
        assert_eq!(Choreography::cut().duration(), 0.0);
    }
}
