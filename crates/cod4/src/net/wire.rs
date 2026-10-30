//! CoD4 1.7's wire descriptor [`Cod4Wire`] ([`PROTOCOL_17`], protocol 6 of
//! [`Game::Cod4`]) and the server's half of the codec: a gamestate built
//! from a configstring table by retail's omission rule, and a snapshot
//! whose change bits the server picks off its own states
//! (docs/research/cod4-handshake.md 5, cod4-netfields.md).
//!
//! The reader and the capture re-encoder are `parse` and `delta`; this
//! module only decides what they are told.

use super::constant_cs;
use super::delta::{
    self, CLIENTNUM_BITS, Change, ClientState, Ctx, ENTITYNUM_NONE, EntityState, GENTITYNUM_BITS,
    PlayerState, PsWire, Sent, Wire,
};
use super::fields;
use super::parse::{
    CS_MAPCENTER, Gamestate, GamestateOp, MAX_CONFIGSTRINGS, SVC_EOF, SVC_SNAPSHOT,
};
use std::collections::BTreeMap;
use vcod_common::game::Game;
use vcod_common::net::codec::{FieldCodec, MsgWriter};
use vcod_common::net::netchan::NetchanParams;
use vcod_common::net::protocol::{CsLayout, PakRules, SnapshotHead, Transport, WireGame};

/// From this slot on a string equal to its default but for case is still
/// its default: `SV_SendClientGameState` (0x816e10a) compares a slot past
/// 820 (`cmpl $0x334` at 0x816e2e2 and 0x816e3f2) with 0x81aa3b2's
/// case-blind compare, the rest with `strcmp`, and `SV_SetConfigstring`
/// (0x8173e54) interns a string past 820 (0x8173eca) through its own
/// string call. VERIFIED (the constants); the rule INFERRED (the branches).
pub const CS_CASE_INSENSITIVE: u16 = 821;

/// The configstring blocks, each the first slot and the last its indexer
/// scans in `cod4_lnxded` 1.7. VERIFIED (the constants); every index the
/// 1.7 captures carry sits inside its kind's block (VERIFIED live,
/// cod4-handshake.md 5).
pub const CS_LAYOUT_17: CsLayout = CsLayout {
    ranges: [
        // An unrolled scan of 0x8d3..0x8da (0x80bc95d..0x80bca91, then
        // 0x80bcabd..0x80bcbdb for a free slot).
        (2259, 2266), // status icons
        // A loop from 0x8db to 0x8ea (0x80bc62e, 0x80bc664).
        (2267, 2281), // head icons
        // G_FindConfigstringIndex (0x80cae70) from 0x8ea, 32 slots (0x80cb1b2).
        (2282, 2313), // tags
        // The model index plus 0x33e (0x80cac45), 512 slots (0x80cad3d).
        (830, 1341), // models
        // 0x80cae70 from 0x53e, 256 slots (0x80cb114).
        (1342, 1597), // sound aliases
        // 0x80cae70 from 0x63e, 100 slots (0x80cb17f).
        (1598, 1697), // effects
        // 0x80cae70 from 0x6a2, 256 slots (0x80bdfa5).
        (1698, 1953), // effect tags
        // 0x80cae70 from 0x7a2, 16 slots (0x80cb14a).
        (1954, 1969), // shellshocks
        // A loop from 0x7b2 to 0x7d2 (0x80bcc6c, 0x80bcc7c).
        (1970, 2001), // menus
        // 0x80cae70 from 0x135, 512 slots (0x80cb266).
        (309, 820), // localized strings
        // 0x80cae70 from 0x7d2, 256 slots (0x80cb202).
        (2002, 2257), // materials
    ],
    // The mirrored dvars' names and values (0x8175017, 0x8174378), the
    // hint strings (0x80c6270), the vote (0x80ae074) and the motd
    // (0x80c82bd) are their writers' slots; fog, the end time and the
    // winner the slots the captures carry (cod4-handshake.md 5).
    cvar_names: (20, 147),
    cvar_values: 148,
    hint_strings: 277,
    match_timeout: None,
    vote: 13,
    fog: 9,
    // `CS_GAMEENDTIME`: 1.7 has no level start time slot.
    level_start: 11,
    motd: 10,
    winner: 19,
};

/// 1.7's wire: the transport and the configstring layout. The codec is this
/// crate's own (`delta`, `huffman`); `field_codec` is never read for it.
pub struct Cod4Wire {
    pub transport: Transport,
    pub cs: CsLayout,
}

