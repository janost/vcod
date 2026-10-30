//! 1.7's draw-surf key and the orders the lit list draws in
//! (cod4-world.md 12): `Material_Compare`'s material order, which numbers
//! `materialSortedIndex`, and each soup's place in the list.

use crate::assets::Assets;
use crate::assets::material::{Material, Pass, Shader, TechSet};
use std::cmp::Ordering;

/// Technique slots `Material_Compare` reads (`techniques[]` at
/// `MaterialTechniqueSet`+0xC): the two depth prepasses, the emissive
/// and the first lit technique.
const TECH_PREPASS: usize = 0;
const TECH_PREPASS_ALT: usize = 1;
const TECH_EMISSIVE: usize = 5;
const TECH_LIT: usize = 7;

/// What `Material_Compare` (iw3mp.exe 0x621250) orders a material by, read
/// off its record and its technique set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialOrder {
    pub lit: bool,
    pub emissive: bool,
    pub sort_key: u8,
    /// `gameFlags` 0x2.
    pub lightmap: bool,
    /// `gameFlags` 0x40, the draw-surf key's `customIndex` as
    /// `Material_Sort` sets it (0x6216a1).
    pub custom: bool,
    /// `R_DrawSurfStandardPrepassSortKey` (0x6211d0), 0..3.
    pub prepass: u8,
    /// `stateFlags` 0x8.
    pub writes_depth: bool,
    /// The lit technique's (else the emissive one's) first pass's pixel
    /// and vertex shader names.
    pub pixel: String,
    pub vertex: String,
    pub techset: String,
    pub name: String,
    /// The set has `TECHNIQUE_BUILD_FLOAT_Z` (`techniques[1]`), which
    /// `R_SetPrepassMaterial` (iw3mp.exe 0x648df0) draws `floatZ` with.
    pub floatz_technique: bool,
    /// `stateFlags`, which the prepass key reads.
    pub state_flags: u8,
    /// That pass's pixel constants (`R_ComparePixelConsts`, 0x64cbf0).
    pub consts: PixelConsts,
    /// The material's constant table (hash, literal), which the pass's
    /// named constants read.
    pub table: Vec<(u32, [f32; 4])>,
}

/// A pass's pixel constants as 0x64cbf0 compares them: past the
/// per-primitive and per-object arguments and those of a type below 5,
/// the code constants' indices (type 5), then (0x64cb30) the material's
/// named constants (type 6, looked up by hash) and the literals (type 7)
/// by destination register, a later one after an earlier on a tie.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PixelConsts {
    pub code: Vec<u16>,
    pub literal: Vec<(u16, [f32; 4])>,
}

impl PixelConsts {
    fn of(pass: &Pass, table: &[(u32, [f32; 4])]) -> PixelConsts {
        let first = usize::from(pass.per_prim_args) + usize::from(pass.per_obj_args);
        let stable = pass
            .args
            .iter()
            .skip(first)
            .take(usize::from(pass.stable_args));
        let mut out = PixelConsts::default();
        let mut args = stable.skip_while(|a| a.kind < 5).peekable();
        while let Some(a) = args.next_if(|a| a.kind == 5) {
            out.code.push(a.value as u16);
        }
        let mut add = |dest: u16, v: [f32; 4]| {
            let at = out.literal.partition_point(|&(d, _)| d <= dest);
            out.literal.insert(at, (dest, v));
        };
        let mut args = args.skip_while(|a| a.kind < 6).peekable();
        while let Some(a) = args.next_if(|a| a.kind == 6) {
            // retail drops the material with an error when it is missing
            if let Some(&(_, v)) = table.iter().find(|(h, _)| *h == a.value) {
                add(a.dest, v);
            }
        }
        while let Some(a) = args.next_if(|a| a.kind == 7) {
            add(a.dest, a.literal.unwrap_or_default());
        }
        out
    }

