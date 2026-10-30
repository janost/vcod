// r_textureMode's samplers (texture_settings::TexSamplers), in group 0 so a
// filter change rebuilds only the group-0 bind groups. A stage's clamped
// bundles sample through the clamp ones: F_CLAMP_S and F_CLAMP_T pick the
// axes for bundle 0 (renderer::STAGE_FLAG_CLAMP_S, _T), and bundle 1's two
// bits sit two higher, so its caller passes the flags shifted down by 2. An
// image without mipmaps (one level: `nomipmaps`, the dlight blob) samples
// GL_LINEAR whatever the mode, as retail sets it on images without flag 1
// (CoDMP.exe 0x4ec07e).
@group(0) @binding(9) var s_tex: sampler;
@group(0) @binding(10) var s_tex_clamp: sampler;
@group(0) @binding(11) var s_lin: sampler;
@group(0) @binding(12) var s_lin_clamp: sampler;
@group(0) @binding(13) var s_tex_clamp_s: sampler;
@group(0) @binding(14) var s_tex_clamp_t: sampler;
@group(0) @binding(15) var s_lin_clamp_s: sampler;
@group(0) @binding(16) var s_lin_clamp_t: sampler;

const F_CLAMP_S: u32 = 16777216u;
const F_CLAMP_T: u32 = 33554432u;
const F_CLAMP: u32 = 50331648u;

fn sample_image(t: texture_2d<f32>, uv: vec2<f32>, flags: u32) -> vec4<f32> {
    return sample_mipped(t, uv, flags, textureNumLevels(t) > 1u);
}

// sample_image with the image's mipmaps known to the caller, which then
// asks no `textureNumLevels` per pixel.
fn sample_mipped(t: texture_2d<f32>, uv: vec2<f32>, flags: u32, mipped: bool) -> vec4<f32> {
    let clamp = flags & F_CLAMP;
    if (clamp == F_CLAMP) {
        if (mipped) {
            return textureSample(t, s_tex_clamp, uv);
        }
        return textureSample(t, s_lin_clamp, uv);
    }
    if (clamp == F_CLAMP_S) {
        if (mipped) {
            return textureSample(t, s_tex_clamp_s, uv);
        }
        return textureSample(t, s_lin_clamp_s, uv);
    }
    if (clamp == F_CLAMP_T) {
        if (mipped) {
            return textureSample(t, s_tex_clamp_t, uv);
        }
        return textureSample(t, s_lin_clamp_t, uv);
    }
    if (mipped) {
        return textureSample(t, s_tex, uv);
    }
    return textureSample(t, s_lin, uv);
}
