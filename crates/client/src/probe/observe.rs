//! The print-only modes: `--probe-follow`, `--probe-fall`, `--probe-compass`, `--probe-items`,
//! `--probe-ride`, `--probe-killcam`.

use super::hit::TARGET_USE_RETRY;
use super::*;

/// The follow probe's presses, from the moment it goes active: when, how long
/// the bits are held, and what they are. Retail's `SpectatorThink`
/// (`game.mp.i386.so` 0x3fab8) cycles forward on an attack press, backward on
/// a melee press, and stops a free follow on either edge of the sight bit.
const FOLLOW_SCRIPT: &[(u64, u64, u8, &str)] = &[
    (8_000, 200, net::msg::BUTTON_ATTACK, "attack"),
    (14_000, 200, net::msg::BUTTON_ATTACK, "attack"),
    (20_000, 200, net::msg::BUTTON_MELEE, "melee"),
    (26_000, 2_000, net::msg::BUTTON_ADS, "ads"),
    (32_000, 200, net::msg::BUTTON_ATTACK, "attack"),
];

/// `--probe-follow`: stays a spectator, presses [`FOLLOW_SCRIPT`] and prints
/// every snapshot whose `clientNum`, `pm_type`, `pm_flags`, `eFlags`,
/// `health` or `weapon` moved, and the [`FOLLOW_BURST`] snapshots after each
/// such change, with the origin, velocity, view and the player entities the
/// snapshot carried. Writes no fixture.
#[derive(Default)]
pub(super) struct FollowProbe {
    active_at: Option<Instant>,
    announced: usize,
    last: Option<(i32, i32, i32, i32, i32, i32)>,
    burst: usize,
}

/// How many snapshots the follow probe prints after a change, so what a
/// frame keeps of the one before it is in the log beside the change.
const FOLLOW_BURST: usize = 6;

impl FollowProbe {
    pub(super) fn cmd(&mut self, now: Instant) -> net::msg::UserCmd {
        let t0 = *self.active_at.get_or_insert(now);
        let ms = now.duration_since(t0).as_millis() as u64;
        let mut cmd = net::msg::UserCmd::default();
        for (i, (at, hold, bits, name)) in FOLLOW_SCRIPT.iter().enumerate() {
            if (*at..at + hold).contains(&ms) {
                cmd.buttons |= bits;
                if i >= self.announced {
                    self.announced = i + 1;
                    println!("FOLLOW press {name} at {ms} ms, held {hold} ms");
                }
            }
        }
        cmd
    }

    /// A `b` scoreboard: the probe never sends `score`, so every one is a
    /// push. Printed beside the snapshot of the packet that carried it, whose
    /// commands the client parses ahead of the snapshot.
    pub(super) fn on_scoreboard(
        &self,
        now: Instant,
        s: Option<&net::snapshot::Snapshot>,
        tokens: &[String],
    ) {
        let p = wire();
        let ms = self
            .active_at
            .map_or(0, |t| now.saturating_duration_since(t).as_millis());
        let (snap, st, num, pm_type) = s.map_or((0, 0, -1, -1), |s| {
            (
                s.message_num,
                s.server_time.raw(),
                s.ps.field_i32(p, "clientNum"),
                s.ps.field_i32(p, "pm_type"),
            )
        });
        println!(
            "FOLLOW b t={ms} snap {snap} st={st} clientNum={num} pm_type={pm_type}: {}",
            tokens.join(" ")
        );
    }

