//! Walkthrough playback: turns an agent's validated script into staged scenes,
//! camera moves, a travelling trace token, and the labels the agent supplied.
//! Visualyze writes no explanation of its own; the agent narrates in its chat.
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use bevy::prelude::*;
use serde_json::{Value, json};
use visualyze_model::{
    architecture::{Architecture, EntityKind},
    walkthrough::{CameraIntent, Context, Ownership, Presentation, Reveal, Walkthrough},
};

use crate::{
    camera::{CameraDirector, CameraInputLock, OrbitCamera},
    layout_scene::{
        DevChrome, LayoutStatus, PlacedObject, PlacedRelation, SpotContext, Spotlight, StagedScene,
        ViewerAction, ViewerActions,
    },
};

/// Seconds a step holds in autoplay before its own motion and labels are added.
const BASE_HOLD: f32 = 2.5;
const HOLD_PER_LABEL: f32 = 0.8;
/// World units per second for the trace token, and the bounds on one hop.
const TOKEN_SPEED: f32 = 9.0;
const HOP_SECONDS: (f32, f32) = (0.7, 1.8);
const HOP_PAUSE: f32 = 0.2;
const LOOP_PAUSE: f32 = 0.7;
const TRAIL: usize = 6;
const TRAIL_SPACING: f32 = 0.06;
const REQUEST_COLOR: Color = Color::srgb(0.45, 0.85, 1.0);
const RETURN_COLOR: Color = Color::srgb(1.0, 0.72, 0.35);
const HONESTY: &str = "Explanatory order, not a live trace";

pub(crate) enum Control {
    Play,
    Pause,
    Next,
    Previous,
    Goto(usize),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Submit the step's scene to the viewer.
    Stage,
    /// Wait for the viewer to accept the scene.
    Staging(u64),
    /// Scene shown: frame the camera, place labels, start the trace.
    Present,
    /// Holding on the step; autoplay advances when the hold elapses.
    Showing { since: f32, hold: f32 },
}

struct Active {
    walkthrough: Walkthrough,
    model: Arc<Architecture>,
    step: usize,
    phase: Phase,
    autoplay: bool,
    /// Autoplay is paused by a command or by the person taking the camera.
    paused: bool,
    interrupted: bool,
    /// Step chrome (progress, hints) for walkthroughs; single views go without.
    chrome: bool,
    error: Option<String>,
}

#[derive(Resource, Default)]
pub(crate) struct Playback {
    active: Option<Active>,
    /// Bumped whenever the shown step changes, so overlays rebuild once.
    generation: u64,
}

impl Playback {
    pub(crate) fn start(
        &mut self,
        walkthrough: Walkthrough,
        model: Arc<Architecture>,
        step: usize,
        autoplay: bool,
        chrome: bool,
    ) {
        let last = walkthrough.script().steps.len() - 1;
        self.active = Some(Active {
            walkthrough,
            model,
            step: step.min(last),
            phase: Phase::Stage,
            autoplay,
            paused: false,
            interrupted: false,
            chrome,
            error: None,
        });
        self.generation += 1;
    }

    pub(crate) fn stop(&mut self) {
        if self.active.take().is_some() {
            self.generation += 1;
        }
    }

    fn goto(&mut self, step: usize) -> Result<(), String> {
        let active = self.active.as_mut().ok_or("no walkthrough is playing")?;
        let count = active.walkthrough.script().steps.len();
        if step >= count {
            return Err(format!("step {step} is out of range (0..{count})"));
        }
        active.step = step;
        active.phase = Phase::Stage;
        active.interrupted = false;
        active.error = None;
        self.generation += 1;
        Ok(())
    }

    pub(crate) fn control(&mut self, control: Control) -> Result<(), String> {
        let active = self.active.as_mut().ok_or("no walkthrough is playing")?;
        let step = active.step;
        let last = active.walkthrough.script().steps.len() - 1;
        match control {
            Control::Play => {
                active.autoplay = true;
                active.paused = false;
                active.interrupted = false;
                Ok(())
            }
            Control::Pause => {
                active.paused = true;
                Ok(())
            }
            Control::Next if step < last => self.goto(step + 1),
            Control::Next => Err("already at the last step".into()),
            Control::Previous => self.goto(step.saturating_sub(1)),
            Control::Goto(step) => self.goto(step),
            Control::Stop => {
                self.stop();
                Ok(())
            }
        }
    }

