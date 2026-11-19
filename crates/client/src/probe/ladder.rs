//! `--probe-ladder`, `--save-ladder` and `--probe-weapsel`: the play client's own input on a ladder
//! and its weapon selection.

use super::fixture::{
    PLAYERSTATE_FIXTURE_DIR, cmd_fields, nonzero_pairs, pairs_str, serverinfo_key, tagged_name,
    write_fixture_file,
};
use super::*;

/// `--probe-ladder` and `--probe-weapsel`: every cmd built by the play
/// client's own `PlayInput`, with the playerstate, the own events and the
/// `a` command routed into its weapon selection as the play client routes
/// them, so the weapon byte is the one the play client sends a retail
/// server. The ladder script climbs mp_ship's hold ladder and off the top,
/// then presses the two weapon binds a stuck player would reach for; the
/// weapon-select script pulls the trigger on the rifle
/// `client-probes/probe_weapsel` empties.
/// With `--save-ladder` the climb runs the script its value picks
/// ([`LADDER_SCRIPT`], [`LADDER_CLIMB_SCRIPT`], [`LADDER_TURN_SCRIPT`] or
/// [`LADDER_GRAB_SCRIPT`]) and records every cmd and snapshot.
#[derive(Default)]
pub(super) struct LadderProbe {
    pub(super) input: crate::play::input::PlayInput,
    pub(super) weapons: Vec<Option<vcod_common::weapon::WeaponDef>>,
    /// Configstring 7's names, which `weapon <name>` resolves against.
    pub(super) names: Vec<String>,
    pub(super) events: vcod_common::net::events::EventTracker,
    pub(super) weapsel: bool,
    pub(super) start: Option<Instant>,
    pub(super) pressed: u8,
    pub(super) sent: u8,
    pub(super) last: Option<Vec<i32>>,
    /// `--save-ladder`'s `[phase]`, `!cmd` and `!snap` lines; `None` without it.
    pub(super) capture: Option<Vec<String>>,
    pub(super) notes: Vec<String>,
    /// The script's phase the last cmd was built in.
    pub(super) phase: Option<usize>,
    /// Wall clock of the first snapshot at the foot after the placement.
    pub(super) placed: Option<Instant>,
    /// Where the body first held the rungs.
    pub(super) foot: [f32; 3],
    /// `delta_angles[1]` at the placement: the turn script's yaw goes on the
    /// wire rebased on it, as a mouse's would.
    pub(super) placed_delta: i32,
    /// `--save-ladder`'s script.
    pub(super) script: LadderScript,
    /// The weapon-select fixture's `!cmd`, `!server` and `!trace` rows, each
    /// timed from the first cmd.
    pub(super) rows: Vec<String>,
    pub(super) t0: Option<Instant>,
    /// The last snapshot a `!trace` was written for.
    pub(super) traced: Option<i32>,
}

/// Which of the four `--save-ladder` scripts runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum LadderScript {
    #[default]
    Full,
    Climb,
    Turn,
    Grab,
}

impl LadderScript {
    fn phases(self) -> &'static [LadderPhase] {
        match self {
            LadderScript::Full => LADDER_SCRIPT,
            LadderScript::Climb => LADDER_CLIMB_SCRIPT,
            LadderScript::Turn => LADDER_TURN_SCRIPT,
            LadderScript::Grab => LADDER_GRAB_SCRIPT,
        }
    }

    /// The fixture header's name for it.
    fn name(self) -> &'static str {
        match self {
            LadderScript::Full => "full",
            LadderScript::Climb => "climb",
            LadderScript::Turn => "turn",
            LadderScript::Grab => "grab",
        }
    }
}

/// How long the climb holds forward: the hold ladder is 560 units.
const LADDER_CLIMB: Duration = Duration::from_secs(14);

/// `--save-ladder`'s script: (phase, ms, forwardmove, rightmove, view pitch
/// in degrees, positive down, view yaw offset in degrees). Run once the player has stood at the foot
/// for [`LADDER_SETTLE`]. Pitches straddle the climb rate's clamps (fz
/// -0.25 and 0.15) and its zero (fz = -0.25 is pitch 14.5).
const LADDER_SCRIPT: &[LadderPhase] = &[
    ("up-level", 2500, 127, 0, 0.0, 0.0),
    ("hang", 1000, 0, 0, 0.0, 0.0),
    ("up-look-up", 1200, 127, 0, -45.0, 0.0),
    ("down-back", 1500, -127, 0, 0.0, 0.0),
    ("strafe-right", 400, 0, 127, 0.0, 0.0),
    ("drift", 600, 0, 0, 0.0, 0.0),
    ("up-strafe-left", 600, 127, -127, -20.0, 0.0),
    ("release", 600, 0, 0, -20.0, 0.0),
    ("down-look-down", 1000, 127, 0, 60.0, 0.0),
    ("up-slight-down", 1000, 127, 0, 10.0, 0.0),
    ("up-top", 11000, 127, 0, 0.0, 0.0),
    ("off", 1000, 0, 0, 0.0, 0.0),
];

