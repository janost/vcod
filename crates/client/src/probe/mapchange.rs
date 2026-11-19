//! `--save-mapchange` and `--save-roundrestart`: the capture either side of a map change or a
//! restart.

use super::fixture::{
    nonzero_pairs, pairs_str, serverinfo_key, tagged_name, vec_str, write_fixture_file,
};
use super::hit::keep_sample;
use super::*;

/// Directory the map-cycle captures land in. They pin what the wire does
/// across a map change and a round restart, which no playerstate fixture holds.
const NETCHAN_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/netchan"
);

/// `pmove_t`'s intermission state, the one retail's `spawnIntermission` puts a
/// client in at a map end.
const PM_INTERMISSION: i32 = 5;

/// One `!trace` line: the playerstate fields a map end and a round restart
/// move, plus the weapon the `pers[]` carry is measured through.
struct NetchanSample {
    elapsed_ms: u128,
    server_time: i32,
    pm_type: i32,
    e_flags: i32,
    health: i32,
    damage_event: i32,
    weapon: i32,
    clip: Vec<(usize, i16)>,
    event_sequence: i32,
    events: [i32; 4],
    viewangles: [f32; 3],
    origin: [f32; 3],
}

impl NetchanSample {
    fn take(elapsed_ms: u128, snap: &net::snapshot::Snapshot) -> NetchanSample {
        let p = wire();
        let f = |n: &str| snap.ps.field_i32(p, n);
        NetchanSample {
            elapsed_ms,
            server_time: snap.server_time,
            pm_type: f("pm_type"),
            e_flags: f("eFlags"),
            health: snap.ps.health(),
            damage_event: f("damageEvent"),
            weapon: f("weapon"),
            clip: nonzero_pairs(&snap.ps.arrays.ammoclip),
            event_sequence: f("eventSequence"),
            events: [
                ev_v1(f("events[0]")),
                ev_v1(f("events[1]")),
                ev_v1(f("events[2]")),
                ev_v1(f("events[3]")),
            ],
            viewangles: [
                snap.ps.field_f32(p, "viewangles[0]"),
                snap.ps.field_f32(p, "viewangles[1]"),
                snap.ps.field_f32(p, "viewangles[2]"),
            ],
            origin: snap.ps.origin(p),
        }
    }

    /// Neither clock, and neither the view nor the origin: a walking approach
    /// moves both every frame and the heartbeat in [`keep_sample`] is what
    /// keeps that stretch down to a line a second.
    fn watched(&self) -> Vec<i32> {
        let mut v = vec![
            self.pm_type,
            self.e_flags,
            self.health,
            self.damage_event,
            self.weapon,
            self.event_sequence,
        ];
        v.extend_from_slice(&self.events);
        v.extend(self.clip.iter().map(|&(i, c)| (i as i32) << 16 | c as i32));
        v
    }

    fn line(&self) -> String {
        format!(
            "!trace ms={} serverTime={} pm_type={} eFlags={} health={} damageEvent={} weapon={} \
ammoclip={} eventSequence={} events={},{},{},{} viewangles={} origin={}\n",
            self.elapsed_ms,
            self.server_time,
            self.pm_type,
            self.e_flags,
            self.health,
            self.damage_event,
            self.weapon,
            pairs_str(&self.clip),
            self.event_sequence,
            self.events[0],
            self.events[1],
            self.events[2],
            self.events[3],
            vec_str(self.viewangles),
            vec_str(self.origin),
        )
    }
}

/// Records everything either side of a map change or a round restart: each
/// gamestate, each serverCommand with its reliable sequence, each out-of-band
/// packet and one trace line per snapshot whose watched fields moved. It
/// drives no input of its own; the mode it runs under supplies the motion.
#[derive(Default)]
pub(super) struct MapChangeProbe {
    started: Option<Instant>,
    trace: Vec<NetchanSample>,
    last_kept: Option<(Vec<i32>, u128)>,
    traced: Option<u32>,
    /// `(ms, reliable sequence, verbatim text)` per serverCommand.
    commands: Vec<(u128, i32, String)>,
    /// `(ms, command word and the rest)` per out-of-band packet.
    oob: Vec<(u128, String)>,
    /// `(ms, serverId, netchan sequence, non-empty configstrings, mapname)`.
    gamestates: Vec<(u128, i32, u32, usize, String)>,
    /// When `pm_type` first read [`PM_INTERMISSION`].
    intermission_at: Option<u128>,
    /// The first snapshot after the newest gamestate, which is what says how
    /// long the client sat with no world.
    first_snapshot_after_gamestate: Option<u128>,
    awaiting_snapshot: bool,
}

impl MapChangeProbe {
    fn elapsed_ms(&mut self, now: Instant) -> u128 {
        let started = *self.started.get_or_insert(now);
        now.duration_since(started).as_millis()
    }