/// Protocol 6 of CoD4: `shortversion` 1.7 (`EXE_SERVER_IS_DIFFERENT_VER`
/// names it, VERIFIED live), CoD2's netchan framing (cod4-handshake.md 4),
/// the stock `iw_00`..`iw_13` iwds.
pub static PROTOCOL_17: Cod4Wire = Cod4Wire {
    transport: Transport {
        game: Game::Cod4,
        version: 6,
        short_version: "1.7",
        own_master: None,
        max_configstrings: MAX_CONFIGSTRINGS as usize,
        field_codec: FieldCodec::COD2,
        netchan: NetchanParams::COD2,
        paks: PakRules {
            id_pak_numbers: &[
                "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13",
            ],
            id_pak_dirs: &["main"],
            id_pak_prefixes: &["iw_"],
            skips_svr_paks: false,
        },
    },
    cs: CS_LAYOUT_17,
};

/// One snapshot as the server built it: the client's playerstate, the
/// entities and clients it sees (each sorted by number), and the map
/// centre its origins are coded against.
#[derive(Clone, Debug)]
pub struct Frame4 {
    pub server_time: i32,
    pub map_center: [f32; 3],
    pub ps: PlayerState,
    pub entities: Vec<EntityState>,
    pub clients: Vec<ClientState>,
}

impl WireGame for Cod4Wire {
    type UserCmd = super::clmsg::UserCmd;
    type EntityState = EntityState;
    type PlayerState = PlayerState;
    type Frame = Frame4;
    type Gamestate = Gamestate;
    type Baselines = BTreeMap<u32, EntityState>;

    fn transport(&self) -> &Transport {
        &self.transport
    }

    fn cs(&self) -> &CsLayout {
        &self.cs
    }

    /// The text's bytes as they are, no trailing NUL (VERIFIED live).
    fn oob_packet(&self, text: &str) -> Vec<u8> {
        let mut p = vec![0xff; 4];
        p.extend_from_slice(text.as_bytes());
        p
    }

    fn write_gamestate(&self, w: &mut MsgWriter, gs: &Gamestate) {
        super::parse::write_gamestate(w, gs);
    }

    fn write_snapshot(
        &self,
        w: &mut MsgWriter,
        base: Option<&Frame4>,
        frame: &Frame4,
        head: SnapshotHead,
        baselines: &BTreeMap<u32, EntityState>,
    ) {
        write_snapshot(w, base, frame, head, baselines);
    }
}

/// Whether `value` in slot `i` is the slot's `constantConfigStrings`
/// default, compared case-blind from [`CS_CASE_INSENSITIVE`].
fn is_default(i: u16, value: &str) -> Option<bool> {
    let d = constant_cs::default_of(i)?;
    Some(if i >= CS_CASE_INSENSITIVE {
        d.eq_ignore_ascii_case(value)
    } else {
        d == value
    })
}

impl Gamestate {
    /// `SV_SendClientGameState`'s gamestate for a server holding `table`
    /// (index = slot): a slot equal to its default is left out, an empty one
    /// that has a default goes out empty, an empty one without is left out
    /// (INFERRED; on the 1.7 captures no carried slot holds
    /// its default, VERIFIED live). The baselines go out whole from nothing.
    pub fn from_table(
        server_command_sequence: i32,
        table: &[String],
        baselines: &BTreeMap<u32, EntityState>,
        client_num: i32,
        checksum_feed: i32,
    ) -> Gamestate {
        let mut gs = Gamestate {
            server_command_sequence,
            client_num,
            checksum_feed,
            baselines: baselines.clone(),
            ..Gamestate::default()
        };
        let mut sent = Vec::new();
        for (i, s) in table.iter().enumerate().take(MAX_CONFIGSTRINGS as usize) {
            let i = i as u16;
            let keep = match is_default(i, s) {
                Some(d) => !d,
                None => !s.is_empty(),
            };
            if keep {
                gs.configstrings.insert(i, s.as_bytes().to_vec());
                sent.push((i, s.as_bytes().to_vec()));
            }
        }
        gs.map_center = map_center(gs.configstring(CS_MAPCENTER));
        gs.ops.push(GamestateOp::Configstrings(sent));
        for (&n, e) in baselines {
            let wire = entity_wire(&EntityState::default(), e, true).unwrap_or(Wire::Unchanged);
            gs.ops.push(GamestateOp::Baseline(n, wire));
        }
        gs
    }
}

