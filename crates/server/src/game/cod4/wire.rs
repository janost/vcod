//! The shared host's entities as CoD4 1.7 sends them (`Ent4`, design
//! decision D4 a): the host builds an entity in CoD2's field names
//! ([`super::COD4_HOST_PROTO`]), and [`ent4`] moves each field into the
//! word of CoD4's `entityState_t` of the same meaning (`lerp.` before the
//! trajectories and `eFlags`, `leanf` in the player union). The types 1.7
//! shares with CoD2's numbering (0 general .. 9 loop fx) go through; any
//! other is left out until its own stage builds it.

use vcod_cod4::net::delta::{EntityState, HudElem, MAX_HUDELEMS};
use vcod_cod4::net::fields::HUDELEM;
use vcod_common::net::msg;

/// CoD2's field name and CoD4's for the same word.
fn cod4_name(cod2: &str) -> Option<String> {
    let lerp = ["eFlags", "pos.", "apos."];
    if lerp.iter().any(|p| cod2.starts_with(p)) {
        return Some(format!("lerp.{cod2}"));
    }
    match cod2 {
        "leanf" => Some("lerp.u.player.leanf".into()),
        "time2" | "otherEntityNum" | "attackerEntityNum" | "groundEntityNum" | "loopSound"
        | "surfType" | "index" | "clientNum" | "iHeadIcon" | "iHeadIconTeam" | "solid"
        | "eventParm" | "eventSequence" | "weapon" | "legsAnim" | "torsoAnim" | "fTorsoPitch"
        | "fWaistPitch" => Some(cod2.into()),
        n if n.starts_with("events[") || n.starts_with("eventParms[") => Some(n.into()),
        _ => None,
    }
}

/// The highest `eType` CoD4 and CoD2 number alike: `ET_LOOP_FX` (9), which
/// the host builds only as CoD2's turret. The walk capture's script models
/// are 6 and its brush entities 0 in both.
const LAST_SHARED_TYPE: i32 = 9;
/// The host's turret type, CoD2's number, which no other CoD4 entity takes.
const COD2_ET_TURRET: i32 = 9;

/// One host entity in CoD4's words, `None` for a type 1.7 numbers
/// otherwise.
pub fn ent4(e: &msg::EntityState) -> Option<EntityState> {
    let p = &super::COD4_HOST_PROTO;
    let etype = e.field_i32(p, "eType");
    if !(0..=LAST_SHARED_TYPE).contains(&etype) {
        return None;
    }
    // CoD2's `ET_TURRET` (9) is 1.7's `ET_MG42` (11), on the general
    // table (the mp_backlot capture's SAWs, cod4-gsc.md 9.2).
    let etype = if etype == COD2_ET_TURRET {
        super::turret::ET_MG42
    } else {
        etype as u32
    };
    let mut out = EntityState::default();
    out.0[0] = e.number;
    out.0[1] = etype;
    let table = EntityState::table(etype);
    for (f, &v) in p.entity_fields.iter().zip(&e.fields) {
        if v == 0 {
            continue;
        }
        let Some(name) = cod4_name(f.name) else {
            continue;
        };
        let v = if name.ends_with(".trType") {
            tr_type4(v)
        } else {
            v
        };
        if let Some(t) = table.iter().find(|t| t.name == name) {
            out.0[t.word()] = v as u32;
        }
    }
    Some(out)
}

/// A host `trType` (CoD2's numbering) as 1.7 numbers it: CoD4 has no
/// `TR_GRAVITY_PAUSED`, so accelerate and decelerate are 6 and 7, not
/// CoD2's 7 and 8. VERIFIED (`play-1.7-hardpoint.txt`: a `moveTo` with
/// both eases sends 6, then 3, then 7; cod4-gsc.md 3.2).
fn tr_type4(t: i32) -> i32 {
    use vcod_cod2::trajectory::{TR_ACCELERATE, TR_DECELERATE};
    match t {
        TR_ACCELERATE => 6,
        TR_DECELERATE => 7,
        t => t,
    }
}

/// `name`'s word in a `hudelem_s`, looked up at compile time through
/// [`hw!`]: a name `HUDELEM` lacks fails the build.
const fn hud_word(name: &str) -> usize {
    let mut i = 0;
    while i < HUDELEM.len() {
        if eq(HUDELEM[i].name, name) {
            return HUDELEM[i].offset as usize / 4;
        }
        i += 1;
    }
    panic!("no such hudelem field")
}

const fn eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut k = 0;
    while k < a.len() && a[k] == b[k] {
        k += 1;
    }
    k == a.len()
}