    pub(super) fn observe(&mut self, now: Instant, s: &net::snapshot::Snapshot) {
        let p = wire();
        let key = (
            s.ps.field_i32(p, "clientNum"),
            s.ps.field_i32(p, "pm_type"),
            s.ps.field_i32(p, "pm_flags"),
            s.ps.field_i32(p, "eFlags"),
            s.ps.arrays.stats[0],
            s.ps.field_i32(p, "weapon"),
        );
        if self.last == Some(key) {
            if self.burst == 0 {
                return;
            }
            self.burst -= 1;
        } else {
            self.burst = FOLLOW_BURST;
        }
        self.last = Some(key);
        let ms = self
            .active_at
            .map_or(0, |t| now.saturating_duration_since(t).as_millis());
        let o = s.ps.origin(p);
        let players: Vec<u32> = s.entities.keys().copied().filter(|n| *n < 64).collect();
        let v = [
            s.ps.field_f32(p, "velocity[0]"),
            s.ps.field_f32(p, "velocity[1]"),
            s.ps.field_f32(p, "velocity[2]"),
        ];
        println!(
            "FOLLOW t={ms} snap {} st={} clientNum={} pm_type={} pm_flags={:#x} eFlags={:#x} \
ct={} origin=[{:.1},{:.1},{:.1}] vel=[{:.1},{:.1},{:.1}] view=[{:.1},{:.1}] viewheight={} \
viewcur={:.1} health={} weapon={} damageEvent={} evseq={} players={players:?}",
            s.message_num,
            s.server_time,
            key.0,
            key.1,
            key.2,
            key.3,
            s.ps.field_i32(p, "commandTime"),
            o[0],
            o[1],
            o[2],
            v[0],
            v[1],
            v[2],
            s.ps.field_f32(p, "viewangles[0]"),
            s.ps.field_f32(p, "viewangles[1]"),
            s.ps.field_i32(p, "viewHeightTarget"),
            s.ps.field_f32(p, "viewHeightCurrent"),
            s.ps.arrays.stats[0],
            s.ps.field_i32(p, "weapon"),
            s.ps.field_i32(p, "damageEvent"),
            s.ps.field_i32(p, "eventSequence"),
        );
    }
}

/// How long after a death the killcam probe first presses use, so a stock
/// 9 s killcam (2 s dying, `archivetime` 9) runs out on its own first.
const KILLCAM_USE_AFTER_DEATH: Duration = Duration::from_secs(20);

/// How long each use press is held: one server frame and a half, so the
/// script's 50 ms `useButtonPressed` poll sees it.
const KILLCAM_PRESS: Duration = Duration::from_millis(80);

/// How long the probe keeps tracing once it is alive again.
const KILLCAM_TAIL: Duration = Duration::from_secs(3);

/// `--probe-fall`: joins and stands, and prints a `FALL` line per snapshot
/// whose ground entity, `pm_flags`, `pm_time`, event ring or health moved,
/// which is a landing's whole footprint, and a `CMDS` line per
/// [`FALL_CMDS_PER_LINE`] cmds sent, their `serverTime`s as a first stamp
/// and the steps after it, and under `--probe-fall-walk` the yaw word each
/// went out with, so a gate can replay the fall on the cmds retail ran.
/// Each line also names, by index into it, the cmds whose buttons,
/// `wbuttons` or `up` differ from the cmd before (`in=<k>:<buttons>:
/// <wbuttons>:<up>`) and the cmds sent on a newer snapshot than the cmd
/// before (`snap=<k>:<serverTime>`), so a replay presses on retail's cmd,
/// and the `commandTime` of every snapshot parsed (`ct=<serverTime>:
/// <commandTime>`), the last cmd the server ran before that frame, so a
/// replay runs each cmd on retail's side of a frame. `w=` is each cmd's
/// wall clock at the send, the first in microseconds since the epoch and
/// the steps after it, which orders two probes' packets on one host as
/// the server received them. `q=` is the netchan sequence of each cmd's
/// message, the first absolute and the steps after it, which a retail
/// server under `showpackets 1` prints as it reads each packet
/// (docs/dev/retail.md, "Receive order"). A `SPAWNED` line gives the first
/// `PM_NORMAL` snapshot's time, `commandTime`, origin, view and velocity:
/// where the spawn pick (or a placement in its frame) put the player, and
/// what the cmds run there left him.
/// `client-probes/probe_fall` drops the player from a height. Writes no
/// fixture.
#[derive(Default)]
pub(super) struct FallProbe {
    last: Option<Vec<i32>>,
    cmds: Vec<(i32, Option<i32>)>,
    walls: Vec<u64>,
    seqs: Vec<u32>,
    inputs: Vec<String>,
    snaps: Vec<String>,
    ran: Vec<String>,
    last_input: Option<(u8, u8, i8)>,
    last_snap: Option<i32>,
    last_msg: Option<u32>,
    pub(super) spawned: bool,
}

const FALL_CMDS_PER_LINE: usize = 60;

