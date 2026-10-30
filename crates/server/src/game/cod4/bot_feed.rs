//! What the bots (`crate::bots::cod4`) and their measuring runs read of a
//! level beside its playerstates: the weapon defs, the hits scored and the
//! script's `game[]`, and the stock objective scripts' state
//! ([`Cod4Level::objectives`]).

use vcod_cod4::pmove::weapon::WeaponDef;
use vcod_gsc::{ArrayKey, Cx, EntId, Host, Target, Value};

use crate::game::host::GameHost;
use crate::game::script::{Bounds, TEAM_ALLIES, TEAM_AXIS};
use crate::game::trigger::entity_abs_bounds;

use super::level::Cod4Level;

impl Cod4Level {
    /// The level's weapon defs by number, as pmove and the shots read them.
    pub fn weapon_defs(&self) -> &[WeaponDef] {
        &self.combat.defs
    }

    /// The bullet and melee hits scored from the `from`th on, as
    /// `(attacker, victim, hit location)`.
    pub fn hits_since(&self, from: usize) -> &[(usize, usize, u8)] {
        self.combat.hits.get(from..).unwrap_or(&[])
    }

    /// `game[key]` as text, `""` unset: `state` reads `playing` and
    /// `postgame` once `endGame` ran, `roundsplayed` the rounds done
    /// (`_globallogic.gsc`).
    pub fn game_value(&mut self, key: &str) -> String {
        self.rt.with_host(|_, cx| {
            let k = ArrayKey::Str(cx.intern_exact(key));
            match cx.get_index(cx.game(), k) {
                Value::String(s) => cx.resolve(s).to_string(),
                Value::Int(i) => i.to_string(),
                Value::Float(f) => f.to_string(),
                _ => String::new(),
            }
        })
    }
}

/// What the bots read of a CoD4 level's stock objective scripts once a
/// tick (`_gameobjects.gsc` objects: docs/research/bot-objectives.md 12).
#[derive(Clone, Debug)]
pub enum Cod4Objectives {
    /// `sd` and `sab`: the bomb zones, the carried bomb, the planted one.
    Bomb(BombGame),
    /// `dom`'s flags, `koth`'s radio: zones taken by standing in them.
    Zones(Vec<Zone>),
}

/// `sd.gsc`'s and `sab.gsc`'s state.
#[derive(Clone, Debug, Default)]
pub struct BombGame {
    /// Each bomb zone still usable and the team that plants at it.
    pub zones: Vec<(i32, Bounds)>,
    /// The client carrying the bomb.
    pub carrier: Option<usize>,
    /// Where the bomb lies while nobody carries it and it may be taken, and
    /// the team that may take it (0: either).
    pub lying: Option<([f32; 3], i32)>,
    pub planted: Option<Planted>,
    /// `level.bombExploded`: the planted bomb's timer ran out.
    pub exploded: bool,
    /// `level.plantTime` and `level.defuseTime`, ms.
    pub plant_ms: i32,
    pub defuse_ms: i32,
}

/// The bomb down: the team that planted it, where it lies and the
/// bounds of the trigger its defuse is held in.
#[derive(Clone, Copy, Debug)]
pub struct Planted {
    pub by: i32,
    pub at: [f32; 3],
    pub zone: Bounds,
}

/// A `dom` flag or the `koth` radio: its owners (0 neutral), the trigger,
/// and whether it can be taken now (a radio not yet up cannot).
#[derive(Clone, Copy, Debug)]
pub struct Zone {
    pub team: i32,
    pub bounds: Bounds,
    pub live: bool,
    /// Both teams stand in it (`numTouching`), so `updateUseRate` holds
    /// its capture: a player in it sees his bar stop.
    pub contested: bool,
}

/// A trigger `_gameobjects::updateTrigger` sent 50000 down (no team may
/// use it) is not there to touch.
const GONE_Z: f32 = -5000.0;

/// `"axis"` and `"allies"` as the team numbers; anything else 0.
fn team_of(s: &str) -> i32 {
    match s {
        "axis" => TEAM_AXIS,
        "allies" => TEAM_ALLIES,
        _ => 0,
    }
}

fn other(team: i32) -> i32 {
    match team {
        TEAM_AXIS => TEAM_ALLIES,
        TEAM_ALLIES => TEAM_AXIS,
        _ => 0,
    }
}

/// One frame's reads of the script's structs and entities.
struct Reader<'a, 'b> {
    host: &'a mut GameHost,
    cx: &'a mut Cx<'b>,
}

impl Reader<'_, '_> {
    fn field(&mut self, of: Value, name: &str) -> Value {
        let f = self.cx.intern_folded(name);
        match of {
            Value::Struct(s) => self.cx.get_field(s, f),
            Value::Entity(e) => self.host.get_field(self.cx, e, f),
            _ => Value::Undefined,
        }
    }

