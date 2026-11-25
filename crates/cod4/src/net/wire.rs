//! CoD4's wire descriptors [`Cod4Wire`]: 1.7's ([`PROTOCOL_17`], protocol 6
//! of [`Game::Cod4`]) and CoD4x 21's ([`PROTOCOL_X21`]), told apart in the
//! codec by their [`Dialect`]; and the server's half of the 1.7 codec: a gamestate built
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
use vcod_common::net::Msec;
use vcod_common::net::codec::{FieldCodec, MsgWriter};
use vcod_common::net::netchan::NetchanParams;
use vcod_common::net::protocol::{CsLayout, PakRules, SnapshotHead, Transport, WireGame, WireId};

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

/// A CoD4 wire: the transport, the configstring layout and the codec's
/// [`Dialect`]. The codec is this crate's own (`delta`, `huffman`);
/// `field_codec` is never read for it.
pub struct Cod4Wire {
    pub transport: Transport,
    pub cs: CsLayout,
    pub dialect: Dialect,
}

/// What the codec reads differently on each CoD4 wire: retail 1.7, or
/// CoD4x 21 (docs/research/cod4x-protocol.md 8 to 12): origins as whole
/// floats, the gamestate over the side channel ([`super::reliable`]),
/// `svc 11` client config, the 16-byte client header and 4883
/// configstring slots with no defaults. [`Self::wire`] names its wire;
/// two wires never share one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    V17,
    X21,
}

impl Dialect {
    pub fn wire(self) -> &'static Cod4Wire {
        match self {
            Dialect::V17 => &PROTOCOL_17,
            Dialect::X21 => &PROTOCOL_X21,
        }
    }

    pub fn id(self) -> WireId {
        self.wire().id()
    }

    /// The dialect of a CoD4 wire, `None` for another game's.
    pub fn of(id: WireId) -> Option<Dialect> {
        [Dialect::V17, Dialect::X21]
            .into_iter()
            .find(|d| d.id() == id)
    }

    /// Slots in the configstring table.
    pub fn max_configstrings(self) -> u32 {
        self.wire().transport.max_configstrings as u32
    }

    /// Whether a slot the gamestate leaves out holds its
    /// `constantConfigStrings` default. CoD4x sends every slot
    /// (cod4x-protocol.md 9), so its table has none.
    pub fn has_cs_defaults(self) -> bool {
        self == Dialect::V17
    }
}

/// The stock `iw_00`..`iw_13` iwds.
const PAKS_17: PakRules = PakRules {
    id_pak_numbers: &[
        "00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "10", "11", "12", "13",
    ],
    id_pak_dirs: &["main"],
    id_pak_prefixes: &["iw_"],
    skips_svr_paks: false,
};

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
        paks: PAKS_17,
    },
    cs: CS_LAYOUT_17,
    dialect: Dialect::V17,
};

/// `SV_WriteGameState`'s slot count on CoD4x 21: every slot of 0..0x1312
/// goes out (cod4x-protocol.md 9). VERIFIED (the instructions) and live.
pub const MAX_CONFIGSTRINGS_X21: u32 = 4883;

/// CoD4x 21's wire: `connect` with protocol 21 after an `xproto`
/// challenge, `shortversion` `x21` (cod4x-protocol.md 2, 3), 1.7's netchan
/// framing, Huffman code, netfield tables and configstring layout below
/// slot 2442 (8, 10, 12). The snapshot it writes picks fields by 1.7's
/// equality test (10.1); vcod-server does not speak this wire (18).
pub static PROTOCOL_X21: Cod4Wire = Cod4Wire {
    transport: Transport {
        game: Game::Cod4,
        version: 21,
        short_version: "x21",
        own_master: None,
        max_configstrings: MAX_CONFIGSTRINGS_X21 as usize,
        field_codec: FieldCodec::COD2,
        netchan: NetchanParams::COD2,
        paks: PAKS_17,
    },
    cs: CS_LAYOUT_17,
    dialect: Dialect::X21,
};

/// One snapshot as the server built it: the client's playerstate, the
/// entities and clients it sees (each sorted by number), and the map
/// centre its origins are coded against.
#[derive(Clone, Debug)]
pub struct Frame4 {
    pub server_time: Msec,
    pub map_center: [f32; 3],
    pub ps: PlayerState,
    pub entities: Vec<EntityState>,
    pub clients: Vec<ClientState>,
}

impl Cod4Wire {
    /// `svc_gamestate` to its `svc_EOF`. The caller writes the
    /// `reliableAcknowledge` and the pending server commands ahead of it.
    pub fn write_gamestate(&self, w: &mut MsgWriter, gs: &Gamestate) {
        super::parse::write_gamestate(w, gs);
    }
}

impl WireGame for Cod4Wire {
    type Frame = Frame4;
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

