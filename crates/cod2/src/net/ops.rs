//! CoD2's message ops (docs/research/cod2-protocol.md 3.1, 4.1). The svc ops
//! sit one below 1.1's (no `svc_bad`); the clc op is 3 bits wide.

/// `svc_ops_e`, a whole byte each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Svc {
    Nop = 0,
    Gamestate = 1,
    Configstring = 2,
    Baseline = 3,
    ServerCommand = 4,
    Download = 5,
    Snapshot = 6,
    Eof = 7,
}

impl Svc {
    pub fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            0 => Svc::Nop,
            1 => Svc::Gamestate,
            2 => Svc::Configstring,
            3 => Svc::Baseline,
            4 => Svc::ServerCommand,
            5 => Svc::Download,
            6 => Svc::Snapshot,
            7 => Svc::Eof,
            _ => return None,
        })
    }
}

/// `clc_ops_e`, [`CLC_BITS`] wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Clc {
    Move = 0,
    MoveNoDelta = 1,
    ClientCommand = 2,
    Eof = 3,
}

pub const CLC_BITS: i32 = 3;

/// Gamestate limits (3.2, 3.3).
pub const MAX_CONFIGSTRINGS: usize = 2048;
pub const MAX_GENTITIES: usize = 1024;
pub const GENTITYNUM_BITS: i32 = 10;
pub const ENTITYNUM_NONE: u32 = 1023;
pub const CLIENTNUM_BITS: i32 = 6;
pub const PACKET_BACKUP: usize = 32;
/// The command rings on both ends (2.4, 3.4).
pub const MAX_RELIABLE_COMMANDS: usize = 128;
pub const PROTOCOL: i32 = 118;
