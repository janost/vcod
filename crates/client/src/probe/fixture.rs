//! Fixture writers the modes share: gamestate, snapshots, configstrings, playerstate, scripted, and
//! the one write every mode's fixture goes through ([`write_fixture_file`], [`write_capture`]),
//! which refuses to replace a fixture.

use super::join::SPAWN_SETTLE;
use super::*;

/// `save_fixture` writes gamestate.bin; `save_snapshots` writes snapshots.bin
/// only and sends the gamestate capture to tmp/, so the pinned gamestate is
/// safe to refresh snapshots against. A `--capture-tag` names a pair beside
/// them (`gamestate-<tag>.bin`, `snapshots-<tag>.bin`), both written, and
/// refuses to replace one without `--overwrite-fixture`.
pub(super) fn finish_probe(
    client: &mut NetClient<UdpTransport>,
    save_fixture: bool,
    save_snapshots: bool,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let tagged = |path: &str| match (tag, path.strip_suffix(".bin")) {
        (Some(t), Some(stem)) if save_fixture || save_snapshots => {
            format!("{stem}{}.bin", tag_suffix(Some(t)))
        }
        _ => path.to_string(),
    };
    let save_fixture = save_fixture || (tag.is_some() && save_snapshots);
    // The scratch copies under `tmp/` are rewritten freely; a fixture is not.
    let write = |path: &str, data: &[u8]| {
        let scratch = path == SCRATCH_PATH || path == SNAP_SCRATCH_PATH;
        write_capture(path, data, overwrite || scratch)
    };
    client.disconnect();
    let Some(cap): Option<SnapshotCapture> = client.take_capture() else {
        return Ok(());
    };
    println!(
        "captured {} snapshot message(s); serverTime {:?}..{:?}",
        cap.count,
        cap.times.first(),
        cap.times.last()
    );
    if let Some(gs_bytes) = client.captured_gamestate() {
        let path = if save_fixture {
            tagged(&FIXTURE_PATH)
        } else {
            SCRATCH_PATH.to_string()
        };
        write(&path, gs_bytes)?;
        println!("gamestate: {} bytes -> {path}", gs_bytes.len());
    }
    if cap.count == 0 {
        println!("no snapshots captured");
        return Ok(());
    }
    let path = if save_fixture || save_snapshots {
        tagged(&SNAP_FIXTURE_PATH)
    } else {
        SNAP_SCRATCH_PATH.to_string()
    };
    write(&path, &cap.triples)?;
    println!(
        "snapshots: {} message(s), {} bytes -> {path}",
        cap.count,
        cap.triples.len()
    );
    if !save_fixture && !save_snapshots {
        println!("(pass --save-snapshots to replace the committed snapshot fixture)");
    }
    Ok(())
}

/// Directory `crates/server/tests/configstrings_ab.rs` reads its fixtures
/// from.
static CONFIGSTRINGS_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/configstrings");

/// Writes the non-empty configstring table to
/// `<map>-<gametype>.txt`. The map, gametype, client count and pureness come
/// out of cs 0 (serverinfo) itself, so nothing here is hand-typed.
pub(super) fn write_configstrings_fixture(
    configstrings: &[String],
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let (out, written) = configstrings_text(configstrings);
    let path = write_fixture_file(
        &CONFIGSTRINGS_FIXTURE_DIR,
        &tagged_name(&format!("{}-{}", key("mapname"), key("g_gametype")), tag),
        &out,
        overwrite,
    )?;
    println!("configstrings: {written} non-empty slots -> {path}");
    Ok(())
}

/// [`write_configstrings_fixture`]'s text and how many slots it holds.
fn configstrings_text(configstrings: &[String]) -> (String, usize) {
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    let gametype = key("g_gametype");
    let max_clients = key("sv_maxclients");
    let pure = key("sv_pure");

    // Values are raw: a configstring cannot contain a newline, so the
    // `<index> <value>` line format needs no escaping.
    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server configstring table at gamestate.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, sv_maxclients {max_clients}, sv_pure {pure}, dedicated 1;\n\
         # cs 0 below holds the rest of the serverinfo cvars.\n"
    ));
    out.push_str("# Captured with tools/run_server.sh and --net-probe at debug level.\n");
    out.push_str("# One '<index> <value>' line per non-empty slot, ascending. Values are raw;\n");
    out.push_str("# a configstring cannot contain a newline, so no escaping is needed.\n");
    out.push_str("# 0 (serverinfo) and 1 (systeminfo) are recorded for provenance and excluded\n");
    out.push_str(
        "# from the diff: they carry pak checksums and server config, not script output.\n",
    );
    let mut written = 0;
    for (i, cs) in configstrings.iter().enumerate() {
        if !cs.is_empty() {
            out.push_str(&format!("{i} {cs}\n"));
            written += 1;
        }
    }
    (out, written)
}

