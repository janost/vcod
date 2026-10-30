// CoD4's flag vertex shaders (`lp_flag_*_s_tc0n0*`, cod4-world.md 3.4),
// appended to dynamic_model.wgsl and cod2_world.wgsl: three sine waves
// over (1 - u, 1 - v) and the clock push the vertex along its normal by
// up to 12.2 units times `amount` (the vertex colour's alpha) and tilt the
// normal along the tangent and the binormal by their slopes. The binormal
// the waves read is cross(N, T) times the frame's handedness, the one
// handed on cross(tilted N, T) times it. Each phase is wrapped into
// [-pi, pi) as the bytecode does before `sincos`.
struct Cod4Waved {
    pos: vec3<f32>,
    normal: vec3<f32>,
    binormal: vec3<f32>,
};

fn cod4_flag_wave(p: vec3<f32>, n: vec3<f32>, t: vec3<f32>, b: vec3<f32>, uv: vec2<f32>, amount: f32, time: f32) -> Cod4Waved {
    let f = vec2<f32>(1.0) - uv;
    let phase = vec3<f32>(
        dot(f, vec2<f32>(3.623051881790161, 0.2533484697341919)) + time * 5.654866695404053,
        dot(f, vec2<f32>(8.250306129455566, -1.4547516107559204)) + time * 9.42477798461914,
        dot(f, vec2<f32>(19.577713012695312, 5.245832443237305)) + time * 7.539822578430176,
    );
    let w = fract(phase * 0.15915493667125702 + 0.5) * 6.2831854820251465 - 3.1415927410125732;
    let s = sin(w);
    let c = cos(w);
    let push = dot(vec3<f32>(7.5, 3.0, 1.7), s) * amount;
    let slope_t = dot(vec3<f32>(27.172889709472656, 24.750919342041016, 33.28211212158203), c) * amount * 0.025;
    let slope_b = dot(vec3<f32>(1.9001134634017944, -4.364254474639893, 8.917915344238281), c) * amount * 0.025;
    let hand = select(-1.0, 1.0, dot(cross(n, t), b) >= 0.0);
    let n2 = n + slope_t * t + slope_b * hand * cross(n, t);
    return Cod4Waved(p + push * n, n2, hand * cross(n2, t));
}
