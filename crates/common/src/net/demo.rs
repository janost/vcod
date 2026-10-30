//! Retail demo files (docs/research/cod11-demos.md): `demos/<name>.dm_<n>`,
//! one record per server message the client parsed, the sequence and the
//! length as little-endian longs ahead of the message as the netchan
//! delivered it (the plain `reliableAcknowledge`, then the unscrambled,
//! still compressed op block), closed by two -1 longs. Recording opens on
//! a gamestate the client builds out of its own state.

use super::gamestate::{self, Gamestate};
use super::huffman::Huffman;
use super::msg::{MsgWriter, SVC_EOF};
use super::protocol::Protocol;
use std::io::{self, Write};

/// The `.dm_<n>` number `CL_Record_f` formats in: 1 on 1.1 (CoDMP.exe
/// 0x40e540), 2 on 1.5 (CoDMP15.exe 0x40f210), 3 on UO (CoDUOMP.exe
/// 0x40fb80).
pub fn extension_number(p: &Protocol) -> u32 {
    match p.transport.version {
        6 => 2,
        22 => 3,
        _ => 1,
    }
}

/// The protocol a `.dm_<n>` file was recorded under.
pub fn protocol_of_extension(n: u32) -> Option<&'static Protocol> {
    use super::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};
    match n {
        1 => Some(&PROTOCOL_V1),
        2 => Some(&PROTOCOL_V6),
        3 => Some(&PROTOCOL_V22),
        _ => None,
    }
}

/// `name`'s `.dm_<n>` number, when it ends in one.
pub fn extension_of(name: &str) -> Option<u32> {
    let (_, n) = name.rsplit_once(".dm_")?;
    n.parse().ok()
}

/// `CL_ReadDemoMessage`'s buffer: 16384 bytes on 1.1 and 1.5, 32768 on UO
/// (CoDMP.exe 0x40e9c0, CoDUOMP.exe 0x410060). A longer record is fatal.
pub fn max_message(p: &Protocol) -> usize {
    if p.is_uo() { 0x8000 } else { 0x4000 }
}

/// `record` with no name: the first of `demo0000`..`demo9999` `exists`
/// says is free, `demo9999` when none is (CoDMP.exe 0x40e4b0).
pub fn auto_name(exists: impl Fn(&str) -> bool) -> String {
    (0..10000)
        .map(|i| format!("demo{i:04}"))
        .find(|n| !exists(n))
        .unwrap_or_else(|| "demo9999".to_string())
}

/// The message `CL_Record_f` opens a demo with: the client's
/// `reliableSequence` where a server message has its acknowledge, then
/// the gamestate as the client holds it now (its configstrings with every
/// `d` applied, the baselines but entity 0's, `serverCommandSequence`),
/// compressed, with the `svc_EOF` a server message ends on.
pub fn gamestate_message(
    p: &Protocol,
    huff: &Huffman,
    reliable_sequence: i32,
    gs: &Gamestate,
) -> Vec<u8> {
    let mut gs_now = Gamestate {
        configstrings: gs.configstrings.clone(),
        baselines: gs.baselines.clone(),
        ..*gs
    };
    gs_now.baselines.remove(&0);
    let mut w = MsgWriter::new(huff);
    gamestate::write(&mut w, p, &gs_now);
    w.write_byte(SVC_EOF);
    let mut out = reliable_sequence.to_le_bytes().to_vec();
    out.extend(w.finish());
    out
}

/// `CL_WriteDemoMessage` (CoDMP.exe 0x40e3d0) and `CL_StopRecord_f`'s
/// trailer (0x40e430).
pub struct DemoWriter<W: Write> {
    out: W,
    written: u64,
}

impl<W: Write> DemoWriter<W> {
    pub fn new(out: W) -> Self {
        DemoWriter { out, written: 0 }
    }

    /// One record: the message's sequence, then its length and bytes.
    pub fn write_message(&mut self, sequence: u32, msg: &[u8]) -> io::Result<()> {
        self.out.write_all(&sequence.to_le_bytes())?;
        self.out.write_all(&(msg.len() as u32).to_le_bytes())?;
        self.out.write_all(msg)?;
        self.written += 8 + msg.len() as u64;
        Ok(())
    }

    /// Bytes written so far, buffered or not: the `ftell` the
    /// `RECORDING` line reads (CoDMP.exe 0x416b10).
    pub fn written(&self) -> u64 {
        self.written
    }

