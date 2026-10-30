//! The compass's radar (docs/research/cod4-hud.md 15.3, 15.7, 15.9): the
//! UAV's sweep across the minimap, what the client last knew of each
//! player, and the helicopters and planes it saw. Renderer-free: the HUD
//! steps it once a frame and asks it what to draw.

/// `compassMaxRange` (`iw3mp.exe` 0x425416): the world distance the
/// compass spans, edge to edge.
pub(crate) const COMPASS_RANGE: f32 = 2500.0;
/// `compassRadarUpdateTime` 4 s, one sweep (0x4258f5's operand).
const SWEEP_MS: f32 = 4000.0;
/// `compassRadarPingFadeTime` 4 s (0x4258eb's operands).
const SWEEP_PING_FADE_MS: f32 = 4000.0;
/// `compassRadarLineThickness` 0.4 of the compass (0x42595a's operand).
const LINE_THICKNESS: f32 = 0.4;
/// `cg_hudMapRadarLineThickness` 0.15 of the full map (0x426295).
const MAP_LINE_THICKNESS: f32 = 0.15;
/// `compassSoundPingFadeTime` 2 s (the float at 0x700c0c, 0x425531): a
/// shot's or a talker's ping fades over it.
const SOUND_PING_FADE_MS: f32 = 2000.0;
/// How long a player stays on the compass past the last frame that placed
/// him (0x429a5c), and how recent a sighting the sweep needs to ping him.
const HEARD_MS: i32 = 1500;
/// The actor table at 0x742b20 holds one slot a client.
const MAX_CLIENTS: usize = 64;
/// `perks` bit 0, `specialty_gpsjammer`: the sweep never pings its owner.
const PERK_GPS_JAMMER: u32 = 1;
/// The spectator team.
const TEAM_SPECTATOR: u32 = 3;
/// `pingPlayer`'s flash runs 3 s (0x429728, 0x429867).
const FLASH_MS: i32 = 3000;
/// A talker's ping holds 50 ms past the frame (0x429711).
const TALK_HOLD_MS: i32 = 50;
/// The vehicle memory's slot count.
const VEHICLE_SLOTS: usize = 8;

/// The sweep texture's half size in S and T on the compass: half the map
/// over the line's thickness (cod4-hud.md 15.7).
pub(crate) const SWEEP_HALF: f32 = 0.5 / LINE_THICKNESS;

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

/// Where the minimap lies in the world (configstring 823's corners,
/// 822's north yaw; cod4-hud.md 6): the map's right is `east`, its down
/// `south`, and it spans `extent` along each from `corner`, its upper
/// left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct MapAxes {
    pub north_yaw: f32,
    pub east: [f32; 2],
    pub south: [f32; 2],
    pub corner: [f32; 2],
    pub extent: [f32; 2],
}

impl MapAxes {
    /// The map from its upper left and lower right corners. An axis the
    /// corners do not span reads 1000 units.
    pub(crate) fn new(north_yaw: f32, corner: [f32; 2], far: [f32; 2]) -> Self {
        let (sin, cos) = north_yaw.to_radians().sin_cos();
        let east = [sin, -cos];
        let south = [-cos, -sin];
        let diagonal = [far[0] - corner[0], far[1] - corner[1]];
        let span = |axis: [f32; 2]| {
            let d = dot(axis, diagonal);
            if d == 0.0 { 1000.0 } else { d }
        };
        MapAxes {
            north_yaw,
            east,
            south,
            corner,
            extent: [span(east), span(south)],
        }
    }

    /// A world point as a fraction of the map: 0 at the upper left
    /// corner, 1 at the far edge, along east then south.
    pub(crate) fn fraction(&self, p: [f32; 2]) -> [f32; 2] {
        let rel = [p[0] - self.corner[0], p[1] - self.corner[1]];
        [
            dot(self.east, rel) / self.extent[0],
            dot(self.south, rel) / self.extent[1],
        ]
    }
}

/// The sweep's line at one point of its pass. It runs north to south and
/// moves east, from a margin west of the map to a margin east of it: half
/// the wider of the compass's line and the full map's (cod4-hud.md 15.3).
#[derive(Clone, Copy, Debug)]
struct SweepLine {
    east: [f32; 2],
    /// The line's coordinate along `east`.
    at: f32,
}