/// Directory `crates/server/tests/playerstate_ab.rs` reads its fixtures from.
pub(super) static PLAYERSTATE_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/playerstate");

/// Writes a text fixture as `dir/name`, the write every `--save-*` mode's fixture takes: refuses
/// a name that leaves `dir`, creates `dir` and refuses to replace a file without
/// `--overwrite-fixture` ([`write_capture`]). Returns the path for the mode's summary line.
pub(crate) fn write_fixture_file(
    dir: &str,
    name: &str,
    body: &str,
    overwrite: bool,
) -> anyhow::Result<String> {
    let path = fixture_path(dir, name)?;
    write_capture(&path, body, overwrite)?;
    Ok(path)
}

/// `<stem>[-<tag>].txt`, the name every text fixture takes but bump's, which puts the tag before
/// its role.
pub(crate) fn tagged_name(stem: &str, tag: Option<&str>) -> String {
    format!("{stem}{}.txt", tag_suffix(tag))
}

/// A cs 0 (serverinfo) value, `?` when cs 0 or the key is missing.
pub(super) fn serverinfo_key<'a>(configstrings: &'a [String], key: &str) -> &'a str {
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    net::info_value_for_key(serverinfo, key).unwrap_or("?")
}

/// Writes a fixture, creating its directory, and refuses to land on a file that is already
/// there unless `--overwrite-fixture` says to. The fixed names hold retail
/// evidence, and a tag is free text that can spell one of them
/// (`--save-ads --capture-tag sniper` is the scoped-rifle fixture's path):
/// a capture against vcod's own server silently replacing the oracle is the
/// one mistake these directories cannot survive. A refused capture is kept
/// under `tmp/` rather than thrown away.
pub(super) fn write_capture(
    path: &str,
    out: impl AsRef<[u8]>,
    overwrite: bool,
) -> anyhow::Result<()> {
    write_capture_in(path, out.as_ref(), overwrite, std::path::Path::new("tmp"))
}

/// [`write_capture`] keeping a refused capture in `keep` instead of `tmp/`.
fn write_capture_in(
    path: &str,
    out: &[u8],
    overwrite: bool,
    keep: &std::path::Path,
) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    if overwrite || !p.exists() {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, out)?;
        return Ok(());
    }
    let kept = refused_copy(path, keep);
    let saved = kept
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&kept, out));
    let kept = match saved {
        Ok(()) => format!("this capture is in {}", kept.display()),
        Err(e) => format!("keeping it in {} failed too: {e}", kept.display()),
    };
    anyhow::bail!(
        "{path} already exists; pass --overwrite-fixture to replace it, or \
         --capture-tag for a new name ({kept})"
    )
}

/// Where [`write_capture`] keeps a capture it would not write: `keep` and
/// the fixture's file name.
fn refused_copy(path: &str, keep: &std::path::Path) -> std::path::PathBuf {
    let name = std::path::Path::new(path)
        .file_name()
        .map_or_else(|| "capture.txt".into(), |n| n.to_os_string());
    keep.join(name)
}

/// `-<tag>` for a `--capture-tag`, empty without one.
fn tag_suffix(tag: Option<&str>) -> String {
    tag.map(|t| format!("-{t}")).unwrap_or_default()
}

/// The non-zero entries of a playerstate ammo array, as `index:value` pairs.
pub(super) fn nonzero_pairs(a: &[i16]) -> Vec<(usize, i16)> {
    a.iter()
        .enumerate()
        .filter(|&(_, &v)| v != 0)
        .map(|(i, &v)| (i, v))
        .collect()
}

