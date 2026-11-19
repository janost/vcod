//! `--save-legs`: the walk behind the moving-body pose capture. Waits for
//! `client-probes/probe_legs` to put the player on its mark on mp_carentan,
//! then runs, strafes and crouch-walks a fixed script at world yaw 0, and
//! writes every usercmd and every snapshot's movement and anim fields to
//! `crates/server/tests/fixtures/movers/<map>-dm-legs-client.txt`, which
//! `legs_ab.rs` replays cmd for cmd beside the gsc's bone log. Under a
//! `--capture-tag` starting `torso` it runs [`TORSO_SCRIPT`] instead, firing,
//! reloading and aiming on the move beside `client-probes/probe_torso`, for
//! `legs_ab.rs`. Under one starting `stopgo` it runs [`STOPGO_SCRIPT`],
//! whose short stops and swaps take a legs clip back while it still blends
//! out, beside the same `probe_legs`.

use std::time::Instant;
use vcod_common::net;
use vcod_common::net::msg::{
    BUTTON_ADS, BUTTON_ATTACK, NULL_USERCMD, UserCmd, WBUTTON_CROUCH, WBUTTON_RELOAD,
};
use vcod_common::net::snapshot::Snapshot;

/// Where `probe_legs.gsc` puts the player: mp_carentan's flat bottom floor,
/// under the garden terrain (ground at z -7), not on a street.
pub const LEGS_SPOT: [f32; 3] = [932.0, -376.0, -151.875];

/// One step of the walk: how long it holds and what it holds.
struct Step {
    label: &'static str,
    ms: u128,
    forward: i8,
    right: i8,
    crouch: bool,
    /// Fire taps every [`FIRE_PERIOD_MS`]: the carbine is semi-automatic.
    fire: bool,
    /// One reload tap at the step's start.
    reload: bool,
    ads: bool,
}

const fn step(label: &'static str, ms: u128, forward: i8, right: i8, crouch: bool) -> Step {
    Step {
        label,
        ms,
        forward,
        right,
        crouch,
        fire: false,
        reload: false,
        ads: false,
    }
}

const fn fire(s: Step) -> Step {
    Step { fire: true, ..s }
}

const fn reload(s: Step) -> Step {
    Step { reload: true, ..s }
}

const fn ads(s: Step) -> Step {
    Step { ads: true, ..s }
}

/// A fire tap is held this long and starts this often: two cmds down at
/// the probe's 60 Hz, and well past m1carbine_mp's 0.135 s `fireTime`.
const TAP_MS: u128 = 32;
const FIRE_PERIOD_MS: u128 = 300;

/// From the snapshot that showed the placement. The floor is clear 680
/// units either side of the mark, and each run is paired with its way back.
const SCRIPT: &[Step] = &[
    step("settle", 1500, 0, 0, false),
    step("run", 1800, 127, 0, false),
    step("stop", 1000, 0, 0, false),
    step("back", 1800, -127, 0, false),
    step("stop2", 1000, 0, 0, false),
    step("strafe_right", 1500, 0, 127, false),
    step("strafe_left", 1500, 0, -127, false),
    step("stop3", 1000, 0, 0, false),
    step("diag", 1200, 127, 127, false),
    step("diag_back", 1200, -127, -127, false),
    step("stop4", 1000, 0, 0, false),
    step("crouch", 800, 0, 0, true),
    step("crouch_run", 1500, 127, 0, true),
    step("crouch_back", 1500, -127, 0, true),
    step("crouch_stop", 800, 0, 0, true),
    step("stand", 1500, 0, 0, false),
];