impl FallProbe {
    /// One `SPAWNED` line at the first `PM_NORMAL` snapshot, so a gate can
    /// pin the spawn point the pick took and replay the spawn's fall.
    pub(super) fn spawned(&mut self, snap: Option<&net::snapshot::Snapshot>) {
        let Some(s) = snap.filter(|_| !self.spawned) else {
            return;
        };
        let p = wire();
        if s.ps.field_i32(p, "pm_type") != PM_NORMAL {
            return;
        }
        self.spawned = true;
        let o = s.ps.origin(p);
        let v = s.ps.viewangles(p);
        let vel = ["velocity[0]", "velocity[1]", "velocity[2]"].map(|n| s.ps.field_f32(p, n));
        println!(
            "SPAWNED t={} ct={} origin={},{},{} view={},{},{} vel={},{},{}",
            s.server_time,
            s.ps.field_i32(p, "commandTime"),
            o[0],
            o[1],
            o[2],
            v[0],
            v[1],
            v[2],
            vel[0],
            vel[1],
            vel[2]
        );
    }

    pub(super) fn record_cmd(
        &mut self,
        server_time: i32,
        yaw: Option<i32>,
        input: (u8, u8, i8),
        seq: u32,
        ring: &vcod_common::net::snapshot::SnapshotRing,
    ) {
        let k = self.cmds.len();
        if self.last_input != Some(input) {
            self.inputs
                .push(format!("{k}:{}:{}:{}", input.0, input.1, input.2));
            self.last_input = Some(input);
        }
        if let Some(s) = ring
            .newest()
            .filter(|s| self.last_snap != Some(s.server_time.raw()))
        {
            // Every snapshot parsed since the last cmd, not only the newest:
            // two that land in one loop pass would hide a frame's `ct`.
            let from = self.last_msg.map_or(s.message_num, |n| {
                (n + 1).max(s.message_num.saturating_sub(31))
            });
            for snap in (from..=s.message_num).filter_map(|n| ring.get(n)) {
                let ct = snap.ps.field_i32(wire(), "commandTime");
                self.ran.push(format!("{}:{ct}", snap.server_time));
            }
            self.snaps.push(format!("{k}:{}", s.server_time));
            self.last_snap = Some(s.server_time.raw());
            self.last_msg = Some(s.message_num);
        }
        self.cmds.push((server_time, yaw));
        self.seqs.push(seq);
        self.walls.push(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_micros() as u64),
        );
        if self.cmds.len() >= FALL_CMDS_PER_LINE
            && let Some(line) = self.take_line()
        {
            println!("{line}");
        }
    }

    /// The `CMDS` line of the cmds recorded since the last one, cleared;
    /// `None` with none.
    fn take_line(&mut self) -> Option<String> {
        if self.cmds.is_empty() {
            return None;
        }
        let steps: Vec<String> = self
            .cmds
            .windows(2)
            .map(|w| (w[1].0 - w[0].0).to_string())
            .collect();
        let yaws: Vec<String> = self
            .cmds
            .iter()
            .filter_map(|c| c.1.map(|y| y.to_string()))
            .collect();
        let yaws = if yaws.is_empty() {
            String::new()
        } else {
            format!(" yaw={}", yaws.join(","))
        };
        let mut tail = String::new();
        for (key, list) in [
            ("in", &mut self.inputs),
            ("snap", &mut self.snaps),
            ("ct", &mut self.ran),
        ] {
            if !list.is_empty() {
                tail += &format!(" {key}={}", list.join(","));
                list.clear();
            }
        }
        let walls: Vec<String> = std::iter::once(self.walls[0])
            .chain(self.walls.windows(2).map(|w| w[1].saturating_sub(w[0])))
            .map(|w| w.to_string())
            .collect();
        let seqs: Vec<String> = std::iter::once(self.seqs[0])
            .chain(self.seqs.windows(2).map(|w| w[1].wrapping_sub(w[0])))
            .map(|q| q.to_string())
            .collect();
        let line = format!(
            "CMDS st={} d={}{yaws}{tail} w={} q={}",
            self.cmds[0].0,
            steps.join(","),
            walls.join(","),
            seqs.join(",")
        );
        self.cmds.clear();
        self.walls.clear();
        self.seqs.clear();
        Some(line)
    }

    /// The last block, under [`FALL_CMDS_PER_LINE`] cmds, at the run's end.
    pub(super) fn flush(&mut self) {
        if let Some(line) = self.take_line() {
            println!("{line}");
        }
    }

    pub(super) fn observe(&mut self, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let i = |n: &str| snap.ps.field_i32(p, n);
        let f = |n: &str| f32::from_bits(snap.ps.field_i32(p, n) as u32);
        let mut key = vec![
            i("groundEntityNum"),
            i("pm_flags"),
            i("pm_time"),
            i("eventSequence"),
            snap.ps.health(),
        ];
        // a fall moves only the origin, so print the first airborne frames too
        key.push(i("commandTime") * (key[0] == net::protocol::ENTITYNUM_NONE as i32) as i32);
        // and a corpse's slide, which moves nothing else either
        if i("pm_type") == PM_DEAD {
            key.extend([i("origin[0]"), i("origin[1]"), i("origin[2]")]);
        }
        // and a prone body's bend, printed once it is non-zero
        let bend = ["fTorsoHeight", "fTorsoPitch", "fWaistPitch"].map(i);
        key.extend(bend);
        // and the prone heading, which 1.1's flat body never shows otherwise
        let heading = i("proneDirection");
        let prone = i("pm_flags") & net::flags::PMF_PRONE != 0;
        key.push(heading * prone as i32);
        // and the body's clips, which a prone turn changes without a move
        let anims = [i("legsAnim"), i("torsoAnim")];
        key.extend(anims);
        if self.last.as_ref() == Some(&key) {
            return;
        }
        self.last = Some(key);
        let dir = f32::from_bits(heading as u32);
        let bend = if bend != [0; 3] {
            let [h, p, w] = bend.map(|b| f32::from_bits(b as u32));
            format!(" prone={h},{p},{w} dir={dir}")
        } else if prone {
            format!(" dir={dir}")
        } else {
            String::new()
        };
        println!(
            "FALL t={} ct={} origin={:.3},{:.3},{:.3} vel={},{},{} ground={} pm_type={} pm_flags=0x{:x} pm_time={} \
health={} seq={} events=[{},{},{},{}] parms=[{},{},{},{}]{bend} legs={} torso={}",
            snap.server_time,
            i("commandTime"),
            f("origin[0]"),
            f("origin[1]"),
            f("origin[2]"),
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            i("groundEntityNum"),
            i("pm_type"),
            i("pm_flags"),
            i("pm_time"),
            snap.ps.health(),
            i("eventSequence"),
            ev_v1(i("events[0]")),
            ev_v1(i("events[1]")),
            ev_v1(i("events[2]")),
            ev_v1(i("events[3]")),
            i("eventParms[0]"),
            i("eventParms[1]"),
            i("eventParms[2]"),
            i("eventParms[3]"),
            anims[0],
            anims[1],
        );
    }
}