/// `CL_ParseMapCenter`'s `"%f %f %f"`, unread axes 0.
pub fn map_center(s: &[u8]) -> [f32; 3] {
    let s = String::from_utf8_lossy(s);
    let mut out = [0.0; 3];
    for (o, v) in out.iter_mut().zip(s.split_whitespace()) {
        match v.parse() {
            Ok(f) => *o = f,
            Err(_) => break,
        }
    }
    out
}

/// `MSG_WriteEntityDelta`'s choice for one entity: the general table's
/// eType row first, then the eType's own rows, up to the last that
/// differs; `None` for an unforced unchanged one.
pub(crate) fn entity_wire(from: &EntityState, to: &EntityState, force: bool) -> Option<Wire> {
    let table = EntityState::table(to.etype());
    let rows = (0..table.len()).map(|i| {
        let f = if i == 0 {
            &fields::ENTITY_GENERAL[0]
        } else {
            &table[i]
        };
        Change::natural(f, from.0[f.word()], to.0[f.word()])
    });
    let mut v: Vec<Change> = rows.collect();
    let lc = v
        .iter()
        .rposition(|&c| c != Change::Kept)
        .map_or(0, |i| i + 1);
    v.truncate(lc);
    if lc > 0 {
        Some(Wire::Changed(Sent(v)))
    } else {
        force.then_some(Wire::Unchanged)
    }
}

/// `SV_WriteSnapshotToClient` from `svc_snapshot` on: the playerstate
/// deltaed from `base`'s (origin and velocity always sent, D8), every
/// entity and client against `base`'s or, new to the frame, its baseline
/// or nothing, and `svc_EOF`'s callers' terminator left to the caller.
pub fn write_snapshot(
    w: &mut MsgWriter,
    base: Option<&Frame4>,
    frame: &Frame4,
    head: SnapshotHead,
    baselines: &BTreeMap<u32, EntityState>,
) {
    let ctx = Ctx {
        time: frame.server_time,
        map_center: frame.map_center,
    };
    w.write_byte(SVC_SNAPSHOT);
    w.write_long(frame.server_time);
    w.write_byte(head.delta_num);
    w.write_byte(head.snap_flags as u8);
    let zero = PlayerState::default();
    let from_ps = base.map_or(&zero, |b| &b.ps);
    let mut to_ps = frame.ps.clone();
    to_ps.origin_sent = true;
    let ps_wire = PsWire::natural(from_ps, &to_ps);
    delta::write_delta_playerstate(w, &ctx, base.map(|b| &b.ps), &to_ps, &ps_wire);

    let old: &[EntityState] = base.map_or(&[], |b| &b.entities);
    let mut last = -1;
    let (mut oi, mut ni) = (0, 0);
    loop {
        let o = old.get(oi);
        let n = frame.entities.get(ni);
        let (num, wire, from, to) = match (o, n) {
            (None, None) => break,
            (Some(o), Some(n)) if o.number() == n.number() => {
                oi += 1;
                ni += 1;
                match entity_wire(o, n, false) {
                    Some(wire) => (n.number(), wire, *o, *n),
                    None => continue,
                }
            }
            (Some(o), n) if n.is_none_or(|n| o.number() < n.number()) => {
                oi += 1;
                (o.number(), Wire::Removed, *o, *o)
            }
            (_, Some(n)) => {
                ni += 1;
                let b = baselines.get(&n.number()).copied().unwrap_or_default();
                let wire = entity_wire(&b, n, true).unwrap_or(Wire::Unchanged);
                (n.number(), wire, b, *n)
            }
            (Some(_), None) => unreachable!(),
        };
        delta::write_entity_index(w, &mut last, num, GENTITYNUM_BITS);
        delta::write_delta_entity(w, &ctx, &from, &to, &wire);
    }
    delta::write_entity_index(w, &mut last, ENTITYNUM_NONE, GENTITYNUM_BITS);

    let old: &[ClientState] = base.map_or(&[], |b| &b.clients);
    let table = &fields::CLIENT_STATE;
    let mut last = -1;
    let (mut oi, mut ni) = (0, 0);
    loop {
        let o = old.get(oi);
        let n = frame.clients.get(ni);
        let (num, wire, from, to) = match (o, n) {
            (None, None) => break,
            (Some(o), Some(n)) if o.0[0] == n.0[0] => {
                oi += 1;
                ni += 1;
                match Wire::natural(table, &o.0, Some(&n.0), false) {
                    Some(wire) => (n.0[0], wire, *o, *n),
                    None => continue,
                }
            }
            (Some(o), n) if n.is_none_or(|n| o.0[0] < n.0[0]) => {
                oi += 1;
                (o.0[0], Wire::Removed, *o, *o)
            }
            (_, Some(n)) => {
                ni += 1;
                let z = ClientState::default();
                let wire = Wire::natural(table, &z.0, Some(&n.0), true).unwrap();
                (n.0[0], wire, z, *n)
            }
            (Some(_), None) => unreachable!(),
        };
        w.write_bits(1, 1);
        delta::write_entity_index(w, &mut last, num, CLIENTNUM_BITS);
        delta::write_delta_client(w, &ctx, &from, &to, &wire);
    }
    w.write_bits(0, 1);
}