    fn level(&mut self, name: &str) -> Value {
        let Target::Struct(level) = self.cx.level() else {
            return Value::Undefined;
        };
        self.field(Value::Struct(level), name)
    }

    fn text(&mut self, v: Value) -> String {
        match v {
            Value::String(a) => self.cx.resolve(a).to_string(),
            _ => String::new(),
        }
    }

    fn index(&mut self, of: Value, key: &str) -> Value {
        let Value::Array(a) = of else {
            return Value::Undefined;
        };
        let k = ArrayKey::Str(self.cx.intern_exact(key));
        self.cx.get_index(a, k)
    }

    fn items(&mut self, of: Value) -> Vec<Value> {
        let Value::Array(a) = of else {
            return Vec::new();
        };
        (0..self.cx.array_len(a))
            .map(|i| self.cx.get_index(a, ArrayKey::Int(i as i32)))
            .collect()
    }

    fn vector(&mut self, of: Value, name: &str) -> Option<[f32; 3]> {
        match self.field(of, name) {
            Value::Vector(v) => Some(v),
            _ => None,
        }
    }

    /// A game object's trigger bounds, where its trigger stands now, and
    /// whether it is up (not sent down by `updateTrigger`). A round trigger
    /// keeps the square inside its circle, so a body standing in the box
    /// stands in the trigger.
    fn trigger(&mut self, object: Value) -> Option<(Bounds, bool)> {
        let Value::Entity(id) = self.field(object, "trigger") else {
            return None;
        };
        let (mut lo, mut hi) = entity_abs_bounds(self.host, self.cx, id);
        if let Some(r) = self.host.triggers.get(id).and_then(|t| t.round) {
            let at = self.vector(Value::Entity(id), "origin")?;
            let half = r.radius * std::f32::consts::FRAC_1_SQRT_2;
            for i in 0..2 {
                lo[i] = at[i] - half;
                hi[i] = at[i] + half;
            }
        }
        Some(((lo, hi), hi[2] > GONE_Z))
    }

    /// The client whose entity `v` is.
    fn slot(&self, v: Value, clients: &[(usize, EntId)]) -> Option<usize> {
        let Value::Entity(e) = v else { return None };
        clients.iter().find(|c| c.1 == e).map(|c| c.0)
    }

    fn ms(&mut self, name: &str) -> i32 {
        match self.level(name) {
            Value::Float(f) => (f * 1000.0) as i32,
            Value::Int(i) => i * 1000,
            _ => 0,
        }
    }

    fn truthy(&mut self, name: &str) -> bool {
        matches!(self.level(name), Value::Int(i) if i != 0)
    }

    /// A carry object's carrier, and where it lies when nobody carries it
    /// and its `interactTeam` lets a team take it.
    fn carry(
        &mut self,
        bomb: Value,
        clients: &[(usize, EntId)],
    ) -> (Option<usize>, Option<([f32; 3], i32)>) {
        let carrier = {
            let c = self.field(bomb, "carrier");
            self.slot(c, clients)
        };
        let interact = {
            let v = self.field(bomb, "interactTeam");
            self.text(v)
        };
        let owner = {
            let v = self.field(bomb, "ownerTeam");
            self.text(v)
        };
        let lying = match (carrier, interact.as_str()) {
            (None, "any") => self.vector(bomb, "curOrigin").map(|at| (at, 0)),
            (None, "friendly") => self
                .vector(bomb, "curOrigin")
                .map(|at| (at, team_of(&owner))),
            _ => None,
        };
        (carrier, lying)
    }

    /// `sd.gsc`: `level.bombZones` planted at by `game["attackers"]`,
    /// `level.sdBomb`, and once planted `level.sdBombModel` with the zone's
    /// `bombDefuseTrig` moved onto it.
    fn sd(&mut self, clients: &[(usize, EntId)]) -> BombGame {
        let attackers = {
            let game = Value::Array(self.cx.game());
            let a = self.index(game, "attackers");
            team_of(&self.text(a))
        };
        let zones_v = self.level("bombZones");
        let zones = self.items(zones_v);
        let mut g = BombGame {
            plant_ms: self.ms("plantTime"),
            defuse_ms: self.ms("defuseTime"),
            exploded: self.truthy("bombExploded"),
            ..Default::default()
        };
        for &z in &zones {
            if let Some((b, true)) = self.trigger(z) {
                g.zones.push((attackers, b));
            }
        }
        let bomb = self.level("sdBomb");
        if matches!(bomb, Value::Struct(_)) {
            (g.carrier, g.lying) = self.carry(bomb, clients);
        }
        if self.truthy("bombPlanted") && !self.truthy("bombExploded") && !self.truthy("bombDefused")
        {
            let model = self.level("sdBombModel");
            let at = self.vector(model, "origin");
            let defuse = zones.iter().find_map(|&z| {
                let Value::Entity(id) = self.field(z, "bombDefuseTrig") else {
                    return None;
                };
                let b = entity_abs_bounds(self.host, self.cx, id);
                (b.1[2] > GONE_Z).then_some(b)
            });
            if let (Some(at), Some(zone)) = (at, defuse) {
                g.planted = Some(Planted {
                    by: attackers,
                    at,
                    zone,
                });
            }
        }
        g
    }