/// `--probe-compass`: one `COMPASS` line per snapshot whose
/// `iCompassFriendInfo`, eye, yaw or player entity list changed, with the
/// field decoded the way the cgame reads it (offsets `field * 4 - 1020`, yaw
/// the signed top byte times 360/256) and the players the snapshot carries
/// with their origins, `eFlags` and `solid`. Two of these on one team measure
/// `G_GetNonPVSFriendlyInfo` (docs/research/cod11-hud-protocol.md, section
/// 9, "Compass friendlies"). Writes no fixture.
#[derive(Default)]
pub(super) struct CompassProbe {
    last: String,
}

impl CompassProbe {
    pub(super) fn observe(&mut self, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let ps = &snap.ps;
        let o = ps.origin(p);
        let unpack = |info: u32| {
            if info == 0 {
                return "-".to_string();
            }
            let off = |shift: u32| ((info >> shift) & 0x1ff) as i32 * 4 - 1020;
            format!(
                "c{} dx={} dy={} yaw={:.2}",
                info & 0x3f,
                off(6),
                off(15),
                f32::from((info >> 24) as u8 as i8) * 1.40625
            )
        };
        let info = ps.field_i32(p, "iCompassFriendInfo") as u32;
        let decoded = unpack(info);
        // UO's tank, packed the same way with the vehicle's pool slot.
        let tank = if p.is_uo() {
            let t = ps.field_i32(p, "iCompassTankInfo") as u32;
            let vehicles: Vec<String> = snap
                .entities
                .iter()
                .filter(|(_, e)| e.field_i32(p, "eType") == net::flags::ET_VEHICLE_V22)
                .map(|(n, e)| {
                    let t = Trajectory::read(e, p, "pos");
                    format!(
                        "{n}@{:.1},{:.1} slot={} driver={}",
                        t.base.x,
                        t.base.y,
                        e.field_i32(p, "attackerEntityNum"),
                        e.field_i32(p, "otherEntityNum")
                    )
                })
                .collect();
            format!(
                " tank={t:#010x} {} vehicles=[{}]",
                unpack(t),
                vehicles.join(" ")
            )
        } else {
            String::new()
        };
        let players: Vec<String> = snap
            .entities
            .iter()
            .filter(|(_, e)| e.field_i32(p, "eType") == net::flags::ET_PLAYER)
            .map(|(n, e)| {
                let t = Trajectory::read(e, p, "pos");
                format!(
                    "{n}@{:.1},{:.1},{:.1} ef={:#x} solid={:#x}",
                    t.base.x,
                    t.base.y,
                    t.base.z,
                    e.field_i32(p, "eFlags"),
                    e.field_i32(p, "solid")
                )
            })
            .collect();
        let line = format!(
            "num={} pm={} o={:.3},{:.3},{:.3} vh={:.3} leanf={:.3} yaw={:.3} ef={:#x} info={info:#010x} {decoded} players=[{}]{tank}",
            ps.field_i32(p, "clientNum"),
            ps.field_i32(p, "pm_type"),
            o[0],
            o[1],
            o[2],
            ps.field_f32(p, "viewHeightCurrent"),
            ps.field_f32(p, "leanf"),
            ps.field_f32(p, "viewangles[1]"),
            ps.field_i32(p, "eFlags"),
            players.join(" "),
        );
        if line != self.last {
            println!("COMPASS t={} {line}", snap.server_time);
            self.last = line;
        }
    }
}

