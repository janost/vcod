//! Counts the operations a WGSL entry point runs along one material's path
//! (cod4-client-perf.md 26), to set beside the instruction slots of
//! retail's compiled technique for the same material (`tools/re/d3d9dis.py
//! --count`). A path is the source with its uniform switches written in as
//! literals ([`Path::subs`]); the walk follows naga's IR from the entry,
//! inlines every call with its arguments, takes only the branch a known
//! condition picks, runs a counted loop its count of times and skips what
//! folds to a constant. `SHADER_OPS=1` prints the table.

use std::collections::HashMap;
use wgpu::naga;
use wgpu::naga::{BinaryOperator as B, Expression as E, Handle, MathFunction as M, Statement as S};

/// What one path runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Ops {
    /// texture samples and loads
    pub tex: u32,
    /// arithmetic, compare and select expressions, one each
    pub alu: u32,
    /// `alu` weighted as ps_3_0 expands its macros (`nrm` 3, `pow` 3,
    /// `lrp` 2, `sincos` 8; [`math_slots`])
    pub slots: u32,
    /// branches whose condition is not known on the path
    pub branches: u32,
    /// texture size and level queries
    pub queries: u32,
}

/// One material's path through an entry point: `subs` writes each
/// uniform switch the path fixes into the source as a literal.
pub(super) struct Path<'a> {
    pub entry: &'a str,
    pub subs: &'a [(&'a str, &'a str)],
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum V {
    U(u32),
    I(i32),
    F(f32),
    B(bool),
}

fn lit(l: &naga::Literal) -> Option<V> {
    use naga::Literal as L;
    Some(match *l {
        L::U32(v) => V::U(v),
        L::I32(v) => V::I(v),
        L::F32(v) => V::F(v),
        L::Bool(v) => V::B(v),
        L::AbstractInt(v) => V::I(v as i32),
        L::AbstractFloat(v) => V::F(v as f32),
        _ => return None,
    })
}

fn binary(op: B, a: V, b: V) -> Option<V> {
    Some(match (a, b) {
        (V::U(a), V::U(b)) => match op {
            B::And => V::U(a & b),
            B::InclusiveOr => V::U(a | b),
            B::Add => V::U(a.wrapping_add(b)),
            B::Subtract => V::U(a.wrapping_sub(b)),
            B::Multiply => V::U(a.wrapping_mul(b)),
            B::Equal => V::B(a == b),
            B::NotEqual => V::B(a != b),
            B::Less => V::B(a < b),
            B::LessEqual => V::B(a <= b),
            B::Greater => V::B(a > b),
            B::GreaterEqual => V::B(a >= b),
            _ => return None,
        },
        (V::I(a), V::I(b)) => match op {
            B::Add => V::I(a.wrapping_add(b)),
            B::Equal => V::B(a == b),
            B::NotEqual => V::B(a != b),
            B::Less => V::B(a < b),
            B::Greater => V::B(a > b),
            _ => return None,
        },
        (V::F(a), V::F(b)) => match op {
            B::Equal => V::B(a == b),
            B::NotEqual => V::B(a != b),
            B::Less => V::B(a < b),
            B::LessEqual => V::B(a <= b),
            B::Greater => V::B(a > b),
            B::GreaterEqual => V::B(a >= b),
            _ => return None,
        },
        (V::B(a), V::B(b)) => match op {
            B::LogicalAnd => V::B(a && b),
            B::LogicalOr => V::B(a || b),
            B::Equal => V::B(a == b),
            B::NotEqual => V::B(a != b),
            _ => return None,
        },
        _ => return None,
    })
}

/// ps_3_0's slots for a math function, `nrm`, `pow`, `lrp` and `sincos`
/// as their macro expansions take them; `abs` and a `[0, 1]` clamp are
/// source and result modifiers there.
fn math_slots(f: M) -> u32 {
    match f {
        M::Abs | M::Transpose => 0,
        M::Normalize | M::Length | M::Pow | M::Distance => 3,
        M::Mix | M::Cross | M::Exp | M::Log | M::Sqrt | M::Floor | M::Ceil | M::Clamp => 2,
        M::Sin | M::Cos => 8,
        M::SmoothStep => 4,
        _ => 1,
    }
}