    /// `sab.gsc`: `level.bombZones[team]` is `team`'s own, planted at by
    /// the other; `level.sabBomb` is neutral until a side takes it; once
    /// `level.bombPlanted` by `level.bombPlantedBy` the defuse is held in
    /// the same zone, the bomb lying where it went down.
    fn sab(&mut self, clients: &[(usize, EntId)]) -> BombGame {
        let mut g = BombGame {
            plant_ms: self.ms("plantTime"),
            defuse_ms: self.ms("defuseTime"),
            exploded: self.truthy("bombExploded"),
            ..Default::default()
        };
        let zones_v = self.level("bombZones");
        let mut own = Vec::new();
        for team in ["allies", "axis"] {
            let z = self.index(zones_v, team);
            if let Some((b, live)) = self.trigger(z) {
                own.push((team_of(team), b));
                if live {
                    g.zones.push((other(team_of(team)), b));
                }
            }
        }
        let bomb = self.level("sabBomb");
        if matches!(bomb, Value::Struct(_)) {
            (g.carrier, g.lying) = self.carry(bomb, clients);
        }
        if self.truthy("bombPlanted") && !self.truthy("bombExploded") {
            let by_v = self.level("bombPlantedBy");
            let by = team_of(&self.text(by_v));
            let at = self.vector(bomb, "curOrigin");
            let zone = own.iter().find(|o| o.0 == other(by)).map(|o| o.1);
            if let (Some(at), Some(zone)) = (at, zone) {
                g.planted = Some(Planted { by, at, zone });
                g.zones.clear();
            }
        }
        g
    }

    /// `_gameobjects.gsc` `updateUseRate`'s stop: someone of each team
    /// touching the object's trigger.
    fn contested(&mut self, object: Value) -> bool {
        let touching = self.field(object, "numTouching");
        ["axis", "allies"]
            .iter()
            .all(|t| matches!(self.index(touching, t), Value::Int(n) if n > 0))
    }

    fn zone(&mut self, object: Value) -> Option<Zone> {
        let owner = {
            let v = self.field(object, "ownerTeam");
            team_of(&self.text(v))
        };
        let ((lo, hi), live) = self.trigger(object)?;
        // A trigger sent down keeps its extent about where it will stand.
        let bounds = if live {
            (lo, hi)
        } else {
            let Value::Entity(id) = self.field(object, "trigger") else {
                return None;
            };
            let at = self.vector(Value::Entity(id), "origin")?;
            let home = self.vector(object, "curOrigin")?;
            (
                std::array::from_fn(|i| lo[i] - at[i] + home[i]),
                std::array::from_fn(|i| hi[i] - at[i] + home[i]),
            )
        };
        let contested = self.contested(object);
        Some(Zone {
            team: owner,
            bounds,
            live,
            contested,
        })
    }
}

impl Cod4Level {
    /// The stock objective scripts' state for the level's gametype: `sd`,
    /// `sab`, `dom` (`level.domFlags`) and `koth` (`level.radioObject`, a
    /// radio not yet up read as not live); `None` for the others.
    pub fn objectives(&mut self) -> Option<Cod4Objectives> {
        let gametype = self.rt.cvars().get("g_gametype").to_string();
        let clients: Vec<(usize, EntId)> = (0..self.players.len())
            .filter_map(|s| Some((s, self.rt.client_entity(s)?)))
            .collect();
        self.rt.with_host(|host, cx| {
            let mut r = Reader { host, cx };
            match gametype.as_str() {
                "sd" => Some(Cod4Objectives::Bomb(r.sd(&clients))),
                "sab" => Some(Cod4Objectives::Bomb(r.sab(&clients))),
                "dom" => {
                    let flags = r.level("domFlags");
                    let flags = r.items(flags);
                    Some(Cod4Objectives::Zones(
                        flags.into_iter().filter_map(|f| r.zone(f)).collect(),
                    ))
                }
                "koth" => {
                    let radio = r.level("radioObject");
                    let zones = if matches!(radio, Value::Struct(_)) {
                        r.zone(radio).into_iter().collect()
                    } else {
                        Vec::new()
                    };
                    Some(Cod4Objectives::Zones(zones))
                }
                _ => None,
            }
        })
    }
}