    pub(super) fn on_gamestate(&mut self, now: Instant, client: &NetClient<UdpTransport>) {
        let ms = self.elapsed_ms(now);
        let map = net::info_value_for_key(client.configstring(0), "mapname")
            .unwrap_or("?")
            .to_string();
        let set = client
            .configstrings()
            .iter()
            .filter(|s| !s.is_empty())
            .count();
        println!(
            "NETCHAN: gamestate at +{ms}ms serverId={} messageNum={} configstrings={set} map={map}",
            client.server_id(),
            client.incoming_sequence(),
        );
        self.gamestates
            .push((ms, client.server_id(), client.incoming_sequence(), set, map));
        self.awaiting_snapshot = true;
    }

    pub(super) fn on_oob(&mut self, now: Instant, text: &str) {
        let ms = self.elapsed_ms(now);
        println!("NETCHAN: oob at +{ms}ms {text:?}");
        self.oob.push((ms, text.to_string()));
    }

    /// `cmds` in order, ending at the client's current reliable sequence.
    pub(super) fn on_commands(&mut self, now: Instant, sequence: i32, cmds: &[String]) {
        let ms = self.elapsed_ms(now);
        let first = sequence - cmds.len() as i32 + 1;
        for (i, text) in cmds.iter().enumerate() {
            self.commands.push((ms, first + i as i32, text.clone()));
        }
    }

    pub(super) fn sample(&mut self, now: Instant, snap: &net::snapshot::Snapshot) {
        if self.traced == Some(snap.message_num) {
            return;
        }
        self.traced = Some(snap.message_num);
        let ms = self.elapsed_ms(now);
        if self.awaiting_snapshot {
            self.awaiting_snapshot = false;
            self.first_snapshot_after_gamestate = Some(ms);
        }
        let s = NetchanSample::take(ms, snap);
        if s.pm_type == PM_INTERMISSION && self.intermission_at.is_none() {
            println!("NETCHAN: intermission at +{ms}ms, eFlags {}", s.e_flags);
            self.intermission_at = Some(ms);
        }
        if keep_sample(&mut self.last_kept, s.watched(), ms) {
            self.trace.push(s);
        }
    }
}

/// Writes the map-cycle capture to
/// `crates/server/tests/fixtures/netchan/<map>-<gametype>-<role>.txt`, with
/// `<map>` the map the run started on. Every line is in the order the wire
/// carried it, interleaved by `ms`.
pub(super) fn write_mapchange_fixture(
    role: &str,
    configstrings: &[String],
    join: &JoinProbe,
    probe: &MapChangeProbe,
    map: &str,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let gametype = serverinfo_key(configstrings, "g_gametype");
    let out = mapchange_text(role, gametype, join, probe, map);
    let path = write_fixture_file(
        NETCHAN_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}-{role}"), tag),
        &out,
        overwrite,
    )?;
    println!("netchan: {role} -> {path}");
    Ok(())
}