/// `--probe-items`: one `ITEM` line per snapshot an item entity (`eType` 3)
/// first appears in or changes in, and an `ITEM_GONE` line when it leaves:
/// the flight a `dropItem` launches, the landing, and a respawn's return
/// with its `EV_ITEM_RESPAWN`. Trajectories are `trType,trTime,base,delta`,
/// shortest round-trip floats. `client-probes/probe_itemdrop` and
/// `probe_respawnsign` make the items.
/// Writes no fixture.
#[derive(Default)]
pub(super) struct ItemsProbe {
    last: std::collections::BTreeMap<u32, String>,
}

impl ItemsProbe {
    pub(super) fn observe(&mut self, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let mut seen = std::collections::BTreeSet::new();
        for (&num, e) in &snap.entities {
            if e.field_i32(p, "eType") != crate::entities::ET_ITEM {
                continue;
            }
            seen.insert(num);
            let i = |n: &str| e.field_i32(p, n);
            let tr = |g: &str| {
                let t = Trajectory::read(e, p, g);
                // Shortest round-trip floats: a gate reads the wire's bits back.
                format!(
                    "{},{},{},{},{},{},{},{}",
                    t.tr_type,
                    t.tr_time,
                    t.base.x,
                    t.base.y,
                    t.base.z,
                    t.delta.x,
                    t.delta.y,
                    t.delta.z
                )
            };
            let line = format!(
                "index={} clientNum={} eFlags={} ground={} pos={} apos={} seq={} events={},{},{},{} parms={},{},{},{}",
                i("index"),
                i("clientNum"),
                i("eFlags"),
                i("groundEntityNum"),
                tr("pos"),
                tr("apos"),
                i("eventSequence"),
                ev_v1(i("events[0]")),
                ev_v1(i("events[1]")),
                ev_v1(i("events[2]")),
                ev_v1(i("events[3]")),
                i("eventParms[0]"),
                i("eventParms[1]"),
                i("eventParms[2]"),
                i("eventParms[3]"),
            );
            if self.last.get(&num) != Some(&line) {
                println!("ITEM t={} num={num} {line}", snap.server_time);
                self.last.insert(num, line);
            }
        }
        let gone: Vec<u32> = self
            .last
            .keys()
            .copied()
            .filter(|n| !seen.contains(n))
            .collect();
        for num in gone {
            println!("ITEM_GONE t={} num={num}", snap.server_time);
            self.last.remove(&num);
        }
    }
}

/// `--probe-ride`: one `RIDE` line per snapshot, the movement fields a mover
/// push or ride writes: origin, velocity, ground entity, `pm_type` and the
/// view yaw with the `delta_angles` yaw a rotating pusher adds to; and a
/// `RIDE_ENT` line whenever a mover's `solid`, `index` or `eFlags`
/// changes, and a `RIDE_GONE` line when one leaves the snapshot.
/// `client-probes/probe_ride` moves the mover; the trajectory lines
/// `check_movers` prints are the mover's own half. After the first snapshot,
/// a `RIDE_NEW` line names each entity that enters it, whatever its type.
/// Writes no fixture.
#[derive(Default)]
pub(super) struct RideProbe {
    movers: std::collections::HashMap<u32, [i32; 3]>,
    /// The last snapshot's entity numbers; `None` before the first.
    present: Option<std::collections::HashSet<u32>>,
    /// The last snapshot printed: the loop asks once per cmd.
    last: Option<i32>,
}

