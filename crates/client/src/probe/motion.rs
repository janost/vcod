//! `--save-motion`: the motion script and its fixture.

use super::fixture::{PLAYERSTATE_FIXTURE_DIR, write_capture};
use super::*;

/// One held input and the label the playerstate settled under it is captured
/// under.
struct MotionStep {
    label: &'static str,
    cmd: net::msg::UserCmd,
    until: Until,
}

/// When a step's capture is taken.
enum Until {
    /// After the input has been held this long, so the pose has settled and
    /// the value is a steady state rather than a point on a lerp.
    Held(Duration),
    /// At the first snapshot off the ground, which is what puts the jump's
    /// takeoff velocity in the capture. The duration bounds the wait.
    Airborne(Duration),
    /// At the first snapshot back on the ground, which is where the landing
    /// anim is. An event anim expires, so a settled sample cannot hold one.
    Grounded(Duration),
}

/// The poses, in an order where each starts from the one before: lean from
/// standing, prone from crouched, run from standing again. Bit names:
/// docs/protocol-1.1.md, "Usercmd input bits".
fn motion_script() -> Vec<MotionStep> {
    use net::msg::{
        NULL_USERCMD, WBUTTON_CROUCH, WBUTTON_LEAN_LEFT, WBUTTON_LEAN_RIGHT, WBUTTON_PRONE,
    };
    let held = |label, cmd, ms| MotionStep {
        label,
        cmd,
        until: Until::Held(Duration::from_millis(ms)),
    };
    vec![
        held("stand", NULL_USERCMD, 1500),
        held(
            "lean_left",
            net::msg::UserCmd {
                wbuttons: WBUTTON_LEAN_LEFT,
                ..NULL_USERCMD
            },
            2000,
        ),
        // Between the leans, so the capture shows whether a released lean
        // returns to centre server-side.
        held("center", NULL_USERCMD, 1500),
        held(
            "lean_right",
            net::msg::UserCmd {
                wbuttons: WBUTTON_LEAN_RIGHT,
                ..NULL_USERCMD
            },
            2000,
        ),
        // A crouched or prone client holds `up` at -127 for as long as it is
        // down, so the capture holds it too.
        held(
            "crouch",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                ..NULL_USERCMD
            },
            2500,
        ),
        // Standing between the two lowered stances: a prone held straight out
        // of a crouch was refused in one capture and taken in another.
        held("stand_between", NULL_USERCMD, 1500),
        held(
            "prone",
            net::msg::UserCmd {
                wbuttons: WBUTTON_PRONE,
                up: -127,
                ..NULL_USERCMD
            },
            3000,
        ),
        // Turning the view while already prone: this is what separates a
        // `proneDirection` the server writes from one it leaves at zero, and
        // it is where `bg_prone_yawcap` (85 degrees) bites.
        held(
            "prone_yaw_60",
            net::msg::UserCmd {
                angles: [0, 60 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 0,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "prone_yaw_150",
            net::msg::UserCmd {
                angles: [0, 150 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 0,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "prone_crawl_150",
            net::msg::UserCmd {
                angles: [0, 150 * 65536 / 360, 0],
                wbuttons: WBUTTON_PRONE,
                up: -127,
                forward: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held("stand_up", NULL_USERCMD, 2500),
        held(
            "run_forward",
            net::msg::UserCmd {
                forward: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "run_back",
            net::msg::UserCmd {
                forward: -127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "strafe_left",
            net::msg::UserCmd {
                right: -127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "strafe_right",
            net::msg::UserCmd {
                right: 127,
                ..NULL_USERCMD
            },
            2000,
        ),
        held(
            "crouch_run",
            net::msg::UserCmd {
                wbuttons: WBUTTON_CROUCH,
                up: -127,
                forward: 127,
                ..NULL_USERCMD
            },
            2500,
        ),
        held("stand_after_crouch", NULL_USERCMD, 1500),
        // Turning on the spot. Nothing derives the turn movetypes yet, so
        // this is evidence rather than a gate: it records whether retail
        // leaves the idle anim alone while the view sweeps.
        held(
            "turn_left",
            net::msg::UserCmd {
                angles: [0, -60 * 65536 / 360, 0],
                ..NULL_USERCMD
            },
            1500,
        ),
        held(
            "turn_right",
            net::msg::UserCmd {
                angles: [0, 60 * 65536 / 360, 0],
                ..NULL_USERCMD
            },
            1500,
        ),
        MotionStep {
            label: "jump_takeoff",
            cmd: net::msg::UserCmd {
                up: 127,
                ..NULL_USERCMD
            },
            until: Until::Airborne(Duration::from_millis(1500)),
        },
        MotionStep {
            label: "land",
            cmd: NULL_USERCMD,
            until: Until::Grounded(Duration::from_millis(1500)),
        },
        // Ads standing last, and not in the middle: the `weapon_position ads`
        // clauses are the only ones a rifleman can otherwise never reach, and
        // retail keeps the ads flag and the ads legs pose for every pose that
        // follows the one that pressed the bit, so anywhere else in the
        // script this step takes the rest of the capture with it.
        held(
            "ads_stand",
            net::msg::UserCmd {
                buttons: 0x10,
                ..NULL_USERCMD
            },
            2000,
        ),
    ]
}

/// Walks [`motion_script`], holding each input and keeping the playerstate it
/// settles at.
pub(super) struct MotionProbe {
    steps: Vec<MotionStep>,
    idx: usize,
    started: Option<Instant>,
    captured: Vec<(&'static str, Vec<i32>)>,
    done: bool,
    /// Newest snapshot already traced, so the trace prints per snapshot
    /// rather than per loop iteration.
    traced: Option<u32>,
    /// The view yaw at the spawn, which every pose's own yaw is an offset
    /// from ([`hold_view_yaw`]).
    pub(super) spawn_yaw: ViewHold,
}

impl Default for MotionProbe {
    fn default() -> Self {
        Self {
            steps: motion_script(),
            idx: 0,
            started: None,
            captured: Vec::new(),
            done: false,
            traced: None,
            spawn_yaw: ViewHold::default(),
        }
    }
}

impl MotionProbe {
    pub(super) fn running(&self) -> bool {
        !self.done
    }

    pub(super) fn cmd(&self) -> net::msg::UserCmd {
        self.steps
            .get(self.idx)
            .map(|s| s.cmd)
            .unwrap_or(net::msg::NULL_USERCMD)
    }

    /// Feeds the newest snapshot in. Returns true once the last pose is
    /// captured and the fixture is ready to write.
    pub(super) fn step(&mut self, now: Instant, snap: &net::snapshot::Snapshot) -> bool {
        let Some(step) = self.steps.get(self.idx) else {
            self.done = true;
            return true;
        };
        let started = *self.started.get_or_insert(now);
        let elapsed = now.duration_since(started);
        let p = wire();
        // A settled sample cannot tell a ramp from a constant; the trace can.
        if self.traced != Some(snap.message_num) {
            self.traced = Some(snap.message_num);
            println!(
                "  trace {} +{:>4}ms st={} vh={:>5.1} proneDir={:>8.2} dirPitch={:>7.2} torso={:>7.2} origin=[{:>8.1},{:>8.1}] deltaYaw={} eFlags={} pm=0x{:x} legsAnim={} torsoAnim={} weapAnim={}",
                self.steps[self.idx].label,
                elapsed.as_millis(),
                snap.server_time,
                f32::from_bits(snap.ps.field_i32(p, "viewHeightCurrent") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneDirection") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneDirectionPitch") as u32),
                f32::from_bits(snap.ps.field_i32(p, "proneTorsoPitch") as u32),
                f32::from_bits(snap.ps.field_i32(p, "origin[0]") as u32),
                f32::from_bits(snap.ps.field_i32(p, "origin[1]") as u32),
                snap.ps.field_i32(p, "delta_angles[1]"),
                snap.ps.field_i32(p, "eFlags"),
                snap.ps.field_i32(p, "pm_flags"),
                snap.ps.field_i32(p, "legsAnim"),
                snap.ps.field_i32(p, "torsoAnim"),
                snap.ps.field_i32(p, "weapAnim"),
            );
        }
        let take = match step.until {
            Until::Held(d) => elapsed >= d,
            Until::Airborne(limit) => {
                let airborne = snap.ps.field_i32(p, "groundEntityNum")
                    != net::protocol::ENTITYNUM_WORLD as i32;
                (airborne && elapsed >= Duration::from_millis(50)) || elapsed >= limit
            }
            Until::Grounded(limit) => {
                let grounded = snap.ps.field_i32(p, "groundEntityNum")
                    == net::protocol::ENTITYNUM_WORLD as i32;
                (grounded && elapsed >= Duration::from_millis(50)) || elapsed >= limit
            }
        };
        if !take {
            return false;
        }
        println!(
            "MOTION {}: leanf={} viewHeightCurrent={} viewHeightTarget={} viewHeightLerp[target={} time={} down={} posAdj={}] \
bobCycle={} groundEntityNum={} pm_flags=0x{:x} pm_time={} jumpTime={} velocity_z={} eventSequence={} events=[{},{},{},{}] \
legsAnim={} torsoAnim={} weapAnim={}",
            step.label,
            f32::from_bits(snap.ps.field_i32(p, "leanf") as u32),
            f32::from_bits(snap.ps.field_i32(p, "viewHeightCurrent") as u32),
            snap.ps.field_i32(p, "viewHeightTarget"),
            snap.ps.field_i32(p, "viewHeightLerpTarget"),
            snap.ps.field_i32(p, "viewHeightLerpTime"),
            snap.ps.field_i32(p, "viewHeightLerpDown"),
            f32::from_bits(snap.ps.field_i32(p, "viewHeightLerpPosAdj") as u32),
            snap.ps.field_i32(p, "bobCycle"),
            snap.ps.field_i32(p, "groundEntityNum"),
            snap.ps.field_i32(p, "pm_flags"),
            snap.ps.field_i32(p, "pm_time"),
            snap.ps.field_i32(p, "jumpTime"),
            f32::from_bits(snap.ps.field_i32(p, "velocity[2]") as u32),
            snap.ps.field_i32(p, "eventSequence"),
            ev_v1(snap.ps.field_i32(p, "events[0]")),
            ev_v1(snap.ps.field_i32(p, "events[1]")),
            ev_v1(snap.ps.field_i32(p, "events[2]")),
            ev_v1(snap.ps.field_i32(p, "events[3]")),
            snap.ps.field_i32(p, "legsAnim"),
            snap.ps.field_i32(p, "torsoAnim"),
            snap.ps.field_i32(p, "weapAnim"),
        );
        self.captured.push((step.label, snap.ps.fields.clone()));
        self.idx += 1;
        self.started = None;
        if self.idx >= self.steps.len() {
            self.done = true;
            return true;
        }
        false
    }
}

/// Writes one section per pose to `<map>-<gametype>-motion.txt`. Each carries
/// the input that produced it on a `!input` line, so the gate replays the
/// capture's own script rather than a copy of it.
pub(super) fn write_motion_fixture(
    snap: &net::snapshot::Snapshot,
    configstrings: &[String],
    join: &JoinProbe,
    motion: &MotionProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let p = wire();
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server playerstate under each movement input.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str("# Captured with tools/run_server.sh and --net-probe --save-motion. Each pose\n");
    out.push_str("# holds one usercmd until the state settles, so the values are steady states\n");
    out.push_str("# rather than points on a lerp; jump_takeoff is the first snapshot off the\n");
    out.push_str("# ground instead, which is where the takeoff velocity is.\n");
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns.\n");
    for (label, fields) in &motion.captured {
        let cmd = motion
            .steps
            .iter()
            .find(|s| s.label == *label)
            .map(|s| s.cmd)
            .unwrap_or(net::msg::NULL_USERCMD);
        out.push_str(&format!("[pose {label}]\n"));
        let sample = motion
            .steps
            .iter()
            .find(|s| s.label == *label)
            .map(|s| match s.until {
                Until::Held(_) => "settled",
                Until::Airborne(_) => "airborne",
                Until::Grounded(_) => "grounded",
            })
            .unwrap_or("settled");
        out.push_str(&format!(
            "!input buttons={} wbuttons={} up={} forward={} right={} yaw={} sample={sample}\n",
            cmd.buttons, cmd.wbuttons, cmd.up, cmd.forward, cmd.right, cmd.angles[1]
        ));
        for (f, v) in p.player_fields.iter().zip(fields) {
            out.push_str(&format!("{} {v}\n", f.name));
        }
    }
    let path = fixture_path(
        PLAYERSTATE_FIXTURE_DIR,
        &format!("{map}-{gametype}-motion{}.txt", tag_suffix(tag)),
    )?;
    write_capture(&path, &out, overwrite)?;
    println!(
        "motion: {} poses ({} fields each) -> {path}",
        motion.captured.len(),
        snap.ps.fields.len()
    );
    Ok(())
}
