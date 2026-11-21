//! What the bots read of the level's objectives and pickups.

use super::*;

impl ScriptRuntime {
    /// Every weapon item lying in the world to be picked up, as `(weapon
    /// index, origin, empty)`: placed `mpweapon_*` entities while shown and
    /// drops. Empty is no round in the clip or the reserve: a drop writes
    /// -1 into each count it found empty (`cod11-items.md` 8), and 0 is
    /// unset, which the pickup fills.
    /// A taken one waits on its respawn off every snapshot and is left out.
    pub fn weapon_items(&mut self) -> Vec<(u8, [f32; 3], bool)> {
        use vcod_gsc::Host;
        let defs = self.host.defs;
        let items: Vec<(EntId, u8, i32)> = self
            .host
            .ents
            .iter_inuse()
            .filter_map(|(id, e)| {
                let item = e.item?;
                let weapon = !item.taken && defs.weapon_name(item.index as usize).is_some();
                weapon.then_some((id, item.index, item.clip))
            })
            .collect();
        let host = &mut self.host;
        let counts: Vec<i32> = self.vm.with_cx(|cx| {
            let count = cx.intern_folded("count");
            items
                .iter()
                .map(|&(id, ..)| match host.get_field(cx, id, count) {
                    Value::Int(n) => n,
                    _ => 0,
                })
                .collect()
        });
        items
            .into_iter()
            .zip(counts)
            .filter_map(|((id, w, clip), count)| {
                Some((w, self.entity_origin_of(id)?, clip < 0 && count < 0))
            })
            .collect()
    }

    /// Every health item lying in the world to be picked up (`item_health*`,
    /// a death's `dropHealth`), not taken: where a hurt bot heals
    /// (`bots/retreat.rs`).
    pub fn health_items(&mut self) -> Vec<[f32; 3]> {
        let defs = self.host.defs;
        let health: Vec<usize> = defs
            .static_items()
            .filter(|(_, n, _)| n.starts_with("item_health"))
            .map(|(i, ..)| i)
            .collect();
        let ids: Vec<EntId> = self
            .host
            .ents
            .iter_inuse()
            .filter_map(|(id, e)| {
                let item = e.item?;
                (!item.taken && health.contains(&(item.index as usize))).then_some(id)
            })
            .collect();
        ids.into_iter()
            .filter_map(|id| self.entity_origin_of(id))
            .collect()
    }