/// `svc_EOF`, the byte every server message ends on.
pub const EOF: u8 = SVC_EOF;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::huffman::Huffman;
    use crate::net::parse::Client;

    fn table() -> Vec<String> {
        let mut t = vec![String::new(); MAX_CONFIGSTRINGS as usize];
        t[0] = "\\protocol\\6".into();
        t[12] = "10.000000 -20.000000 30.000000".into();
        // Slot 20 holds its default and is left out; 350's empty one goes out.
        t[20] = constant_cs::default_of(20).unwrap().into();
        t
    }

    /// What the server writes, the client's parser reads back to the same
    /// table and re-encodes byte for byte.
    #[test]
    fn a_built_gamestate_and_snapshots_parse_back() {
        let gs = Gamestate::from_table(3, &table(), &BTreeMap::new(), 1, 77);
        assert!(!gs.configstrings.contains_key(&20));
        assert_eq!(gs.configstrings.get(&350).map(Vec::len), Some(0));
        let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        let mut ps = PlayerState::default();
        let set = |ps: &mut PlayerState, name: &str, v: u32| {
            let f = fields::PLAYER_STATE
                .iter()
                .find(|f| f.name == name)
                .unwrap();
            ps.words[f.word()] = v;
        };
        set(&mut ps, "origin[0]", 336.0f32.to_bits());
        set(&mut ps, "pm_type", 4);
        let mut c = ClientState::default();
        c.0[0] = 1;
        c.0[1] = 3;
        let f1 = Frame4 {
            server_time: 1000,
            map_center: gs.map_center,
            ps: ps.clone(),
            entities: Vec::new(),
            clients: vec![c],
        };
        let head = SnapshotHead {
            delta_num: 0,
            snap_flags: 4,
        };
        PROTOCOL_17.write_snapshot(&mut w, None, &f1, head, &BTreeMap::new());
        w.write_byte(EOF);
        let huff = Huffman::new();
        let block = huff.compress(&w.into_ops());
        let mut client = Client::new();
        let parsed = client.parse(1, &block).unwrap();
        assert!(parsed.exact);
        let back = client.gamestate.as_ref().unwrap();
        assert_eq!(
            back.configstring(20),
            constant_cs::default_of(20).unwrap().as_bytes()
        );
        assert_eq!(back.map_center, [10.0, -20.0, 30.0]);
        let s = &client.snapshots[0];
        assert_eq!(s.ps.get_f32("origin[0]"), Some(336.0));
        assert_eq!(s.clients, vec![c]);
        assert_eq!(
            client.write(&parsed).unwrap(),
            huff.decompress(&block)[..parsed.eof_at]
        );

        // A delta one message on: the playerstate moves, the client leaves.
        let mut f2 = f1.clone();
        f2.server_time = 1050;
        set(&mut f2.ps, "origin[0]", 340.0f32.to_bits());
        f2.clients.clear();
        let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        let head = SnapshotHead {
            delta_num: 1,
            snap_flags: 4,
        };
        PROTOCOL_17.write_snapshot(&mut w, Some(&f1), &f2, head, &BTreeMap::new());
        w.write_byte(EOF);
        let block = huff.compress(&w.into_ops());
        let parsed = client.parse(2, &block).unwrap();
        assert!(parsed.exact);
        let s = &client.snapshots[1];
        assert_eq!(s.ps.get_f32("origin[0]"), Some(340.0));
        assert!(s.clients.is_empty());
    }
}