/// `--save-ladder climb`: straight up looking up, and off the top.
const LADDER_CLIMB_SCRIPT: &[LadderPhase] = &[
    ("up-look-up", 9000, 127, 0, -45.0, 0.0),
    ("off", 1500, 0, 0, -45.0, 0.0),
];

/// `--save-ladder turn`: up off the ground, then the view swept away from
/// the rungs past `bg_ladder_yawcap` (100) both ways while hanging, a climb
/// and a flick while turned, and the same sweep back at the foot on the
/// ground. The yaw column is the offset off the climb yaw each phase ends
/// at, swept linearly from the last phase's, and goes on the wire as a
/// mouse would send it: rebased on the `delta_angles` the script started
/// with, not on each snapshot's.
const LADDER_TURN_SCRIPT: &[LadderPhase] = &[
    ("up", 1500, 127, 0, 0.0, 0.0),
    ("hang", 500, 0, 0, 0.0, 0.0),
    ("turn-left", 1500, 0, 0, 0.0, 160.0),
    ("hold-left", 800, 0, 0, 0.0, 160.0),
    ("back-left", 1000, 0, 0, 0.0, 0.0),
    ("turn-right", 1500, 0, 0, 0.0, -170.0),
    ("hold-right", 800, 0, 0, 0.0, -170.0),
    ("back-right", 1000, 0, 0, 0.0, 0.0),
    ("up-turning", 1500, 127, 0, -20.0, 130.0),
    ("flick", 150, 0, 0, 0.0, -140.0),
    ("hold-flick", 800, 0, 0, 0.0, -140.0),
    ("back-flick", 500, 0, 0, 0.0, 0.0),
    ("down", 4000, -127, 0, 0.0, 0.0),
    ("foot-turn", 1500, 0, 0, 0.0, 160.0),
    ("foot-hold", 800, 0, 0, 0.0, 160.0),
];

/// `--save-ladder grab`: from the first live snapshot, still and facing the
/// probe's yaw while the gsc sets the body down at `probe_spot`, then turned
/// 180 still and forward into the rungs: a grab that only the view's trace
/// meets (cod15-movement.md 2, the second grab). Its yaw stays absolute.
const LADDER_GRAB_SCRIPT: &[LadderPhase] = &[
    ("wait", 2500, 0, 0, 0.0, 0.0),
    ("face", 500, 0, 0, 0.0, -180.0),
    ("still", 500, 0, 0, 0.0, -180.0),
    ("walk", 2500, 127, 0, 0.0, -180.0),
    ("hold", 500, 0, 0, 0.0, -180.0),
];