impl SweepLine {
    fn new(map: &MapAxes, progress: f32) -> Self {
        let compass = COMPASS_RANGE * (std::f32::consts::SQRT_2 + LINE_THICKNESS);
        let full = MAP_LINE_THICKNESS * map.extent[0];
        let margin = 0.5 * compass.max(full);
        let travel = map.extent[0] + 2.0 * margin;
        SweepLine {
            east: map.east,
            at: dot(map.east, map.corner) - margin + progress * travel,
        }
    }

    /// How far east of the line `p` is.
    fn east_of(&self, p: [f32; 2]) -> f32 {
        dot(self.east, p) - self.at
    }

    fn west_of(&self, p: [f32; 2]) -> bool {
        self.east_of(p) < 0.0
    }
}

/// The sweep texture's S at the compass's centre: the view's distance
/// east of the line in compass widths, over the line's thickness, from
/// the texture's middle (cod4-hud.md 15.7). T is 0 there.
pub(crate) fn sweep_texture(map: &MapAxes, progress: f32, view: [f32; 2]) -> f32 {
    let line = SweepLine::new(map, progress);
    line.east_of(view) / COMPASS_RANGE / LINE_THICKNESS + 0.5
}

/// One client's state as the snapshot's client list gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Info {
    pub valid: bool,
    pub team: u32,
    pub perks: u32,
}

/// A player entity of the newest snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Body {
    pub client: usize,
    pub pos: [f32; 2],
    /// `eFlags` 0x20000: on a turret, which hides him from the compass.
    pub turret: bool,
    pub yaw: f32,
    /// `eFlags` 0x400000: `pingPlayer`'s flash.
    pub pinged: bool,
    pub talking: bool,
}

/// What one frame gives the radar.
pub(crate) struct Frame<'a> {
    pub time: i32,
    /// `ps.radarEnabled`: the UAV is up.
    pub radar: bool,
    pub map: Option<MapAxes>,
    /// Our own client number.
    pub me: usize,
    pub infos: &'a [Info; MAX_CLIENTS],
    pub bodies: &'a [Body],
    /// `ps.iCompassPlayerInfo`: one player out of the snapshot.
    pub compass_info: u32,
    /// `ps.eFlags` 0x800000: that player's `pingPlayer` flash.
    pub compass_pinged: bool,
    /// The view's origin.
    pub view: [f32; 2],
    /// The unsilenced shots this frame's events fired, in event order.
    pub shots: &'a [Shot],
}

/// A shot the compass hears (cod4-hud.md 15.7): another player's
/// unsilenced, non-grenade fire, or a turret's gunner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Shot {
    pub client: usize,
    /// Where it was heard: the shooter's, or the turret's, origin.
    pub pos: [f32; 2],
    /// How long its ping holds before fading: the fire sound's length on
    /// retail, 50 for a turret.
    pub hold_ms: i32,
}

/// What the client knows of one player (cod4-hud.md 15.9).
#[derive(Clone, Copy, Debug, Default)]
struct Actor {
    enemy: bool,
    perks: u32,
    /// The last place a frame put him, the frame's time and his yaw.
    pos: [f32; 2],
    heard_at: i32,
    yaw: f32,
    /// Where his enemy pings draw: the sweep's or his last shot's place.
    ping_pos: [f32; 2],
    /// When the sweep last pinged him.
    swept_at: i32,
    /// A shot's ping holds until this time, then fades.
    shot_until: i32,
    /// A talker's ping, likewise.
    talk_until: i32,
    /// `pingPlayer`'s flash runs until this time.
    flash_until: i32,
}

/// A teammate the compass draws: where and which way he last was, whether
/// the flash shows the chat icon this frame, and the alphas of his shot's
/// and voice's pings over his arrow.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Friendly {
    pub client: usize,
    pub pos: [f32; 2],
    pub yaw: f32,
    pub chat_icon: bool,
    pub firing: Option<f32>,
    pub voice: Option<f32>,
}

/// An enemy ping to draw: where, its alpha, and whether it is the shot's
/// flash (`compassping_enemyfiring`) or the sweep's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ping {
    pub client: usize,
    pub pos: [f32; 2],
    pub alpha: f32,
    pub firing: bool,
}