/// The torso capture's walk. The carbine holds 15: the first two fire steps
/// spend 11 before the first reload, the next 5 before the second, and the
/// crouched and aimed ones the rest. Each reload step is followed by a stop
/// long enough for the reload to end before the next shot.
const TORSO_SCRIPT: &[Step] = &[
    step("settle", 1500, 0, 0, false),
    fire(step("fire", 1500, 0, 0, false)),
    fire(step("run_fire", 1800, 127, 0, false)),
    step("stop", 1000, 0, 0, false),
    reload(step("back_reload", 1800, -127, 0, false)),
    step("stop2", 2000, 0, 0, false),
    fire(step("strafe_fire", 1500, 0, 127, false)),
    reload(step("strafe_reload", 1500, 0, -127, false)),
    step("stop3", 2000, 0, 0, false),
    step("crouch", 800, 0, 0, true),
    fire(step("crouch_fire", 1200, 0, 0, true)),
    fire(step("crouch_run_fire", 1200, 127, 0, true)),
    reload(step("crouch_back_reload", 1500, -127, 0, true)),
    step("crouch_stop", 2000, 0, 0, true),
    step("stand", 1000, 0, 0, false),
    ads(step("ads", 800, 0, 0, false)),
    ads(fire(step("ads_fire", 1500, 0, 0, false))),
    ads(fire(step("ads_walk_fire", 1500, 127, 0, false))),
    step("end", 1500, 0, 0, false),
];

/// Stop and go: the legs leave a run some 350 ms after its keys let go,
/// once the body has slowed, so each short stop holds 400-500 ms and the
/// next go takes the run back inside its 120-250 ms blend to the idle,
/// while it still has weight (`legs_ab.rs`, cod11-combat.md 3.1, "The
/// setter on a weighted node"); the strafe swap does the same in one
/// 150 ms step. Each long stop lets every node settle first. Forward runs
/// 3 s and backward 1.7 s, inside the floor's 680 units.
const STOPGO_SCRIPT: &[Step] = &[
    step("settle", 1500, 0, 0, false),
    step("run", 1000, 127, 0, false),
    step("stop", 1000, 0, 0, false),
    step("back", 1000, -127, 0, false),
    step("back_stop", 400, 0, 0, false),
    step("back_go", 700, -127, 0, false),
    step("stop2", 1000, 0, 0, false),
    step("go", 700, 127, 0, false),
    step("go_stop", 400, 0, 0, false),
    step("go2", 500, 127, 0, false),
    step("go_stop2", 450, 0, 0, false),
    step("go3", 400, 127, 0, false),
    step("go_stop3", 500, 0, 0, false),
    step("go4", 400, 127, 0, false),
    step("stop3", 1000, 0, 0, false),
    step("strafe_right", 700, 0, 127, false),
    step("strafe_swap", 150, 0, -127, false),
    step("strafe_right2", 600, 0, 127, false),
    step("stop4", 1000, 0, 0, false),
    step("strafe_left", 1300, 0, -127, false),
    step("stop5", 1000, 0, 0, false),
];

/// Which walk a `--save-legs` run takes, off its `--capture-tag`.
pub enum Walk {
    Legs,
    Torso,
    StopGo,
}

impl Walk {
    pub fn of_tag(tag: Option<&str>) -> Self {
        match tag {
            Some(t) if t.starts_with("torso") => Walk::Torso,
            Some(t) if t.starts_with("stopgo") => Walk::StopGo,
            _ => Walk::Legs,
        }
    }
}

/// The last cmds a placement probe sent before it saw the placement, as
/// `!cmd` lines at `ms=0`. The probe sends a frame's cmd before it reads
/// that frame's snapshot, so the ones past the placement snapshot's
/// `commandTime` ran after the placement on the server and belong in the
/// log (cod11-movers.md 15, "The cmd at the placement").
#[derive(Default)]
pub struct BeforePlacement(std::collections::VecDeque<(i32, String)>);

impl BeforePlacement {
    pub fn push(&mut self, server_time: i32, line: String) {
        if self.0.len() == 8 {
            self.0.pop_front();
        }
        self.0.push_back((server_time, line));
    }

    /// The kept lines the server had not run by `command_time`.
    pub fn past(&mut self, command_time: i32) -> impl Iterator<Item = String> + '_ {
        self.0
            .drain(..)
            .filter(move |(st, _)| *st > command_time)
            .map(|(_, l)| l)
    }
}

pub struct LegsProbe {
    script: &'static [Step],
    torso: bool,
    /// Wall clock and server time of the snapshot that showed the placement.
    placed: Option<(Instant, i32)>,
    before: BeforePlacement,
    seen: Option<u32>,
    lines: Vec<String>,
    done: bool,
}

impl LegsProbe {
    pub fn new(walk: Walk) -> Self {
        Self {
            script: match walk {
                Walk::Legs => SCRIPT,
                Walk::Torso => TORSO_SCRIPT,
                Walk::StopGo => STOPGO_SCRIPT,
            },
            torso: matches!(walk, Walk::Torso),
            placed: None,
            before: BeforePlacement::default(),
            seen: None,
            lines: Vec::new(),
            done: false,
        }
    }