    /// The stock `re.gsc` objectives as they stand this frame
    /// (docs/research/bot-objectives.md, "Retrieval"): the sides from
    /// `game["re_attackers"]` and `game["re_defenders"]`, and each entity of
    /// `level.retrieval_objective` the script has not deleted, through its
    /// `trigger`, `goal` and `objnum` fields and the compass record `objnum`
    /// names. `clients` bounds the carrier search.
    pub fn re_objectives(&mut self, clients: usize) -> ReObjectives {
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::{ArrayKey, Host};
        let level = self.vm.level_id();
        let client_ents: Vec<(usize, EntId)> = (0..clients)
            .filter_map(|slot| Some((slot, self.client_entity(slot)?)))
            .collect();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let game = cx.game();
            let mut team = |key: &str| {
                let k = ArrayKey::Str(cx.intern_exact(key));
                match cx.get_index(game, k) {
                    Value::String(a) => cx.resolve(a).to_string(),
                    _ => String::new(),
                }
            };
            let attackers = team("re_attackers");
            let defenders = team("re_defenders");
            let [list, trigger, goal, objnum, origin, startorigin] = [
                "retrieval_objective",
                "trigger",
                "goal",
                "objnum",
                "origin",
                "startorigin",
            ]
            .map(|f| cx.intern_folded(f));
            let Value::Array(list) = cx.get_field(level, list) else {
                return ReObjectives {
                    attackers,
                    defenders,
                    objectives: Vec::new(),
                };
            };
            let mut objectives = Vec::new();
            for i in 0..cx.array_len(list) {
                let Value::Entity(obj) = cx.get_index(list, ArrayKey::Int(i as i32)) else {
                    continue;
                };
                if host.ents.get(obj).is_none() {
                    continue;
                }
                let (Value::Entity(t), Value::Entity(g)) = (
                    host.get_field(cx, obj, trigger),
                    host.get_field(cx, obj, goal),
                ) else {
                    continue;
                };
                let pickup = Some(entity_abs_bounds(host, cx, t)).filter(|b| b.1[2] > -5000.0);
                // Lying away from where the round put it: dropped, and
                // `objective_timeout` sends it back (`re.gsc` 2422-2432).
                let start = match host.get_field(cx, t, startorigin) {
                    Value::Vector(v) => Some(v),
                    _ => None,
                };
                let at = match host.get_field(cx, t, origin) {
                    Value::Vector(v) => Some(v),
                    _ => None,
                };
                let dropped = match (pickup, start, at) {
                    (Some(_), Some(s), Some(a)) => {
                        (0..3).any(|i| (s[i] - a[i]).abs() > 1.0).then_some((s, a))
                    }
                    _ => None,
                };
                let goal = entity_abs_bounds(host, cx, g);
                // The compass record follows its carrier (`objective_onEntity`,
                // `re.gsc` 2062) until a drop pins it again (`retrieval_think`,
                // 1983). `hasobj` is no guide: a death drop never clears it
                // (`drop_objective_on_disconnect_or_death`, 2617).
                let record = match host.get_field(cx, obj, objnum) {
                    Value::Int(n) => host.objectives.get(n as usize).filter(|o| o.state != 0),
                    _ => None,
                };
                let carrier = record.and_then(|o| {
                    client_ents
                        .iter()
                        .find(|(_, ent)| ent.0 as i32 == o.ent_num)
                        .map(|(slot, _)| *slot)
                });
                objectives.push(ReObjective {
                    pickup,
                    goal,
                    carrier,
                    shown_to: record.map_or(0, |o| o.team_num),
                    laid: record.map(|o| o.origin_f32()),
                    dropped,
                });
            }
            ReObjectives {
                attackers,
                defenders,
                objectives,
            }
        })
    }

    /// What the bots read off stock `sd.gsc` this frame
    /// (docs/research/bot-objectives.md): `game["attackers"]` and
    /// `game["defenders"]`, the live `bombzone_A` / `bombzone_B` triggers'
    /// bounds in that order, and once `level.bombplanted` the defuse
    /// trigger's origin and bounds. Entity handles do not survive a
    /// `map_restart`, so the caller asks again every frame.
    pub fn sd_objectives(&mut self) -> SdObjectives {
        use crate::game::builtins::entity::get_ent;
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::ArrayKey;
        let level = self.vm.level_id();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
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
            let mut find = |cx: &mut vcod_gsc::Cx, name: &str| {
                let args = [
                    Value::String(cx.intern_exact(name)),
                    Value::String(cx.intern_exact("targetname")),
                ];
                match get_ent(host, cx, None, &args) {
                    Ok(Value::Entity(id)) => Some(id),
                    _ => None,
                }
            };
            let zones: Vec<EntId> = ["bombzone_A", "bombzone_B"]
                .iter()
                .filter_map(|n| find(cx, n))
                .collect();
            let trigger = find(cx, "bombtrigger");
            let planted = cx.intern_folded("bombplanted");
            let planted = !matches!(
                cx.get_field(level, planted),
                Value::Undefined | Value::Int(0)
            );
            let sites = zones
                .into_iter()
                .map(|id| entity_abs_bounds(host, cx, id))
                .collect();
            let bomb = trigger.filter(|_| planted).map(|id| {
                let origin = cx.intern_folded("origin");
                let at = host.vec_field_at(cx, id, origin).to_array();
                (at, entity_abs_bounds(host, cx, id))
            });
            SdObjectives {
                attackers,
                defenders,
                sites,
                bomb,
            }
        })
    }
}