macro_rules! hw {
    ($name:literal) => {
        const { hud_word($name) }
    };
}

/// The script fields of a hud element the wire reads, by [`hf!`] index.
const HUD_READS: [&str; 17] = [
    "archived",
    "alignx",
    "aligny",
    "horzalign",
    "vertalign",
    "x",
    "y",
    "z",
    "fontscale",
    "font",
    "color",
    "label",
    "sort",
    "glowcolor",
    "foreground",
    "hidewhendead",
    "hidewheninmenu",
];

const fn hud_read(name: &str) -> usize {
    let mut i = 0;
    while i < HUD_READS.len() {
        if eq(HUD_READS[i], name) {
            return i;
        }
        i += 1;
    }
    panic!("not in HUD_READS")
}

macro_rules! hf {
    ($name:literal) => {
        const { hud_read($name) }
    };
}

/// `HudElem_UpdateClient`: the element pool walked from slot 0,
/// a team element kept for `team` (the client's `sess.cs.team`), an owned
/// one for its owner, each into the archival or the current array by its
/// `archived` flag, 31 a side (cod4-hud.md 5). INFERRED, the walk and the
/// filter; the records are `hudelem_s` as the 1.7 table lays them out.
pub fn hud_elems(
    host: &crate::game::host::GameHost,
    slot: usize,
    team: i32,
) -> ([HudElem; MAX_HUDELEMS], [HudElem; MAX_HUDELEMS]) {
    use crate::game::entity::HUD_OWNER_ALL;
    let mut archival = [HudElem::default(); MAX_HUDELEMS];
    let mut current = archival;
    let (mut na, mut nc) = (0, 0);
    for (_, e) in host.ents.iter_hud_elems() {
        let Some(state) = e.hud else {
            continue;
        };
        if state.elem_type == 0
            || (state.team != 0 && state.team != team)
            || (state.owner != HUD_OWNER_ALL && state.owner != slot as u32)
        {
            continue;
        }
        let (out, n) = if hud_i32(e, hf!("archived")) != 0 {
            (&mut archival, &mut na)
        } else {
            (&mut current, &mut nc)
        };
        if *n < MAX_HUDELEMS {
            out[*n] = hud_elem(e, &state);
            *n += 1;
        }
    }
    (archival, current)
}

/// The host's element type (1.1's tags) in 1.7's `he_type_t`, which is
/// CoD2's: material 6, the four timers 7..10, the two clocks 11 and 12
/// (the capture's elements are 1 and 6, cod4-hud.md 5).
fn elem_type(t: i32) -> u32 {
    use crate::game::builtins::cod4_script::HOST_PLAYERNAME;
    (match t {
        3 => 6,
        HOST_PLAYERNAME => 3,
        4..=9 => t + 3,
        t => t,
    }) as u32
}