    pub(crate) fn state(&self) -> Value {
        let Some(active) = &self.active else {
            return json!({"state": "idle"});
        };
        let script = active.walkthrough.script();
        let state = if !matches!(active.phase, Phase::Showing { .. }) {
            "staging"
        } else if active.interrupted {
            "interrupted"
        } else if active.autoplay && !active.paused {
            "playing"
        } else {
            "paused"
        };
        json!({
            "state": state,
            "walkthrough_id": script.id,
            "step": active.step,
            "step_id": script.steps[active.step].id,
            "step_title": script.steps[active.step].title,
            "total": script.steps.len(),
            "autoplay": active.autoplay,
            "error": active.error,
        })
    }
}

/// The viewer scene for one step: where to stand, what to open, what to light.
fn scene(active: &Active) -> StagedScene {
    let model = active.model.snapshot();
    let own = Ownership::new(model);
    let script = active.walkthrough.script();
    let step = &script.steps[active.step];
    let resolved = &active.walkthrough.resolved()[active.step];
    let mut lit: BTreeSet<String> = step.focus.iter().cloned().collect();
    for hop in &resolved.hops {
        lit.insert(hop.from.clone());
        lit.insert(hop.to.clone());
    }
    if step.reveal == Reveal::Cumulative {
        for earlier in &script.steps[..active.step] {
            lit.extend(earlier.focus.iter().cloned());
        }
    }
    let root = resolved.root.as_str();
    // Open only the scopes between the root and what the step is about; a
    // component's own interfaces show without opening it.
    let mut expand = BTreeSet::new();
    let labelled = step
        .labels
        .iter()
        .filter(|l| own.entity(&l.target).is_some())
        .map(|l| l.target.as_str());
    for id in lit.iter().map(String::as_str).chain(labelled) {
        let lineage = own.lineage(id);
        let Some(at_root) = lineage.iter().position(|o| *o == root) else {
            continue;
        };
        // The root itself, or something directly inside it, opens nothing.
        for owner in lineage.get(1..at_root).unwrap_or_default() {
            if own.entity(owner) == Some(EntityKind::Scope) {
                expand.insert((*owner).to_owned());
            }
        }
    }
    StagedScene {
        root: (root != model.system_id).then(|| root.to_owned()),
        expand: expand.into_iter().collect(),
        spotlight: Some(Spotlight {
            lit,
            relations: resolved.hops.iter().map(|h| h.relation.clone()).collect(),
            context: match step.context() {
                Context::Dim => SpotContext::Dim,
                Context::Hide => SpotContext::Hide,
                Context::Show => SpotContext::Show,
            },
        }),
        explode: step.presentation == Presentation::Exploded,
    }
}

/// Owned overlays and tokens for the current step.
#[derive(Component)]
struct StepOwned;

#[derive(Component)]
struct Callout {
    anchor: Vec3,
}

#[derive(Component)]
struct Token {
    /// One polyline per hop, in travel order, with its colour.
    legs: Vec<(Vec<Vec3>, Color)>,
    repeat: bool,
    clock: f32,
    /// Trail followers sample the same path a little behind.
    lag: f32,
}

pub(crate) struct PlaybackPlugin;

impl Plugin for PlaybackPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Playback>()
            .add_systems(
                Update,
                (playback_keys, advance)
                    .chain()
                    .after(crate::layout_scene::LayoutUpdate)
                    .before(crate::camera::CameraControls),
            )
            .add_systems(Update, (move_tokens, presentation_chrome))
            .add_systems(
                PostUpdate,
                place_callouts.after(bevy::camera::CameraUpdateSystems),
            );
    }
}

