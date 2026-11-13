// Fixed-function emulation for W3D meshes.
//
// Lighting follows Direct3D 8's per-vertex model as WW3D2 configures it
// (vertmaterial.cpp, meshmatdesc.cpp): emissive + ambient * scene ambient +
// diffuse * sum(light colour * max(N . L, 0)), clamped, alpha from diffuse.
// Texture stages follow ShaderClass::Apply (shader.cpp): stage 0 combines the
// texture with the lit colour per the primary gradient; stage 1 applies the
// detail colour/alpha function. Colours stay in gamma space, as DX8 blended them.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    eye: vec4<f32>,
    ambient: vec4<f32>,
    light_dir: array<vec4<f32>, 3>,
    light_color: array<vec4<f32>, 3>,
};

struct Draw {
    // x: primary gradient, y: texturing, z: detail colour func, w: detail alpha func
    modes: vec4<u32>,
    // x: alpha test (0 off, 1 >= ref, 2 <= ref), y: lighting, z: has detail texture
    flags: vec4<u32>,
    // x: alpha reference
    params: vec4<f32>,
    // x, y: generator per stage (0 uv, 1 camera reflection, 2 camera normal,
    // 3 world reflection, 4 world normal); z, w: axis per stage (0 X, 1 Y, 2 Z)
    uvgen: vec4<u32>,
    // UV transform rows per stage: u' = a*u + b*v + c
    tm0u: vec4<f32>,
    tm0v: vec4<f32>,
    tm1u: vec4<f32>,
    tm1v: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var<storage, read> bones: array<mat4x4<f32>>;
@group(2) @binding(0) var<uniform> d: Draw;
@group(2) @binding(1) var tex0: texture_2d<f32>;
@group(2) @binding(2) var samp0: sampler;
@group(2) @binding(3) var tex1: texture_2d<f32>;
@group(2) @binding(4) var samp1: sampler;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv0: vec2<f32>,
    @location(3) uv1: vec2<f32>,
    @location(4) diffuse: vec4<f32>,
    @location(5) ambient: vec3<f32>,
    @location(6) emissive: vec3<f32>,
    @location(7) bone: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv0: vec2<f32>,
    @location(2) uv1: vec2<f32>,
};

// Texture coordinates for one stage, as Direct3D generated them for WW3D's mappers
// (mapper.cpp): passthrough UVs through the texture matrix, or a reflection/normal
// vector mapped with u = 0.5x + 0.5, v = 0.5y + 0.5.
fn gen_uv(mode: u32, axis: u32, uv: vec2<f32>, tu: vec4<f32>, tv: vec4<f32>, n_world: vec3<f32>, p_world: vec3<f32>) -> vec2<f32> {
    if (mode == 0u) {
        return vec2<f32>(dot(tu.xyz, vec3<f32>(uv, 1.0)), dot(tv.xyz, vec3<f32>(uv, 1.0)));
    }
    let e_world = normalize(g.eye.xyz - p_world);
    var v: vec3<f32>;
    if (mode == 1u || mode == 2u) {
        // Camera space; reflection is computed there (x and y match Direct3D's).
        let n = normalize((g.view * vec4<f32>(n_world, 0.0)).xyz);
        let e = normalize((g.view * vec4<f32>(e_world, 0.0)).xyz);
        v = select(n, 2.0 * dot(n, e) * n - e, mode == 1u);
    } else {
        v = select(n_world, 2.0 * dot(n_world, e_world) * n_world - e_world, mode == 3u);
    }
    var xy = v.xy;
    if (mode >= 3u) {
        if (axis == 0u) { xy = v.yz; } else if (axis == 1u) { xy = v.xz; }
    }
    return xy * 0.5 + vec2<f32>(0.5);
}

@vertex
fn vs_main(v: VsIn) -> VsOut {
    let m = bones[v.bone];
    let world = m * vec4<f32>(v.position, 1.0);
    let n = normalize((m * vec4<f32>(v.normal, 0.0)).xyz);
    var color = v.diffuse;
    if (d.flags.y != 0u) {
        var lit = v.emissive + v.ambient * g.ambient.xyz;
        for (var i = 0u; i < 3u; i = i + 1u) {
            let ndl = max(dot(n, -g.light_dir[i].xyz), 0.0);
            lit = lit + v.diffuse.xyz * g.light_color[i].xyz * ndl;
        }
        color = vec4<f32>(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), v.diffuse.w);
    }
    var o: VsOut;
    o.clip = g.view_proj * world;
    o.color = color;
    o.uv0 = gen_uv(d.uvgen.x, d.uvgen.z, v.uv0, d.tm0u, d.tm0v, n, world.xyz);
    o.uv1 = gen_uv(d.uvgen.y, d.uvgen.w, v.uv1, d.tm1u, d.tm1v, n, world.xyz);
    return o;
}

@fragment
fn fs_main(i: VsOut) -> @location(0) vec4<f32> {
    let diffuse = i.color;
    var c = diffuse;
    let texturing = d.modes.y != 0u;
    if (texturing) {
        let t = textureSample(tex0, samp0, i.uv0);
        switch d.modes.x {
            case 0u: { c = t; }                                                   // DISABLE: texture only
            case 2u: { c = vec4<f32>(min(t.xyz + diffuse.xyz, vec3<f32>(1.0)), t.w * diffuse.w); } // ADD
            case 5u: { c = vec4<f32>(min(t.xyz * diffuse.xyz * 2.0, vec3<f32>(1.0)), t.w * diffuse.w); } // MODULATE2X
            case 3u, 4u: { c = diffuse; }                                        // BUMPENVMAP fallback
            default: { c = t * diffuse; }                                         // MODULATE
        }
        if (d.flags.z != 0u) {
            let t1 = textureSample(tex1, samp1, i.uv1);
            var rgb = c.xyz;
            switch d.modes.z {
                case 1u: { rgb = t1.xyz; }                                        // DETAIL
                case 2u: { rgb = t1.xyz * c.xyz; }                                // SCALE
                case 3u: { rgb = t1.xyz + c.xyz - t1.xyz * c.xyz; }               // INVSCALE (add smooth)
                case 4u: { rgb = t1.xyz + c.xyz; }                                // ADD
                case 5u: { rgb = t1.xyz - c.xyz; }                                // SUB (texture - current)
                case 6u: { rgb = c.xyz - t1.xyz; }                                // SUBR
                case 7u: { rgb = mix(c.xyz, t1.xyz, t1.w); }                      // BLEND (texture alpha)
                case 8u: { rgb = mix(c.xyz, t1.xyz, c.w); }                       // DETAILBLEND (current alpha)
                case 9u: { rgb = t1.xyz + c.xyz - vec3<f32>(0.5); }               // ADDSIGNED
                case 10u: { rgb = (t1.xyz + c.xyz - vec3<f32>(0.5)) * 2.0; }      // ADDSIGNED2X
                case 11u: { rgb = t1.xyz * c.xyz * 2.0; }                         // SCALE2X
                case 12u: { rgb = c.xyz + c.w * t1.xyz; }                         // MODALPHAADDCOLOR
                default: {}
            }
            var a = c.w;
            switch d.modes.w {
                case 1u: { a = t1.w; }
                case 2u: { a = t1.w * c.w; }
                case 3u: { a = t1.w + c.w - t1.w * c.w; }
                default: {}
            }
            c = vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), a);
        }
    }
    if (d.flags.x == 1u && c.w < d.params.x) { discard; }
    if (d.flags.x == 2u && c.w > d.params.x) { discard; }
    return c;
}