    pub fn running(&self) -> bool {
        !self.done
    }

    fn ms(&self, now: Instant) -> Option<u128> {
        self.placed
            .map(|(t, _)| now.saturating_duration_since(t).as_millis())
    }

    /// The step at `now` and how far into it.
    fn step_at(&self, now: Instant) -> Option<(&'static Step, u128)> {
        let mut ms = self.ms(now)?;
        for s in self.script {
            if ms < s.ms {
                return Some((s, ms));
            }
            ms -= s.ms;
        }
        None
    }

    fn step(&self, now: Instant) -> Option<&'static Step> {
        self.step_at(now).map(|(s, _)| s)
    }

    /// The cmd for `now`, in world angles: `send_frame` takes the
    /// playerstate's `delta_angles` off.
    pub fn cmd(&self, now: Instant) -> UserCmd {
        let mut cmd = UserCmd {
            angles: [0, 0, 0],
            ..NULL_USERCMD
        };
        if let Some((s, into)) = self.step_at(now) {
            cmd.forward = s.forward;
            cmd.right = s.right;
            if s.crouch {
                cmd.wbuttons |= WBUTTON_CROUCH;
                cmd.up = -127;
            }
            if s.fire && into % FIRE_PERIOD_MS < TAP_MS {
                cmd.buttons |= BUTTON_ATTACK;
            }
            if s.reload && into < TAP_MS {
                cmd.wbuttons |= WBUTTON_RELOAD;
            }
            if s.ads {
                cmd.buttons |= BUTTON_ADS;
            }
        }
        cmd
    }

    /// A cmd as it went on the wire, beside the world angles it was built
    /// from.
    /// Before the placement it goes in as `!pcmd`, without `ms`, once a
    /// snapshot has come, and [`BeforePlacement`] keeps it for the `ms=0`
    /// lines.
    pub fn record(&mut self, now: Instant, sent: UserCmd, world: [i32; 3]) {
        if self.done {
            return;
        }
        let line = |head: &str| {
            format!(
                "{head} {} world={},{},{}",
                crate::probe::cmd_fields(&sent),
                world[0],
                world[1],
                world[2],
            )
        };
        match self.ms(now) {
            Some(ms) => self.lines.push(line(&format!("!cmd ms={ms}"))),
            None => {
                if self.seen.is_some() {
                    self.lines.push(line("!pcmd"));
                }
                self.before.push(sent.server_time, line("!cmd ms=0"));
            }
        }
    }

    /// Takes the placement off the first snapshot that stands on the mark,
    /// and logs every snapshot: `!psnap` from the join to the placement,
    /// `!snap` from it on. True once the script has run out and a second
    /// has passed.
    pub fn observe(&mut self, now: Instant, s: &Snapshot) -> bool {
        let p = crate::probe::wire();
        let o = s.ps.origin(p);
        let mut pre = Vec::new();
        if self.placed.is_none()
            && (o[0] - LEGS_SPOT[0]).abs() < 0.5
            && (o[1] - LEGS_SPOT[1]).abs() < 0.5
        {
            self.placed = Some((now, s.server_time));
            println!("legs: placed at serverTime {}", s.server_time);
            pre = self.before.past(s.ps.field_i32(p, "commandTime")).collect();
        }
        if self.seen != Some(s.message_num) {
            self.seen = Some(s.message_num);
            let v = [0, 1, 2].map(|i| s.ps.field_f32(p, &format!("velocity[{i}]")));
            let (head, label) = match self.placed {
                Some(_) => ("!snap", self.step(now).map_or("end", |st| st.label)),
                None => ("!psnap", "join"),
            };
            self.lines.push(format!(
                "{head} msg={} t={} ct={} step={label} origin={},{},{} vel={},{},{} legs={} torso={} movementDir={} pm_flags={} view={},{} weaponstate={} weapAnim={} pm_type={} delta={},{}",
                s.message_num,
                s.server_time,
                s.ps.field_i32(p, "commandTime"),
                o[0],
                o[1],
                o[2],
                v[0],
                v[1],
                v[2],
                s.ps.field_i32(p, "legsAnim"),
                s.ps.field_i32(p, "torsoAnim"),
                s.ps.field_i32(p, "movementDir"),
                s.ps.field_i32(p, "pm_flags"),
                s.ps.viewangles(p)[0],
                s.ps.viewangles(p)[1],
                s.ps.field_i32(p, "weaponstate"),
                s.ps.field_i32(p, "weapAnim"),
                s.ps.field_i32(p, "pm_type"),
                s.ps.field_i32(p, "delta_angles[0]"),
                s.ps.field_i32(p, "delta_angles[1]"),
            ));
        }
        self.lines.extend(pre);
        let total: u128 = self.script.iter().map(|s| s.ms).sum();
        if self.ms(now).is_some_and(|ms| ms >= total + 1000) {
            self.done = true;
        }
        self.done
    }

    /// The fixture: the recipe, then every `!cmd` and `!snap` in order.
    pub fn write(
        &self,
        configstrings: &[String],
        team: &str,
        weapon: &str,
        path_dir: &str,
        tag: Option<&str>,
        overwrite: bool,
    ) -> anyhow::Result<()> {
        let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
        let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
        let map = key("mapname");
        let mut out = String::new();
        out.push_str(&format!(
            "# Retail CoD {} dedicated server: one player walking a fixed script, the\n\
             # client half of the moving-body pose capture.\n",
            crate::probe::retail_build()
        ));
        out.push_str(&format!(
            "# map {map}, gametype {}, team={team} weapon={weapon}\n",
            key("g_gametype")
        ));
        out.push_str(&format!(
            "# placed serverTime={} spot={},{},{}\n",
            self.placed.map_or(0, |p| p.1),
            LEGS_SPOT[0],
            LEGS_SPOT[1],
            LEGS_SPOT[2]
        ));
        out.push_str(if self.torso {
            "# client-probes/probe_torso under tools/run_probe.sh puts the player on the spot at\n\
             # level time 25000 and logs its bones per frame (the server half, -torso.txt).\n\
             # Script from the snapshot that showed the placement, world yaw 0 throughout:\n#"
        } else {
            "# client-probes/probe_legs under tools/run_probe.sh puts the player on the spot at\n\
             # level time 25000 and logs its bones per frame (the server half, -legs.txt).\n\
             # Script from the snapshot that showed the placement, world yaw 0 throughout:\n#"
        });
        for s in self.script {
            out.push_str(&format!(
                " {} {}ms f{} r{}{}{}{}{};",
                s.label,
                s.ms,
                s.forward,
                s.right,
                if s.crouch { " crouch" } else { "" },
                if s.fire { " fire" } else { "" },
                if s.reload { " reload" } else { "" },
                if s.ads { " ads" } else { "" },
            ));
        }
        out.push('\n');
        out.push_str(
            "# !cmd is a usercmd as it went on the wire (angles rebased on delta_angles; world=\n\
             # the angles before), ms after the placement; ms=0 after the first !snap are the\n\
             # cmds sent before the probe saw the placement that its commandTime had not run.\n\
             # !snap is the playerstate per snapshot: t serverTime, ct commandTime, legs/torso\n\
             # the wire anim values. !pcmd and !psnap are the same from the join to the\n\
             # placement.\n",
        );
        if !self.done {
            out.push_str("# BROKEN the run ended before the script did\n");
        }
        for l in &self.lines {
            out.push_str(l);
            out.push('\n');
        }
        let path = crate::probe::write_fixture_file(
            path_dir,
            &crate::probe::tagged_name(&format!("{map}-dm-legs-client"), tag),
            &out,
            overwrite,
        )?;
        println!("legs: {} lines -> {path}", self.lines.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 1.5 legs capture's placement: the snapshot at 25000 had run the
    /// cmds to 24982, so the one at 25000 goes in the log and the older
    /// ones do not.
    #[test]
    fn the_cmds_past_the_placement_commandtime_are_kept() {
        let mut before = BeforePlacement::default();
        for st in [24950, 24966, 24982, 25000] {
            before.push(st, format!("st={st}"));
        }
        assert_eq!(before.past(24982).collect::<Vec<_>>(), ["st=25000"]);
        assert_eq!(before.past(0).count(), 0, "drained");
    }
}