fn playback_keys(
    keys: Res<ButtonInput<KeyCode>>,
    lock: Option<Res<CameraInputLock>>,
    mut playback: ResMut<Playback>,
) {
    if lock.is_some_and(|lock| lock.0) || playback.active.is_none() {
        return;
    }
    let result = if keys.just_pressed(KeyCode::Period) {
        playback.control(Control::Next)
    } else if keys.just_pressed(KeyCode::Comma) {
        playback.control(Control::Previous)
    } else if keys.just_pressed(KeyCode::Slash) && keys.pressed(KeyCode::ShiftLeft) {
        // Shift+/ is "?": toggle autoplay.
        let playing = playback
            .active
            .as_ref()
            .is_some_and(|a| a.autoplay && !a.paused);
        playback.control(if playing {
            Control::Pause
        } else {
            Control::Play
        })
    } else {
        Ok(())
    };
    let _ = result;
}

#[allow(clippy::too_many_arguments)]
fn advance(
    mut commands: Commands,
    mut playback: ResMut<Playback>,
    mut actions: ResMut<ViewerActions>,
    status: Res<LayoutStatus>,
    time: Res<Time>,
    mut director: ResMut<CameraDirector>,
    mut camera: Query<&mut OrbitCamera>,
    objects: Query<(&PlacedObject, &Transform)>,
    relations: Query<&PlacedRelation>,
    owned: Query<Entity, With<StepOwned>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut shown: Local<u64>,
) {
    let generation = playback.generation;
    if *shown != generation {
        // The step changed or playback stopped: clear the previous step's overlays.
        for entity in &owned {
            commands.entity(entity).despawn();
        }
        *shown = generation;
    }
    let now = time.elapsed_secs();
    let Some(active) = playback.active.as_mut() else {
        return;
    };
    match active.phase {
        Phase::Stage => {
            let scene = scene(active);
            match actions.submit(ViewerAction::Stage(Box::new(scene))) {
                Ok(ticket) => active.phase = Phase::Staging(ticket),
                Err(error) => active.error = Some(error.into()),
            }
        }
        Phase::Staging(ticket) => {
            if status.action_finished(ticket) && !status.loading {
                active.error = status.action_error.clone();
                active.phase = Phase::Present;
            }
        }
        Phase::Present => {
            let Ok(mut orbit) = camera.single_mut() else {
                return;
            };
            director.interrupted = false;
            active.interrupted = false;
            let step = &active.walkthrough.script().steps[active.step];
            let resolved = &active.walkthrough.resolved()[active.step];
            let positions = object_positions(&objects);
            let legs = trace_legs(resolved, &positions, &relations);
            frame(
                step.camera,
                step,
                &legs,
                &positions,
                &mut orbit,
                &mut director,
            );
            if !legs.is_empty() {
                spawn_token(
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    legs,
                    step.repeat_trace,
                );
            }
            let total = active.walkthrough.script().steps.len();
            let title = &active.walkthrough.script().title;
            spawn_caption(
                &mut commands,
                title,
                step,
                active.step,
                total,
                active.chrome,
                !resolved.hops.is_empty(),
            );
            for label in &step.labels {
                if let Some(anchor) = anchor(&label.target, &positions, &relations) {
                    spawn_callout(&mut commands, &label.text, anchor);
                }
            }
            let trace_time: f32 = resolved.hops.len() as f32 * (HOP_SECONDS.1 + HOP_PAUSE);
            let hold = BASE_HOLD
                + HOLD_PER_LABEL * step.labels.len() as f32
                + trace_time * if step.repeat_trace { 2.0 } else { 1.0 };
            active.phase = Phase::Showing { since: now, hold };
        }
        Phase::Showing { since, hold } => {
            if director.interrupted && !active.interrupted {
                active.interrupted = true;
            }
            let last = active.walkthrough.script().steps.len() - 1;
            if active.autoplay
                && !active.paused
                && !active.interrupted
                && !director.moving()
                && now - since >= hold
                && active.step < last
            {
                let next = active.step + 1;
                let _ = playback.goto(next);
            }
        }
    }
}

