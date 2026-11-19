//! Fixture writers the modes share: gamestate, snapshots, configstrings, playerstate, scripted, and
//! the capture write that refuses to replace a fixture.

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
    let tagged = |path: &str| match tag {
        Some(t) if save_fixture || save_snapshots => {
            path.replace(".bin", &format!("{}.bin", tag_suffix(Some(t))))
        }
        _ => path.to_string(),
    };
    let save_fixture = save_fixture || (tag.is_some() && save_snapshots);
    // The scratch copies under `tmp/` are rewritten freely; a fixture is not.
    let guard = |path: &str| -> anyhow::Result<()> {
        let scratch = path == SCRATCH_PATH || path == SNAP_SCRATCH_PATH;
        if !scratch && !overwrite && std::path::Path::new(path).exists() {
            anyhow::bail!(
                "{path} already exists; pass --overwrite-fixture to replace it, or \
                 --capture-tag for a new name"
            );
        }
        Ok(())
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
            tagged(FIXTURE_PATH)
        } else {
            SCRATCH_PATH.to_string()
        };
        guard(&path)?;
        write_fixture(&path, gs_bytes)?;
        println!("gamestate: {} bytes -> {path}", gs_bytes.len());
    }
    if cap.count == 0 {
        println!("no snapshots captured");
        return Ok(());
    }
    let path = if save_fixture || save_snapshots {
        tagged(SNAP_FIXTURE_PATH)
    } else {
        SNAP_SCRATCH_PATH.to_string()
    };
    guard(&path)?;
    write_fixture(&path, &cap.triples)?;
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

fn write_fixture(path: &str, data: &[u8]) -> anyhow::Result<()> {
    let p = std::path::Path::new(path);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, data)?;
    Ok(())
}

/// Directory `crates/server/tests/configstrings_ab.rs` reads its fixtures
/// from.
const CONFIGSTRINGS_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/configstrings"
);

/// Writes the non-empty configstring table to
/// `<map>-<gametype>.txt`. The map, gametype, client count and pureness come
/// out of cs 0 (serverinfo) itself, so nothing here is hand-typed.
pub(super) fn write_configstrings_fixture(
    configstrings: &[String],
    tag: Option<&str>,
    overwrite: bool,
) -> anyhow::Result<()> {
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
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

    let path = fixture_path(
        CONFIGSTRINGS_FIXTURE_DIR,
        &format!("{map}-{gametype}{}.txt", tag_suffix(tag)),
    )?;
    write_capture(&path, &out, overwrite)?;
    println!("configstrings: {written} non-empty slots -> {path}");
    Ok(())
}

/// Directory `crates/server/tests/playerstate_ab.rs` reads its fixtures from.
pub(super) const PLAYERSTATE_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/playerstate"
);

/// Writes a fixture, refusing to land on a file that is already there
/// unless `--overwrite-fixture` says to. The fixed names hold retail
/// evidence, and a tag is free text that can spell one of them
/// (`--save-ads --capture-tag sniper` is the scoped-rifle fixture's path):
/// a capture against vcod's own server silently replacing the oracle is the
/// one mistake these directories cannot survive. A refused capture is kept
/// under `tmp/` rather than thrown away.
pub(super) fn write_capture(path: &str, out: &str, overwrite: bool) -> anyhow::Result<()> {
    write_capture_in(path, out, overwrite, std::path::Path::new("tmp"))
}

/// [`write_capture`] keeping a refused capture in `keep` instead of `tmp/`.
pub(super) fn write_capture_in(
    path: &str,
    out: &str,
    overwrite: bool,
    keep: &std::path::Path,
) -> anyhow::Result<()> {
    if overwrite || !std::path::Path::new(path).exists() {
        std::fs::write(path, out)?;
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
pub(crate) fn tag_suffix(tag: Option<&str>) -> String {
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

/// Where `--save-scripted` writes.
const GAMETYPES_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/gametypes"
);

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
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let probe_gt = net::info_value_for_key(serverinfo, "g_gametype").unwrap_or("?");
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
    std::fs::create_dir_all(GAMETYPES_FIXTURE_DIR)?;
    let path = fixture_path(
        GAMETYPES_FIXTURE_DIR,
        &format!("{map}-{gametype}-{role}{}.txt", tag_suffix(tag)),
    )?;
    write_capture(&path, &out, overwrite)?;
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
    let serverinfo = configstrings.first().map(String::as_str).unwrap_or("");
    let key = |k: &str| net::info_value_for_key(serverinfo, k).unwrap_or("?");
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

    let path = fixture_path(
        PLAYERSTATE_FIXTURE_DIR,
        &format!("{map}-{gametype}{}.txt", tag_suffix(tag)),
    )?;
    std::fs::create_dir_all(PLAYERSTATE_FIXTURE_DIR)?;
    write_capture(&path, &out, overwrite)?;
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
pub(super) const MOVERS_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/movers"
);

/// Where the item fixtures live, one per capture.
pub(super) const ITEMS_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../server/tests/fixtures/items"
);