impl ScriptRuntime {
    /// What the bots read off United Offensive's `ctf`, `dom`, `hq` and
    /// `bas` this frame (docs/research/bot-objectives.md section 7);
    /// `None` on any other gametype or before the script has set its
    /// objectives up. Read fresh every frame: a `map_restart` respawns
    /// them, and the scripts move their triggers about.
    pub fn uo_objectives(&mut self, gametype: &str, clients: usize) -> Option<UoObjectives> {
        use crate::game::builtins::entity::get_ent;
        use crate::game::trigger::entity_abs_bounds;
        use vcod_gsc::Host;
        let level = self.vm.level_id();
        let client_ents: Vec<(usize, EntId)> = (0..clients)
            .filter_map(|slot| Some((slot, self.client_entity(slot)?)))
            .collect();
        let host = &mut self.host;
        self.vm.with_cx(|cx| {
            let team_of = |cx: &mut vcod_gsc::Cx, v: Value| match v {
                Value::String(a) => match cx.resolve(a) {
                    "axis" => TEAM_AXIS,
                    "allies" | "allied" => TEAM_ALLIES,
                    _ => TEAM_NONE,
                },
                _ => TEAM_NONE,
            };
            let truthy = |v: Value| match v {
                Value::Int(i) => i != 0,
                Value::Float(f) => f != 0.0,
                _ => false,
            };
            let number = |v: Value| match v {
                Value::Int(i) => i as f32,
                Value::Float(f) => f,
                _ => 0.0,
            };
            let ent = |v: Value| match v {
                Value::Entity(e) => Some(e),
                _ => None,
            };
            // A trigger the script sent 10000 units down (`triggerOff`) is
            // not there to touch.
            let live = |b: Bounds| (b.1[2] > -5000.0).then_some(b);
            let f = |cx: &mut vcod_gsc::Cx, name: &str| cx.intern_folded(name);
            let origin = f(cx, "origin");
            match gametype {
                "ctf" => {
                    let [team, trigger, goal, mobile, moved, carried, hasflag] = [
                        "team",
                        "trigger",
                        "goal",
                        "mobile_trigger",
                        "moved",
                        "carried_by",
                        "hasflag",
                    ]
                    .map(|n| f(cx, n));
                    let mut flags = Vec::new();
                    for name in ["allies_flag", "axis_flag"] {
                        let at = f(cx, name);
                        let flag = ent(cx.get_field(level, at))?;
                        host.ents.get(flag)?;
                        let (Some(t), Some(g), Some(m)) = (
                            ent(host.get_field(cx, flag, trigger)),
                            ent(host.get_field(cx, flag, goal)),
                            ent(host.get_field(cx, flag, mobile)),
                        ) else {
                            return None;
                        };
                        // `drop_flag` leaves `carried_by` naming the fallen
                        // carrier until `reset_flag`; his `hasflag` it clears
                        // (bot-objectives.md 14.11).
                        let carrier = ent(host.get_field(cx, flag, carried))
                            .filter(|&c| ent(host.get_field(cx, c, hasflag)) == Some(flag))
                            .and_then(|c| {
                                client_ents.iter().find(|(_, e)| *e == c).map(|(s, _)| *s)
                            });
                        let moved = truthy(host.get_field(cx, flag, moved));
                        let team = host.get_field(cx, flag, team);
                        flags.push(CtfFlag {
                            team: team_of(cx, team),
                            home: (!moved)
                                .then(|| live(entity_abs_bounds(host, cx, t)))
                                .flatten(),
                            dropped: (moved && carrier.is_none())
                                .then(|| live(entity_abs_bounds(host, cx, m)))
                                .flatten(),
                            goal: entity_abs_bounds(host, cx, g),
                            carrier,
                        });
                    }
                    Some(UoObjectives::Ctf(flags))
                }
                "dom" => {
                    let team = f(cx, "team");
                    let mut flags = Vec::new();
                    // `Flag_InitTriggers` looks for `flag1` .. `flag14`.
                    for q in 1..15 {
                        let args = [
                            Value::String(cx.intern_exact(&format!("flag{q}"))),
                            Value::String(cx.intern_exact("targetname")),
                        ];
                        let Ok(Value::Entity(id)) = get_ent(host, cx, None, &args) else {
                            continue;
                        };
                        let t = host.get_field(cx, id, team);
                        flags.push(DomFlag {
                            team: team_of(cx, t),
                            bounds: entity_abs_bounds(host, cx, id),
                        });
                    }
                    (!flags.is_empty()).then_some(UoObjectives::Dom(flags))
                }
                "hq" => {
                    let [list, hidden, team, radius] =
                        ["radio", "hidden", "team", "radius"].map(|n| f(cx, n));
                    let Value::Array(list) = cx.get_field(level, list) else {
                        return None;
                    };
                    let mut radios = Vec::new();
                    for i in 0..cx.array_len(list) {
                        let Some(r) = ent(cx.get_index(list, vcod_gsc::ArrayKey::Int(i as i32)))
                        else {
                            continue;
                        };
                        if host.ents.get(r).is_none() || truthy(host.get_field(cx, r, hidden)) {
                            continue;
                        }
                        let at = match host.get_field(cx, r, origin) {
                            Value::Vector(v) => v,
                            _ => continue,
                        };
                        let t = host.get_field(cx, r, team);
                        radios.push(HqRadio {
                            at,
                            team: team_of(cx, t),
                            radius: number(host.get_field(cx, r, radius)),
                        });
                    }
                    // The next radio's prespawn marker (`hq_obj_think`, 1792).
                    let next = host
                        .objectives
                        .first()
                        .filter(|o| o.state != 0 && radios.is_empty())
                        .map(|o| o.origin_f32());
                    Some(UoObjectives::Hq { radios, next })
                }
                "bas" => {
                    let [
                        list,
                        team,
                        health,
                        alive,
                        status,
                        objective,
                        model,
                        countdown,
                    ] = [
                        "bases",
                        "script_team",
                        "health",
                        "alive",
                        "status",
                        "objective",
                        "objectivemodel",
                        "countdowntime",
                    ]
                    .map(|n| f(cx, n));
                    let Value::Array(list) = cx.get_field(level, list) else {
                        return None;
                    };
                    let mut bases = Vec::new();
                    for i in 0..cx.array_len(list) {
                        let Some(b) = ent(cx.get_index(list, vcod_gsc::ArrayKey::Int(i as i32)))
                        else {
                            continue;
                        };
                        if host.ents.get(b).is_none() || !truthy(host.get_field(cx, b, alive)) {
                            continue;
                        }
                        let at = match host.get_field(cx, b, origin) {
                            Value::Vector(v) => v,
                            _ => continue,
                        };
                        let status = match host.get_field(cx, b, status) {
                            Value::String(a) => cx.resolve(a).to_string(),
                            _ => String::new(),
                        };
                        let trigger = ent(host.get_field(cx, b, objective))
                            .filter(|t| host.ents.get(*t).is_some());
                        let model = ent(host.get_field(cx, b, model))
                            .filter(|m| host.ents.get(*m).is_some())
                            .and_then(|m| match host.get_field(cx, m, origin) {
                                Value::Vector(v) => Some(v),
                                _ => None,
                            });
                        let t = host.get_field(cx, b, team);
                        let planted = status == "bomb_planted" || status == "bomb_defusing";
                        bases.push(BasBase {
                            team: team_of(cx, t),
                            at,
                            breached: number(host.get_field(cx, b, health)) <= 0.0,
                            planted,
                            blows_in: planted.then(|| number(host.get_field(cx, b, countdown))),
                            charge: trigger
                                .map(|t| entity_abs_bounds(host, cx, t))
                                .and_then(live)
                                .zip(model),
                        });
                    }
                    Some(UoObjectives::Bas(bases))
                }
                _ => None,
            }
        })
    }
}