    fn cmp(&self, o: &PixelConsts) -> Ordering {
        let floats = |a: &[f32; 4], b: &[f32; 4]| {
            a.iter()
                .zip(b)
                .map(|(x, y)| x.partial_cmp(y).unwrap_or(Ordering::Equal))
                .find(|c| c.is_ne())
                .unwrap_or(Ordering::Equal)
        };
        self.code
            .len()
            .cmp(&o.code.len())
            .then_with(|| self.code.cmp(&o.code))
            .then(self.literal.len().cmp(&o.literal.len()))
            .then_with(|| {
                self.literal
                    .iter()
                    .zip(&o.literal)
                    .map(|((d, v), (e, w))| d.cmp(e).then_with(|| floats(v, w)))
                    .find(|c| c.is_ne())
                    .unwrap_or(Ordering::Equal)
            })
    }
}

impl MaterialOrder {
    /// `m`'s keys with its technique set's from `techset` (`None` when the
    /// zone holds only a stub of it: [`MaterialOrder::set_techset`] fills
    /// them from the zone that has it).
    pub fn of(m: &Material, techset: Option<(&Assets, &TechSet)>) -> MaterialOrder {
        let mut o = MaterialOrder {
            sort_key: m.sort_key,
            lightmap: m.game_flags & 2 != 0,
            custom: m.game_flags & 0x40 != 0,
            writes_depth: m.state_flags & 8 != 0,
            prepass: 3,
            state_flags: m.state_flags,
            name: m.name.clone(),
            table: m
                .constants
                .iter()
                .map(|c| (c.name_hash, c.literal))
                .collect(),
            ..Default::default()
        };
        if let Some((a, t)) = techset {
            o.set_techset(a, t);
        }
        o
    }

    /// Whether 1.7's `floatZ` prepass draws the material: 0x648df0 skips
    /// prepass key 3 (`MTL_PREPASS_NONE`) and a set without technique 1.
    pub fn draws_floatz(&self) -> bool {
        self.prepass != 3 && self.floatz_technique
    }

    /// Whether the technique set's keys are filled.
    pub fn has_techset(&self) -> bool {
        !self.techset.is_empty()
    }

    /// The technique set's keys off `t` (in `a`'s techniques).
    pub fn set_techset(&mut self, a: &Assets, t: &TechSet) {
        let tech = |slot: usize| Some(&a.techniques[(*t.techniques.get(slot)?)?.index()]);
        let (lit, emissive) = (tech(TECH_LIT), tech(TECH_EMISSIVE));
        // 0x6211d0: no prepass technique 3, the alternate alone 2; with
        // one, 3 when `stateFlags` 0x4, else 1 unless its flags hold 0x4
        self.floatz_technique = tech(TECH_PREPASS_ALT).is_some();
        self.prepass = match (tech(TECH_PREPASS), tech(TECH_PREPASS_ALT)) {
            (Some(_), _) if self.state_flags & 4 != 0 => 3,
            (Some(p), _) => u8::from(p.flags & 4 == 0),
            (None, Some(_)) => 2,
            (None, None) => 3,
        };
        let pass = lit.or(emissive).and_then(|t| t.passes.first());
        let name = |s: Option<&Shader>| s.map_or_else(String::new, |s| s.name.clone());
        self.lit = lit.is_some();
        self.emissive = emissive.is_some();
        self.pixel = name(pass.and_then(|p| p.pixel_shader.as_ref()));
        self.vertex = name(pass.and_then(|p| p.vertex_shader.as_ref()));
        self.consts = pass.map_or_else(PixelConsts::default, |p| PixelConsts::of(p, &self.table));
        self.techset = crate::map::stub_name(&t.name).to_owned();
    }