struct Walk<'m> {
    module: &'m naga::Module,
    ops: Ops,
    /// each function's share of `ops.slots`
    by_fn: HashMap<String, u32>,
}

/// One inlined call: its arguments' values, what its locals hold where
/// the path knows it (a loop counter; naga lowers `&&` and `||` through a
/// bool) and its calls' results.
#[derive(Clone, Default)]
struct Frame {
    args: Vec<Option<V>>,
    locals: HashMap<Handle<naga::LocalVariable>, V>,
    calls: HashMap<Handle<E>, V>,
}

/// How a block ends on the path.
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    Next,
    Break,
    Continue,
    Ret(Option<V>),
}

impl Walk<'_> {
    fn value(&self, f: &naga::Function, fr: &Frame, h: Handle<E>) -> Option<V> {
        match &f.expressions[h] {
            E::Literal(l) => lit(l),
            E::Constant(c) => self.global(self.module.constants[*c].init),
            E::FunctionArgument(i) => fr.args.get(*i as usize).copied().flatten(),
            E::Load { pointer } => match f.expressions[*pointer] {
                E::LocalVariable(lv) => fr.locals.get(&lv).copied(),
                _ => None,
            },
            E::CallResult(_) => fr.calls.get(&h).copied(),
            E::Binary { op, left, right } => {
                binary(*op, self.value(f, fr, *left)?, self.value(f, fr, *right)?)
            }
            E::Unary {
                op: naga::UnaryOperator::LogicalNot,
                expr,
            } => match self.value(f, fr, *expr)? {
                V::B(b) => Some(V::B(!b)),
                _ => None,
            },
            E::Select {
                condition,
                accept,
                reject,
            } => match self.value(f, fr, *condition)? {
                V::B(true) => self.value(f, fr, *accept),
                V::B(false) => self.value(f, fr, *reject),
                _ => None,
            },
            _ => None,
        }
    }

    fn init_locals(&self, f: &naga::Function, fr: &mut Frame) {
        for (lv, var) in f.local_variables.iter() {
            if let Some(v) = var.init.and_then(|i| self.value(f, fr, i)) {
                fr.locals.insert(lv, v);
            }
        }
    }

    fn global(&self, h: Handle<E>) -> Option<V> {
        match &self.module.global_expressions[h] {
            E::Literal(l) => lit(l),
            _ => None,
        }
    }

    fn expr(&mut self, f: &naga::Function, fr: &Frame, h: Handle<E>) {
        if self.value(f, fr, h).is_some() {
            return;
        }
        let known_select = match &f.expressions[h] {
            E::Select { condition, .. } => self.value(f, fr, *condition).is_some(),
            _ => false,
        };
        let before = self.ops.slots + self.ops.tex;
        let o = &mut self.ops;
        match &f.expressions[h] {
            E::ImageSample { .. } | E::ImageLoad { .. } => o.tex += 1,
            E::ImageQuery { .. } => o.queries += 1,
            E::Math { fun, .. } => {
                o.alu += 1;
                o.slots += math_slots(*fun);
            }
            E::Binary { .. }
            | E::Unary { .. }
            | E::Relational { .. }
            | E::Derivative { .. }
            | E::As { .. } => {
                o.alu += 1;
                o.slots += 1;
            }
            // a select on a known condition is a move
            E::Select { .. } if !known_select => {
                o.alu += 1;
                o.slots += 1;
            }
            _ => {}
        }
        let name = f.name.clone().unwrap_or_default();
        *self.by_fn.entry(name).or_default() += self.ops.slots + self.ops.tex - before;
    }

    /// A `for (var i = 0; i < N; ...)` loop's N: naga's lowering opens the
    /// body with the `i < N` test.
    fn trips(&self, f: &naga::Function, body: &naga::Block) -> u32 {
        for s in body.iter() {
            if let S::If { condition, .. } = s {
                let mut h = *condition;
                if let E::Unary { expr, .. } = &f.expressions[h] {
                    h = *expr;
                }
                if let E::Binary {
                    op: B::Less, right, ..
                } = &f.expressions[h]
                {
                    match self.value(f, &Frame::default(), *right) {
                        Some(V::U(n)) => return n,
                        Some(V::I(n)) => return n as u32,
                        _ => {}
                    }
                }
                break;
            }
        }
        1
    }

    /// Both arms of an unknown branch: each counted, the locals both
    /// leave alike kept, the flow theirs where they agree.
    fn both(&mut self, f: &naga::Function, fr: &mut Frame, arms: &[&naga::Block]) -> Flow {
        self.ops.branches += 1;
        let mut ends = Vec::new();
        let mut outs = Vec::new();
        for arm in arms {
            let mut a = fr.clone();
            ends.push(self.block(f, &mut a, arm));
            outs.push(a);
        }
        let first = outs[0].locals.clone();
        fr.locals = first
            .into_iter()
            .filter(|(k, v)| outs.iter().all(|o| o.locals.get(k) == Some(v)))
            .collect();
        match ends[0] {
            Flow::Ret(_) if ends.iter().all(|e| matches!(e, Flow::Ret(_))) => Flow::Ret(None),
            e if ends.iter().all(|x| *x == e) => e,
            _ => Flow::Next,
        }
    }

    fn block(&mut self, f: &naga::Function, fr: &mut Frame, b: &naga::Block) -> Flow {
        for s in b.iter() {
            let flow = match s {
                S::Emit(r) => {
                    for h in r.clone() {
                        self.expr(f, fr, h);
                    }
                    Flow::Next
                }
                S::Block(b) => self.block(f, fr, b),
                S::If {
                    condition,
                    accept,
                    reject,
                } => match self.value(f, fr, *condition) {
                    Some(V::B(true)) => self.block(f, fr, accept),
                    Some(V::B(false)) => self.block(f, fr, reject),
                    _ => self.both(f, fr, &[accept, reject]),
                },
                S::Switch { selector, cases } => {
                    let sel = self.value(f, fr, *selector);
                    let hit = cases.iter().position(|c| match (c.value, sel) {
                        (naga::SwitchValue::U32(v), Some(V::U(s))) => v == s,
                        (naga::SwitchValue::I32(v), Some(V::I(s))) => v == s,
                        _ => false,
                    });
                    let hit = hit.or_else(|| {
                        sel.and_then(|_| {
                            cases
                                .iter()
                                .position(|c| c.value == naga::SwitchValue::Default)
                        })
                    });
                    let flow = match hit {
                        Some(i) => self.block(f, fr, &cases[i].body),
                        None => {
                            let arms: Vec<&naga::Block> = cases.iter().map(|c| &c.body).collect();
                            self.both(f, fr, &arms)
                        }
                    };
                    // a break leaves the switch only
                    if flow == Flow::Break {
                        Flow::Next
                    } else {
                        flow
                    }
                }
                S::Loop {
                    body, continuing, ..
                } => {
                    // a counter the path knows ends the loop by its own
                    // test; one it does not runs the test's count
                    let mut out = Flow::Next;
                    for _ in 0..=self.trips(f, body) {
                        match self.block(f, fr, body) {
                            Flow::Break => break,
                            r @ Flow::Ret(_) => {
                                out = r;
                                break;
                            }
                            Flow::Next | Flow::Continue => {}
                        }
                        self.block(f, fr, continuing);
                    }
                    out
                }
                S::Call {
                    function,
                    arguments,
                    result,
                } => {
                    let callee = &self.module.functions[*function];
                    let mut inner = Frame {
                        args: arguments.iter().map(|&h| self.value(f, fr, h)).collect(),
                        ..Frame::default()
                    };
                    self.init_locals(callee, &mut inner);
                    if let (Flow::Ret(Some(v)), Some(r)) =
                        (self.block(callee, &mut inner, &callee.body), result)
                    {
                        fr.calls.insert(*r, v);
                    }
                    Flow::Next
                }
                S::Store { pointer, value } => {
                    if let E::LocalVariable(lv) = f.expressions[*pointer] {
                        match self.value(f, fr, *value) {
                            Some(v) => {
                                fr.locals.insert(lv, v);
                            }
                            None => {
                                fr.locals.remove(&lv);
                            }
                        }
                    }
                    Flow::Next
                }
                S::Break => Flow::Break,
                S::Continue => Flow::Continue,
                S::Return { value } => Flow::Ret(value.and_then(|v| self.value(f, fr, v))),
                _ => Flow::Next,
            };
            if flow != Flow::Next {
                return flow;
            }
        }
        Flow::Next
    }
}

