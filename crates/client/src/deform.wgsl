// deformVertexes, appended to every module that draws a shader stage
// (world, shadow cutouts, entities, viewmodel). The module's `stage`
// carries `deform` and `deform_kinds` (renderer::StageParams) and
// noise.wgsl rides along. Retail runs a shader's records in script order
// on the same vertices (CoDMP.exe 0x5149b0), so each reads the position
// and normal the one before it left.

// renderer::DEFORM_*, retail's deform numbers; a renderer test holds them
const DEFORM_WAVE: u32 = 1u;
const DEFORM_FLAP_S: u32 = 2u;
const DEFORM_FLAP_T: u32 = 3u;
const DEFORM_NORMAL: u32 = 4u;
const DEFORM_BULGE: u32 = 6u;
const DEFORM_MOVE: u32 = 7u;
// renderer::DEFORM_FORM_SHIFT: a wave's or flap's form rides the kind's
// upper bits (renderer::deform_form), 0 for sin
const DEFORM_FORM_SHIFT: u32 = 8u;
const FORM_SQUARE: u32 = 1u;
const FORM_TRIANGLE: u32 = 2u;
const FORM_SAWTOOTH: u32 = 3u;
const FORM_INVERSE_SAWTOOTH: u32 = 4u;
const FORM_NOISE: u32 = 5u;

// Entry `i & 1023` of retail's 1024-entry function tables (tr_init's build,
// material::build_*_table): the sin table spans 360 degrees over 1023
// entries, so entry 256 is sin(90.088 deg).
fn table_entry(form: u32, idx: i32) -> f32 {
    let i = idx & 1023;
    let f = f32(i);
    if (form == FORM_SQUARE) {
        return select(-1.0, 1.0, i < 512);
    } else if (form == FORM_TRIANGLE) {
        let q = i & 511;
        let up = select(1.0 - f32(q - 256) / 256.0, f32(q) / 256.0, q < 256);
        return select(-up, up, i < 512);
    } else if (form == FORM_SAWTOOTH) {
        return f / 1024.0;
    } else if (form == FORM_INVERSE_SAWTOOTH) {
        return 1.0 - f / 1024.0;
    }
    return sin(f * (6.2831853 / 1023.0));
}

struct Deformed {
    pos: vec3<f32>,
    normal: vec3<f32>,
};

fn deform_vertex(pos: vec3<f32>, normal: vec3<f32>, uv: vec2<f32>) -> Deformed {
    var p = pos;
    var n = normal;
    for (var i = 0u; i < 3u; i++) {
        let k = stage.deform_kinds[i] & ((1u << DEFORM_FORM_SHIFT) - 1u);
        let form = stage.deform_kinds[i] >> DEFORM_FORM_SHIFT;
        let d = stage.deform[i];
        if (k == DEFORM_WAVE || k == DEFORM_FLAP_S || k == DEFORM_FLAP_T) {
            // wave (0x513570): travelling along +x+y+z, along the normal,
            // through the form's table at fistp's nearest-even index;
            // flap (0x5136e0) scales it by the base s or t. A noise form
            // finds no table (0x5133d0) and holds its base.
            let at = i32(round((d.z + (p.x + p.y + p.z) * d.w) * 1024.0));
            var scale = d.x;
            if (form != FORM_NOISE) {
                scale += d.y * table_entry(form, at);
            }
            if (k == DEFORM_FLAP_S) {
                scale *= uv.x;
            } else if (k == DEFORM_FLAP_T) {
                scale *= uv.y;
            }
            p = p + n * scale;
        } else if (k == DEFORM_NORMAL) {
            // normal (0x513990): each axis takes amp times the noise at
            // 0.98 * the vertex (x offset 0, 100, 200), then renormalizes
            let q = p * 0.98;
            let m = n + d.x * vec3<f32>(
                noise_get4f(vec4<f32>(q, d.y)),
                noise_get4f(vec4<f32>(q.x + 100.0, q.y, q.z, d.y)),
                noise_get4f(vec4<f32>(q.x + 200.0, q.y, q.z, d.y)),
            );
            n = select(m, normalize(m), dot(m, m) > 0.0);
        } else if (k == DEFORM_BULGE) {
            // bulge (0x513ad0): the sin table at _ftol((s * width + t *
            // speed) * 1024 / 2pi), times height, along the normal
            let at = i32((uv.x * d.x + d.z) * 162.97466);
            p = p + n * (table_entry(0u, at) * d.y);
        } else if (k == DEFORM_MOVE) {
            // move (0x513b70): one offset for every vertex
            p = p + d.xyz;
        }
    }
    return Deformed(p, n);
}