/// The sweep's place in its pass (0 west to 1 east) and every client's
/// actor.
#[derive(Clone, Debug)]
pub(crate) struct Radar {
    actors: [Actor; MAX_CLIENTS],
    progress: f32,
    last_time: Option<i32>,
}

impl Default for Radar {
    fn default() -> Self {
        Radar {
            actors: [Actor::default(); MAX_CLIENTS],
            progress: 0.0,
            last_time: None,
        }
    }
}

/// `1 - elapsed / over` once `start` is past, 1 before it.
fn fade(start: i32, time: i32, over: f32) -> f32 {
    if start < time {
        1.0 - (time - start) as f32 / over
    } else {
        1.0
    }
}

/// Whether a ping that started at `start` is still inside its fade.
fn fading(start: i32, time: i32, over: f32) -> bool {
    (time as f32) < start as f32 + over
}

/// `iCompassPlayerInfo`'s player (cod4-hud.md 15.3): the client in bits
/// 0..5, the x and y offsets from the view as `4 n - 1020` in bits 6..14
/// and 15..23, and the yaw as a signed top byte in 256ths of a turn.
fn decode_compass_info(word: u32, view: [f32; 2]) -> (usize, [f32; 2], f32) {
    let offset = |shift: u32| ((word >> shift) & 0x1ff) as f32 * 4.0 - 1020.0;
    let yaw = (word as i32 >> 24) as f32 * (360.0 / 256.0);
    let pos = [view[0] + offset(6), view[1] + offset(15)];
    ((word & 0x3f) as usize, pos, yaw)
}

impl Actor {
    /// Seconds-old sightings count; one stamped past `time` (the clock
    /// went back) is forgotten.
    fn heard(&self, time: i32) -> i32 {
        if self.heard_at > time {
            0
        } else {
            self.heard_at
        }
    }

    fn learn(&mut self, info: Info, my_team: u32) {
        self.enemy = my_team == 0 || info.team != my_team;
        self.perks = info.perks;
    }

    /// The sweep passes him: seen within 1.5 s and unjammed, his ping
    /// starts where he was.
    fn sweep_ping(&mut self, time: i32) {
        self.heard_at = self.heard(time);
        let jammed = self.perks & PERK_GPS_JAMMER != 0;
        if !jammed && self.heard_at >= time - HEARD_MS {
            self.ping_pos = self.pos;
            self.swept_at = time;
        }
    }

    /// He moves to `to`. With the radar up, an enemy heard from lately
    /// whose step crosses the line is pinged where he stood (the footstep
    /// ping is off by default: 0x426263).
    fn move_to(&mut self, to: [f32; 2], time: i32, line: Option<&SweepLine>) {
        let crossed = line.is_some_and(|l| l.west_of(self.pos) != l.west_of(to));
        if self.enemy && crossed && self.heard_at > time - HEARD_MS {
            self.sweep_ping(time);
        }
        self.pos = to;
    }

    /// He fired from `at`; the shot's ping holds until `until`. An enemy
    /// moves there and his pings draw there.
    fn fired(&mut self, at: [f32; 2], time: i32, until: i32, line: Option<&SweepLine>) {
        if self.enemy {
            self.move_to(at, time, line);
            self.ping_pos = at;
        }
        self.shot_until = until;
    }

    /// A frame placed him: where, which way, and `pingPlayer`'s flash,
    /// which a running flash does not restart (0x429717..0x42972d).
    fn place(&mut self, time: i32, at: [f32; 2], yaw: f32, flash: bool, line: Option<&SweepLine>) {
        self.move_to(at, time, line);
        self.heard_at = time;
        self.yaw = yaw;
        if flash && self.flash_until <= time {
            self.flash_until = time + FLASH_MS;
        }
    }
}

impl Radar {
    /// One frame, in the order the client runs it: the sweep moves on by
    /// the frame's time and pings what it passes, the frame's shots are
    /// heard, then the snapshot's players and `iCompassPlayerInfo` place
    /// their actors. Without a minimap only the clock moves.
    pub(crate) fn step(&mut self, f: &Frame) {
        let elapsed = f.time - self.last_time.unwrap_or(f.time);
        self.last_time = Some(f.time);
        let Some(map) = f.map else {
            return;
        };
        let mine = f.infos[f.me.min(MAX_CLIENTS - 1)];
        let my_team = mine.team;
        self.advance_sweep(f, &map, elapsed, my_team);
        let line = f.radar.then(|| SweepLine::new(&map, self.progress));
        let line = line.as_ref();
        for shot in f.shots {
            self.hear(f, shot, line, my_team);
        }
        if mine.valid && mine.team != TEAM_SPECTATOR {
            self.see(f, line, my_team);
        }
    }