/// Each displayed object's box, keyed by every semantic ID it stands for.
fn object_positions(
    objects: &Query<(&PlacedObject, &Transform)>,
) -> BTreeMap<String, (Vec3, Vec3)> {
    let mut out = BTreeMap::new();
    for (object, transform) in objects {
        let entry = (transform.translation, object.half);
        out.insert(object.id.clone(), entry);
        for id in &object.semantic_ids {
            out.entry(id.clone()).or_insert(entry);
        }
    }
    out
}

/// The drawn route for each hop, oriented in travel order. A hop drawn inside one
/// box (both ends share a display object) has nothing to travel and is skipped.
fn trace_legs(
    resolved: &visualyze_model::walkthrough::ResolvedStep,
    positions: &BTreeMap<String, (Vec3, Vec3)>,
    relations: &Query<&PlacedRelation>,
) -> Vec<(Vec<Vec3>, Color)> {
    let mut legs = Vec::new();
    for hop in &resolved.hops {
        let route = relations
            .iter()
            .find(|r| r.id == hop.relation || r.semantic_ids.contains(&hop.relation));
        let from = positions.get(&hop.from).map(|p| p.0);
        let to = positions.get(&hop.to).map(|p| p.0);
        let mut points = match (route, from, to) {
            (Some(route), _, _) if route.points.len() >= 2 => route.points.clone(),
            (None, Some(from), Some(to)) if from.distance(to) > 0.5 => vec![from, to],
            _ => continue,
        };
        // Orient the route so it leaves from the object the hop starts at.
        if let Some(from) = from
            && points[0].distance(from) > points[points.len() - 1].distance(from)
        {
            points.reverse();
        }
        legs.push((
            points,
            if hop.returning {
                RETURN_COLOR
            } else {
                REQUEST_COLOR
            },
        ));
    }
    legs
}

fn frame(
    intent: CameraIntent,
    step: &visualyze_model::walkthrough::Step,
    legs: &[(Vec<Vec3>, Color)],
    positions: &BTreeMap<String, (Vec3, Vec3)>,
    orbit: &mut OrbitCamera,
    director: &mut CameraDirector,
) {
    let mut bounds: Option<(Vec3, Vec3)> = None;
    let mut grow = |lo: Vec3, hi: Vec3| {
        bounds = Some(match bounds {
            None => (lo, hi),
            Some((a, b)) => (a.min(lo), b.max(hi)),
        });
    };
    let boxed = |id: &str| positions.get(id).map(|(c, h)| (*c - *h, *c + *h));
    let (pitch, yaw) = match intent {
        CameraIntent::Hold => return,
        CameraIntent::Overview => {
            for (centre, half) in positions.values() {
                grow(*centre - *half, *centre + *half);
            }
            (0.42, 0.3)
        }
        CameraIntent::CloseUp => {
            if let Some((lo, hi)) = step.focus.first().and_then(|id| boxed(id)) {
                grow(lo, hi);
            }
            (0.3, orbit.yaw())
        }
        CameraIntent::FollowTrace => {
            for (points, _) in legs {
                for p in points {
                    grow(*p - Vec3::splat(0.5), *p + Vec3::splat(0.5));
                }
            }
            for id in &step.focus {
                if let Some((lo, hi)) = boxed(id) {
                    grow(lo, hi);
                }
            }
            (0.62, orbit.yaw())
        }
        CameraIntent::FrameFocus => {
            for id in &step.focus {
                if let Some((lo, hi)) = boxed(id) {
                    grow(lo, hi);
                }
            }
            (0.42, orbit.yaw())
        }
    };
    let Some((lo, hi)) = bounds else {
        return;
    };
    let shot = orbit.frame(lo, hi, yaw, pitch);
    director.move_to(orbit, shot);
}

fn anchor(
    target: &str,
    positions: &BTreeMap<String, (Vec3, Vec3)>,
    relations: &Query<&PlacedRelation>,
) -> Option<Vec3> {
    if let Some((centre, half)) = positions.get(target) {
        return Some(*centre + Vec3::Y * half.y);
    }
    let route = relations
        .iter()
        .find(|r| r.id == target || r.semantic_ids.iter().any(|id| id == target))?;
    point_along(&route.points, 0.5)
}