    fn write_snapshot(
        &self,
        w: &mut MsgWriter,
        base: Option<&Frame4>,
        frame: &Frame4,
        head: SnapshotHead,
        baselines: &BTreeMap<u32, EntityState>,
    ) {
        snapshot_in(self.dialect, w, base, frame, head, baselines);
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
/// deltaed from `base`'s (origin and velocity left out when its
/// `origin_sent` is clear, cod4-netfields.md 3), every entity and
/// client against `base`'s or, new to the frame, its baseline or nothing.
/// The caller writes the `svc_EOF` terminator. 1.7's wire.
pub fn write_snapshot(
    w: &mut MsgWriter,
    base: Option<&Frame4>,
    frame: &Frame4,
    head: SnapshotHead,
    baselines: &BTreeMap<u32, EntityState>,
) {
    snapshot_in(Dialect::V17, w, base, frame, head, baselines);
}

/// [`write_snapshot`] in `dialect`'s field codes.
fn snapshot_in(
    dialect: Dialect,
    w: &mut MsgWriter,
    base: Option<&Frame4>,
    frame: &Frame4,
    head: SnapshotHead,
    baselines: &BTreeMap<u32, EntityState>,
) {
    let ctx = Ctx {
        time: frame.server_time,
        map_center: frame.map_center,
        dialect,
    };
    w.write_byte(SVC_SNAPSHOT);
    w.write_long(frame.server_time.raw());
    w.write_byte(head.delta_num);
    w.write_byte(head.snap_flags as u8);
    let from_ps = base.map_or(&*delta::ZERO_PS, |b| &b.ps);
    let ps_wire = PsWire::natural(from_ps, &frame.ps);
    delta::write_delta_playerstate(w, &ctx, base.map(|b| &b.ps), &frame.ps, &ps_wire);

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
            server_time: Msec::from_raw(1000),
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
        f2.server_time = Msec::from_raw(1050);
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

    /// A wire is its game and version together: CoD4 1.7 and CoD1 1.5 both
    /// speak protocol 6, and only CoD4's two map to a dialect.
    #[test]
    fn each_dialect_names_its_own_wire() {
        use vcod_common::game::Game;
        assert_eq!(Dialect::of(PROTOCOL_17.id()), Some(Dialect::V17));
        assert_eq!(Dialect::of(PROTOCOL_X21.id()), Some(Dialect::X21));
        assert_ne!(PROTOCOL_17.id(), PROTOCOL_X21.id());
        let cod15 = WireId {
            game: Game::Cod1,
            version: 6,
        };
        assert_eq!(Dialect::of(cod15), None);
        assert_eq!(Dialect::X21.max_configstrings(), 4883);
        assert_eq!(Dialect::V17.max_configstrings(), MAX_CONFIGSTRINGS);
    }

    /// A demo's sequence is any word: message 2^31 deltas one back from
    /// 2^31 - 1, as retail's `int` subtraction wraps (iw3mp.exe 0x47378a).
    #[test]
    fn a_delta_across_the_sign_bit_resolves_its_base() {
        let gs = Gamestate::from_table(3, &table(), &BTreeMap::new(), 1, 77);
        let huff = Huffman::new();
        let mut client = Client::new();
        let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        w.write_byte(EOF);
        client.parse(1, &huff.compress(&w.into_ops())).unwrap();
        let mut ps = PlayerState::default();
        let f = fields::PLAYER_STATE
            .iter()
            .find(|f| f.name == "origin[0]")
            .unwrap();
        ps.words[f.word()] = 336.0f32.to_bits();
        let f1 = Frame4 {
            server_time: Msec::from_raw(1000),
            map_center: gs.map_center,
            ps,
            entities: Vec::new(),
            clients: Vec::new(),
        };
        for (n, base) in [(0x7fff_ffff, None), (0x8000_0000, Some(&f1))] {
            let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
            let head = SnapshotHead {
                delta_num: base.map_or(0, |_| 1),
                snap_flags: 4,
            };
            PROTOCOL_17.write_snapshot(&mut w, base, &f1, head, &BTreeMap::new());
            w.write_byte(EOF);
            client.parse(n, &huff.compress(&w.into_ops())).unwrap();
        }
        let s = client.snapshots.last().unwrap();
        assert_eq!(s.message_num, i32::MIN);
        assert_eq!(s.base, Some(0));
        assert_eq!(s.ps.get_f32("origin[0]"), Some(336.0));
    }

    /// A delta byte that reaches message 0 or before takes the no-base arm
    /// (`jg` at iw3mp.exe 0x4737d1): the snapshot reads from nothing.
    #[test]
    fn a_delta_base_of_zero_or_less_reads_from_nothing() {
        let gs = Gamestate::from_table(3, &table(), &BTreeMap::new(), 1, 77);
        let huff = Huffman::new();
        let mut ps = PlayerState::default();
        let f = fields::PLAYER_STATE
            .iter()
            .find(|f| f.name == "origin[0]")
            .unwrap();
        ps.words[f.word()] = 336.0f32.to_bits();
        let f1 = Frame4 {
            server_time: Msec::from_raw(1000),
            map_center: gs.map_center,
            ps,
            entities: Vec::new(),
            clients: Vec::new(),
        };
        for (n, back) in [(3, 3u8), (2, 5)] {
            let mut client = Client::new();
            let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
            PROTOCOL_17.write_gamestate(&mut w, &gs);
            w.write_byte(EOF);
            client.parse(1, &huff.compress(&w.into_ops())).unwrap();
            let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
            let head = SnapshotHead {
                delta_num: back,
                snap_flags: 4,
            };
            // Written from nothing under a non-zero byte.
            PROTOCOL_17.write_snapshot(&mut w, None, &f1, head, &BTreeMap::new());
            w.write_byte(EOF);
            client.parse(n, &huff.compress(&w.into_ops())).unwrap();
            let s = client.snapshots.last().unwrap();
            assert_eq!((s.message_num, s.base), (n as i32, None));
            assert_eq!(s.delta_num, i32::from(back));
            assert_eq!(s.ps.get_f32("origin[0]"), Some(336.0));
        }
    }

    /// A base whose entities the 2048-row parse ring wrote over is one
    /// retail no longer holds: more than 0x780 entities parsed since it
    /// and the message is illegible (`iw3mp.exe` 0x47389f), 0x780 itself
    /// still reads.
    #[test]
    fn a_base_past_the_parse_rings_reach_is_illegible() {
        let gs = Gamestate::from_table(3, &table(), &BTreeMap::new(), 1, 77);
        let huff = Huffman::new();
        let mut client = Client::new();
        let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        w.write_byte(EOF);
        client.parse(1, &huff.compress(&w.into_ops())).unwrap();
        let f = Frame4 {
            server_time: Msec::from_raw(1000),
            map_center: gs.map_center,
            ps: PlayerState::default(),
            entities: (0..960)
                .map(|n| {
                    let mut e = EntityState::default();
                    e.0[0] = n;
                    e
                })
                .collect(),
            clients: Vec::new(),
        };
        let mut parse = |n: u32, back: u8| {
            let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
            let head = SnapshotHead {
                delta_num: back,
                snap_flags: 4,
            };
            let base = (back > 0).then_some(&f);
            PROTOCOL_17.write_snapshot(&mut w, base, &f, head, &BTreeMap::new());
            w.write_byte(EOF);
            client.parse(n, &huff.compress(&w.into_ops()))
        };
        // Message 2 starts the ring at 0; 3 and 4 bring it to 1920, 2880.
        for (n, back) in [(2, 0), (3, 1), (4, 2)] {
            parse(n, back).unwrap();
        }
        let e = parse(5, 3).unwrap_err();
        assert!(
            e.downcast_ref::<crate::net::parse::Illegible>().is_some(),
            "{e:#}"
        );
        parse(5, 1).unwrap();
        assert_eq!(client.snapshots.last().unwrap().entities.len(), 960);
    }

    /// Frames at `i32::MAX` and, one delta on, `i32::MIN`: the parser
    /// reads both times back and re-encodes each message byte for byte.
    #[test]
    fn frames_at_the_ends_of_int_round_trip() {
        let gs = Gamestate::from_table(3, &table(), &BTreeMap::new(), 1, 77);
        let huff = Huffman::new();
        let mut client = Client::new();
        let mut w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        PROTOCOL_17.write_gamestate(&mut w, &gs);
        let f1 = Frame4 {
            server_time: Msec::MAX,
            map_center: gs.map_center,
            ps: PlayerState::default(),
            entities: Vec::new(),
            clients: Vec::new(),
        };
        let f2 = Frame4 {
            server_time: Msec::MAX + 50,
            ..f1.clone()
        };
        assert_eq!(f2.server_time, Msec::MIN + 49);
        for (n, base, f) in [(1, None, &f1), (2, Some(&f1), &f2)] {
            let head = SnapshotHead {
                delta_num: base.map_or(0, |_| 1),
                snap_flags: 4,
            };
            PROTOCOL_17.write_snapshot(&mut w, base, f, head, &BTreeMap::new());
            w.write_byte(EOF);
            let block = huff.compress(&w.into_ops());
            let parsed = client.parse(n, &block).unwrap();
            assert!(parsed.exact);
            assert_eq!(client.snapshots.last().unwrap().server_time, f.server_time);
            assert_eq!(
                client.write(&parsed).unwrap(),
                huff.decompress(&block)[..parsed.eof_at]
            );
            w = MsgWriter::new(&crate::net::parse::UNUSED_HUFF);
        }
    }
}