/// [`ScriptRuntime::uo_objectives`]: one United Offensive gametype's state.
pub enum UoObjectives {
    /// The allied flag, then the axis one.
    Ctf(Vec<CtfFlag>),
    /// Each `flagN` trigger, in number order.
    Dom(Vec<DomFlag>),
    /// The radios up now, and the marker of the next one while none is.
    Hq {
        radios: Vec<HqRadio>,
        next: Option<[f32; 3]>,
    },
    /// Each `gmi_base` still alive.
    Bas(Vec<BasBase>),
}

/// One `ctf.gsc` flag (`ctf_spawn_flag`, `hold_flag`, `drop_flag`).
pub struct CtfFlag {
    /// Its owners; the other side takes it.
    pub team: i32,
    /// Its trigger while it stands at home.
    pub home: Option<Bounds>,
    /// The mobile trigger while it lies where its carrier fell.
    pub dropped: Option<Bounds>,
    /// The trigger a carrier of this flag delivers it to.
    pub goal: Bounds,
    pub carrier: Option<usize>,
}

/// One `dom.gsc` flag zone (`Flag_ZoneThink`).
pub struct DomFlag {
    /// Its owners, `TEAM_NONE` while neutral.
    pub team: i32,
    pub bounds: Bounds,
}

/// One `hq.gsc` radio on show (`hq_radio_think`).
pub struct HqRadio {
    pub at: [f32; 3],
    /// Its holders, `TEAM_NONE` while neutral.
    pub team: i32,
    /// The capture's reach across; `level.zradioradius` (50) is the height.
    pub radius: f32,
}