fn length(points: &[Vec3]) -> f32 {
    points.windows(2).map(|w| w[0].distance(w[1])).sum()
}

/// The point a fraction of the way along a polyline.
fn point_along(points: &[Vec3], fraction: f32) -> Option<Vec3> {
    let total = length(points);
    if points.is_empty() {
        return None;
    }
    let mut remaining = total * fraction.clamp(0.0, 1.0);
    for w in points.windows(2) {
        let segment = w[0].distance(w[1]);
        if remaining <= segment && segment > 0.0 {
            return Some(w[0].lerp(w[1], remaining / segment));
        }
        remaining -= segment;
    }
    points.last().copied()
}

fn hop_seconds(points: &[Vec3]) -> f32 {
    (length(points) / TOKEN_SPEED).clamp(HOP_SECONDS.0, HOP_SECONDS.1)
}

/// Where the token is `clock` seconds into the trace, with its colour; None
/// between loops or after a one-shot trace finishes.
fn token_at(legs: &[(Vec<Vec3>, Color)], repeat: bool, clock: f32) -> Option<(Vec3, Color)> {
    let cycle: f32 = legs
        .iter()
        .map(|(p, _)| hop_seconds(p) + HOP_PAUSE)
        .sum::<f32>();
    let mut t = if repeat {
        clock.rem_euclid(cycle + LOOP_PAUSE)
    } else {
        clock
    };
    if t < 0.0 {
        return None;
    }
    for (points, color) in legs {
        let seconds = hop_seconds(points);
        if t <= seconds {
            let e = t / seconds;
            // Ease in and out of each hop so departures and arrivals read.
            let e = e * e * (3.0 - 2.0 * e);
            return point_along(points, e).map(|p| (p, *color));
        }
        t -= seconds;
        if t <= HOP_PAUSE {
            return points.last().map(|p| (*p, *color));
        }
        t -= HOP_PAUSE;
    }
    // A finished one-shot trace rests where it arrived.
    (!repeat)
        .then(|| legs.last().and_then(|(p, c)| p.last().map(|p| (*p, *c))))
        .flatten()
}

fn spawn_token(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    legs: Vec<(Vec<Vec3>, Color)>,
    repeat: bool,
) {
    let mesh = meshes.add(Sphere::new(0.2).mesh().ico(3).unwrap());
    for i in 0..=TRAIL {
        let fade = 1.0 - i as f32 / (TRAIL + 1) as f32;
        let material = materials.add(StandardMaterial {
            base_color: Color::WHITE.with_alpha(fade),
            // Above white, so the token blooms.
            emissive: LinearRgba::rgb(2.4, 2.4, 2.4) * fade,
            unlit: true,
            alpha_mode: if i == 0 {
                AlphaMode::Opaque
            } else {
                AlphaMode::Blend
            },
            ..default()
        });
        commands.spawn((
            StepOwned,
            Token {
                legs: legs.clone(),
                repeat,
                clock: 0.0,
                lag: i as f32 * TRAIL_SPACING,
            },
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_scale(Vec3::splat(1.0 - 0.12 * i as f32)),
            Visibility::Hidden,
        ));
    }
}