type LadderPhase = (&'static str, u64, i8, i8, f32, f32);

const LADDER_SETTLE: Duration = Duration::from_millis(1500);

/// When the weapon-select script pulls the trigger, from the settled join:
/// a 150 ms press every 700 ms, which `probe_weapsel` has emptied the rifle
/// for by then.
const WEAPSEL_FIRE: std::ops::Range<Duration> = Duration::from_secs(15)..Duration::from_secs(19);

/// The weapon-select script's commands past the dry pulls, from the settled
/// join: `probe_weapsel` has put the thompson in hand by 22 s. Two
/// `weapalt`s switch it to semi and back, `weapon colt_mp` takes the pistol
/// by name, and a `weapalt` on the pistol has no mode to switch to. The
/// thompson then comes back, and the pistol is asked for again 500 ms on,
/// the pistol's 300 ms drop and 200 ms into the thompson's 400 ms raise.
const WEAPSEL_COMMANDS: [(Duration, &str); 6] = [
    (Duration::from_secs(26), "weapalt"),
    (Duration::from_secs(30), "weapalt"),
    (Duration::from_secs(34), "weapon colt_mp"),
    (Duration::from_secs(38), "weapalt"),
    (Duration::from_secs(42), "weapon thompson_mp"),
    (Duration::from_millis(42500), "weapon colt_mp"),
];

impl LadderProbe {
    /// Configstring 7's weapons, once, for the selection's ammo and alt
    /// links.
    pub(super) fn load_weapons(&mut self, fs: Option<&vcod_common::pk3::Pk3Fs>, cs7: &str) {
        if self.weapons.is_empty()
            && !cs7.is_empty()
            && let Some(fs) = fs
        {
            self.weapons = vcod_common::weapon_table::from_configstring(fs, cs7);
            self.names = cs7.split(' ').map(str::to_string).collect();
        }
    }

    /// Every cmd from the first snapshot on, as the play client builds them;
    /// the scripts wait for the join to settle.
    pub(super) fn cmd(
        &mut self,
        now: Instant,
        settled: bool,
        yaw: Option<f32>,
        snap: Option<&net::snapshot::Snapshot>,
    ) -> net::msg::UserCmd {
        use crate::play::input::{Action, Held};
        if self.capture.is_some() {
            return self.script_cmd(now, yaw, snap);
        }
        if settled {
            self.start.get_or_insert(now);
        }
        let t = self.start.map_or(Duration::ZERO, |s| now.duration_since(s));
        if self.weapsel {
            let pulse = WEAPSEL_FIRE.contains(&t) && t.as_millis() % 700 < 150;
            self.input
                .key(Action::Attack, self.start.is_some() && pulse);
            let due = if self.start.is_some() {
                WEAPSEL_COMMANDS.len()
            } else {
                0
            };
            for (at, line) in &WEAPSEL_COMMANDS[usize::from(self.pressed)..due] {
                if t < *at {
                    break;
                }
                // As the play client runs them (main.rs, `Effect::Impulse`
                // and `Effect::Weapon`).
                let outcome = match line.split_once(' ') {
                    Some(("weapon", arg)) => {
                        let names = &self.names;
                        let index = |n: &str| {
                            names
                                .iter()
                                .position(|w| w.eq_ignore_ascii_case(n))
                                .and_then(|i| u8::try_from(i + 1).ok())
                        };
                        format!("{}", self.input.weapon_command(arg, index))
                    }
                    _ => format!("{:?}", self.input.alt_weapon()),
                };
                let line = format!(
                    "line={line:?} outcome={outcome} selected={}",
                    self.input.selected()
                );
                println!("WEAPSEL press {line}");
                self.row(now, "press", line);
                self.pressed += 1;
            }
        } else {
            let binds = if self.start.is_some() { 2 } else { 0 };
            self.input
                .key(Action::Forward, self.start.is_some() && t < LADDER_CLIMB);
            let presses = [
                (LADDER_CLIMB + Duration::from_secs(3), Action::Slot(1)),
                (LADDER_CLIMB + Duration::from_secs(6), Action::NextWeapon),
            ];
            for (at, a) in &presses[usize::from(self.pressed)..binds] {
                if t < *at {
                    break;
                }
                println!("LADDER press {a:?}");
                self.input.key(*a, true);
                self.input.key(*a, false);
                self.pressed += 1;
            }
        }
        let held = snap.map_or_else(Held::default, |s| {
            Held::from_ps(&s.ps, wire(), &self.weapons)
        });
        let mut cmd = self.input.build(0, &held);
        // Absolute: `send_frame` takes `delta_angles` off.
        if let Some(yaw) = yaw {
            cmd.angles[1] = deg_to_short(yaw);
        }
        self.sent = cmd.weapon;
        cmd
    }

    /// `--save-ladder`: the play client's weapon byte with the script's keys
    /// and pitch.
    fn script_cmd(
        &mut self,
        now: Instant,
        yaw: Option<f32>,
        snap: Option<&net::snapshot::Snapshot>,
    ) -> net::msg::UserCmd {
        use crate::play::input::Held;
        let p = wire();
        // Set down at the foot, the body takes the rungs before any key.
        // The grab script starts off the rungs, at the first live snapshot.
        let grab = self.script == LadderScript::Grab;
        if self.placed.is_none()
            && let Some(s) = snap
            && s.ps.field_i32(p, "pm_type") == PM_NORMAL
            && (grab || s.ps.field_i32(p, "pm_flags") & net::flags::PMF_LADDER != 0)
        {
            self.placed = Some(now);
            self.foot = s.ps.origin(p);
            self.placed_delta = s.ps.field_i32(p, "delta_angles[1]");
        }
        let held = snap.map_or_else(Held::default, |s| Held::from_ps(&s.ps, p, &self.weapons));
        let mut cmd = self.input.build(0, &held);
        if let Some(yaw) = yaw {
            cmd.angles[1] = deg_to_short(yaw);
        }
        self.sent = cmd.weapon;
        let Some(t) = self
            .placed
            .and_then(|at| now.checked_duration_since(at + LADDER_SETTLE))
        else {
            return cmd;
        };
        let mut end = 0;
        let Some(i) = self.script.phases().iter().position(|&(_, ms, ..)| {
            end += ms;
            t.as_millis() < u128::from(end)
        }) else {
            self.phase = Some(self.script.phases().len());
            return cmd;
        };
        let phases = self.script.phases();
        let (name, ms, forward, right, pitch, to) = phases[i];
        let from = i.checked_sub(1).map_or(0.0, |j| phases[j].5);
        if from != 0.0 || to != 0.0 {
            // `send_frame` takes the newest snapshot's `delta_angles` off;
            // put back what retail pushed since the placement.
            let into = t.as_millis() as f32 - (end - ms) as f32;
            let off = from + (to - from) * (into / ms as f32).min(1.0);
            let pushed = if grab {
                0
            } else {
                snap.map_or(0, |s| s.ps.field_i32(p, "delta_angles[1]")) - self.placed_delta
            };
            cmd.angles[1] = (deg_to_short(yaw.unwrap_or(0.0) + off) + pushed) & 0xffff;
        }
        if self.phase != Some(i) {
            self.phase = Some(i);
            println!("LADDER phase {name}");
            if let Some(lines) = &mut self.capture {
                lines.push(format!("[phase {name}]"));
            }
        }
        cmd.forward = forward;
        cmd.right = right;
        cmd.angles[0] = deg_to_short(pitch);
        cmd
    }

    /// The script ran to its end.
    pub(super) fn captured(&self) -> bool {
        self.phase == Some(self.script.phases().len())
    }

    /// A sent cmd, once the script has started.
    pub(super) fn record(&mut self, c: net::msg::UserCmd) {
        if self.phase.is_none() {
            return;
        }
        if let Some(lines) = &mut self.capture {
            lines.push(format!("!cmd {}", cmd_fields(&c)));
        }
    }

    /// A fixture row, `!kind ms=<from the first cmd> rest`.
    fn row(&mut self, now: Instant, kind: &str, rest: String) {
        if !self.weapsel {
            return;
        }
        let ms = now.duration_since(*self.t0.get_or_insert(now)).as_millis();
        self.rows.push(format!("!{kind} ms={ms} {rest}"));
    }

    /// The cmd as it went on the wire, with its server time.
    pub(super) fn sent_cmd(&mut self, now: Instant, c: &net::msg::UserCmd) {
        self.row(now, "cmd", cmd_fields(c));
    }

    /// A reliable server command; `a <weapon>` selects, as in the play
    /// client.
    pub(super) fn on_command(&mut self, now: Instant, cmd: &str) {
        self.row(now, "server", escape_ctl(cmd));
        let mut words = cmd.split_whitespace();
        if words.next() == Some("a") {
            self.input.server_select(words.next().unwrap_or(""));
            println!("WEAPSEL cmd {cmd:?} selected={}", self.input.selected());
        }
    }

    pub(super) fn observe(&mut self, now: Instant, snap: &net::snapshot::Snapshot) {
        use vcod_common::net::event_ids::{
            EV_AMMO_PICKUP, EV_DROPWEAPON, EV_ITEM_PICKUP, EV_ITEM_PICKUP_QUIET, EV_NOAMMO,
            EV_PUTAWAY_WEAPON, EV_RAISE_WEAPON, EV_WEAPON_ALT,
        };
        let p = wire();
        let i = |n: &str| snap.ps.field_i32(p, n);
        let f = |n: &str| snap.ps.field_f32(p, n);
        self.input.sync(&crate::play::input::Held::from_ps(
            &snap.ps,
            p,
            &self.weapons,
        ));
        let view_body = crate::fx::registry::view_body(i("pm_flags"), i("clientNum"));
        for ev in drain_v1(&mut self.events, snap, p) {
            if let Some(w) = crate::play::events::picked_up_weapon(&ev, view_body, p)
                && self.input.selected() == 0
            {
                self.input.picked_up(w);
            }
            if crate::play::events::runs_out_of_ammo(&ev, view_body) {
                self.input.out_of_ammo();
            }
            if matches!(
                ev.event,
                EV_NOAMMO
                    | EV_DROPWEAPON
                    | EV_ITEM_PICKUP
                    | EV_ITEM_PICKUP_QUIET
                    | EV_AMMO_PICKUP
                    | EV_RAISE_WEAPON
                    | EV_PUTAWAY_WEAPON
                    | EV_WEAPON_ALT
            ) && view_body.is_some_and(|b| ev.entity_num == u32::MAX || ev.entity_num == b)
            {
                println!(
                    "WEAPSEL t={} event={} parm={} selected={}",
                    snap.server_time,
                    ev.event,
                    ev.parm,
                    self.input.selected()
                );
            }
        }
        if self.weapsel && self.traced.replace(snap.server_time) != Some(snap.server_time) {
            let list = |name: &str, map: fn(i32) -> i32| {
                (0..4)
                    .map(|k| map(i(&format!("{name}[{k}]"))).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let rest = format!(
                "serverTime={} commandTime={} weapon={} weaponstate={} weaponTime={} weaponDelay={} \
weapAnim={} weapons={} slots={},{} clip={} ammo={} eventSequence={} events={} eventParms={}",
                snap.server_time,
                i("commandTime"),
                i("weapon"),
                i("weaponstate"),
                i("weaponTime"),
                i("weaponDelay"),
                i("weapAnim"),
                weapon_bits(&snap.ps, p),
                i("weaponslots[0]"),
                i("weaponslots[4]"),
                pairs_str(&nonzero_pairs(&snap.ps.arrays.ammoclip)),
                pairs_str(&nonzero_pairs(&snap.ps.arrays.ammo)),
                i("eventSequence"),
                list("events", p.event_to_v1),
                list("eventParms", |v| v),
            );
            self.row(now, "trace", rest);
        }
        let o = snap.ps.origin(p);
        let view = snap.ps.viewangles(p);
        if self.phase.is_some()
            && let Some(lines) = &mut self.capture
        {
            lines.push(format!(
                "!snap msg={} t={} ct={} origin={},{},{} vel={},{},{} ground={} pm_type={} pm_flags={} pm_time={} da={},{},{} weapon={} view={},{},{} lv={},{},{}",
                snap.message_num,
                snap.server_time,
                i("commandTime"),
                o[0],
                o[1],
                o[2],
                f("velocity[0]"),
                f("velocity[1]"),
                f("velocity[2]"),
                i("groundEntityNum"),
                i("pm_type"),
                i("pm_flags"),
                i("pm_time"),
                i("delta_angles[0]"),
                i("delta_angles[1]"),
                i("delta_angles[2]"),
                i("weapon"),
                view[0],
                view[1],
                view[2],
                f("vLadderVec[0]"),
                f("vLadderVec[1]"),
                f("vLadderVec[2]"),
            ));
        }
        // Every 64 units of height too, which times the climb.
        let key = vec![
            (o[2] / 64.0).floor() as i32,
            i("weapon"),
            i("weaponstate"),
            i("weapAnim"),
            i("pm_flags"),
            i("pm_type"),
            i("weaponTime").signum(),
            i32::from(self.sent),
        ];
        if self.last.as_ref() == Some(&key) {
            return;
        }
        self.last = Some(key);
        println!(
            "{} t={} ct={} origin={:.1},{:.1},{:.1} pm_type={} pm_flags=0x{:x} weapon={} weaponstate={} \
weaponTime={} weapAnim=0x{:x} slots=0x{:x} sent={}",
            if self.weapsel { "WEAPSEL" } else { "LADDER" },
            snap.server_time,
            i("commandTime"),
            o[0],
            o[1],
            o[2],
            i("pm_type"),
            i("pm_flags"),
            i("weapon"),
            i("weaponstate"),
            i("weaponTime"),
            i("weapAnim"),
            i("weaponslots[0]"),
            self.sent,
        );
    }
}

pub(super) fn write_ladder_fixture(
    configstrings: &[String],
    lp: &LadderProbe,
    yaw: f32,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let map = serverinfo_key(configstrings, "mapname");
    let mut out = String::new();
    let script = lp.script.name();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: one player on a ladder.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, client-probes/probe_ladder under tools/run_probe.sh with +set\n\
# probe_teleport 1, the client --probe-ladder {yaw} --save-ladder {script} (recipe in\n\
# crates/gsc/tests/fixtures/semantics/client-probes/README.md, probe_ladder).\n\
# The gsc puts the player at a ladder's foot facing the rungs (mp_ship's hold\n\
# ladder, or probe_spot's); the script starts {} ms after the first snapshot\n\
# {}. Phases: (name, ms, forward, right, pitch, yaw offset):\n",
        LADDER_SETTLE.as_millis(),
        if script == "grab" {
            "in play"
        } else {
            "on the rungs"
        }
    ));
    out.push_str(&format!(
        "# ladder foot={},{},{} yaw={yaw}\n",
        lp.foot[0], lp.foot[1], lp.foot[2]
    ));
    for (name, ms, f, r, pitch, yaw) in lp.script.phases() {
        out.push_str(&format!("#   {name} {ms} {f} {r} {pitch} {yaw}\n"));
    }
    out.push_str("# !cmd is a usercmd as it went on the wire (angles rebased on delta_angles);\n");
    out.push_str(
        "# !snap the playerstate per snapshot, da= its delta_angles, view= its viewangles,\n\
# lv= its vLadderVec. Floats exact.\n",
    );
    for n in &lp.notes {
        out.push_str(n);
        out.push('\n');
    }
    let lines = lp.capture.as_deref().unwrap_or_default();
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-dm-ladder"), tag),
        &out,
        overwrite,
    )?;
    println!("ladder: {} lines -> {path}", lines.len());
    Ok(())
}

