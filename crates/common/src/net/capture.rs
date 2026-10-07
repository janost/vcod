//! The scripted gametype capture: a client that does what a gsc probe tells
//! it through the client cvar `probe_use`, and the lines it records. The
//! retail probe (`vcod --net-probe --save-scripted`) and the A/B gate in
//! `crates/server/tests/gametypes_ab.rs` both run this, so a fixture line and
//! the line ours produces come out of the same formatter.
//!
//! Every index the wire carries into a configstring range (a HUD string or
//! shader, an objective icon, a head icon, a model, a sound alias) is written
//! as the name it resolves to: the two servers may allocate a name at
//! different slots, and the name is what the client draws or plays.

use super::flags::{ET_CORPSE, ET_ITEM, ET_PLAYER, ET_SCRIPTMOVER};
use super::msg::{BUTTON_USE, HudElem, NULL_USERCMD, Objective, UserCmd, hud_field as h};
use super::protocol::{
    CS_LOCALIZED, CS_MODELS_V1 as CS_MODELS, CS_SHADERS, CS_SOUNDS, CsRange, PROTOCOL_V1,
};
use super::snapshot::Snapshot;

/// The cvar the gsc probe sets on a client with `setClientCvar`.
pub const USE_CVAR: &str = "probe_use";

/// A head icon is 1-based into its range, as a sound or a shader is.
const CS_HEAD_ICONS: usize = CsRange::HeadIcon.bounds().0 - 1;

/// A tap is held this long, under re.gsc's 0.3 s hold-to-drop delay.
const TAP_HOLD_MS: i64 = 100;
const TAP_PERIOD_MS: i64 = 1000;
/// A settled stretch still gets a trace line this often.
const HEARTBEAT_MS: i64 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum UseMode {
    #[default]
    Off,
    /// Press use for [`TAP_HOLD_MS`] every [`TAP_PERIOD_MS`].
    Tap,
    Hold,
}

#[derive(Default)]
pub struct ScriptedCapture {
    pub lines: Vec<String>,
    use_mode: UseMode,
    use_since: i64,
    last_trace: Option<(String, i64)>,
}

impl ScriptedCapture {
    pub fn use_mode(&self) -> UseMode {
        self.use_mode
    }

    pub fn on_gamestate(&mut self, ms: i64, server_id: i32, mapname: &str) {
        self.lines.push(format!(
            "!gamestate ms={ms} serverId={server_id} mapname={mapname}"
        ));
        self.last_trace = None;
    }

    /// `cmds` are what `NetClient::take_server_commands` returned, verbatim.
    /// A `s <idx>` (playLocalSound) gets a `!sound` line with the alias, and
    /// a `v probe_use "<mode>"` switches the input.
    pub fn on_commands(&mut self, ms: i64, cmds: &[String], cs: &[String]) {
        for text in cmds {
            self.lines.push(format!("!cmd ms={ms} text={text}"));
            let mut words = text.split_whitespace();
            match (words.next(), words.next()) {
                (Some("s"), Some(idx)) => {
                    let alias = idx
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| cs.get(CS_SOUNDS + i))
                        .map_or("?", String::as_str);
                    self.lines.push(format!("!sound ms={ms} alias={alias}"));
                }
                (Some("v"), Some(USE_CVAR)) => {
                    let v = words.next().unwrap_or("").trim_matches('"');
                    self.use_mode = match v {
                        "tap" => UseMode::Tap,
                        "hold" => UseMode::Hold,
                        _ => UseMode::Off,
                    };
                    self.use_since = ms;
                }
                _ => {}
            }
        }
    }

    /// One `!trace` line when the rendered state differs from the last kept
    /// one, or once a [`HEARTBEAT_MS`].
    pub fn sample(&mut self, ms: i64, snap: &Snapshot, cs: &[String]) {
        let body = trace_body(snap, cs);
        if let Some((last, at)) = &self.last_trace
            && *last == body
            && ms - at < HEARTBEAT_MS
        {
            return;
        }
        self.lines.push(format!(
            "!trace ms={ms} serverTime={} {body}",
            snap.server_time
        ));
        self.last_trace = Some((body, ms));
    }

    /// The cmd to send this frame: the view the snapshot reports, so a
    /// `setPlayerAngles` sticks, the held weapon, and use as `probe_use` says.
    pub fn cmd(&self, ms: i64, snap: Option<&Snapshot>) -> UserCmd {
        let p = &PROTOCOL_V1;
        let mut cmd = NULL_USERCMD;
        if let Some(s) = snap {
            cmd.weapon = s.ps.field_i32(p, "weapon") as u8;
            for (i, name) in ["viewangles[0]", "viewangles[1]", "viewangles[2]"]
                .iter()
                .enumerate()
            {
                cmd.angles[i] = angle_to_short(s.ps.field_f32(p, name));
            }
        }
        let pressed = match self.use_mode {
            UseMode::Off => false,
            UseMode::Hold => true,
            UseMode::Tap => (ms - self.use_since).rem_euclid(TAP_PERIOD_MS) < TAP_HOLD_MS,
        };
        if pressed {
            cmd.buttons |= BUTTON_USE;
        }
        cmd
    }
}