fn move_tokens(
    time: Res<Time>,
    mut tokens: Query<(
        &mut Token,
        &mut Transform,
        &mut Visibility,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    director: Res<CameraDirector>,
) {
    for (mut token, mut transform, mut visibility, material) in &mut tokens {
        // Let the camera settle before travel starts, so the eye can follow it.
        if director.moving() && token.clock == 0.0 {
            continue;
        }
        token.clock += time.delta_secs();
        match token_at(&token.legs, token.repeat, token.clock - token.lag) {
            Some((point, color)) => {
                transform.translation = point;
                *visibility = Visibility::Visible;
                if let Some(material) = materials.get_mut(&material.0) {
                    let alpha = material.base_color.alpha();
                    material.base_color = color.with_alpha(alpha);
                    let glow = color.to_linear() * 2.6 * alpha;
                    material.emissive = glow;
                }
            }
            None => *visibility = Visibility::Hidden,
        }
    }
}

fn spawn_caption(
    commands: &mut Commands,
    walkthrough_title: &str,
    step: &visualyze_model::walkthrough::Step,
    index: usize,
    total: usize,
    chrome: bool,
    traced: bool,
) {
    let accent = Color::srgb(0.45, 0.85, 1.0);
    commands
        .spawn((
            StepOwned,
            GlobalZIndex(15),
            BackgroundColor(Color::srgba(0.05, 0.07, 0.1, 0.82)),
            BorderColor::all(accent),
            Node {
                position_type: PositionType::Absolute,
                left: px(24),
                bottom: px(40),
                max_width: percent(60),
                padding: UiRect::new(px(18), px(22), px(12), px(14)),
                border: UiRect::left(px(4)),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            },
        ))
        .with_children(|panel| {
            if chrome {
                panel.spawn((
                    Text::new(format!(
                        "{}   {} / {}",
                        walkthrough_title.to_uppercase(),
                        index + 1,
                        total
                    )),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(Color::srgb_u8(140, 170, 196)),
                ));
            }
            panel.spawn((
                Text::new(step.title.clone()),
                TextFont {
                    font_size: 26.0,
                    ..default()
                },
                TextColor(Color::srgb_u8(240, 245, 250)),
            ));
            if chrome {
                panel
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: px(6),
                        margin: UiRect::top(px(6)),
                        ..default()
                    })
                    .with_children(|row| {
                        for i in 0..total {
                            row.spawn((
                                Node {
                                    width: px(if i == index { 22 } else { 8 }),
                                    height: px(4),
                                    ..default()
                                },
                                BackgroundColor(if i <= index {
                                    accent
                                } else {
                                    Color::srgb_u8(70, 86, 102)
                                }),
                            ));
                        }
                        row.spawn((
                            Text::new(",  back     .  next     ?  autoplay"),
                            TextFont {
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(Color::srgb_u8(120, 145, 168)),
                            Node {
                                margin: UiRect::left(px(14)),
                                ..default()
                            },
                        ));
                    });
            }
        });
    if traced {
        commands.spawn((
            StepOwned,
            GlobalZIndex(15),
            Text::new(HONESTY),
            TextFont {
                font_size: 12.0,
                ..default()
            },
            TextColor(Color::srgb_u8(210, 196, 160)),
            BackgroundColor(Color::srgba(0.12, 0.1, 0.05, 0.75)),
            Node {
                position_type: PositionType::Absolute,
                right: px(24),
                top: px(24),
                padding: UiRect::axes(px(10), px(5)),
                ..default()
            },
        ));
    }
}