    /// The sweep's move this frame. A pass that wraps pings nothing.
    fn advance_sweep(&mut self, f: &Frame, map: &MapAxes, elapsed: i32, my_team: u32) {
        if !f.radar {
            self.progress = 0.0;
            return;
        }
        let from = self.progress;
        self.progress = (from + elapsed.max(0) as f32 / SWEEP_MS).fract();
        if self.progress < from {
            return;
        }
        let (a, b) = (
            SweepLine::new(map, from),
            SweepLine::new(map, self.progress),
        );
        for (n, actor) in self.actors.iter_mut().enumerate() {
            let info = f.infos[n];
            if !info.valid {
                continue;
            }
            actor.learn(info, my_team);
            let on_turret = f.bodies.iter().any(|b| b.client == n && b.turret);
            if actor.enemy && !on_turret && a.west_of(actor.pos) != b.west_of(actor.pos) {
                actor.sweep_ping(f.time);
            }
        }
    }

    /// A shot from anyone but us or a spectator holds its ping, and an
    /// enemy's gives him away ([`Actor::fired`]).
    fn hear(&mut self, f: &Frame, shot: &Shot, line: Option<&SweepLine>, my_team: u32) {
        let shooter = (shot.client < MAX_CLIENTS && shot.client != f.me)
            .then(|| f.infos[shot.client])
            .filter(|i| i.team != TEAM_SPECTATOR);
        if let Some(info) = shooter {
            let actor = &mut self.actors[shot.client];
            actor.learn(info, my_team);
            actor.fired(shot.pos, f.time, f.time + shot.hold_ms, line);
        }
    }

    /// The snapshot's players (0x429660..0x429870), then the one
    /// `iCompassPlayerInfo` names.
    fn see(&mut self, f: &Frame, line: Option<&SweepLine>, my_team: u32) {
        for b in f.bodies {
            let Some(&info) = f.infos.get(b.client).filter(|i| i.valid) else {
                continue;
            };
            let actor = &mut self.actors[b.client];
            actor.learn(info, my_team);
            if b.turret {
                continue;
            }
            actor.place(f.time, b.pos, b.yaw, b.pinged, line);
            if b.talking {
                actor.talk_until = f.time + TALK_HOLD_MS;
            }
        }
        if f.compass_info != 0 {
            let (n, pos, yaw) = decode_compass_info(f.compass_info, f.view);
            let actor = &mut self.actors[n];
            actor.learn(f.infos[n], my_team);
            actor.place(f.time, pos, yaw, f.compass_pinged, line);
        }
    }

    /// Stretches the ping of the shot [`Self::step`] heard from `client`
    /// this frame to `ms`, the length of the fire sound the audio started.
    pub(crate) fn hold_shot(&mut self, client: usize, ms: i32) {
        let now = self.time();
        if let Some(actor) = self.actors.get_mut(client)
            && actor.shot_until == now
        {
            actor.shot_until = now + ms.max(0);
        }
    }

    /// The teammates to draw at `time` (0x429880): each placed in the
    /// last 1.5 s, not us. The flash shows the chat icon for the second
    /// half of every 500 ms (0x429c59); shot and voice pings fade over
    /// 2 s.
    pub(crate) fn friendlies(&self, time: i32, me: usize) -> Vec<Friendly> {
        let sound = |start: i32| {
            (time as f32 <= start as f32 + SOUND_PING_FADE_MS)
                .then(|| fade(start, time, SOUND_PING_FADE_MS))
        };
        self.actors
            .iter()
            .enumerate()
            .filter(|&(n, a)| !a.enemy && n != me && a.heard(time) >= time - HEARD_MS)
            .map(|(n, a)| {
                let flash_left = a.flash_until - time;
                Friendly {
                    client: n,
                    pos: a.pos,
                    yaw: a.yaw,
                    chat_icon: flash_left > 0 && flash_left % 500 >= 250,
                    firing: sound(a.shot_until),
                    voice: sound(a.talk_until),
                }
            })
            .collect()
    }