pub(super) fn pairs_str(pairs: &[(usize, i16)]) -> String {
    if pairs.is_empty() {
        return "-".to_string();
    }
    pairs
        .iter()
        .map(|(i, v)| format!("{i}:{v}"))
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn vec_str(v: [f32; 3]) -> String {
    format!("{:.1},{:.1},{:.1}", v[0], v[1], v[2])
}

/// A vector as the shortest decimals that read back as the same f32: a pose
/// a gate places its client at. One decimal put a spawn's -138.0048 view (short
/// 40413) a short off when read back.
pub(super) fn vec_exact(v: [f32; 3]) -> String {
    format!("{},{},{}", v[0], v[1], v[2])
}

/// A sent usercmd as the `!cmd` lines carry it: `st= buttons= wbuttons= weapon= up= forward=
/// right= angles=p,y,r`. The caller puts the line's head before it and its extras after.
/// shock_probe's `!cmd` keeps its own order (forward, right, up) and does not use it.
pub(crate) fn cmd_fields(c: &net::msg::UserCmd) -> String {
    format!(
        "st={} buttons={} wbuttons={} weapon={} up={} forward={} right={} angles={},{},{}",
        c.server_time,
        c.buttons,
        c.wbuttons,
        c.weapon,
        c.up,
        c.forward,
        c.right,
        c.angles[0],
        c.angles[1],
        c.angles[2]
    )
}

/// A capture phase the pickup and turret fixtures group their lines by.
pub(super) trait FixturePhase: Copy + PartialEq {
    /// The phase's place in the file.
    fn order(self) -> u8;
    /// `<label>` in `[phase <label>]`.
    fn label(self) -> &'static str;
}

/// One `[phase <label>]` block per phase a cmd row or a line names, in phase order: the phase's
/// `!station` lines, then its cmd rows and lines interleaved by ms. The sort is stable and cmd
/// rows go in first, so a cmd keeps its place before a line of the same ms and a `!trace` keeps
/// the lines recorded after it. The pickup and turret fixtures.
pub(super) fn write_phase_blocks<P: FixturePhase>(
    out: &mut String,
    stations: &[(P, [f32; 3], [f32; 3])],
    cmds: &[(P, u128, String)],
    lines: &[(P, u128, String)],
) {
    let mut phases: Vec<P> = Vec::new();
    for p in cmds.iter().chain(lines).map(|r| r.0) {
        if !phases.contains(&p) {
            phases.push(p);
        }
    }
    phases.sort_by_key(|p| p.order());
    for phase in phases {
        out.push_str(&format!("[phase {}]\n", phase.label()));
        for (_, o, v) in stations.iter().filter(|s| s.0 == phase) {
            out.push_str(&format!(
                "!station origin={} viewangles={}\n",
                vec_str(*o),
                vec_str(*v)
            ));
        }
        let mut rows: Vec<&(P, u128, String)> =
            cmds.iter().chain(lines).filter(|r| r.0 == phase).collect();
        rows.sort_by_key(|r| r.1);
        for (_, _, r) in rows {
            out.push_str(r);
            out.push('\n');
        }
    }
}

/// Where `--save-scripted` writes.
static GAMETYPES_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/gametypes");

/// Writes the scripted gametype capture to
/// `<map>-<gametype>-<role>.txt`, the gametype as the stock name the gsc
/// probe wraps. The recipe in the header is the one the gate's doc names.
pub(super) fn write_scripted_fixture(
    role: &str,
    configstrings: &[String],
    join: &JoinProbe,
    capture: &net::capture::ScriptedCapture,
    map: &str,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let probe_gt = serverinfo_key(configstrings, "g_gametype");
    let gametype = probe_gt.strip_prefix("probe_").unwrap_or(probe_gt);
    let mut out = String::new();
    let build = retail_build();
    // Only a probe named after a stock gametype wraps that gametype's script.
    const STOCK: [&str; 9] = ["dm", "tdm", "sd", "re", "bel", "hq", "ctf", "dom", "bas"];
    let running = if STOCK.contains(&gametype) {
        format!("stock {gametype}.gsc under the gsc probe")
    } else {
        "the gsc probe".to_string()
    };
    out.push_str(&format!(
        "# Retail CoD {build} dedicated server running {running}\n\
         # client-probes/{probe_gt}.gsc on {map}; this probe joined {}, weapon {}, role {role}.\n",
        join.team, join.weapon
    ));
    out.push_str(
        "# Three shells, the server first, then the two probes, the order the roles join in:\n\
         #   COD_LNXDED_HOME=<absolute, no '+'> PORT=<p> SECS=<s> \\\n\
         #       tools/run_probe.sh client-probes/<probe> <map>\n\
         #   cargo run -p vcod -- --net-probe 127.0.0.1:<p> --probe-team <team> \\\n\
         #       --save-scripted <role> --probe-secs <s>\n",
    );
    out.push_str(match gametype {
        "re" | "bel" => "# docs/research/cod11-gametypes-re-bel.md sections 7 (re) and 8 (bel) have the teams, roles and times.\n",
        _ => "# The gsc probe's header has the teams, roles and times.\n",
    });
    out.push_str(
        "# One !gamestate per gamestate, one !cmd per serverCommand verbatim, a !sound after\n\
         # each `s` (and UO's announcer `S`) with the alias it names, and one !trace per snapshot whose rendering\n\
         # moved plus one a second (vcod_common::net::capture::trace_body has the format).\n\
         # ms is since the probe started. The probe sends `score` every 2 s, so every `b`\n\
         # here was asked for.\n",
    );
    for l in &capture.lines {
        out.push_str(l);
        out.push('\n');
    }
    let path = write_fixture_file(
        &GAMETYPES_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}-{role}"), tag),
        &out,
        overwrite,
    )?;
    println!("scripted: {} lines -> {path}", capture.lines.len());
    Ok(())
}