impl RideProbe {
    pub(super) fn observe(&mut self, snap: &net::snapshot::Snapshot) {
        if self.last.replace(snap.server_time.raw()) == Some(snap.server_time.raw()) {
            return;
        }
        let p = wire();
        if let Some(present) = &self.present {
            for (&num, ent) in snap.entities.iter().filter(|(n, _)| !present.contains(n)) {
                let o = ent.origin(p);
                println!(
                    "RIDE_NEW t={} num={num} eType={} solid=0x{:x} index={} eFlags=0x{:x} \
origin={:.3},{:.3},{:.3}",
                    snap.server_time,
                    ent.field_i32(p, "eType"),
                    ent.field_i32(p, "solid"),
                    ent.field_i32(p, "index"),
                    ent.field_i32(p, "eFlags"),
                    o[0],
                    o[1],
                    o[2]
                );
            }
        }
        self.present = Some(snap.entities.keys().copied().collect());
        for (&num, ent) in &snap.entities {
            // A script mover, or a `func_bobbing` or `func_pendulum`.
            let et = ent.field_i32(p, "eType");
            if et != net::flags::ET_SCRIPTMOVER && et != net::flags::ET_MOVER {
                continue;
            }
            let key = [
                ent.field_i32(p, "solid"),
                ent.field_i32(p, "index"),
                ent.field_i32(p, "eFlags"),
            ];
            if self.movers.insert(num, key) != Some(key) {
                println!(
                    "RIDE_ENT t={} num={num} solid=0x{:x} index={} eFlags=0x{:x}",
                    snap.server_time, key[0], key[1], key[2]
                );
            }
        }
        self.movers.retain(|num, _| {
            let kept = snap.entities.contains_key(num);
            if !kept {
                println!("RIDE_GONE t={} num={num}", snap.server_time);
            }
            kept
        });
        let i = |n: &str| snap.ps.field_i32(p, n);
        let f = |n: &str| f32::from_bits(snap.ps.field_i32(p, n) as u32);
        println!(
            "RIDE t={} ct={} origin={:.3},{:.3},{:.3} vel={},{},{} ground={} pm_type={} pm_flags=0x{:x} \
viewyaw={:.3} delta_yaw={}",
            snap.server_time,
            i("commandTime"),
            f("origin[0]"),
            f("origin[1]"),
            f("origin[2]"),
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            i("groundEntityNum"),
            i("pm_type"),
            i("pm_flags"),
            f("viewangles[1]"),
            i("delta_angles[1]"),
        );
    }
}

/// `--probe-killcam`: stands still, never sends `kill`, and prints a
/// `KILLCAM` line per snapshot from each death until [`KILLCAM_TAIL`] after
/// the respawn. Use is pressed [`KILLCAM_USE_AFTER_DEATH`] after the death
/// and once a second after that, or `skip_ms` after the killcam starts (the
/// first frame following another client). Writes no fixture.
#[derive(Default)]
pub(super) struct KillcamProbe {
    skip_ms: Option<u64>,
    start: Option<Instant>,
    /// Seen alive once: the connect's own spectator frames are not a death.
    spawned: bool,
    died_at: Option<Instant>,
    killcam_at: Option<Instant>,
    skipped: bool,
    alive_at: Option<Instant>,
    used_at: Option<Instant>,
    press_until: Option<Instant>,
    traced: Option<u32>,
    last_key: Option<(i32, i32, i32)>,
}

impl KillcamProbe {
    pub(super) fn new(skip_ms: Option<u64>) -> Self {
        KillcamProbe {
            skip_ms,
            ..Default::default()
        }
    }

    fn ms(&self, now: Instant) -> u128 {
        self.start
            .map_or(0, |t| now.saturating_duration_since(t).as_millis())
    }

    /// From the death until the tail after the respawn has run out.
    fn in_window(&self, now: Instant) -> bool {
        self.died_at.is_some() || self.alive_at.is_some_and(|t| now - t < KILLCAM_TAIL)
    }