    /// The last [`Self::step`]'s time.
    pub(crate) fn time(&self) -> i32 {
        self.last_time.unwrap_or(0)
    }

    /// The sweep's progress at `time`, run on from the last step: the HUD
    /// steps on snapshot time and draws on render time, so the line moves
    /// every frame.
    pub(crate) fn progress_at(&self, time: i32) -> f32 {
        let ahead = (time - self.last_time.unwrap_or(time)) as f32 / SWEEP_MS;
        (self.progress + ahead).rem_euclid(1.0)
    }

    /// The enemy pings to draw at `time` (cod4-hud.md 15.3, 15.7): the
    /// sweep's where it last saw each, fading over 4 s, and over it a
    /// shot's, held then faded over 2 s. With `g_compassShowEnemies`
    /// every enemy placed in the last 1.5 s draws where he is, the sweep's
    /// ping at full alpha. A ping whose alpha went negative is not drawn.
    pub(crate) fn pings(&self, time: i32, show_all: bool) -> Vec<Ping> {
        let enemies = self.actors.iter().enumerate().filter(|(_, a)| a.enemy);
        enemies
            .flat_map(|(client, a)| {
                let shot_live = a.shot_until != 0 && fading(a.shot_until, time, SOUND_PING_FADE_MS);
                let (pos, sweep_alpha, drawn) = if show_all {
                    (a.pos, 1.0, a.heard(time) >= time - HEARD_MS)
                } else {
                    let swept_live =
                        a.swept_at != 0 && fading(a.swept_at, time, SWEEP_PING_FADE_MS);
                    let alpha = fade(a.swept_at, time, SWEEP_PING_FADE_MS);
                    (a.ping_pos, alpha, swept_live || shot_live)
                };
                // The sweep's ping first, the shot's over it.
                let layers = [
                    (sweep_alpha, false),
                    (fade(a.shot_until, time, SOUND_PING_FADE_MS), true),
                ];
                layers
                    .into_iter()
                    .filter(move |&(alpha, _)| drawn && alpha >= 0.0)
                    .map(move |(alpha, firing)| Ping {
                        client,
                        pos,
                        alpha,
                        firing,
                    })
            })
            .collect()
    }
}

/// A helicopter or plane the compass remembers (cod4-hud.md 15.7).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Vehicle {
    pub entity: u32,
    pub etype: u32,
    pub last_update: i32,
    pub pos: [f32; 2],
    pub yaw: f32,
    /// `teamAndOwnerIndex`'s team (bits 0..1) and owner (from bit 2).
    pub team: u32,
    pub owner: u32,
}

/// The last eight vehicles seen, each drawn for 1.5 s past its last
/// sighting (cod4-hud.md 15.7).
#[derive(Clone, Debug, Default)]
pub(crate) struct Vehicles([Vehicle; VEHICLE_SLOTS]);

impl Vehicles {
    /// A sighting at `time` goes in the vehicle's own slot, else the first
    /// one never used, else over the one seen longest ago.
    pub(crate) fn seen(&mut self, time: i32, v: Vehicle) {
        let slots = &mut self.0;
        let own = slots.iter().position(|s| s.entity == v.entity);
        let unused = || slots.iter().position(|s| s.last_update == 0);
        let stalest = || {
            (0..VEHICLE_SLOTS)
                .min_by_key(|&k| (slots[k].last_update, k))
                .unwrap_or(0)
        };
        let k = own.or_else(unused).unwrap_or_else(stalest);
        slots[k] = Vehicle {
            last_update: time,
            ..v
        };
    }

