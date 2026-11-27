//! `--save-slope`: every cmd and snapshot of a scripted walk, for `playerstate_slope_ab.rs`.

use super::fixture::{
    PLAYERSTATE_FIXTURE_DIR, cmd_fields, serverinfo_key, tagged_name, write_fixture_file,
};
use super::*;

/// What `--probe-slope` counts over the route walk, per second and for the
/// run: snapshots with `groundEntityNum` 1023, reversals of the sight ramp
/// while the sight is held, `EV_STEP_VIEW` (143) events with their parms and
/// the mean horizontal speed. The same walk against retail and against ours
/// is the comparison; a server that drops a walker off a slope for a frame
/// shows it here as airborne snapshots and sight reversals it never asked for.
#[derive(Default)]
pub(super) struct SlopeStats {
    seen: Option<u32>,
    prev_seq: Option<i32>,
    prev_frac: Option<f32>,
    rising: bool,
    last_print: Option<Instant>,
    second: SlopeCounts,
    run: SlopeCounts,
}

#[derive(Default, Clone)]
struct SlopeCounts {
    snaps: usize,
    air: usize,
    adsrev: usize,
    ev143: Vec<i32>,
    speed_sum: f32,
}

impl SlopeCounts {
    fn add(&mut self, air: bool, adsrev: bool, parms: &[i32], speed: f32) {
        self.snaps += 1;
        self.air += usize::from(air);
        self.adsrev += usize::from(adsrev);
        self.ev143.extend_from_slice(parms);
        self.speed_sum += speed;
    }

    fn line(&self) -> String {
        let mean = self.speed_sum / self.snaps.max(1) as f32;
        format!(
            "snaps {} air {} adsrev {} ev143 {} parms {:?} speed {mean:.0}",
            self.snaps,
            self.air,
            self.adsrev,
            self.ev143.len(),
            &self.ev143[..self.ev143.len().min(12)]
        )
    }
}

impl SlopeStats {
    pub(super) fn observe(&mut self, now: Instant, snap: &net::snapshot::Snapshot) {
        if self.seen == Some(snap.message_num) {
            return;
        }
        self.seen = Some(snap.message_num);
        let p = wire();
        let air = snap.ps.field_i32(p, "groundEntityNum") == net::protocol::ENTITYNUM_NONE as i32;
        let frac = snap.ps.field_f32(p, "fWeaponPosFrac");
        // A reversal is the ramp turning around: rising to falling or back.
        let adsrev = match self.prev_frac {
            Some(prev) if frac < prev && self.rising => {
                self.rising = false;
                true
            }
            Some(prev) if frac > prev && !self.rising => {
                self.rising = true;
                true
            }
            _ => false,
        };
        self.prev_frac = Some(frac);
        // The ring is written below the sequence (`docs/protocol-1.1.md`);
        // 8 bits on the wire, so the gain is taken modulo 256 and capped at
        // the four slots that can still be read.
        let seq = snap.ps.field_i32(p, "eventSequence");
        let mut parms = Vec::new();
        if let Some(prev) = self.prev_seq {
            let gained = ((seq - prev) & 0xff).min(4);
            for i in (seq - gained)..seq {
                let slot = (i & 3) as usize;
                if ev_v1(snap.ps.field_i32(p, &format!("events[{slot}]")))
                    == net::event_ids::EV_STEP_VIEW
                {
                    parms.push(snap.ps.field_i32(p, &format!("eventParms[{slot}]")) - 128);
                }
            }
        }
        self.prev_seq = Some(seq);
        let speed = snap
            .ps
            .field_f32(p, "velocity[0]")
            .hypot(snap.ps.field_f32(p, "velocity[1]"));
        self.second.add(air, adsrev, &parms, speed);
        self.run.add(air, adsrev, &parms, speed);
        let due = self
            .last_print
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
        if due {
            self.last_print = Some(now);
            let o = snap.ps.origin(p);
            println!(
                "slope: {} frac {frac:.2} origin=[{:.0},{:.0},{:.0}]",
                self.second.line(),
                o[0],
                o[1],
                o[2]
            );
            self.second = SlopeCounts::default();
        }
    }

