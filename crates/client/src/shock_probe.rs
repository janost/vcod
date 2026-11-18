//! `--save-shock`: the client half of the shellshock capture. Waits for
//! `client-probes/probe_shock` to put the player on mp_carentan's street
//! mark, then runs forward and back along world x, a second each way, and
//! holds `wbuttons` 0x4 while the playerstate's shellshock runs, as the
//! retail cgame does (docs/research/cod11-shellshock.md, "Movement"). Writes
//! every usercmd and every snapshot's shellshock and movement fields to
//! `crates/server/tests/fixtures/shellshock/<map>-dm-shock-client[-<tag>].txt`.

use std::time::Instant;
use vcod_common::net;
use vcod_common::net::msg::{NULL_USERCMD, UserCmd, WBUTTON_SHELLSHOCK};
use vcod_common::net::snapshot::Snapshot;

/// `probe_shock.gsc`'s mark, `probe_legs`'s street spot.
pub const SHOCK_SPOT: [f32; 3] = crate::legs_probe::LEGS_SPOT;

/// Each leg of the walk, alternating forward and back from forward.
const LEG_MS: u128 = 1000;
/// From the placement; the gsc's last shock ends at 9500.
const SCRIPT_MS: u128 = 11000;

#[derive(Default)]
pub struct ShockProbe {
    placed: Option<(Instant, i32)>,
    before: crate::legs_probe::BeforePlacement,
    seen: Option<u32>,
    /// The newest snapshot's `(serverTime, shellshockTime, shellshockDuration)`.
    shock: (i32, i32, i32),
    lines: Vec<String>,
    done: bool,
}

impl ShockProbe {
    pub fn running(&self) -> bool {
        !self.done
    }

    fn ms(&self, now: Instant) -> Option<u128> {
        self.placed
            .map(|(t, _)| now.saturating_duration_since(t).as_millis())
    }

    /// The cmd for `now`, world angles 0.
    pub fn cmd(&self, now: Instant) -> UserCmd {
        let mut cmd = NULL_USERCMD;
        if let Some(ms) = self.ms(now).filter(|&ms| ms < SCRIPT_MS) {
            cmd.forward = if (ms / LEG_MS).is_multiple_of(2) {
                127
            } else {
                -127
            };
        }
        let (t, start, duration) = self.shock;
        let elapsed = t.wrapping_sub(start);
        if start != 0 && (0..duration).contains(&elapsed) {
            cmd.wbuttons |= WBUTTON_SHELLSHOCK;
        }
        cmd
    }

    pub fn record(&mut self, now: Instant, sent: UserCmd) {
        if self.done {
            return;
        }
        let line = |ms: u128| {
            format!(
                "!cmd ms={ms} st={} buttons={} wbuttons={} weapon={} forward={} right={} up={} angles={},{},{}",
                sent.server_time,
                sent.buttons,
                sent.wbuttons,
                sent.weapon,
                sent.forward,
                sent.right,
                sent.up,
                sent.angles[0],
                sent.angles[1],
                sent.angles[2],
            )
        };
        match self.ms(now) {
            Some(ms) => self.lines.push(line(ms)),
            None => self.before.push(sent.server_time, line(0)),
        }
    }

    /// Takes the placement off the first snapshot on the mark and logs every
    /// snapshot after it. True once the script has run out and a second has
    /// passed.
    pub fn observe(&mut self, now: Instant, s: &Snapshot) -> bool {
        let p = crate::probe::wire();
        let f = |name: &str| s.ps.field_i32(p, name);
        self.shock = (s.server_time, f("shellshockTime"), f("shellshockDuration"));
        let o = s.ps.origin(p);
        let mut pre = Vec::new();
        if self.placed.is_none() {
            if (o[0] - SHOCK_SPOT[0]).abs() < 0.5 && (o[1] - SHOCK_SPOT[1]).abs() < 0.5 {
                self.placed = Some((now, s.server_time));
                println!("shock: placed at serverTime {}", s.server_time);
                pre = self.before.past(f("commandTime")).collect();
            } else {
                return false;
            }
        }
        if self.seen != Some(s.message_num) {
            self.seen = Some(s.message_num);
            let v = [0, 1, 2].map(|i| s.ps.field_f32(p, &format!("velocity[{i}]")));
            self.lines.push(format!(
                "!snap msg={} t={} ct={} index={} time={} duration={} origin={},{},{} vel={},{},{} pm_flags={}",
                s.message_num,
                s.server_time,
                f("commandTime"),
                f("shellshockIndex"),
                f("shellshockTime"),
                f("shellshockDuration"),
                o[0],
                o[1],
                o[2],
                v[0],
                v[1],
                v[2],
                f("pm_flags"),
            ));
        }
        self.lines.extend(pre);
        if self.ms(now).is_some_and(|ms| ms >= SCRIPT_MS + 1000) {
            self.done = true;
        }
        self.done
    }

    pub fn write(
        &self,
        configstrings: &[String],
        team: &str,
        weapon: &str,
        tag: Option<&str>,
        overwrite: bool,
    ) -> anyhow::Result<()> {
        let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
        let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
        let map = key("mapname");
        let mut out = String::new();
        out.push_str(&format!(
            "# Retail CoD {} dedicated server: the client half of the shellshock capture.\n",
            crate::probe::retail_build()
        ));
        out.push_str(&format!(
            "# map {map}, gametype {}, team={team} weapon={weapon}\n",
            key("g_gametype")
        ));
        out.push_str(&format!(
            "# placed serverTime={} spot={},{},{}\n",
            self.placed.map_or(0, |p| p.1),
            SHOCK_SPOT[0],
            SHOCK_SPOT[1],
            SHOCK_SPOT[2]
        ));
        out.push_str(
            "# client-probes/probe_shock under tools/run_probe.sh places the player at level\n\
             # time 15000 and shellshocks it on a schedule (the server half, -shock.txt).\n\
             # Script from the snapshot that showed the placement, world yaw 0: forward and\n\
             # back a second each for 11 s, wbuttons 0x4 while the newest snapshot's\n\
             # serverTime - shellshockTime is in [0, shellshockDuration). !cmd ms=0 lines after\n\
             # the first !snap were sent before the probe saw the placement, past its ct.\n",
        );
        if !self.done {
            out.push_str("# BROKEN the run ended before the script did\n");
        }
        for l in &self.lines {
            out.push_str(l);
            out.push('\n');
        }
        let dir = "crates/server/tests/fixtures/shellshock";
        std::fs::create_dir_all(dir)?;
        let path = crate::probe::fixture_path(
            dir,
            &format!("{map}-dm-shock-client{}.txt", crate::probe::tag_suffix(tag)),
        )?;
        if !overwrite && std::path::Path::new(&path).exists() {
            anyhow::bail!("{path} already exists; pass --overwrite-fixture to replace it");
        }
        std::fs::write(&path, out)?;
        println!("shock: {} lines -> {path}", self.lines.len());
        Ok(())
    }
}