fn angle_to_short(deg: f32) -> i32 {
    ((deg * 65536.0 / 360.0).round() as i32) & 0xffff
}

fn cs_name(cs: &[String], base: usize, idx: i32) -> String {
    if idx <= 0 {
        return "-".to_string();
    }
    match cs.get(base + idx as usize) {
        Some(s) if !s.is_empty() => s.replace(' ', "_"),
        _ => format!("#{idx}"),
    }
}

/// Everything the trace records, as `key=value` words: the playerstate's
/// vitals and origin, both HUD arrays, the objective block, the client roster and the
/// players, corpses, items and script models in the snapshot.
pub fn trace_body(snap: &Snapshot, cs: &[String]) -> String {
    let p = &PROTOCOL_V1;
    let ps = &snap.ps;
    let f = |n: &str| ps.field_i32(p, n);
    let clips: Vec<String> = (0..64)
        .filter(|&i| ps.clip(i) != 0)
        .map(|i| format!("{i}:{}", ps.clip(i)))
        .collect();
    let ents: Vec<String> = snap
        .entities
        .values()
        .filter_map(|e| entity_str(e, cs))
        .collect();
    let clients: Vec<String> = snap
        .clients
        .values()
        .map(|c| {
            format!(
                "{}:{}:{}",
                c.client_num,
                c.field_i32(p, "team"),
                cs_name(cs, CS_MODELS, c.field_i32(p, "modelindex"))
            )
        })
        .collect();
    format!(
        "pm_type={} eFlags={} health={} weapon={} ammoclip={} origin={:.0},{:.0},{:.0} hud={} objectives={} clients={} ents={}",
        f("pm_type"),
        f("eFlags"),
        ps.health(),
        f("weapon"),
        join_or_dash(&clips, ","),
        ps.origin(p)[0],
        ps.origin(p)[1],
        ps.origin(p)[2],
        hud_str(&ps.arrays.hud_archived, &ps.arrays.hud_current, cs),
        objectives_str(&ps.arrays.objectives, cs),
        join_or_dash(&clients, ","),
        join_or_dash(&ents, ","),
    )
}

fn join_or_dash(v: &[String], sep: &str) -> String {
    if v.is_empty() {
        "-".to_string()
    } else {
        v.join(sep)
    }
}