    /// The two -1 longs that end a demo; hands the sink back.
    pub fn finish(mut self) -> io::Result<W> {
        self.out.write_all(&(-1i32).to_le_bytes())?;
        self.out.write_all(&(-1i32).to_le_bytes())?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Why a demo stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DemoEnd {
    /// The -1 trailer, or the file ended between records.
    Completed,
    /// `Demo file was truncated.`: a record's bytes ran past the file.
    Truncated,
    /// `CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN`, fatal on retail.
    TooLong(usize),
}

/// `CL_ReadDemoMessage` (CoDMP.exe 0x40e9c0) over a whole file.
pub struct DemoReader {
    data: Vec<u8>,
    pos: usize,
    max: usize,
}

impl DemoReader {
    pub fn new(data: Vec<u8>, p: &Protocol) -> Self {
        Self::with_max(data, max_message(p))
    }

    /// A reader refusing records past `max` bytes: another game's
    /// `MAX_MSGLEN` (CoD2's 0x20000, `vcod_cod2::net::demo`).
    pub fn with_max(data: Vec<u8>, max: usize) -> Self {
        DemoReader { data, pos: 0, max }
    }

    fn long(&mut self) -> Option<i32> {
        let b = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(i32::from_le_bytes(b.try_into().unwrap()))
    }

    /// The next record's sequence and message, or why there is none.
    pub fn next_message(&mut self) -> Result<(u32, Vec<u8>), DemoEnd> {
        let seq = self.long().ok_or(DemoEnd::Completed)?;
        let len = match self.long() {
            None | Some(-1) => return Err(DemoEnd::Completed),
            Some(n) => n as u32 as usize,
        };
        if len > self.max {
            return Err(DemoEnd::TooLong(len));
        }
        let Some(msg) = self.data.get(self.pos..self.pos + len) else {
            self.pos = self.data.len();
            return Err(DemoEnd::Truncated);
        };
        self.pos += len;
        Ok((seq as u32, msg.to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::msg::MsgReader;
    use crate::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

    #[test]
    fn records_round_trip_and_end_on_the_trailer() {
        let mut w = DemoWriter::new(Vec::new());
        w.write_message(7, &[1, 2, 3, 4, 5]).unwrap();
        w.write_message(9, &[0; 4]).unwrap();
        let bytes = w.finish().unwrap();
        assert_eq!(&bytes[..8], &[7, 0, 0, 0, 5, 0, 0, 0]);
        assert_eq!(&bytes[bytes.len() - 8..], &[0xff; 8]);
        let mut r = DemoReader::new(bytes, &PROTOCOL_V1);
        assert_eq!(r.next_message(), Ok((7, vec![1, 2, 3, 4, 5])));
        assert_eq!(r.next_message(), Ok((9, vec![0; 4])));
        assert_eq!(r.next_message(), Err(DemoEnd::Completed));
    }

    #[test]
    fn a_short_record_is_truncated_and_a_long_one_refused() {
        let mut bytes = Vec::new();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(10u32.to_le_bytes());
        bytes.extend([0; 6]);
        assert_eq!(
            DemoReader::new(bytes, &PROTOCOL_V1).next_message(),
            Err(DemoEnd::Truncated)
        );
        let mut bytes = Vec::new();
        bytes.extend(3u32.to_le_bytes());
        bytes.extend(0x4001u32.to_le_bytes());
        assert_eq!(
            DemoReader::new(bytes.clone(), &PROTOCOL_V1).next_message(),
            Err(DemoEnd::TooLong(0x4001))
        );
        // UO's buffer is twice 1.1's; the record is then short, not long.
        assert_eq!(
            DemoReader::new(bytes, &PROTOCOL_V22).next_message(),
            Err(DemoEnd::Truncated)
        );
    }

    #[test]
    fn extensions_follow_the_client_binaries() {
        assert_eq!(extension_number(&PROTOCOL_V1), 1);
        assert_eq!(extension_number(&PROTOCOL_V6), 2);
        assert_eq!(extension_number(&PROTOCOL_V22), 3);
        for p in [&PROTOCOL_V1, &PROTOCOL_V6, &PROTOCOL_V22] {
            let n = extension_number(p);
            assert_eq!(
                protocol_of_extension(n).unwrap().transport.version,
                p.transport.version
            );
        }
        assert_eq!(extension_of("demo0003.dm_1"), Some(1));
        assert_eq!(extension_of("a.dm_3"), Some(3));
        assert_eq!(extension_of("plain"), None);
        assert_eq!(auto_name(|n| n < "demo0002"), "demo0002");
        assert_eq!(auto_name(|_| true), "demo9999");
    }

    #[test]
    fn the_opening_gamestate_parses_back_and_ends_on_svc_eof() {
        let data = crate::testing::fixture("net/gamestate.bin");
        let h = Huffman::new();
        let mut r = MsgReader::new(&data[4..], &h);
        let gs = gamestate::parse(&mut r, &PROTOCOL_V1).unwrap();
        let msg = gamestate_message(&PROTOCOL_V1, &h, 5, &gs);
        assert_eq!(&msg[..4], &5i32.to_le_bytes());
        let mut r = MsgReader::new(&msg[4..], &h);
        let back = gamestate::parse(&mut r, &PROTOCOL_V1).unwrap();
        assert_eq!(back.configstrings, gs.configstrings);
        assert_eq!(back.baselines.len(), gs.baselines.len());
        assert_eq!(back.client_num, gs.client_num);
        assert_eq!(back.checksum_feed, gs.checksum_feed);
        assert_eq!(r.read_byte(), SVC_EOF);
        // The server's ops past its leading empty `svc_serverCommand` are
        // the same bytes.
        let plain = |m: &[u8]| h.decompress_block(&m[4..]);
        let ours = plain(&msg);
        let retail = plain(&data);
        assert_eq!(&retail[..6], &[5, 0, 0, 0, 0, 0]);
        let n = ours.len() - 1;
        assert_eq!(&ours[..n], &retail[6..6 + n]);
    }
}