/// One `bas.gsc` base (`BaseAssault_base_think`).
pub struct BasBase {
    pub team: i32,
    pub at: [f32; 3],
    /// Its health is gone: a charge can be planted on it.
    pub breached: bool,
    /// A charge ticks on it.
    pub planted: bool,
    /// While planted, `countdowntime`: whole seconds before the charge
    /// blows (`BaseAssault_bomb_countdown` counts it down once a second).
    pub blows_in: Option<f32>,
    /// The plant trigger (a `trigger_lookat` or `trigger_use`) and the
    /// objective model the plant and defuse measure 150 units from.
    pub charge: Option<(Bounds, [f32; 3])>,
}

/// [`ScriptRuntime::re_objectives`]: the retrieval state a bot plays to.
pub struct ReObjectives {
    pub attackers: String,
    pub defenders: String,
    pub objectives: Vec<ReObjective>,
}

/// One of `level.retrieval_objective` still standing.
pub struct ReObjective {
    /// Its pickup `trigger_use`'s bounds, while it lies there to be taken;
    /// `None` once carried (`triggerOff` sends it 10000 units down).
    pub pickup: Option<Bounds>,
    /// The `trigger_multiple` a carrier delivers it to.
    pub goal: Bounds,
    /// The client its compass record follows.
    pub carrier: Option<usize>,
    /// The record's `teamNum`: 0 shows it to everyone, else to that team
    /// only. A carried one shows to the attackers alone unless
    /// `scr_re_showcarrier` is set (`re.gsc` `objective_think`, 1896-1911).
    pub shown_to: i32,
    /// The record's origin: where it last lay, which a carry leaves in
    /// place.
    pub laid: Option<[f32; 3]>,
    /// Lying dropped away from its start: the trigger's `startorigin`,
    /// where the 60 s timeout puts it back, and its `origin`.
    pub dropped: Option<([f32; 3], [f32; 3])>,
}

/// [`ScriptRuntime::sd_objectives`]: the S&D state a bot plays to.
pub struct SdObjectives {
    pub attackers: String,
    pub defenders: String,
    /// `(mins, maxs)` of each bombzone still standing.
    pub sites: Vec<Bounds>,
    /// The planted bomb's defuse trigger: its origin (what `distance` reads)
    /// and its bounds (what the aim trace enters).
    pub bomb: Option<([f32; 3], Bounds)>,
}

/// Absolute `(mins, maxs)`.
pub type Bounds = ([f32; 3], [f32; 3]);
