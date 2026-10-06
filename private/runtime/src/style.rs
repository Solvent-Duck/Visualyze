//! Shared presentation: camera finish, lighting, backdrop and glass. Purely
//! visual; nothing here affects layout, picking or routing.
use bevy::{
    asset::{load_internal_asset, uuid_handle},
    core_pipeline::tonemapping::{DebandDither, Tonemapping},
    mesh::MeshVertexBufferLayoutRef,
    pbr::{MaterialPipeline, MaterialPipelineKey},
    post_process::bloom::{Bloom, BloomCompositeMode, BloomPrefilter},
    prelude::*,
    render::{
        render_resource::{
            AsBindGroup, RenderPipelineDescriptor, ShaderType, SpecializedMeshPipelineError,
        },
        view::Hdr,
    },
    shader::ShaderRef,
};

const GLASS_SHADER: Handle<Shader> = uuid_handle!("6f2d0c1e-5b7a-4c38-9e51-2a8f3d6b9c47");

pub(crate) struct StylePlugin;

impl Plugin for StylePlugin {
    fn build(&self, app: &mut App) {
        load_internal_asset!(app, GLASS_SHADER, "glass.wgsl", Shader::from_wgsl);
        app.add_plugins(MaterialPlugin::<GlassMaterial>::default());
    }
}

/// The page colour behind the scene; the backdrop darkens it towards the edges.
pub(crate) const CLEAR: Color = Color::srgb(0.082, 0.098, 0.13);

/// HDR so highlights can exceed white and bloom. There is no display
/// transform: kind and relationship colours are a legend and must render
/// exactly, so only deliberately over-bright highlights cross the threshold.
pub(crate) fn camera_finish() -> impl Bundle {
    (
        Hdr,
        Tonemapping::None,
        DebandDither::Enabled,
        Bloom {
            intensity: 0.3,
            prefilter: BloomPrefilter {
                threshold: 1.3,
                threshold_softness: 0.2,
            },
            composite_mode: BloomCompositeMode::Additive,
            ..Bloom::NATURAL
        },
    )
}

/// The selected path: the legacy highlight colour pushed past white so it
/// blooms. It clips to a pale gold on screen.
pub(crate) const PATH_HIGHLIGHT: Color = Color::LinearRgba(LinearRgba {
    red: 2.0,
    green: 1.46,
    blue: 0.41,
    alpha: 1.0,
});

/// A cool ambient, a warm key from above-left-front and a cool fill from the
/// opposite side, so every face of a box reads as a distinct tone.
pub(crate) fn spawn_lights(commands: &mut Commands) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.72, 0.8, 1.0),
        brightness: 200.0,
        ..default()
    });
    commands.spawn((
        DirectionalLight {
            illuminance: 4600.0,
            color: Color::srgb(1.0, 0.97, 0.92),
            ..default()
        },
        Transform::from_xyz(-3.0, 7.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight {
            illuminance: 1500.0,
            color: Color::srgb(0.75, 0.84, 1.0),
            ..default()
        },
        Transform::from_xyz(6.0, -1.5, -4.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
}

/// A vignette over the clear colour. It sits beneath every other UI node.
pub(crate) fn spawn_backdrop(commands: &mut Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        BackgroundGradient::from(RadialGradient::new(
            UiPosition::CENTER,
            RadialGradientShape::FarthestCorner,
            vec![
                ColorStop::new(Color::NONE, percent(30)),
                ColorStop::new(Color::srgba(0.01, 0.012, 0.02, 0.55), percent(100)),
            ],
        )),
        GlobalZIndex(-1),
    ));
}

#[derive(ShaderType, Debug, Clone, Copy)]
pub(crate) struct GlassUniform {
    color: LinearRgba,
    shape: Vec4,
    grid: Vec4,
}

/// Additive, unlit and double sided. See `glass.wgsl`.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct GlassMaterial {
    #[uniform(0)]
    pub settings: GlassUniform,
}

impl GlassMaterial {
    /// A container wall: nearly clear face-on, bright where seen edge-on.
    /// `depth` dims nested containers so deep stacks stay legible.
    pub fn shell(color: Color, depth: usize, emphasis: f32) -> Self {
        let fade = 0.8_f32.powi(depth as i32) * emphasis;
        Self::glow(color, Vec4::new(0.02 * fade, 0.6 * fade, 2.6, 0.5))
    }

    /// A container floor: an even tint that seats its children.
    pub fn tray(color: Color, depth: usize, emphasis: f32) -> Self {
        let fade = 0.8_f32.powi(depth as i32) * emphasis;
        Self::glow(color, Vec4::new(0.09 * fade, 0.12 * fade, 2.0, 0.6))
    }

    /// A faint, anti-aliased ground grid fading out at `radius` from `centre`.
    pub fn grid(spacing: f32, radius: f32, centre: Vec2) -> Self {
        Self {
            settings: GlassUniform {
                color: Color::srgb(0.45, 0.58, 0.72).to_linear(),
                shape: Vec4::new(0.0, 0.13, 1.0, 1.0),
                grid: Vec4::new(spacing, radius, centre.x, centre.y),
            },
        }
    }

    fn glow(color: Color, shape: Vec4) -> Self {
        Self {
            settings: GlassUniform {
                color: color.to_linear(),
                shape,
                grid: Vec4::ZERO,
            },
        }
    }
}

impl Material for GlassMaterial {
    fn fragment_shader() -> ShaderRef {
        ShaderRef::Handle(GLASS_SHADER)
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // The far walls complete the volume's outline.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A round grid step (1, 2 or 5 times a power of ten) giving about a dozen
/// cells across `extent`.
pub(crate) fn grid_spacing(extent: f32) -> f32 {
    let raw = (extent / 12.0).max(1e-6);
    let power = 10_f32.powf(raw.log10().floor());
    [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|step| step * power)
        .find(|step| *step >= raw)
        .unwrap_or(10.0 * power)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_spacing_is_a_round_step_near_a_twelfth_of_the_extent() {
        assert_eq!(grid_spacing(12.0), 1.0);
        assert_eq!(grid_spacing(20.0), 2.0);
        assert_eq!(grid_spacing(50.0), 5.0);
        assert!((grid_spacing(0.3) - 0.05).abs() < 1e-6);
    }
}