/// The `--probe-weapsel` capture: every cmd the play client's input built,
/// every server command, the `weapalt`/`weapon` presses and one `!trace`
/// per snapshot, for `crates/server/tests/weapsel_ab.rs`.
pub(super) fn write_weapsel_fixture(
    configstrings: &[String],
    join: &JoinProbe,
    lp: &LadderProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let (map, gametype) = (key("mapname"), key("g_gametype"));
    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server: the client's weapon selection against script switches,\n",
        retail_build()
    ));
    out.push_str("# a dry trigger and the select-fire switch.\n");
    out.push_str(&format!(
        "# map {map}, gametype {gametype}, joined {}, weapon {}\n",
        join.team, join.weapon
    ));
    out.push_str("# Two shells, the gsc probe first:\n");
    out.push_str("#   COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=120 \\\n");
    out.push_str("#       tools/run_probe.sh client-probes/probe_weapsel mp_carentan\n");
    out.push_str(
        "#   cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-weapsel --probe-secs 75\n",
    );
    out.push_str("# The script switches to the pistol at 4 s, back to the primary at 8 s, empties it at 12 s and\n");
    out.push_str("# puts a loaded thompson in the primary slot at 20 s; the client pulls the trigger every 700 ms\n");
    out.push_str("# from 15 to 19 s and runs `weapalt` at 26 and 30 s, `weapon colt_mp` at 34 s and `weapalt`\n");
    out.push_str(
        "# at 38 s, `weapon thompson_mp` at 42 s and `weapon colt_mp` at 42.5 s, inside the\n",
    );
    out.push_str(
        "# thompson's raise, all from its settled join (WEAPSEL_FIRE, WEAPSEL_COMMANDS in probe.rs).\n",
    );
    out.push_str("# !cmd is a usercmd as it went on the wire; !press a command and what PlayInput made of it.\n");
    out.push_str(
        "# !trace clip and ammo are the non-zero entries as index:value; commandTime is the last\n",
    );
    out.push_str("# cmd the server ran before the snapshot.\n");
    out.push_str("# !server is every server command, tokens joined, control bytes as \\xNN.\n");
    for r in &lp.rows {
        out.push_str(r);
        out.push('\n');
    }
    let path = write_fixture_file(
        PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-dm-weapsel"), tag),
        &out,
        overwrite,
    )?;
    println!("weapsel: wrote {path}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each `--save-ladder` script is told apart by its value, not by a
    /// const slice's address, and names itself in the fixture (BUG-092).
    #[test]
    fn each_ladder_script_runs_and_names_its_own_phases() {
        for (s, name, phases) in [
            (LadderScript::Full, "full", LADDER_SCRIPT),
            (LadderScript::Climb, "climb", LADDER_CLIMB_SCRIPT),
            (LadderScript::Turn, "turn", LADDER_TURN_SCRIPT),
            (LadderScript::Grab, "grab", LADDER_GRAB_SCRIPT),
        ] {
            assert_eq!(s.name(), name);
            assert_eq!(s.phases().len(), phases.len(), "{name}");
            assert_eq!(s.phases()[0].0, phases[0].0, "{name}");
        }
    }
}
