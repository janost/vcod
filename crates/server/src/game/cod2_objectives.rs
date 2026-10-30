//! What CoD2 bots read off the stock `sd` and `ctf` scripts each frame
//! (`maps/mp/gametypes/*.gsc`, iw_15.iwd; docs/research/bot-objectives.md
//! section 11). CoD2's `hq` keeps United Offensive's radio fields
//! (`level.radio`, `.hidden`, `.team`, `.radius`), so it is read by
//! [`ScriptRuntime::uo_objectives`].

use vcod_gsc::{ArrayKey, EntId, Host, Target, Value};

use super::builtins::entity::get_ent_array;
use super::script::{CtfFlag, ScriptRuntime, SdObjectives, TEAM_ALLIES, TEAM_AXIS, UoObjectives};
use super::trigger::entity_abs_bounds;

/// A trigger the script sent 10000 units down (`triggerOff`,
/// `pickupFlag`) is not there to touch.
const GONE_Z: f32 = -5000.0;

fn entities(host: &mut super::host::GameHost, cx: &mut vcod_gsc::Cx, name: &str) -> Vec<EntId> {
    let args = [
        Value::String(cx.intern_exact(name)),
        Value::String(cx.intern_exact("targetname")),
    ];
    let Ok(Value::Array(a)) = get_ent_array(host, cx, None, &args) else {
        return Vec::new();
    };
    (0..cx.array_len(a))
        .filter_map(|i| match cx.get_index(a, ArrayKey::Int(i as i32)) {
            Value::Entity(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn text(cx: &mut vcod_gsc::Cx, v: Value) -> Option<String> {
    match v {
        Value::String(a) => Some(cx.resolve(a).to_string()),
        _ => None,
    }
}

fn truthy(v: Value) -> bool {
    match v {
        Value::Int(i) => i != 0,
        Value::Float(f) => f != 0.0,
        _ => false,
    }
}

impl ScriptRuntime {
    /// CoD2's `sd.gsc`: the `bombzone` triggers (`trigger_use_touch`) the
    /// level's `bombmode` keeps (`bombzones`, 1433-1551: `script_bombmode_
    /// original`, `_single` or `_dual`), A before B by `script_label`, and
    /// once `level.bombplanted` the `bombtrigger` (another
    /// `trigger_use_touch`, moved onto the bomb at 1754).
    pub fn cod2_sd_objectives(&mut self) -> SdObjectives {
        self.with_host(|host, cx| {
            let Target::Struct(level) = cx.level() else {
                unreachable!("level is a struct");
            };
            let game = cx.game();
            let mut team = |key: &str| {
                let k = ArrayKey::Str(cx.intern_exact(key));
                match cx.get_index(game, k) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    _ => String::new(),
                }
            };
            let attackers = team("attackers");
            let defenders = team("defenders");
            let mode = cx.intern_folded("bombmode");
            let key = match cx.get_field(level, mode) {
                Value::Int(1) => "script_bombmode_single",
                Value::Int(2) => "script_bombmode_dual",
                _ => "script_bombmode_original",
            };
            let key = cx.intern_folded(key);
            let label = cx.intern_folded("script_label");
            let mut zones: Vec<(String, EntId)> = Vec::new();
            for id in entities(host, cx, "bombzone") {
                if matches!(host.get_field(cx, id, key), Value::Undefined) {
                    continue;
                }
                let l = host.get_field(cx, id, label);
                zones.push((text(cx, l).unwrap_or_default().to_ascii_uppercase(), id));
            }
            zones.sort();
            let sites = zones
                .into_iter()
                .map(|(_, id)| entity_abs_bounds(host, cx, id))
                .filter(|b| b.1[2] > GONE_Z)
                .collect();
            let planted = cx.intern_folded("bombplanted");
            let bomb = if truthy(cx.get_field(level, planted)) {
                entities(host, cx, "bombtrigger").first().map(|&id| {
                    let origin = cx.intern_folded("origin");
                    let at = match host.get_field(cx, id, origin) {
                        Value::Vector(v) => v,
                        _ => [0.0; 3],
                    };
                    (at, entity_abs_bounds(host, cx, id))
                })
            } else {
                None
            };
            SdObjectives {
                attackers,
                defenders,
                sites,
                bomb,
            }
        })
    }

    /// CoD2's `ctf.gsc` (`initFlags`, `flag`, 1025-1170): each flag is its
    /// own `trigger_radius`, `allied_flag` then `axis_flag`, that stands at
    /// `home_origin` while `atbase`, lies where its carrier fell after
    /// `dropFlag`, and is sent 10000 down while carried (`pickupFlag`); the
    /// carrier is the player whose `.flag` it is. A touch by the other side
    /// takes it, by its own side returns it; a carrier captures at his own
    /// flag at home, so a flag's goal is the other flag's home trigger.
    pub fn cod2_ctf_objectives(&mut self, clients: usize) -> Option<UoObjectives> {
        let client_ents: Vec<(usize, EntId)> = (0..clients)
            .filter_map(|slot| Some((slot, self.client_entity(slot)?)))
            .collect();
        self.with_host(|host, cx| {
            let [team, atbase, home, origin, flag] =
                ["team", "atbase", "home_origin", "origin", "flag"].map(|n| cx.intern_folded(n));
            let mut read = Vec::new();
            for name in ["allied_flag", "axis_flag"] {
                let id = *entities(host, cx, name).first()?;
                let t = host.get_field(cx, id, team);
                let side = match text(cx, t)?.as_str() {
                    "axis" => TEAM_AXIS,
                    _ => TEAM_ALLIES,
                };
                let (Value::Vector(at), Value::Vector(home_at)) =
                    (host.get_field(cx, id, origin), host.get_field(cx, id, home))
                else {
                    return None;
                };
                let (lo, hi) = entity_abs_bounds(host, cx, id);
                // The trigger's extent about its origin, which moves.
                let shift = |p: [f32; 3]| -> super::script::Bounds {
                    (
                        std::array::from_fn(|i| lo[i] - at[i] + p[i]),
                        std::array::from_fn(|i| hi[i] - at[i] + p[i]),
                    )
                };
                let carrier = client_ents
                    .iter()
                    .find(|(_, e)| matches!(host.get_field(cx, *e, flag), Value::Entity(f) if f == id))
                    .map(|(s, _)| *s);
                let at_base = truthy(host.get_field(cx, id, atbase));
                read.push((
                    side,
                    at_base,
                    carrier,
                    (at[2] > GONE_Z).then(|| shift(at)),
                    shift(home_at),
                ));
            }
            let flags = (0..2)
                .map(|i| {
                    let (side, at_base, carrier, now, _) = read[i];
                    CtfFlag {
                        team: side,
                        home: now.filter(|_| at_base && carrier.is_none()),
                        dropped: now.filter(|_| !at_base && carrier.is_none()),
                        goal: read[1 - i].4,
                        carrier,
                    }
                })
                .collect();
            Some(UoObjectives::Ctf(flags))
        })
    }
}