    /// The vehicles of `etype` to draw at `time`.
    pub(crate) fn drawn(&self, time: i32, etype: u32) -> Vec<Vehicle> {
        let recent = (time - HEARD_MS)..=time;
        self.0
            .iter()
            .filter(|v| v.etype == etype && recent.contains(&v.last_update))
            .copied()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame<'a>(
        time: i32,
        radar: bool,
        infos: &'a [Info; MAX_CLIENTS],
        bodies: &'a [Body],
    ) -> Frame<'a> {
        Frame {
            time,
            radar,
            map: Some(MapAxes::new(90.0, [-2000.0, 2000.0], [2000.0, -2000.0])),
            me: 0,
            infos,
            bodies,
            compass_info: 0,
            compass_pinged: false,
            view: [0.0, 0.0],
            shots: &[],
        }
    }

    /// An enemy standing still is pinged once a sweep, 4 s apart, and
    /// fades out over the 4 s between; a teammate never is; one with the
    /// GPS jammer never is; with the radar off nothing is.
    #[test]
    fn the_sweep_pings_a_standing_enemy_once_a_pass() {
        let mut infos = [Info::default(); MAX_CLIENTS];
        infos[0] = Info {
            valid: true,
            team: 2,
            perks: 0,
        };
        infos[1] = Info {
            valid: true,
            team: 1,
            perks: 0,
        };
        infos[2] = Info {
            valid: true,
            team: 2,
            perks: 0,
        };
        infos[3] = Info {
            valid: true,
            team: 1,
            perks: PERK_GPS_JAMMER,
        };
        let bodies = [
            Body {
                client: 1,
                pos: [500.0, 300.0],
                turret: false,
                yaw: 0.0,
                pinged: false,
                talking: false,
            },
            Body {
                client: 2,
                pos: [-500.0, 300.0],
                turret: false,
                yaw: 0.0,
                pinged: false,
                talking: false,
            },
            Body {
                client: 3,
                pos: [100.0, 100.0],
                turret: false,
                yaw: 0.0,
                pinged: false,
                talking: false,
            },
        ];
        let mut r = Radar::default();
        let mut pinged = Vec::new();
        for t in (0..=20_000).step_by(16) {
            r.step(&frame(t, t >= 1000, &infos, &bodies));
            let p = r.pings(t, false);
            assert!(p.iter().all(|p| p.client == 1), "{p:?}");
            if let Some(p) = p.first()
                && p.alpha == 1.0
            {
                pinged.push(t);
            }
        }
        assert!(pinged.len() >= 4, "{pinged:?}");
        for w in pinged.windows(2) {
            assert!((3980..=4020).contains(&(w[1] - w[0])), "{pinged:?}");
        }
        let mut off = Radar::default();
        for t in (0..=10_000).step_by(16) {
            off.step(&frame(t, false, &infos, &bodies));
            assert!(off.pings(t, false).is_empty());
        }
    }

    /// A shot flashes the enemy with no UAV up, held then faded over 2 s,
    /// and past a level's first 4 s never the sweep's ping (drawn at
    /// `1 - time / 4000` while no sweep has pinged); a teammate's shot
    /// fades the same way.
    #[test]
    fn a_shot_flashes_and_fades_over_two_seconds() {
        let mut infos = [Info::default(); MAX_CLIENTS];
        for (n, team) in [(0, 2), (1, 1), (2, 2)] {
            infos[n] = Info {
                valid: true,
                team,
                perks: 0,
            };
        }
        let mut r = Radar::default();
        r.step(&frame(10_000, false, &infos, &[]));
        let shots = [
            Shot {
                client: 1,
                pos: [100.0, 0.0],
                hold_ms: 0,
            },
            Shot {
                client: 2,
                pos: [0.0, 100.0],
                hold_ms: 0,
            },
        ];
        let mate = [Body {
            client: 2,
            pos: [0.0, 100.0],
            turret: false,
            yaw: 0.0,
            pinged: false,
            talking: false,
        }];
        r.step(&Frame {
            shots: &shots,
            ..frame(11_000, false, &infos, &mate)
        });
        let p = r.pings(11_000, false);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].firing && p[0].client == 1 && p[0].alpha == 1.0);
        assert_eq!(p[0].pos, [100.0, 0.0]);
        let half = r.pings(12_000, false);
        assert!((half[0].alpha - 0.5).abs() < 1e-3, "{half:?}");
        assert!(r.pings(13_001, false).is_empty());
        let f = r.friendlies(12_000, 0);
        assert_eq!((f.len(), f[0].client, f[0].firing), (1, 2, Some(0.5)));
        assert!(r.friendlies(12_501, 0).is_empty(), "1.5 s past his last");
    }

    /// The sweep's texture centre on the compass: the view's distance east
    /// of the line over `compassMaxRange`, over the 0.4 thickness, from
    /// 0.5; radius 1.25 (cod4-hud.md 15.7).
    #[test]
    fn the_sweep_texture_sits_at_the_views_distance_from_the_line() {
        let m = MapAxes::new(20.0, [-2000.0, 2500.0], [3000.0, -1500.0]);
        let want = [
            5.801039, 3.3060176, 7.5957823, 1.7992766, -0.6957449, 3.5940197, -3.2029264,
            -5.697948, -1.4081833,
        ];
        assert_eq!(SWEEP_HALF, 1.25);
        let mut k = 0;
        for p in [0.0, 0.4, 0.9] {
            for v in [[0.0, 0.0], [-1800.0, 2000.0], [2500.0, -1000.0]] {
                let s = sweep_texture(&m, p, v);
                assert!((s - want[k]).abs() < 1e-3, "{p} {v:?}: {s}");
                k += 1;
            }
        }
    }

    /// `iCompassPlayerInfo`: client 5, offsets `4 * n - 1020` off the
    /// view, the top byte a signed yaw in 256ths of a turn; a teammate it
    /// names draws there 1.5 s.
    #[test]
    fn compass_player_info_places_its_player() {
        let mut infos = [Info::default(); MAX_CLIENTS];
        for n in [0, 5] {
            infos[n] = Info {
                valid: true,
                team: 1,
                perks: 0,
            };
        }
        let mut r = Radar::default();
        let w = 5 | (300 << 6) | (200 << 15) | (0xc0 << 24);
        r.step(&Frame {
            compass_info: w,
            view: [100.0, 100.0],
            ..frame(5000, false, &infos, &[])
        });
        let f = r.friendlies(5000, 0);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(
            (f[0].client, f[0].pos, f[0].yaw),
            (5, [280.0, -120.0], -90.0)
        );
        assert!(r.friendlies(6501, 0).is_empty());
    }

    /// An enemy who walks across the line while it stands still is pinged
    /// where he stood before the step, then fades over 4 s; without a geometry nothing
    /// is tracked.
    #[test]
    fn an_enemy_walking_across_the_line_is_pinged() {
        let mut infos = [Info::default(); MAX_CLIENTS];
        infos[0] = Info {
            valid: true,
            team: 2,
            perks: 0,
        };
        infos[1] = Info {
            valid: true,
            team: 1,
            perks: 0,
        };
        let body = |x: f32| Body {
            client: 1,
            pos: [x, 0.0],
            turret: false,
            yaw: 0.0,
            pinged: false,
            talking: false,
        };
        let mut r = Radar::default();
        // The line starts 2649 west of the map's west edge (margin half
        // of 2500 * (sqrt 2 + 0.4)), the edge at -2000.
        let start = -2000.0 - 0.5 * 2500.0 * (std::f32::consts::SQRT_2 + 0.4);
        r.step(&frame(4000, true, &infos, &[body(start - 10.0)]));
        assert!(r.pings(4000, false).is_empty());
        r.step(&frame(4000, true, &infos, &[body(start + 10.0)]));
        let p = r.pings(4000, false);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!((p[0].pos, p[0].alpha), ([start - 10.0, 0.0], 1.0));
        let half = r.pings(6000, false);
        assert!((half[0].alpha - 0.5).abs() < 1e-3, "{half:?}");
        let mut none = Radar::default();
        none.step(&Frame {
            map: None,
            ..frame(4000, true, &infos, &[body(0.0)])
        });
        assert!(none.friendlies(4000, 1).is_empty() && none.pings(4000, true).is_empty());
    }

    /// Eight vehicle slots, each drawn 1.5 s past its last sighting; a
    /// ninth takes the oldest's slot.
    #[test]
    fn vehicles_draw_for_a_second_and_a_half_past_their_sighting() {
        let mut v = Vehicles::default();
        for n in 0..9u32 {
            v.seen(
                100 + n as i32,
                Vehicle {
                    entity: 100 + n,
                    etype: 13,
                    ..Vehicle::default()
                },
            );
        }
        let drawn = v.drawn(200, 13);
        assert_eq!(drawn.len(), 8);
        assert!(drawn.iter().all(|d| d.entity != 100), "{drawn:?}");
        assert!(v.drawn(200, 12).is_empty());
        assert_eq!(v.drawn(1608, 13).len(), 1);
        assert!(v.drawn(1609, 13).is_empty());
    }
}
