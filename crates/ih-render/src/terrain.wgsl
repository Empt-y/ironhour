// Terrain, following TerrainShader2Stage (W3DShaderManager.cpp) and
// renderExtraBlendTiles (HeightMap.cpp).
//
// Pass 0 draws the base tile modulated by the lit vertex colour; pass 1 draws the
// blend tile the same way, alpha-blended with the vertex alpha; pass 2 multiplies
// the frame buffer by the cloud and noise (macro) textures, whose coordinates are
// world x, y scaled to cover 31.5 cells (STRETCH_FACTOR), the clouds sliding with
// time. Here the three passes are one: mix(base, blend, a) * diffuse * cloud * noise.
// The extra (three-way) blend layer is a second draw of tex * diffuse * cloud * noise
// with alpha blending.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    eye: vec4<f32>,
    ambient: vec4<f32>,
    light_dir: array<vec4<f32>, 3>,
    light_color: array<vec4<f32>, 3>,
};

struct TerrainU {
    // xy: cloud offset; z: use cloud; w: use noise
    cloud: vec4<f32>,
    // x: 0 terrain, 1 extra blend, 2 road, 3 alpha-tested bridge
    mode: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<uniform> t: TerrainU;
@group(1) @binding(1) var atlas: texture_2d<f32>;
@group(1) @binding(2) var atlas_samp: sampler;
@group(1) @binding(3) var cloud_tex: texture_2d<f32>;
@group(1) @binding(4) var noise_tex: texture_2d<f32>;
@group(1) @binding(5) var wrap_samp: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
    @location(3) world: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(v.position, 1.0);
    o.color = v.color;
    o.uv0 = v.uv0;
    o.uv1 = v.uv1;
    o.world = v.position.xy;
    return o;
}

const STRETCH: f32 = 1.0 / (63.0 * 10.0 / 2.0);

fn macro_tint(world: vec2<f32>) -> vec3<f32> {
    var m = vec3<f32>(1.0);
    let uv = world * STRETCH;
    if t.cloud.z > 0.5 {
        m = m * textureSample(cloud_tex, wrap_samp, uv + t.cloud.xy).rgb;
    }
    if t.cloud.w > 0.5 {
        m = m * textureSample(noise_tex, wrap_samp, uv).rgb;
    }
    return m;
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(atlas, atlas_samp, i.uv0);
    let tint = macro_tint(i.world);
    if t.mode.x > 2.5 && base.a < 96.0 / 255.0 { discard; }
    if t.mode.x > 1.5 {
        // Roads wrap the texture, retaining its alpha to soften the edges.
        return vec4<f32>(base.rgb * i.color.rgb * tint, base.a * i.color.a);
    }
    if t.mode.x > 0.5 {
        // Extra blend layer: alpha from the vertex (texture alpha is opaque).
        return vec4<f32>(base.rgb * i.color.rgb * tint, i.color.a * base.a);
    }
    let blend = textureSample(atlas, atlas_samp, i.uv1);
    let c = mix(base.rgb, blend.rgb, i.color.a * blend.a);
    return vec4<f32>(c * i.color.rgb * tint, 1.0);
}