    pub(super) fn report(&self) {
        println!("slope total: {}", self.run.line());
    }
}

/// What `--save-slope` records: every usercmd as it went on the wire and every
/// snapshot's movement fields, in the order they happened, from the first
/// spawned snapshot on. `crates/server/tests/playerstate_slope_ab.rs` replays
/// the cmds on our mover from the first snapshot's origin and diffs the
/// origin at every snapshot's `commandTime`.
#[derive(Default)]
pub(super) struct SlopeCapture {
    /// Every `pm_type`'s snapshots, not only a playing or linked body's.
    pub(super) every_state: bool,
    pub(super) seen: Option<u32>,
    pub(super) prev_seq: Option<i32>,
    pub(super) cmds: Vec<net::msg::UserCmd>,
    /// `(index into cmds at which the snapshot arrived, the line)`.
    pub(super) snaps: Vec<(usize, String)>,
    /// The last cmds sent before the first snapshot was logged, which go
    /// in ahead of it: the one its `commandTime` names is the replay's
    /// base for the first pair (a teleport's view and `delta_angles` hide
    /// its angles).
    pub(super) before: std::collections::VecDeque<net::msg::UserCmd>,
    /// `--probe-moveshot`: from the first spawned snapshot until the capture
    /// opens, a `!wsnap` line per snapshot and a `!wcmd` per cmd the server
    /// had not run by the first, so a replay falls from the placement on
    /// retail's cmds (cod11-combat.md 2.1.1.2).
    pub(super) wait_log: bool,
    pub(super) wait: Vec<String>,
    pub(super) wait_seen: Option<u32>,
}

impl SlopeCapture {
    fn cmd_line(head: &str, c: &net::msg::UserCmd) -> String {
        format!("{head} {}", cmd_fields(c))
    }

    /// A snapshot before the capture opens, under [`Self::wait_log`].
    pub(super) fn observe_wait(&mut self, snap: &net::snapshot::Snapshot) {
        let p = wire();
        let i = |n: &str| snap.ps.field_i32(p, n);
        if !self.wait_log
            || self.recording()
            || self.wait_seen == Some(snap.message_num)
            || (self.wait_seen.is_none() && i("pm_type") != PM_NORMAL)
        {
            return;
        }
        if self.wait_seen.is_none() {
            let ct = i("commandTime");
            let past: Vec<String> = self
                .before
                .iter()
                .filter(|c| c.server_time > ct)
                .map(|c| Self::cmd_line("!wcmd", c))
                .collect();
            self.wait.extend(past);
        }
        self.wait_seen = Some(snap.message_num);
        let f = |n: &str| snap.ps.field_f32(p, n);
        self.wait.push(format!(
            "!wsnap msg={} t={} ct={} origin={},{},{} vel={},{},{} ground={} pm_flags={}",
            snap.message_num,
            snap.server_time,
            i("commandTime"),
            f("origin[0]"),
            f("origin[1]"),
            f("origin[2]"),
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            i("groundEntityNum"),
            i("pm_flags"),
        ));
    }

    fn recording(&self) -> bool {
        !self.snaps.is_empty()
    }

    pub(super) fn sent(&mut self, c: net::msg::UserCmd) {
        if self.recording() {
            self.cmds.push(c);
        } else {
            if self.wait_seen.is_some() {
                self.wait.push(Self::cmd_line("!wcmd", &c));
            }
            if self.before.len() == 8 {
                self.before.pop_front();
            }
            self.before.push_back(c);
        }
    }