/// Writes the spawned player's wire state to `<map>-<gametype>.txt`. The map
/// and gametype come out of cs 0 (serverinfo); the server settings line is
/// fixed text, not read off the server.
pub(super) fn write_playerstate_fixture(
    snap: &net::snapshot::Snapshot,
    configstrings: &[String],
    join: &JoinProbe,
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let p = wire();
    let key = |k: &str| serverinfo_key(configstrings, k);
    let map = key("mapname");
    let gametype = key("g_gametype");

    let mut out = String::new();
    out.push_str(&format!(
        "# Retail CoD {} dedicated server state after a stock menu join.\n",
        retail_build()
    ));
    out.push_str(&format!(
        "# map {map}, g_gametype {gametype}, joined {}, weapon {}, dedicated 1,\n",
        join.team, join.weapon
    ));
    out.push_str("# sv_maxclients 8, sv_pure 0, stock scr_* defaults, one client on the server.\n");
    out.push_str("# Captured with tools/run_server.sh and --net-probe --save-playerstate,\n");
    out.push_str(&format!(
        "# {} s after the weapon menu was answered; snapshot #{}, serverTime {}.\n",
        SPAWN_SETTLE.as_secs(),
        snap.message_num,
        snap.server_time
    ));
    out.push_str("# Values are the raw i32 wire words, floats as their bit patterns: the gate\n");
    out.push_str("# compares bits and a rendered float loses them.\n");
    out.push_str("# [playerstate] is Protocol::player_fields order; [entity] is the probe's\n");
    out.push_str("# own client entity in Protocol::entity_fields order, or a comment saying so\n");
    out.push_str("# when retail sent none; [servercommands] is every serverCommand retail sent\n");
    out.push_str("# after the gamestate, verbatim and in order.\n");

    out.push_str("[playerstate]\n");
    for (f, v) in p.player_fields.iter().zip(&snap.ps.fields) {
        out.push_str(&format!("{} {v}\n", f.name));
    }

    out.push_str("[entity]\n");
    let self_num = snap.ps.field_i32(p, "clientNum") as u32;
    let self_ent = snap.entities.get(&self_num);
    match self_ent {
        Some(ent) => {
            for (f, v) in p.entity_fields.iter().zip(&ent.fields) {
                out.push_str(&format!("{} {v}\n", f.name));
            }
        }
        None => out.push_str("# retail sent no self-entity\n"),
    }

    out.push_str("[servercommands]\n");
    for cmd in &join.commands {
        out.push_str(cmd);
        out.push('\n');
    }

    let path = write_fixture_file(
        &PLAYERSTATE_FIXTURE_DIR,
        &tagged_name(&format!("{map}-{gametype}"), tag),
        &out,
        overwrite,
    )?;
    println!(
        "playerstate: {} fields, {}, {} serverCommands -> {path}",
        snap.ps.fields.len(),
        match self_ent {
            Some(_) => "self-entity present",
            None => "no self-entity",
        },
        join.commands.len(),
    );
    Ok(())
}

/// Where the mover and link fixtures live.
pub(super) static MOVERS_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/movers");