fn spawn_callout(commands: &mut Commands, text: &str, anchor: Vec3) {
    commands.spawn((
        StepOwned,
        Callout { anchor },
        GlobalZIndex(14),
        Text::new(text.to_owned()),
        TextFont {
            font_size: 15.0,
            ..default()
        },
        TextColor(Color::srgb_u8(20, 26, 34)),
        BackgroundColor(Color::srgb_u8(236, 242, 248)),
        BorderColor::all(Color::srgb(0.45, 0.85, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            padding: UiRect::axes(px(10), px(5)),
            border: UiRect::bottom(px(3)),
            ..default()
        },
        Visibility::Hidden,
    ));
}

fn presentation_chrome(
    playback: Res<Playback>,
    mut chrome: Query<&mut Visibility, With<DevChrome>>,
) {
    if !playback.is_changed() {
        return;
    }
    let shown = if playback.active.is_some() {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut visibility in &mut chrome {
        visibility.set_if_neq(shown);
    }
}

fn place_callouts(
    camera: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    mut callouts: Query<(&Callout, &mut Node, &mut Visibility, &ComputedNode)>,
) {
    let Ok((camera, eye)) = camera.single() else {
        return;
    };
    for (callout, mut node, mut visibility, computed) in &mut callouts {
        match camera.world_to_viewport(eye, callout.anchor) {
            Ok(point) => {
                let size = computed.size() * computed.inverse_scale_factor();
                node.left = px(point.x - size.x * 0.5);
                // Clear the object's own name label, which sits just above it.
                node.top = px(point.y - size.y - 66.0);
                *visibility = Visibility::Visible;
            }
            Err(_) => *visibility = Visibility::Hidden,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = include_str!("../../../public/examples/notes-crud.json");
    const WALKTHROUGH: &str = include_str!("../../../public/examples/notes-crud.walkthrough.json");

    fn active(step: usize) -> Active {
        let model = Arc::new(Architecture::from_json(MODEL.as_bytes()).unwrap());
        let walkthrough = Walkthrough::from_json(WALKTHROUGH.as_bytes(), &model).unwrap();
        Active {
            walkthrough,
            model,
            step,
            phase: Phase::Stage,
            autoplay: false,
            paused: false,
            interrupted: false,
            chrome: true,
            error: None,
        }
    }

    #[test]
    fn a_step_lights_its_focus_and_trace_and_stands_at_its_root() {
        let scene = scene(&active(2));
        // scope:notes is the system, so the scene stands at the system root.
        assert_eq!(scene.root, None);
        let spot = scene.spotlight.unwrap();
        for id in [
            "service:notes",
            "store:notes",
            "client:web",
            "interface:sessions",
        ] {
            assert!(spot.lit.contains(id), "{id} not lit");
        }
        assert!(spot.relations.contains("rel:read-database"));
        assert_eq!(spot.context, SpotContext::Hide);
        // Cumulative: the previous step's focus stays lit.
        assert!(spot.lit.contains("scope:notes"));
    }

    #[test]
    fn a_step_only_lights_its_own_focus_unless_cumulative() {
        let spot = scene(&active(3)).spotlight.unwrap();
        assert!(!spot.lit.contains("store:notes"));
        assert!(spot.lit.contains("concern:authentication"));
    }

    #[test]
    fn the_token_eases_along_each_leg_pauses_and_loops() {
        let legs = vec![
            (vec![Vec3::ZERO, Vec3::new(9.0, 0.0, 0.0)], REQUEST_COLOR),
            (vec![Vec3::new(9.0, 0.0, 0.0), Vec3::ZERO], RETURN_COLOR),
        ];
        let (start, color) = token_at(&legs, false, 0.0).unwrap();
        assert_eq!(start, Vec3::ZERO);
        assert_eq!(color, REQUEST_COLOR);
        let (middle, _) = token_at(&legs, false, 0.5).unwrap();
        assert!((middle.x - 4.5).abs() < 0.01);
        let (back, color) = token_at(&legs, false, 1.0 + HOP_PAUSE + 0.5).unwrap();
        assert!((back.x - 4.5).abs() < 0.01);
        assert_eq!(color, RETURN_COLOR);
        // A one-shot trace rests at its end; a loop starts over.
        assert_eq!(token_at(&legs, false, 100.0).unwrap().0, Vec3::ZERO);
        let cycle = 2.0 * (1.0 + HOP_PAUSE) + LOOP_PAUSE;
        let (again, _) = token_at(&legs, true, cycle + 0.5).unwrap();
        assert!((again.x - 4.5).abs() < 0.01);
    }

    #[test]
    fn controls_move_between_steps_and_report_state() {
        let mut playback = Playback::default();
        assert_eq!(playback.state()["state"], "idle");
        let a = active(0);
        playback.start(a.walkthrough, a.model, 0, false, true);
        assert_eq!(playback.state()["state"], "staging");
        playback.control(Control::Next).unwrap();
        assert_eq!(playback.state()["step"], 1);
        playback.control(Control::Goto(3)).unwrap();
        assert!(playback.control(Control::Next).is_err());
        assert!(playback.control(Control::Goto(9)).is_err());
        playback.control(Control::Previous).unwrap();
        assert_eq!(playback.state()["step_id"], "lookup");
        playback.control(Control::Stop).unwrap();
        assert!(playback.control(Control::Next).is_err());
    }
}