/// One element as its `hudelem_s` words. `targetEntNum` reads
/// `ENTITYNUM_NONE` on every element of the captures (VERIFIED live) until
/// `setTargetEnt`;
/// `fromAlignOrg` and `fromAlignScreen` are 0 until `moveOverTime`, which
/// vcod snapshots as the current alignment (INFERRED).
fn hud_elem(e: &crate::game::entity::GEntity, s: &crate::game::entity::HudState) -> HudElem {
    let mut h = HudElem::default();
    let f = |v: f32| v.to_bits();
    let org = (hud_i32(e, hf!("alignx")) << 2 | hud_i32(e, hf!("aligny"))) as u32;
    let screen = (hud_i32(e, hf!("horzalign")) << 3 | hud_i32(e, hf!("vertalign"))) as u32;
    h.0[hw!("type")] = elem_type(s.elem_type);
    h.0[hw!("x")] = f(hud_f32(e, hf!("x")));
    h.0[hw!("y")] = f(hud_f32(e, hf!("y")));
    h.0[hw!("z")] = f(hud_f32(e, hf!("z")));
    h.0[hw!("targetEntNum")] = s.target_ent;
    h.0[hw!("offscreenMaterialIdx")] = s.offscreen_material as u32;
    h.0[hw!("fontScale")] = f(hud_f32(e, hf!("fontscale")));
    h.0[hw!("font")] = hud_i32(e, hf!("font")) as u32;
    h.0[hw!("alignOrg")] = org;
    h.0[hw!("alignScreen")] = screen;
    h.0[hw!("color.rgba")] = hud_i32(e, hf!("color")) as u32;
    h.0[hw!("fromColor.rgba")] = s.from_color as u32;
    h.0[hw!("fadeStartTime")] = s.fade_start as u32;
    h.0[hw!("fadeTime")] = s.fade_ms as u32;
    h.0[hw!("label")] = hud_i32(e, hf!("label")) as u32;
    h.0[hw!("width")] = s.width as u32;
    h.0[hw!("height")] = s.height as u32;
    h.0[hw!("materialIndex")] = s.shader as u32;
    h.0[hw!("fromWidth")] = s.from_width as u32;
    h.0[hw!("fromHeight")] = s.from_height as u32;
    h.0[hw!("scaleStartTime")] = s.scale_start as u32;
    h.0[hw!("scaleTime")] = s.scale_ms as u32;
    h.0[hw!("fromX")] = f(s.from_x as f32);
    h.0[hw!("fromY")] = f(s.from_y as f32);
    if s.move_start != 0 {
        h.0[hw!("fromAlignOrg")] = org;
        h.0[hw!("fromAlignScreen")] = screen;
    }
    h.0[hw!("moveStartTime")] = s.move_start as u32;
    h.0[hw!("moveTime")] = s.move_ms as u32;
    h.0[hw!("time")] = s.time as u32;
    h.0[hw!("duration")] = s.duration as u32;
    h.0[hw!("value")] = f(s.value);
    h.0[hw!("text")] = s.text as u32;
    h.0[hw!("sort")] = f(hud_f32(e, hf!("sort")));
    h.0[hw!("glowColor.rgba")] = hud_i32(e, hf!("glowcolor")) as u32;
    h.0[hw!("fxBirthTime")] = s.pulse[0] as u32;
    h.0[hw!("fxLetterTime")] = s.pulse[1] as u32;
    h.0[hw!("fxDecayStartTime")] = s.pulse[2] as u32;
    h.0[hw!("fxDecayDuration")] = s.pulse[3] as u32;
    h.0[hw!("soundID")] = s.sound_id as u32;
    // `foreground` 1, `hidewhendead` 2, `hidewheninmenu` 4 of `flags`
    // (the capture's perk icons read 1, the team splash 4, VERIFIED live).
    let flags = u32::from(hud_i32(e, hf!("foreground")) != 0)
        | u32::from(hud_i32(e, hf!("hidewhendead")) != 0) << 1
        | u32::from(hud_i32(e, hf!("hidewheninmenu")) != 0) << 2;
    h.0[hw!("flags")] = flags;
    h
}

/// Each of [`HUD_READS`]' engine cell (`route_hud_cod4`), resolved once:
/// every client's elements read them every frame.
fn hud_value(e: &crate::game::entity::GEntity, read: usize) -> Option<vcod_gsc::Value> {
    static SLOTS: std::sync::OnceLock<[Option<usize>; HUD_READS.len()]> =
        std::sync::OnceLock::new();
    let slots = SLOTS.get_or_init(|| {
        HUD_READS.map(|n| match crate::game::fields::route_hud_cod4(n) {
            crate::game::fields::Route::Engine { slot, .. } => Some(slot),
            _ => None,
        })
    });
    slots[read].map(|slot| e.engine[slot])
}

fn hud_i32(e: &crate::game::entity::GEntity, read: usize) -> i32 {
    match hud_value(e, read) {
        Some(vcod_gsc::Value::Int(i)) => i,
        Some(vcod_gsc::Value::Float(f)) => f as i32,
        _ => 0,
    }
}

fn hud_f32(e: &crate::game::entity::GEntity, read: usize) -> f32 {
    match hud_value(e, read) {
        Some(vcod_gsc::Value::Int(i)) => i as f32,
        Some(vcod_gsc::Value::Float(f)) => f,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_model_keeps_its_place_and_model() {
        let p = &super::super::COD4_HOST_PROTO;
        let mut e = msg::EntityState::null(p);
        e.number = 77;
        let set = |e: &mut msg::EntityState, n: &str, v: i32| {
            let i = msg::EntityState::field_index(p, n).unwrap();
            e.fields[i] = v;
        };
        set(&mut e, "eType", 6);
        set(&mut e, "pos.trBase[0]", (-235.0f32).to_bits() as i32);
        set(&mut e, "index", 2);
        let c4 = ent4(&e).unwrap();
        assert_eq!(c4.number(), 77);
        assert_eq!(c4.etype(), 6);
        assert_eq!(c4.origin()[0], -235.0);
        let w = EntityState::table(6)
            .iter()
            .find(|f| f.name == "index")
            .unwrap()
            .word();
        assert_eq!(c4.0[w], 2);
    }
}