    pub(super) fn observe(&mut self, snap: &net::snapshot::Snapshot) {
        if self.seen == Some(snap.message_num) {
            return;
        }
        let p = wire();
        // A seat is `pm_type` 1, the link (docs/research/coduo-vehicles.md).
        if !self.every_state
            && !matches!(
                snap.ps.field_i32(p, "pm_type"),
                PM_NORMAL | net::flags::PM_NORMAL_LINKED
            )
        {
            return;
        }
        self.seen = Some(snap.message_num);
        let f = |n: &str| snap.ps.field_f32(p, n);
        let i = |n: &str| snap.ps.field_i32(p, n);
        // The step events this snapshot brought, the way `SlopeStats` reads
        // the ring.
        let seq = i("eventSequence");
        let mut steps = Vec::new();
        let mut events = Vec::new();
        if let Some(prev) = self.prev_seq {
            let gained = ((seq - prev) & 0xff).min(4);
            for k in (seq - gained)..seq {
                let slot = (k & 3) as usize;
                let (ev, parm) = (
                    ev_v1(i(&format!("events[{slot}]"))),
                    i(&format!("eventParms[{slot}]")),
                );
                if ev == net::event_ids::EV_STEP_VIEW {
                    steps.push((parm - 128).to_string());
                }
                events.push(format!("{ev}:{parm}"));
            }
        }
        self.prev_seq = Some(seq);
        let line = format!(
            "!snap msg={} t={} ct={} origin={},{},{} vel={},{},{} ground={} view={},{},{} \
             frac={} da={},{},{} pm_flags={} step={} leanf={} weapon={} weaponstate={} \
             eflags={} vh={} pdir={} pdirpitch={} ptorso={} torso={},{},{} mdir={} ev={}",
            snap.message_num,
            snap.server_time,
            i("commandTime"),
            f("origin[0]"),
            f("origin[1]"),
            f("origin[2]"),
            f("velocity[0]"),
            f("velocity[1]"),
            f("velocity[2]"),
            i("groundEntityNum"),
            f("viewangles[0]"),
            f("viewangles[1]"),
            f("viewangles[2]"),
            f("fWeaponPosFrac"),
            i("delta_angles[0]"),
            i("delta_angles[1]"),
            i("delta_angles[2]"),
            i("pm_flags"),
            steps.join(","),
            f("leanf"),
            i("weapon"),
            i("weaponstate"),
            i("eFlags"),
            f("viewHeightCurrent"),
            f("proneDirection"),
            f("proneDirectionPitch"),
            f("proneTorsoPitch"),
            f("fTorsoHeight"),
            f("fTorsoPitch"),
            f("fWaistPitch"),
            i("movementDir"),
            events.join(","),
        );
        // The whole playerstate as decoded, so a replay can rebuild every
        // field the predictor reads: each non-zero netfield by name, then the
        // non-zero ammo and clip slots.
        let wire: Vec<String> = p
            .player_fields
            .iter()
            .zip(&snap.ps.fields)
            .filter(|(_, v)| **v != 0)
            .map(|(f, v)| format!("{}:{v}", f.name))
            .collect();
        let slots = |a: &[i16; 64]| {
            a.iter()
                .enumerate()
                .filter(|(_, v)| **v != 0)
                .map(|(k, v)| format!("{k}:{v}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        // The seat's vehicle angles, which the seated view clamps read.
        let vang = snap
            .entities
            .get(&(i("viewlocked_entNum") as u32))
            .filter(|_| i("eFlags") & net::flags::EF_IN_VEHICLE_V22 != 0)
            .map(|e| {
                let a = e.angles(p);
                format!(" vang={},{},{}", a[0], a[1], a[2])
            })
            .unwrap_or_default();
        // The seat's vehicle's whole entity state, every non-zero field by
        // name, for the server's drive replay.
        let veh = snap
            .entities
            .get(&(i("viewlocked_entNum") as u32))
            .filter(|_| i("eFlags") & net::flags::EF_IN_VEHICLE_V22 != 0)
            .map(|e| {
                let fields: Vec<String> = p
                    .entity_fields
                    .iter()
                    .zip(&e.fields)
                    .filter(|(_, v)| **v != 0)
                    .map(|(f, v)| format!("{}:{v}", f.name))
                    .collect();
                format!(" veh={}", fields.join(","))
            })
            .unwrap_or_default();
        // Every bullet-impact temp entity on the snapshot, `num:x,y,z`
        // `;`-separated: where the probe's own rounds struck. One impact
        // rides several snapshots.
        let impacts: Vec<String> = snap
            .entities
            .iter()
            .filter(|(_, e)| {
                p.event_of(e.field_i32(p, "eType"))
                    .map(nearest_v1)
                    .is_some_and(|ev| {
                        ev == net::event_ids::EV_BULLET_HIT_SMALL
                            || ev == net::event_ids::EV_BULLET_HIT_LARGE
                    })
            })
            .map(|(num, e)| {
                let at = vcod_common::net::trajectory::Trajectory::read(e, p, "pos").base;
                format!("{num}:{},{},{}", at.x, at.y, at.z)
            })
            .collect();
        let imp = if impacts.is_empty() {
            String::new()
        } else {
            format!(" imp={}", impacts.join(";"))
        };
        let line = format!(
            "{line}{vang}{veh}{imp} wire={} ammo={} clip={}",
            wire.join(","),
            slots(&snap.ps.arrays.ammo),
            slots(&snap.ps.arrays.ammoclip)
        );
        if self.snaps.is_empty() {
            self.cmds.extend(self.before.drain(..));
        }
        self.snaps.push((self.cmds.len(), line));
    }
}

/// Which walk a slope capture recorded.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SlopeWalk {
    Route,
    Prone,
    Moves,
    Turns,
    Sprint,
    Autofire,
    Talk,
    MoveShot(MoveShot),
    ProneMove(ProneMove),
    Vehicle,
    UoFoot,
    UoTurret,
    CrouchJump,
    PatchWalk,
}

/// Writes the route walk as `<map>-<gametype>-slope-<cmd_ms>ms.txt`: `!cmd`
/// lines carry the usercmd as sent, `!snap` lines the playerstate the server
/// answered with, interleaved in the order the probe saw them. The header's
/// server settings and its `<spot>`, `<yaw>`, `<gun yaw>` and `<type>` are a
/// template the committer fills in, not what the run used.
pub(super) fn write_slope_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    capture: &SlopeCapture,
    cmd_ms: u64,
    walk: SlopeWalk,
    protocol: &net::protocol::Cod1Wire,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    // A client-probe gametype runs `dm::main` under its own name; the replay
    // needs the gametype whose rules the map was loaded under.
    let gametype = match key("g_gametype") {
        // `probe_vehicle` runs tdm's.
        "probe_vehicle" => "tdm",
        g if g.starts_with("probe_") => "dm",
        g => g,
    };
    let mut out = String::new();
    let server = retail_build();
    out.push_str(&format!(
        "# Retail CoD {server} dedicated server playerstate along a walked route, per usercmd.\n"
    ));
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    if let SlopeWalk::MoveShot(part) = walk {
        let env = if protocol.is_uo() {
            "COD_VERSION=uo "
        } else {
            "COD_VERSION=1.5 "
        };
        let spot = match part {
            MoveShot::Walk => "street",
            MoveShot::Bipod => "wall",
        };
        let name = part.name();
        out.push_str(&format!(
            "# Captured with {env}tools/run_probe.sh client-probes/probe_moveshot {map} +set probe_teleport 1\n\
             # +set probe_spot {spot} and --net-probe --save-slope --probe-moveshot {name} --probe-cmd-ms {cmd_ms}:\n\
             # moveshot_script({name}) from the placement, facing world yaw 0. !cmd is a\n"
        ));
    } else if let SlopeWalk::ProneMove(part) = walk {
        let env = match protocol.transport.version {
            6 => "COD_VERSION=1.5 ",
            22 => "COD_VERSION=uo ",
            _ => "",
        };
        let name = part.name();
        let (probe, spot) = match part {
            ProneMove::Lmg => ("probe_prone", "street +set probe_binoculars 1"),
            ProneMove::Rifle => ("probe_prone", "street"),
            ProneMove::Deploy => ("probe_uofoot", "mound"),
            ProneMove::DeployCrouch => ("probe_uofoot", "moundcrouch"),
        };
        out.push_str(&format!(
            "# Captured with {env}tools/run_probe.sh client-probes/{probe} {map} +set probe_teleport 1\n\
             # +set probe_spot {spot} and --net-probe --save-slope --probe-pronemove {name} --probe-cmd-ms {cmd_ms}:\n\
             # pronemove_script({name}) from the placement, facing world yaw 0. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::PatchWalk {
        let env = match protocol.transport.version {
            6 => "COD_VERSION=1.5 ",
            22 => "COD_VERSION=uo ",
            _ => "",
        };
        out.push_str(&format!(
            "# Captured with {env}tools/run_probe.sh client-probes/probe_patchwalk {map}\n\
             # +set probe_teleport 1 +set probe_spot <spot> and --net-probe --save-slope\n\
             # --probe-patchwalk <yaw> --probe-cmd-ms {cmd_ms}: patchwalk_script() from the\n\
             # placement, facing the --probe-patchwalk yaw. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::CrouchJump {
        out.push_str(&format!(
            "# Captured with COD_VERSION=uo tools/run_probe.sh client-probes/probe_crouchjump {map}\n\
             # +set probe_teleport 1 +set probe_spot <spot> and --net-probe --save-slope\n\
             # --probe-crouchjump <yaw> --probe-cmd-ms {cmd_ms}: crouchjump_script() from the\n\
             # placement, facing the --probe-crouchjump yaw. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::UoTurret {
        out.push_str(&format!(
            "# Captured with COD_VERSION=uo tools/run_probe.sh client-probes/probe_turret {map}\n\
             # +set probe_teleport 1 and --net-probe --save-slope --slope-every-state --probe-team allies\n\
             # --probe-uoturret <gun yaw> --probe-cmd-ms {cmd_ms}: uoturret_script() from the\n\
             # placement 40 units behind the gun. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::UoFoot {
        out.push_str(&format!(
            "# Captured with COD_VERSION=uo tools/run_probe.sh client-probes/probe_uofoot {map}\n\
             # +set probe_teleport 1 +set probe_spot <spot> and --net-probe --save-slope\n\
             # --probe-uofoot --probe-cmd-ms {cmd_ms}: uofoot_script() from the placement,\n\
             # facing the --probe-uofoot yaw. !cmd is a\n"
        ));
    } else if matches!(
        walk,
        SlopeWalk::Moves
            | SlopeWalk::Turns
            | SlopeWalk::Sprint
            | SlopeWalk::Autofire
            | SlopeWalk::Talk
    ) {
        let env = match protocol.transport.version {
            6 => "COD_VERSION=1.5 ",
            22 => "COD_VERSION=uo ",
            _ => "",
        };
        let (flag, script, facing) = match walk {
            SlopeWalk::Moves => ("--probe-moves", "moves_script", 0),
            SlopeWalk::Sprint => ("--probe-sprint", "sprint_script", 0),
            SlopeWalk::Autofire => ("--probe-autofire", "autofire_script", 0),
            SlopeWalk::Talk => ("--probe-talk", "talk_script", 0),
            _ => ("--probe-turns", "turns_script", 90),
        };
        out.push_str(&format!(
            "# Captured with {env}tools/run_probe.sh client-probes/probe_prone {map} +set probe_teleport 1\n\
             # +set probe_spot street and --net-probe --save-slope {flag} --probe-cmd-ms {cmd_ms}:\n\
             # {script}() from the placement, facing world yaw {facing}. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::Vehicle {
        out.push_str(&format!(
            "# Captured with COD_VERSION=uo tools/run_probe.sh client-probes/probe_vehicle {map}\n\
             # +set probe_vehicletype <type> and --net-probe --probe-team axis --probe-vehicle\n\
             # seats|drive|gun --save-slope --probe-cmd-ms {cmd_ms}: vehicle_probe.rs SEATS (one\n\
             # player through every seat), DRIVE (one driver) or GUN (one gunner, the gun swept\n\
             # past its spans). vang is the seat's vehicle's angles as the\n\
             # snapshot sent them, veh its every non-zero entity field. !cmd is a\n"
        ));
    } else if walk == SlopeWalk::Prone {
        out.push_str(&format!(
            "# Captured with tools/run_probe.sh client-probes/probe_prone {map} +set probe_teleport 1\n\
             # +set probe_spot <spot> and --net-probe --save-slope --probe-prone --probe-cmd-ms {cmd_ms}:\n"
        ));
        out.push_str(
            "# the ProneScript crawl from the gsc's placement (g_gametype probe_prone runs\n",
        );
        out.push_str("# dm::main, so the file is named dm). !cmd is a\n");
    } else {
        out.push_str(&format!(
            "# Captured with tools/run_server.sh and --net-probe --save-slope --probe-cmd-ms {cmd_ms}:\n"
        ));
        out.push_str(
            "# the --probe-pvs route with the sight held, from a random spawn. !cmd is a\n",
        );
    }
    out.push_str("# usercmd as it went on the wire (angles already rebased on delta_angles),\n");
    out.push_str("# !snap the playerstate the next snapshot carried, with ct its commandTime.\n");
    out.push_str("# Floats are printed exactly; step lists the EV_STEP_VIEW parms the snapshot\n");
    out.push_str("# brought, minus the 128 bias, and ev every event:parm it brought; wire is\n");
    out.push_str(
        "# every non-zero netfield as decoded (pm_flags in 1.1's layout), ammo and clip\n",
    );
    out.push_str("# the non-zero slots. The cmds ahead of the first !snap went out before it.\n");
    out.push_str("# imp, where present, is every bullet-impact temp entity the snapshot carried\n");
    out.push_str("# as entnum:x,y,z, ;-separated; one impact rides several snapshots.\n");
    out.push_str(&format!("# cmd_ms {cmd_ms}\n"));
    // The weapon list a replay loads its defs by, where it is not 1.1's.
    if protocol.is_uo() {
        let cs7 = configstrings.get(7).map(String::as_str).unwrap_or("");
        out.push_str(&format!("# cs7 {cs7}\n"));
    }
    if !capture.wait.is_empty() {
        out.push_str(
            "# !wsnap and !wcmd: the wait from the first spawned snapshot until the capture opens,\n\
             # each cmd the server had not run by that snapshot, in the order the probe saw them.\n",
        );
        for l in &capture.wait {
            out.push_str(l);
            out.push('\n');
        }
    }
    let mut next_snap = capture.snaps.iter().peekable();
    for (i, c) in capture.cmds.iter().enumerate() {
        while let Some((at, line)) = next_snap.peek() {
            if *at > i {
                break;
            }
            out.push_str(line);
            out.push('\n');
            next_snap.next();
        }
        out.push_str(&format!("!cmd {}\n", cmd_fields(c)));
    }
    for (_, line) in next_snap {
        out.push_str(line);
        out.push('\n');
    }
    let path = write_fixture_file(
        &PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}-slope-{cmd_ms}ms"), tag),
        &out,
        overwrite,
    )?;
    println!(
        "slope: {} cmds, {} snapshots -> {path}",
        capture.cmds.len(),
        capture.snaps.len()
    );
    Ok(())
}
