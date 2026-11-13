// Standing water (W3DWater.cpp: drawTrapezoidWater, setupFlatWaterShader and the
// trapezoid pixel shader; BaseHeightMap.cpp: shoreline destination alpha).
//
//   uv0   = world / 150 plus a slow wobble (m_riverVOrigin grows 0.002 per update)
//   noise = world / 16 + m_riverVOrigin
//   rgb   = diffuse * water(uv0) + sparkles(uv0) * noise(noise uv)
//   alpha = min(depth / (TransparentWaterDepth * MinOpacity), MinOpacity)
//
// The original blends with destination alpha written by the shoreline pass; the
// same opacity is computed here from the height map. Output is premultiplied.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    eye: vec4<f32>,
    ambient: vec4<f32>,
    light_dir: array<vec4<f32>, 3>,
    light_color: array<vec4<f32>, 3>,
};

struct WaterU {
    color: vec4<f32>,
    // x: river v origin, y: depth scale (TransparentWaterDepth * MinOpacity),
    // z: min opacity, w: soft edge
    params: vec4<f32>,
    // x: border, y: width, z: height (samples)
    map: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<uniform> w: WaterU;
@group(1) @binding(1) var water_tex: texture_2d<f32>;
@group(1) @binding(2) var sparkle_tex: texture_2d<f32>;
@group(1) @binding(3) var noise_tex: texture_2d<f32>;
@group(1) @binding(4) var heights: texture_2d<f32>;
@group(1) @binding(5) var wrap_samp: sampler;
@group(1) @binding(6) var clamp_samp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv0: vec2<f32>,
    @location(1) noise_uv: vec2<f32>,
    @location(2) world: vec3<f32>,
};

const PI: f32 = 3.14159265;

@vertex
fn vs_main(@location(0) position: vec3<f32>) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(position, 1.0);
    let r = w.params.x;
    let d = PI / (4.0 * 10.0);
    o.uv0 = vec2<f32>(
        position.x / 150.0 + 0.02 * cos(11.0 * r) * sin(25.0 * r + position.x * d),
        position.y / 150.0 + 0.02 * cos(5.0 * r) * sin(25.0 * r + position.y * d),
    );
    o.noise_uv = position.xy / 16.0 + vec2<f32>(r, r);
    o.world = position;
    return o;
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let base = textureSample(water_tex, wrap_samp, i.uv0);
    let sparkle = textureSample(sparkle_tex, wrap_samp, i.uv0);
    let noise = textureSample(noise_tex, wrap_samp, i.noise_uv);
    let rgb = w.color.rgb * base.rgb + sparkle.rgb * noise.rgb;
    var a = w.color.a * base.a;
    if w.params.w > 0.5 {
        let cell = i.world.xy / 10.0 + vec2<f32>(w.map.x + 0.5);
        let h = textureSampleLevel(heights, clamp_samp, cell / w.map.yz, 0.0).r * 255.0 * 0.625;
        let depth = i.world.z - h;
        a = clamp(min(depth / w.params.y, w.params.z), 0.0, 1.0);
    }
    return vec4<f32>(min(rgb, vec3<f32>(1.0)) * a, a);
}
