// Additive glass for containers, trays and the floor grid. Additive blending
// is order independent, so nested translucent volumes never sort wrongly.
#import bevy_pbr::{forward_io::VertexOutput, mesh_view_bindings::view}

struct Glass {
    color: vec4<f32>,
    // x: face glow, y: rim glow, z: rim falloff, w: back-face weight
    shape: vec4<f32>,
    // x: grid spacing (0 = no grid), y: fade radius, zw: fade centre (x, z)
    grid: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> glass: Glass;

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var strength: f32;
    if glass.grid.x > 0.0 {
        let cell = in.world_position.xz / glass.grid.x;
        let width = max(fwidth(cell), vec2(1e-5));
        let distance = abs(fract(cell - 0.5) - 0.5) / width;
        let line = 1.0 - min(min(distance.x, distance.y), 1.0);
        let reach = length(in.world_position.xz - glass.grid.zw) / glass.grid.y;
        let fade = 1.0 - smoothstep(0.35, 1.0, reach);
        strength = (glass.shape.x + glass.shape.y * line) * fade;
    } else {
        let n = normalize(in.world_normal);
        let v = normalize(view.world_position - in.world_position.xyz);
        let rim = pow(1.0 - abs(dot(n, v)), glass.shape.z);
        strength = glass.shape.x + glass.shape.y * rim;
        if !front {
            strength *= glass.shape.w;
        }
    }
    // Premultiplied output with zero alpha adds light without occluding.
    return vec4(glass.color.rgb * strength, 0.0);
}