/// Where the item fixtures live, one per capture.
pub(super) static ITEMS_FIXTURE_DIR: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/../server/tests/fixtures/items");

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture name already on disk, tagged or not, is refused without
    /// `--overwrite-fixture`, and the refused capture is kept under `tmp/`.
    /// The first write creates the fixture's directory.
    #[test]
    fn a_write_refuses_to_replace_a_fixture() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("items/mp_carentan-tdm-ads-sniper.txt");
        let path = path.to_str().unwrap();

        let keep = dir.path().join("tmp");
        let write = |out: &str, overwrite| write_capture_in(path, out.as_bytes(), overwrite, &keep);

        write("retail", false).expect("a name nothing holds yet");
        let err = write("ours", false).expect_err("the second write lands on the first");
        assert!(err.to_string().contains("--overwrite-fixture"), "{err}");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "retail");
        let kept = keep.join("mp_carentan-tdm-ads-sniper.txt");
        assert_eq!(std::fs::read_to_string(kept).unwrap(), "ours");

        write("ours", true).expect("the flag says to");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "ours");
    }

    /// The committed fixtures under `crates/server/tests/fixtures`, with their paths.
    fn committed_fixtures() -> Vec<(std::path::PathBuf, String)> {
        let root =
            std::path::Path::new(vcod_common::manifest_dir!()).join("../server/tests/fixtures");
        let mut dirs = vec![root];
        let mut out = Vec::new();
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "txt") {
                    out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        out
    }

    /// Every committed `!cmd`, `!pcmd` and `!wcmd` line in [`cmd_fields`]'s order reads back
    /// into a usercmd that [`cmd_fields`] writes out again byte for byte, across the item,
    /// turret, bump, ladder, weapsel, slope, sd and legs fixtures the probe wrote.
    #[test]
    fn cmd_fields_writes_every_committed_cmd_line_back() {
        const NAMES: [&str; 8] = [
            "st", "buttons", "wbuttons", "weapon", "up", "forward", "right", "angles",
        ];
        let mut checked: BTreeMap<String, usize> = BTreeMap::new();
        for (path, text) in committed_fixtures() {
            for line in text.lines() {
                if !["!cmd ", "!pcmd ", "!wcmd "]
                    .iter()
                    .any(|h| line.starts_with(h))
                {
                    continue;
                }
                let Some(at) = line.find(" st=") else {
                    continue;
                };
                let fields: Vec<&str> = line[at + 1..].split(' ').take(8).collect();
                let named = fields.len() == 8
                    && fields
                        .iter()
                        .zip(NAMES)
                        .all(|(f, n)| f.split_once('=').is_some_and(|(k, _)| k == n));
                if !named {
                    continue;
                }
                let v = |i: usize| fields[i].split_once('=').unwrap().1;
                let num = |i: usize| {
                    v(i).parse::<i64>()
                        .unwrap_or_else(|e| panic!("{line}: {e}"))
                };
                let angles: Vec<i32> = v(7).split(',').map(|a| a.parse().unwrap()).collect();
                let c = net::msg::UserCmd {
                    server_time: num(0) as i32,
                    buttons: num(1) as u8,
                    wbuttons: num(2) as u8,
                    weapon: num(3) as u8,
                    up: num(4) as i8,
                    forward: num(5) as i8,
                    right: num(6) as i8,
                    angles: angles.try_into().unwrap(),
                    ..Default::default()
                };
                assert_eq!(
                    cmd_fields(&c),
                    fields.join(" "),
                    "{}: {line}",
                    path.display()
                );
                let dir = path
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy();
                *checked.entry(dir.into_owned()).or_default() += 1;
            }
        }
        for dir in ["items", "turret", "playerstate"] {
            assert!(
                checked.get(dir).is_some_and(|&n| n > 0),
                "no {dir} cmd lines: {checked:?}"
            );
        }
    }

    /// The configstring table round-trips: the table read out of a committed fixture comes out
    /// of [`configstrings_text`] with the same first header line and the same slot lines. The
    /// other header lines are older than the writer and say the same with other words.
    #[test]
    fn configstrings_text_writes_a_committed_table_back() {
        let committed =
            include_str!("../../../server/tests/fixtures/configstrings/mp_carentan-dm.txt");
        let mut cs: Vec<String> = Vec::new();
        for line in committed.lines().filter(|l| !l.starts_with('#')) {
            let (i, v) = line.split_once(' ').unwrap();
            let i: usize = i.parse().unwrap();
            cs.resize(cs.len().max(i + 1), String::new());
            cs[i] = v.to_string();
        }
        let (out, written) = configstrings_text(&cs);
        let slots = |t: &str| {
            t.lines()
                .filter(|l| !l.starts_with('#'))
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(written, slots(committed).len());
        assert_eq!(out.lines().next(), committed.lines().next());
        assert_eq!(slots(&out), slots(committed));
    }
}