/// `path` counted through `src`, its switches written in first.
pub(super) fn count(src: &str, path: &Path) -> Ops {
    let mut src = src.to_string();
    for (from, to) in path.subs {
        src = src.replace(from, to);
    }
    let module =
        naga::front::wgsl::parse_str(&src).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&src)));
    let ep = module
        .entry_points
        .iter()
        .find(|e| e.name == path.entry)
        .unwrap_or_else(|| panic!("no entry {}", path.entry));
    let mut w = Walk {
        module: &module,
        ops: Ops::default(),
        by_fn: HashMap::new(),
    };
    let mut fr = Frame::default();
    w.init_locals(&ep.function, &mut fr);
    w.block(&ep.function, &mut fr, &ep.function.body);
    if std::env::var("SHADER_OPS").is_ok_and(|v| v == "2") {
        let mut by: Vec<_> = w.by_fn.iter().collect();
        by.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
        println!("{}: {by:?}", path.entry);
    }
    w.ops
}

/// The world's uniform switches on a CoD4 path: the material's flags,
/// sampler clamps and alpha test, the light kind (`Sun::src_col.w`: -1
/// none, 0 the sun, 1 omni, 2 spot), exp fog, `r_specular` 1, the sun
/// shadow map drawn, every image mipmapped (`flags.w`, and the
/// `textureNumLevels` test of the paths that still ask it).
fn world_subs(flags: u32, light: &str) -> Vec<(&'static str, String)> {
    vec![
        ("mat.flags.x", format!("{flags}u")),
        ("mat.flags.y", "0u".into()),
        ("mat.flags.z", "0u".into()),
        ("mat.flags.w", "15u".into()),
        ("sun.src_col.w", light.into()),
        ("camera.time_pad.w", "0.0".into()),
        ("camera.eye_fog_mode.w", "1.0".into()),
        ("cod4_sun.on.x", "1.0".into()),
        ("textureNumLevels(t) > 1u", "true".into()),
    ]
}