    pub(super) fn cmd(&self, now: Instant) -> net::msg::UserCmd {
        let press = self.press_until.is_some_and(|t| now < t);
        net::msg::UserCmd {
            buttons: if press { BUTTON_USE } else { 0 },
            ..net::msg::NULL_USERCMD
        }
    }

    pub(super) fn on_server_command(&self, now: Instant, tokens: &[String]) {
        if self.in_window(now) {
            println!("KILLCAM t={} server: {}", self.ms(now), tokens.join(" "));
        }
    }

    pub(super) fn observe(&mut self, now: Instant, me: i32, s: &net::snapshot::Snapshot) {
        let p = wire();
        self.start.get_or_insert(now);
        let ms = self.ms(now);
        let i = |n: &str| s.ps.field_i32(p, n);
        let (client_num, pm_type, pm_flags) = (i("clientNum"), i("pm_type"), i("pm_flags"));
        let following = pm_flags & net::flags::PMF_FOLLOW != 0;
        let alive = pm_type == PM_NORMAL && client_num == me && !following;

        if alive {
            self.spawned = true;
            if let Some(d) = self.died_at.take() {
                println!(
                    "KILLCAM t={ms} alive again at serverTime {}, {} ms after the death",
                    s.server_time,
                    now.duration_since(d).as_millis()
                );
                self.alive_at = Some(now);
                self.killcam_at = None;
                self.skipped = false;
                self.used_at = None;
            }
        } else if self.spawned && self.died_at.is_none() {
            println!(
                "KILLCAM t={ms} death seen at serverTime {} (pm_type {pm_type})",
                s.server_time
            );
            self.died_at = Some(now);
            self.alive_at = None;
        }
        if self.died_at.is_some() && self.killcam_at.is_none() && (following || client_num != me) {
            println!(
                "KILLCAM t={ms} killcam starts at serverTime {} (clientNum {client_num}, pm_flags {pm_flags:#x})",
                s.server_time
            );
            self.killcam_at = Some(now);
        }

        // The presses.
        if let Some(dead) = self.died_at {
            let skip_due = match (self.skip_ms, self.killcam_at) {
                (Some(skip), Some(kc)) => {
                    !self.skipped && now.duration_since(kc) >= Duration::from_millis(skip)
                }
                _ => false,
            };
            let respawn_due = now.duration_since(dead) >= KILLCAM_USE_AFTER_DEATH
                && self
                    .used_at
                    .is_none_or(|t| now.duration_since(t) >= TARGET_USE_RETRY);
            if skip_due || respawn_due {
                println!(
                    "KILLCAM t={ms} use press ({}) at serverTime {}",
                    if skip_due { "skip" } else { "respawn" },
                    s.server_time
                );
                self.skipped |= skip_due;
                self.used_at = Some(now);
                self.press_until = Some(now + KILLCAM_PRESS);
            }
        }

        if !self.in_window(now) || self.traced == Some(s.message_num) {
            return;
        }
        self.traced = Some(s.message_num);
        let key = (client_num, pm_type, pm_flags);
        if self.last_key != Some(key) {
            println!(
                "KILLCAM t={ms} change at serverTime {}: clientNum {client_num} pm_type {pm_type} pm_flags {pm_flags:#x} (was {:?})",
                s.server_time, self.last_key
            );
            self.last_key = Some(key);
        }
        let o = s.ps.origin(p);
        let v = s.ps.viewangles(p);
        let players: Vec<String> = s
            .entities
            .iter()
            .filter(|(n, _)| **n < 64)
            .map(|(n, e)| {
                let eo = e.origin(p);
                let et = e.field_i32(p, "eType");
                format!("{n}:e{et}@{:.0},{:.0},{:.0}", eo[0], eo[1], eo[2])
            })
            .collect();
        let bodies: Vec<u32> = s
            .entities
            .keys()
            .copied()
            .filter(|n| (64..72).contains(n))
            .collect();
        let roster: Vec<String> = s
            .clients
            .iter()
            .map(|(n, c)| format!("{n}:{}", c.field_i32(p, "team")))
            .collect();
        let command_time = i("commandTime");
        println!(
            "KILLCAM t={ms} msg={} st={} clientNum={client_num} pm_type={pm_type} pm_flags={pm_flags:#x} \
eFlags={:#x} ct={command_time} st-ct={} deltaTime={} origin={:.1},{:.1},{:.1} view={:.1},{:.1} \
weapon={} health={} ents={} players=[{}] bodies={bodies:?} roster=[{}] hudA={} [{}] hudC={} [{}]",
            s.message_num,
            s.server_time,
            i("eFlags"),
            s.server_time - command_time,
            i("deltaTime"),
            o[0],
            o[1],
            o[2],
            v[0],
            v[1],
            i("weapon"),
            s.ps.arrays.stats[0],
            s.entities.len(),
            players.join(" "),
            roster.join(" "),
            hud_count(&s.ps.arrays.hud_archived),
            killcam_hud_str(&s.ps.arrays.hud_archived),
            hud_count(&s.ps.arrays.hud_current),
            killcam_hud_str(&s.ps.arrays.hud_current),
        );
    }
}