    /// 0x621250's order: lit first, then (lit) sort key and lightmapped
    /// first or (unlit) emissive first and sort key; the prepass key,
    /// depth writers first, the pixel shader's name, the vertex shader's,
    /// the technique set's and the material's; between the two shader
    /// names the pixel constants (0x621200), for a lit material only when
    /// it writes depth.
    pub fn compare(&self, o: &MaterialOrder) -> Ordering {
        o.lit
            .cmp(&self.lit)
            .then_with(|| match self.lit {
                true => self
                    .sort_key
                    .cmp(&o.sort_key)
                    .then(o.lightmap.cmp(&self.lightmap)),
                false => o
                    .emissive
                    .cmp(&self.emissive)
                    .then(self.sort_key.cmp(&o.sort_key)),
            })
            .then(self.prepass.cmp(&o.prepass))
            .then(o.writes_depth.cmp(&self.writes_depth))
            .then_with(|| self.pixel.cmp(&o.pixel))
            .then_with(|| match !self.lit || self.writes_depth {
                true => self.consts.cmp(&o.consts),
                false => Ordering::Equal,
            })
            .then_with(|| self.vertex.cmp(&o.vertex))
            .then_with(|| self.techset.cmp(&o.techset))
            .then_with(|| self.name.cmp(&o.name))
    }
}

/// Each material's place in `Material_Compare`'s order among `orders`:
/// `materialSortedIndex`'s order, which `Material_Sort` (0x621600) takes
/// over every loaded material, so the map's keep it among themselves.
pub fn ranks(orders: &[MaterialOrder]) -> Vec<u32> {
    let mut by: Vec<usize> = (0..orders.len()).collect();
    by.sort_by(|&a, &b| orders[a].compare(&orders[b]));
    let mut rank = vec![0; orders.len()];
    for (r, &m) in by.iter().enumerate() {
        rank[m] = r as u32;
    }
    rank
}

/// Which of the lit stage's lists a soup is drawn from: the world's
/// surfaces, the static models', the brush models' (scene entities). The
/// lit stage merges its lists 0..2 (iw3mp.exe 0x5fa2a6) through 0x63cce0,
/// which emits each primary sort key's run from every list in list order;
/// that the lists are these three in this order is INFERRED
/// (cod4-world.md 12.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum List {
    World,
    StaticModel,
    BrushModel,
}

/// One soup's place in the lit list as a key that sorts as 1.7 draws:
/// the material's primary sort key, then the list, then within the world
/// the surface's index (the zone's surfaces are stored in
/// `R_CompareSurfaces`' order, 0x5ff060), within the other two the draw-surf
/// key's own fields below the sort key: primary light, material order,
/// `customIndex`, reflection probe, then `id` (`objectId`, the static
/// model or surface).
pub fn soup_key(
    list: List,
    sort_key: u8,
    light: u8,
    material_rank: u32,
    custom: bool,
    probe: u8,
    id: u32,
) -> u64 {
    let top = u64::from(sort_key & 0x3f) << 56;
    match list {
        List::World => top | u64::from(id),
        List::StaticModel | List::BrushModel => {
            let list = if list == List::StaticModel { 1 } else { 2 };
            top | list << 54
                | u64::from(light) << 46
                | u64::from(material_rank & 0x7ff) << 35
                | u64::from(custom) << 34
                | u64::from(probe) << 26
                | u64::from(id & 0x3ff_ffff)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A soup key sorts by sort key, then list, then (world) surface or
    /// (the other lists) primary light ahead of the material.
    #[test]
    fn soup_keys_sort_by_sort_key_list_then_light() {
        let world = |sk, id| soup_key(List::World, sk, 9, 900, true, 7, id);
        let smodel = |sk, light, rank| soup_key(List::StaticModel, sk, light, rank, false, 0, 0);
        assert!(world(4, 8000) < smodel(4, 0, 0));
        assert!(smodel(4, 255, 2047) < world(5, 0));
        assert!(smodel(4, 1, 0) > smodel(4, 0, 2047));
        assert!(smodel(4, 0, 3) < soup_key(List::BrushModel, 4, 0, 0, false, 0, 0));
    }
}