/// The CoD4 world paths cod4-client-perf.md 26 sets beside retail's
/// techniques: (label, entry, flags, light).
pub(super) const WORLD_PATHS: &[(&str, &str, u32, &str)] = &[
    // M_COD4 | M_NORMAL | M_SPECULAR | M_SUN_SHADOW
    (
        "wc_l_sm_r0c0n0s0 t9 (sun, shadow map)",
        "fs_main",
        16 | 32 | 1 | 256,
        "0.0",
    ),
    (
        "wc_l_sm_r0c0n0s0 t7 (no primary)",
        "fs_main",
        16 | 32 | 1 | 256,
        "-1.0",
    ),
    (
        "wc_l_sm_r0c0n0s0 t8 (sun, no shadow map)",
        "fs_main",
        16 | 32 | 1,
        "0.0",
    ),
    (
        "wc_l_sm_r0c0n0s0 t11 (spot, shadow map)",
        "fs_main",
        16 | 32 | 1 | 256,
        "2.0",
    ),
    (
        "mc_l_sm_r0c0n0s0 t9",
        "fs_cod4_model",
        16 | 32 | 1 | 256,
        "0.0",
    ),
    (
        "mc_l_sm_r0c0n0s0 t11 (spot, shadow map)",
        "fs_cod4_model",
        16 | 32 | 1 | 256,
        "2.0",
    ),
    (
        "mc_l_sm_r0c0n0 t9 (no specular)",
        "fs_cod4_model",
        16 | 32 | 256,
        "0.0",
    ),
    (
        "mc_l_t0c0n0s0 t8 (no shadow map)",
        "fs_cod4_model",
        16 | 32 | 1,
        "0.0",
    ),
];