/// A player as `num:p:headicon:headiconteam:weapon`, a corpse as
/// `num:c:clientNum`, an item as `num:i:index`, a script model as
/// `num:m:model:eFlags:x,y,z` with the base of its trajectory (`hide` is an
/// eFlags bit, not an absence). Nothing else: a map's
/// other entities do not move under these gametypes.
fn entity_str(e: &super::msg::EntityState, cs: &[String]) -> Option<String> {
    let p = &PROTOCOL_V1;
    let f = |n: &str| e.field_i32(p, n);
    let n = e.number;
    Some(match f("eType") {
        ET_PLAYER => format!(
            "{n}:p:{}:{}:{}",
            cs_name(cs, CS_HEAD_ICONS, f("iHeadIcon")),
            f("iHeadIconTeam"),
            f("weapon")
        ),
        ET_CORPSE => format!("{n}:c:{}", f("clientNum")),
        ET_ITEM => format!("{n}:i:{}", f("index")),
        ET_SCRIPTMOVER => {
            let o = e.origin(p);
            format!(
                "{n}:m:{}:{:#x}:{:.0},{:.0},{:.0}",
                cs_name(cs, CS_MODELS, f("index")),
                f("eFlags"),
                o[0],
                o[1],
                o[2]
            )
        }
        _ => return None,
    })
}

/// The non-empty elements of both arrays, archived first, `|` between. One
/// element is `type:x,y:label:text:value:shader:w,h:timer:color`, with the
/// strings and the shader resolved. `timer` is `T` when the element carries a
/// timer's server time and `-` when not: the time itself is the server's own
/// clock, which two servers never share.
pub fn hud_str(archived: &[HudElem], current: &[HudElem], cs: &[String]) -> String {
    let half = |elems: &[HudElem]| {
        let v: Vec<String> = elems
            .iter()
            .filter(|e| **e != HudElem::default())
            .map(|e| hud_elem_str(e, cs))
            .collect();
        join_or_dash(&v, ";")
    };
    format!("{}|{}", half(archived), half(current))
}

fn hud_elem_str(e: &HudElem, cs: &[String]) -> String {
    format!(
        "{}:{},{}:{}:{}:{}:{}:{},{}:{}:{:#x}",
        e.get(h::TYPE),
        e.get(h::X),
        e.get(h::Y),
        cs_name(cs, CS_LOCALIZED, e.get(h::LABEL)),
        cs_name(cs, CS_LOCALIZED, e.get(h::TEXT)),
        e.get_f32(h::VALUE),
        cs_name(cs, CS_SHADERS, e.get(h::SHADER)),
        e.get(h::WIDTH),
        e.get(h::HEIGHT),
        if e.get(h::TIME) != 0 { "T" } else { "-" },
        e.get(h::COLOR) as u32,
    )
}

/// Every slot that differs from the default, as
/// `i:state:icon:entNum:team:x,y,z`.
pub fn objectives_str(objs: &[Objective], cs: &[String]) -> String {
    let v: Vec<String> = objs
        .iter()
        .enumerate()
        .filter(|(_, o)| **o != Objective::default())
        .map(|(i, o)| {
            let g = o.origin_f32();
            format!(
                "{i}:{}:{}:{}:{}:{:.0},{:.0},{:.0}",
                o.state,
                cs_name(cs, CS_SHADERS, o.icon),
                o.ent_num,
                o.team_num,
                g[0],
                g[1],
                g[2]
            )
        })
        .collect();
    join_or_dash(&v, ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_use_switches_the_input_and_taps_on_a_period() {
        let mut c = ScriptedCapture::default();
        c.on_commands(1000, &["v probe_use \"tap\"".to_string()], &[]);
        assert_eq!(c.use_mode(), UseMode::Tap);
        assert_eq!(c.cmd(1050, None).buttons & BUTTON_USE, BUTTON_USE);
        assert_eq!(c.cmd(1500, None).buttons & BUTTON_USE, 0);
        assert_eq!(c.cmd(2010, None).buttons & BUTTON_USE, BUTTON_USE);
        c.on_commands(3000, &["v probe_use \"0\"".to_string()], &[]);
        assert_eq!(c.cmd(3000, None).buttons & BUTTON_USE, 0);
    }

    #[test]
    fn a_local_sound_is_named_by_its_alias() {
        let mut cs = vec![String::new(); CS_SOUNDS + 4];
        cs[CS_SOUNDS + 3] = "re_pickup_paper".to_string();
        let mut c = ScriptedCapture::default();
        c.on_commands(5, &["s 3".to_string()], &cs);
        assert_eq!(c.lines[1], "!sound ms=5 alias=re_pickup_paper");
    }
}