fn hud_count(elems: &[net::msg::HudElem]) -> usize {
    elems
        .iter()
        .filter(|e| **e != net::msg::HudElem::default())
        .count()
}

/// Every non-empty element as `type/text/shader/value x,y col time dur
/// fade scale move`, the last five being the fields that carry a server time.
fn killcam_hud_str(elems: &[net::msg::HudElem]) -> String {
    use net::msg::hud_field as h;
    elems
        .iter()
        .enumerate()
        .filter(|(_, e)| **e != net::msg::HudElem::default())
        .map(|(n, e)| {
            format!(
                "#{n}:ty{}/tx{}/sh{}/v{} {},{} col={:#x} time={} dur={} fade={}+{} scale={}+{} move={}+{}",
                e.get(h::TYPE),
                e.get(h::TEXT),
                e.get(h::SHADER),
                e.get_f32(h::VALUE),
                e.get(h::X),
                e.get(h::Y),
                e.get(h::COLOR),
                e.get(h::TIME),
                e.get(h::DURATION),
                e.get(h::FADE_START_TIME),
                e.get(h::FADE_TIME),
                e.get(h::SCALE_START_TIME),
                e.get(h::SCALE_TIME),
                e.get(h::MOVE_START_TIME),
                e.get(h::MOVE_TIME),
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::pickup_snap;
    use vcod_common::net::Msec;

    /// A run's last partial block of cmds is printed too: 61 cmds make two
    /// `CMDS` lines (BUG-091).
    #[test]
    fn the_last_partial_block_of_cmds_is_kept() {
        let ring = vcod_common::net::snapshot::SnapshotRing::default();
        let mut fall = FallProbe::default();
        for i in 0..61 {
            fall.record_cmd(1000 + 8 * i, None, (0, 0, 0), i as u32, &ring);
        }
        let line = fall.take_line().expect("the 61st cmd's line");
        assert!(line.starts_with("CMDS st=1480 d= "), "{line}");
        assert_eq!(fall.take_line(), None);
    }

    /// `FallProbe`'s `CMDS` line: the first `serverTime` and each step,
    /// the yaws, the inputs and snapshots by the cmd they came at, every
    /// snapshot's `commandTime` since the last cmd, and the sequences
    /// with their wrap, as the fall and turret gates read them.
    #[test]
    fn a_fall_cmds_line_carries_its_cmds_by_index() {
        let mut ring = vcod_common::net::snapshot::SnapshotRing::default();
        let snap = |num, t: i32, ct| {
            let mut s = pickup_snap(num, &[("commandTime", ct)], &[]);
            s.server_time = Msec::from_raw(t);
            s
        };
        let mut fall = FallProbe::default();
        let seq0 = u32::MAX - 1;
        for k in 0..4u32 {
            if k == 1 {
                ring.insert(snap(1, 1050, 1040));
            }
            if k == 3 {
                ring.insert(snap(2, 1100, 1090));
                ring.insert(snap(3, 1150, 1140));
            }
            let input = if k < 2 { (1, 0, 0) } else { (0, 4, 127) };
            let yaw = (k == 0).then_some(16384);
            fall.record_cmd(
                1000 + 16 * k as i32,
                yaw,
                input,
                seq0.wrapping_add(k),
                &ring,
            );
        }
        let line = fall.take_line().unwrap();
        let (head, tail) = line.split_once(" w=").unwrap();
        assert_eq!(
            head,
            "CMDS st=1000 d=16,16,16 yaw=16384 in=0:1:0:0,2:0:4:127 snap=1:1050,3:1150 \
             ct=1050:1040,1100:1090,1150:1140"
        );
        assert_eq!(tail.split_once(" q=").unwrap().1, "4294967294,1,1,1");
    }
}