/// A split pipeline's constants (cod2_world.wgsl's `LIT_*`,
/// renderer::cod2::LitPath) for `flags` under light kind `light`, alpha
/// test off.
fn split_subs(flags: u32, light: &str) -> Vec<(&'static str, String)> {
    vec![
        (
            "override LIT_SPLIT: bool = false;",
            "const LIT_SPLIT: bool = true;".into(),
        ),
        (
            "override LIT_FLAGS: u32 = 0u;",
            format!("const LIT_FLAGS: u32 = {flags}u;"),
        ),
        (
            "override LIT_TEST: u32 = 0u;",
            "const LIT_TEST: u32 = 0u;".into(),
        ),
        (
            "override LIT_LIGHT: f32 = 0.0;",
            format!("const LIT_LIGHT: f32 = {light};"),
        ),
    ]
}

/// `entry` counted through the world shader under `subs`.
fn world_count(entry: &str, subs: &[(&'static str, String)]) -> Ops {
    let subs: Vec<(&str, &str)> = subs.iter().map(|(a, b)| (*a, b.as_str())).collect();
    count(super::cod2::COD2_WORLD_WGSL, &Path { entry, subs: &subs })
}

/// `flags` under `light` through its split pipeline with the frame's
/// switches fixed as [`world_subs`] fixes them: what the path runs.
fn split_path(flags: u32, light: &str) -> Vec<(&'static str, String)> {
    let mut subs = split_subs(flags, light);
    subs.extend(
        world_subs(flags, light)
            .into_iter()
            .filter(|(k, _)| !matches!(*k, "mat.flags.x" | "mat.flags.y" | "sun.src_col.w")),
    );
    subs
}

/// Per path: what it runs ([`split_path`]); the whole fragment stage one
/// unsplit pipeline compiles, every uniform unknown; and the split
/// pipeline's, its `LIT_*` constants known and the frame's switches
/// (`r_specular`, fog mode, the sun shadow map drawn, `flags.w`) not.
fn world_table() -> Vec<(String, Ops)> {
    let mut rows = Vec::new();
    for entry in ["fs_main", "fs_cod4_model"] {
        rows.push((format!("{entry} compiled unsplit"), world_count(entry, &[])));
    }
    for &(label, entry, flags, light) in WORLD_PATHS {
        rows.push((
            format!("{label} {entry}"),
            world_count(entry, &split_path(flags, light)),
        ));
        rows.push((
            format!("{label} compiled split"),
            world_count(entry, &split_subs(flags, light)),
        ));
    }
    rows.push((
        "world vs_main".into(),
        world_count("vs_main", &world_subs(16 | 32 | 1 | 256, "0.0")),
    ));
    rows
}

/// An entity's CoD4 `lp_*` stage through dynamic_model.wgsl
/// (cod2_model.wgsl's `cod4_model`): the probe and specular switches, the
/// normal map, the sun with its shadow map (or `light` 2, a spot), exp
/// fog on the forward distance (only CoD2's is radial).
fn entity_rows() -> Vec<(String, Ops)> {
    let mut rows = Vec::new();
    for (label, light) in [
        ("entity lp_sm_sun (t9)", "0.0"),
        ("entity lp_sm_spot (t11)", "2.0"),
    ] {
        let subs = [
            ("stage.flags", "805306368u"),
            ("light_sets[in.light_set].ambient.w >= -1.5", "false"),
            ("ls.ambient.w < -1.5", "true"),
            ("stage.skip[1].x > 0.5", "true"),
            ("stage.skip[1].y > 0.5", "false"),
            ("stage.skip[1].z > 0.5", "true"),
            ("src.ambient.w", light),
            ("stage.deform_kinds.w", "0u"),
            ("camera.fog_range.z", "0.0"),
            ("camera.eye_fog_mode.w", "1.0"),
            ("cod4_sun.on.x", "1.0"),
            ("textureNumLevels(t) > 1u", "true"),
            // the vertex: no deform, no clock offset, a CoD4 set's GL
            // lighting skipped (ambient.w -2)
            ("stage.deform_kinds[i]", "0u"),
            ("time_offset != 0.0", "false"),
            ("ls.ambient.w < 0.0", "true"),
        ];
        let src = crate::entity_material::DYNAMIC_WGSL;
        rows.push((
            format!("{label} fs_main"),
            count(
                src,
                &Path {
                    entry: "fs_main",
                    subs: &subs,
                },
            ),
        ));
        if light == "0.0" {
            rows.push((
                "entity vs_main".into(),
                count(
                    src,
                    &Path {
                        entry: "vs_main",
                        subs: &subs,
                    },
                ),
            ));
        }
    }
    rows
}

/// CoD2's lit paths cod2-client-perf.md 8 sets beside the Mac build's ARB
/// translation of retail's compiled shaders (`tools/re/cod2_macarb.py`):
/// (label, entry, `M_*` flags) under exp fog, the sun lighting.
pub(super) const COD2_PATHS: &[(&str, &str, u32)] = &[
    ("lmap (phong_replace)", "fs_main", 0),
    ("lmap_s (phong_replace_spec)", "fs_main", 1),
    ("lmap_dtl (phong_replace_detail)", "fs_main", 2),
    ("lmap_s_dtl (phong_replace_spec_detail)", "fs_main", 3),
    (
        "lmap_a_s_dtl (phong_blend_spec_detail)",
        "fs_main",
        1 | 2 | 4,
    ),
    ("lprobe_smc_s (smodel phong_replace_spec)", "fs_smodel", 1),
    ("lprobe_smc_s_dtl", "fs_smodel", 3),
];

/// Per CoD2 path: what it runs, and the fragment stage its split and
/// unsplit pipelines compile (the frame's switches unknown).
fn cod2_table() -> Vec<(String, Ops)> {
    let mut rows = Vec::new();
    for entry in ["fs_main", "fs_smodel"] {
        rows.push((format!("{entry} compiled unsplit"), world_count(entry, &[])));
    }
    for &(label, entry, flags) in COD2_PATHS {
        rows.push((
            format!("{label} runs"),
            world_count(entry, &split_path(flags, "0.0")),
        ));
        rows.push((
            format!("{label} compiled split"),
            world_count(entry, &split_subs(flags, "0.0")),
        ));
    }
    rows.push((
        "CoD2 world vs_main".into(),
        world_count("vs_main", &world_subs(3, "0.0")),
    ));
    rows.extend(cod2_entity_rows());
    rows
}

/// CoD2's entity `lprobe*` stages through dynamic_model.wgsl
/// (cod2_model.wgsl's `cod2_probe_sided`) and an effect's
/// `vertcol_simple` quad through fx.wgsl, under radial exp fog.
fn cod2_entity_rows() -> Vec<(String, Ops)> {
    // F_COD2_PROBE, F_COD2_SPEC, F_COD2_DETAIL
    let (probe, spec, detail) = (268435456u32, 536870912u32, 1073741824u32);
    let fog = [
        ("camera.fog_range.z", "1.0"),
        ("camera.eye_fog_mode.w", "1.0"),
        ("textureNumLevels(t) > 1u", "true"),
    ];
    let mut rows = Vec::new();
    for (label, flags) in [
        ("entity lprobe", probe),
        ("entity lprobe_s", probe | spec),
        ("entity lprobe_s_dtl", probe | spec | detail),
    ] {
        let flags = format!("{flags}u");
        let mut subs = vec![
            ("stage.flags", flags.as_str()),
            ("light_sets[in.light_set].ambient.w >= -1.5", "true"),
            ("ls.ambient.w < -1.5", "false"),
            ("stage.deform_kinds.w", "0u"),
            ("stage.deform_kinds[i]", "0u"),
            ("time_offset != 0.0", "false"),
        ];
        subs.extend(fog);
        for entry in ["fs_main", "vs_main"] {
            if entry == "vs_main" && label != "entity lprobe_s" {
                continue;
            }
            rows.push((
                format!("{label} {entry}"),
                count(
                    crate::entity_material::DYNAMIC_WGSL,
                    &Path { entry, subs: &subs },
                ),
            ));
        }
    }
    let mut subs = vec![
        ("in.flags", "0u"),
        ("flags & ", "0u & "),
        ("in.feather > 0.0", "false"),
    ];
    subs.extend(fog);
    for entry in ["fs_main", "vs_main"] {
        rows.push((
            format!("effect vertcol_simple {entry}"),
            count(super::FX_WGSL, &Path { entry, subs: &subs }),
        ));
    }
    rows
}

/// The cookie receiver's pixel, per cookie it tests (`fs_receive`'s loop
/// body, `cod2_cookie.wgsl`) against `shadowcookie_receiver.hlsl`'s pass.
fn cookie_rows() -> Vec<(String, Ops)> {
    let src = super::cod2_cookie::COOKIE_WGSL;
    vec![
        (
            "cookie fs_receive, no cookie on the mask".into(),
            count(
                src,
                &Path {
                    entry: "fs_receive",
                    subs: &[("in.mask & ", "0u & ")],
                },
            ),
        ),
        (
            "cookie vs_receive".into(),
            count(
                src,
                &Path {
                    entry: "vs_receive",
                    subs: &[],
                },
            ),
        ),
        (
            "cookie fs_caster".into(),
            count(
                src,
                &Path {
                    entry: "fs_caster",
                    subs: &[],
                },
            ),
        ),
        (
            "cookie fs_blur".into(),
            count(
                src,
                &Path {
                    entry: "fs_blur",
                    subs: &[],
                },
            ),
        ),
    ]
}

#[test]
fn cod2_shader_ops() {
    let mut rows = cod2_table();
    rows.extend(cookie_rows());
    // a split CoD2 path runs its own arm alone, as the unsplit shader
    // does under its material's uniforms
    for &(label, entry, flags) in COD2_PATHS {
        let split = world_count(entry, &split_path(flags, "0.0"));
        let unsplit = world_count(entry, &world_subs(flags, "0.0"));
        assert_eq!(
            (split.tex, split.alu, split.slots, split.queries),
            (unsplit.tex, unsplit.alu, unsplit.slots, unsplit.queries),
            "{label}"
        );
    }
    if std::env::var_os("SHADER_OPS").is_some() {
        println!("| path | tex | alu | slots | branches | queries |");
        println!("|---|---|---|---|---|---|");
        for (l, o) in &rows {
            println!(
                "| {l} | {} | {} | {} | {} | {} |",
                o.tex, o.alu, o.slots, o.branches, o.queries
            );
        }
    }
}

/// A split pipeline runs the path its constants name and no other arm:
/// with the frame's switches fixed it does the work the unsplit shader
/// does with the material's uniforms written in, and branches no more.
#[test]
fn split_pipelines_run_their_path_alone() {
    for &(label, entry, flags, light) in WORLD_PATHS {
        let split = world_count(entry, &split_path(flags, light));
        let unsplit = world_count(entry, &world_subs(flags, light));
        assert_eq!(
            (split.tex, split.alu, split.slots, split.queries),
            (unsplit.tex, unsplit.alu, unsplit.slots, unsplit.queries),
            "{label}"
        );
        assert!(split.branches <= unsplit.branches, "{label}");
    }
}

#[test]
fn cod4_world_shader_ops() {
    let mut rows = world_table();
    rows.extend(entity_rows());
    if std::env::var_os("SHADER_OPS").is_some() {
        println!("| path | tex | alu | slots | branches | queries |");
        println!("|---|---|---|---|---|---|");
        for (l, o) in &rows {
            println!(
                "| {l} | {} | {} | {} | {} | {} |",
                o.tex, o.alu, o.slots, o.branches, o.queries
            );
        }
    }
}