/// [`write_mapchange_fixture`]'s text.
fn mapchange_text(
    role: &str,
    gametype: &str,
    join: &JoinProbe,
    probe: &MapChangeProbe,
    map: &str,
) -> String {
    let mapchange = role == "mapchange";

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server across {}.\n",
        retail_build(),
        if mapchange {
            "a map end and the rotation after it"
        } else {
            "an S&D round restart"
        }
    ));
    out.push_str(&format!(
        "# map {map}, gametype {gametype}, joined {}, weapon {}, role {role}\n",
        join.team, join.weapon
    ));
    out.push_str("# dedicated 1, sv_maxclients 8, sv_pure 0, stock scr_* defaults but for the\n");
    if mapchange {
        out.push_str("# limit cvars the recipe sets. One probe:\n");
        out.push_str(
            "#   tools/run_server.sh mp_carentan +set g_gametype dm +set scr_dm_timelimit 1 \\\n",
        );
        out.push_str("#     +set sv_mapRotation \"gametype dm map mp_carentan map mp_brecourt\"\n");
        out.push_str(
            "#   --net-probe <ip:port> --probe-team allies --save-mapchange --probe-secs 150\n",
        );
    } else {
        out.push_str(
            "# limit cvars the recipe sets. Two probes on opposite teams, target first:\n",
        );
        out.push_str(
            "#   tools/run_server.sh mp_carentan +set g_gametype sd +set scr_sd_roundlength 1 \\\n",
        );
        out.push_str("#     +set scr_friendlyfire 1\n");
        out.push_str(
            "#   --net-probe <ip:port> --probe-team axis --probe-target --save-roundrestart \\\n",
        );
        out.push_str("#     --probe-secs 200\n");
        out.push_str(
            "#   --net-probe <ip:port> --probe-team allies --save-roundrestart --probe-secs 160\n",
        );
    }
    // Retail pushes no scoreboard on its own, so a `b` line here is never
    // evidence that it does; whether the file has any is whether its role asked.
    out.push_str(if role == "roundrestart-shooter" {
        "# This half never sends `score`, and retail sends the `b` scoreboard only in\n# answer to one, so this file carries no `b` line at all.\n"
    } else {
        "# The probe sends `score` every 2 s, and retail sends the `b` scoreboard only\n# in answer to one, so every `b` line here was asked for, not pushed.\n"
    });
    out.push_str("# One !gamestate per gamestate, one !cmd per serverCommand with its reliable\n");
    out.push_str("# sequence, one !oob per connectionless packet, and one !trace per snapshot\n");
    out.push_str("# whose watched fields moved plus one a second so a settled stretch still\n");
    out.push_str("# carries a timeline. ms is since the capture started; serverTime is the\n");
    out.push_str("# server's own clock. ammoclip is the non-zero entries, as index:value.\n");
    if probe.gamestates.len() < 2 && mapchange {
        out.push_str(
            "# BROKEN only one gamestate: the run ended before the rotation loaded the \
second map, so this file measures no map change.\n",
        );
    }
    let restart = wire().map_restart_command;
    if role.starts_with("roundrestart")
        && !probe
            .commands
            .iter()
            .any(|(_, _, t)| t == restart || t == "n")
    {
        out.push_str(&format!(
            "# BROKEN no `{restart}` command: no round restarted inside the run, so this file \
measures no restart.\n",
        ));
    }
    out.push_str(&format!(
        "role {role}\n!observed gamestates={} commands={} oob={} traced={} intermission_at_ms={} \
first_snapshot_after_gamestate_ms={}\n",
        probe.gamestates.len(),
        probe.commands.len(),
        probe.oob.len(),
        probe.trace.len(),
        probe
            .intermission_at
            .map_or("none".to_string(), |m| m.to_string()),
        probe
            .first_snapshot_after_gamestate
            .map_or("none".to_string(), |m| m.to_string()),
    ));

    // Every line carries its own `ms`, so one merge by time reads as the wire
    // did rather than as four grouped blocks. A pump drains its events and its
    // commands before it samples the snapshot, so lines sharing an `ms` are
    // ranked by category rather than by their position in their own vector.
    let (gamestate, cmd, oob, trace) = (0u8, 1u8, 2u8, 3u8);
    let mut lines: Vec<(u128, u8, usize, String)> = Vec::new();
    for (i, (ms, id, num, set, map)) in probe.gamestates.iter().enumerate() {
        lines.push((
            *ms,
            gamestate,
            i,
            format!(
                "!gamestate ms={ms} serverId={id} messageNum={num} configstrings={set} mapname={map}\n"
            ),
        ));
    }
    for (i, (ms, seq, text)) in probe.commands.iter().enumerate() {
        lines.push((*ms, cmd, i, format!("!cmd ms={ms} seq={seq} text={text}\n")));
    }
    for (i, (ms, text)) in probe.oob.iter().enumerate() {
        lines.push((
            *ms,
            oob,
            i,
            format!("!oob ms={ms} text={}\n", text.replace('\n', "\\n")),
        ));
    }
    for (i, s) in probe.trace.iter().enumerate() {
        lines.push((s.elapsed_ms, trace, i, s.line()));
    }
    lines.sort_by_key(|(ms, rank, i, _)| (*ms, *rank, *i));
    for (_, _, _, l) in lines {
        out.push_str(&l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::tests::pickup_snap;

    /// The commands of one batch end at the client's reliable sequence;
    /// the fixture interleaves every kind by ms, and at one ms puts the
    /// gamestate first, then the commands, the out-of-band text and the
    /// trace, each kind in its own order.
    #[test]
    fn a_mapchange_capture_numbers_its_commands_and_orders_lines_by_ms_then_kind() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut m = MapChangeProbe::default();
        m.elapsed_ms(ms(0));
        m.on_commands(ms(0), 10, &["a".into(), "b".into(), "c".into()]);
        let restart = wire().map_restart_command;
        m.on_commands(ms(500), 11, &[restart.into()]);
        m.on_oob(ms(500), "disconnect");
        m.sample(ms(500), &pickup_snap(1, &[], &[]));
        m.gamestates.push((500, 7, 3, 12, "mp_brecourt".into()));
        m.on_commands(ms(500), 12, &["e".into()]);
        let seqs: Vec<i32> = m.commands.iter().map(|c| c.1).collect();
        assert_eq!(seqs, [8, 9, 10, 11, 12]);
        let join = JoinProbe::new(Some("allies"), None);
        let text = mapchange_text("mapchange", "dm", &join, &m, "mp_carentan");
        let lines: Vec<&str> = text
            .lines()
            .filter(|l| l.starts_with('!') && !l.starts_with("!observed"))
            .map(|l| l.split(' ').take(3).collect::<Vec<_>>())
            .map(|w| w[0])
            .collect();
        assert_eq!(
            lines,
            [
                "!cmd",
                "!cmd",
                "!cmd",
                "!gamestate",
                "!cmd",
                "!cmd",
                "!oob",
                "!trace"
            ]
        );
        assert!(text.contains(&format!(
            "!cmd ms=500 seq=11 text={restart}\n!cmd ms=500 seq=12 text=e\n"
        )));
        assert!(text.contains("# BROKEN only one gamestate"));
        let text = mapchange_text("roundrestart-target", "sd", &join, &m, "mp_carentan");
        assert!(!text.contains("# BROKEN"), "the wire's restart command");
    }
}
