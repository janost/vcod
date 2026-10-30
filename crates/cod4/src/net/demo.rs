//! CoD4 1.7 demo files (docs/research/cod4-demos.md): `demos/<name>.dm_1`,
//! a stream of typed records. Type 0 is a server message as the client
//! parsed it (its sequence, its length, the plain `reliableAcknowledge`
//! and the unscrambled, still compressed op block), type 1 one row of the
//! client archive (the predicted origin, velocity, view and bob at a
//! client time); a type 0 record with a -1 sequence and length closes the
//! file. Recording opens on a gamestate the client builds out of its own
//! state, then the whole archive.

use std::io::{self, Write};

use vcod_common::net::codec::MsgWriter;

use super::constant_cs;
use super::delta::{self, Ctx, EntityState, GENTITYNUM_BITS};
use super::huffman::Huffman;
use super::parse::{
    CS_MAPCENTER, Gamestate, MAX_CONFIGSTRINGS, SVC_BASELINE, SVC_CONFIGSTRING, SVC_EOF,
    SVC_GAMESTATE, UNUSED_HUFF,
};
use super::wire::{entity_wire, map_center};

/// The number after `.dm_`: `CL_Record_f` and `CL_PlayDemo_f` push a
/// literal 1 (`iw3mp.exe` 0x4689f5, 0x46919d).
pub const EXTENSION: u32 = 1;

/// `CL_ReadDemoMessage`'s buffer (`iw3mp.exe` 0x468fce): a longer record
/// is fatal.
pub const MAX_MSGLEN: usize = 0x20000;

/// Rows in `cl.clientArchive` (the `& 0xff` at 0x46882b).
pub const ARCHIVE_ROWS: usize = 256;

/// The `time` `CL_Record_f` hands `MSG_WriteDeltaEntity` for the
/// baselines (the `push 0xfffea070` at 0x468cce): every time field goes
/// out whole.
pub const BASELINE_TIME: i32 = -90000;

const REC_MESSAGE: u8 = 0;
const REC_ARCHIVE: u8 = 1;

/// `demos/<name>`'s file for `demo` / `timedemo`: a name already ending in
/// `.dm_1` (case aside) as it is, any other with `.dm_1` added
/// (`CL_PlayDemo_f`, 0x4691bf..0x469239).
pub fn file_name(name: &str) -> String {
    let ext = format!(".dm_{EXTENSION}");
    let ends = name.len() >= ext.len()
        && name.as_bytes()[name.len() - ext.len()..].eq_ignore_ascii_case(ext.as_bytes());
    if ends {
        name.to_string()
    } else {
        format!("{name}{ext}")
    }
}

/// One `cl.clientArchive` row (48 bytes, `CL_SetUserCmdOrigin`'s copy at
/// 0x472c10): what the cgame predicted at client time `server_time`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Archive {
    pub server_time: i32,
    pub origin: [f32; 3],
    pub velocity: [f32; 3],
    pub bob_cycle: i32,
    pub movement_dir: i32,
    pub view_angles: [f32; 3],
}

/// The archive ring and its write index (`cl` +0x666cc, +0x696cc).
#[derive(Clone, Debug)]
pub struct ArchiveRing {
    pub rows: Vec<Archive>,
    pub index: usize,
}

impl Default for ArchiveRing {
    fn default() -> Self {
        ArchiveRing {
            rows: vec![Archive::default(); ARCHIVE_ROWS],
            index: 0,
        }
    }
}

impl ArchiveRing {
    /// `CL_WritePacket`'s add (0x46423e): a row unless the last one
    /// written holds the same time.
    pub fn add(&mut self, a: Archive) {
        let last = (self.index + ARCHIVE_ROWS - 1) % ARCHIVE_ROWS;
        if self.rows[last].server_time == a.server_time {
            return;
        }
        self.rows[self.index] = a;
        self.index = (self.index + 1) % ARCHIVE_ROWS;
    }
}

/// What `CL_Record_f` (0x468960) writes as the first record's message:
/// `clc.reliableSequence` where a server message has its acknowledge,
/// then, compressed, `svc_gamestate`, `serverCommandSequence`, one
/// `svc_configstring` op carrying every slot the client holds a string in
/// (a default included), each behind an explicit 12-bit index, every
/// baseline but entity 0's whole from nothing, `svc_EOF`, `clientNum`,
/// `checksumFeed` and the `svc_EOF` that ends a message.
pub fn gamestate_message(
    huff: &Huffman,
    reliable_sequence: i32,
    server_command_sequence: i32,
    gs: &Gamestate,
) -> Vec<u8> {
    let mut w = MsgWriter::new(&UNUSED_HUFF);
    w.write_byte(SVC_GAMESTATE);
    w.write_long(server_command_sequence);
    let held: Vec<(u16, &[u8])> = (0..MAX_CONFIGSTRINGS as u16)
        .filter_map(|i| match gs.configstrings.get(&i) {
            Some(s) => Some((i, s.as_slice())),
            None => constant_cs::default_of(i).map(|d| (i, d.as_bytes())),
        })
        .collect();
    w.write_byte(SVC_CONFIGSTRING);
    w.write_short(held.len() as i16);
    for (i, s) in &held {
        w.write_bits(0, 1);
        w.write_bits(*i as i32, 12);
        w.write_string_bytes(s);
    }
    let ctx = Ctx {
        time: BASELINE_TIME,
        map_center: map_center(gs.configstring(CS_MAPCENTER)),
    };
    let mut last = -1;
    for (&n, e) in gs.baselines.iter().filter(|&(&n, _)| n != 0) {
        w.write_byte(SVC_BASELINE);
        delta::write_entity_index(&mut w, &mut last, n, GENTITYNUM_BITS);
        let from = EntityState::default();
        let wire = entity_wire(&from, e, true).unwrap_or(delta::Wire::Unchanged);
        delta::write_delta_entity(&mut w, &ctx, &from, e, &wire);
    }
    w.write_byte(SVC_EOF);
    w.write_long(gs.client_num);
    w.write_long(gs.checksum_feed);
    w.write_byte(SVC_EOF);
    let mut out = reliable_sequence.to_le_bytes().to_vec();
    out.extend(huff.compress(&w.into_ops()));
    out
}

/// One record read back.
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// A server message: its sequence and bytes (acknowledge, then block).
    Message(u32, Vec<u8>),
    /// An archive row and its index.
    Archive(usize, Archive),
}

/// Why a demo stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DemoEnd {
    /// The -1 trailer, or the file ended between records.
    Completed,
    /// `Demo file truncated.`: a record's bytes ran past the file.
    Truncated,
    /// `Demo file corrupt.`: an archive index past 255 (0x468ec6).
    Corrupt,
    /// `CL_ReadDemoMessage: demoMsglen > MAX_MSGLEN`, fatal on retail.
    TooLong(usize),
}

/// `CL_ReadDemoMessage` (0x4690c0) over a whole file.
pub struct DemoReader {
    data: Vec<u8>,
    pos: usize,
}

impl DemoReader {
    pub fn new(data: Vec<u8>) -> Self {
        DemoReader { data, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let b = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(b)
    }

    fn long(&mut self) -> Option<i32> {
        self.take(4)
            .map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }

    fn f3(&mut self) -> Option<[f32; 3]> {
        let mut v = [0.0; 3];
        for x in &mut v {
            *x = f32::from_bits(self.long()? as u32);
        }
        Some(v)
    }

    /// The next record, or why there is none.
    pub fn next_record(&mut self) -> Result<Record, DemoEnd> {
        let kind = self.take(1).ok_or(DemoEnd::Completed)?[0];
        match kind {
            REC_ARCHIVE => {
                let index = self.long().ok_or(DemoEnd::Completed)? as u32 as usize;
                if index >= ARCHIVE_ROWS {
                    return Err(DemoEnd::Corrupt);
                }
                // Field order as written (0x468ea0): origin, velocity, the
                // +0x20 word, the +0x1c word, the time, the view.
                let read = |r: &mut Self| -> Option<Archive> {
                    let origin = r.f3()?;
                    let velocity = r.f3()?;
                    let movement_dir = r.long()?;
                    let bob_cycle = r.long()?;
                    let server_time = r.long()?;
                    let view_angles = r.f3()?;
                    Some(Archive {
                        server_time,
                        origin,
                        velocity,
                        bob_cycle,
                        movement_dir,
                        view_angles,
                    })
                };
                let a = read(self).ok_or(DemoEnd::Completed)?;
                Ok(Record::Archive(index, a))
            }
            REC_MESSAGE => {
                let seq = self.long().ok_or(DemoEnd::Completed)?;
                let len = match self.long() {
                    None | Some(-1) => return Err(DemoEnd::Completed),
                    Some(n) => n as u32 as usize,
                };
                if len > MAX_MSGLEN {
                    return Err(DemoEnd::TooLong(len));
                }
                let Some(msg) = self.take(len).map(<[u8]>::to_vec) else {
                    self.pos = self.data.len();
                    return Err(DemoEnd::Truncated);
                };
                Ok(Record::Message(seq as u32, msg))
            }
            // Any other byte reads nothing and stops nothing (the switch at
            // 0x4690f0 falls out); retail reads the next byte on the next
            // call.
            _ => self.next_record(),
        }
    }
}

/// `record`'s state on the client (`clc.demorecording` +0x40190,
/// `demowaiting` +0x4019c, `demoName` +0x40150, the file +0x401a4, the
/// last archive row written +0x4014c).
pub struct Recording {
    out: Box<dyn Write + Send>,
    written: u64,
    pub name: String,
    /// No full snapshot since the record began: messages are not written
    /// and the client sends `clc_moveNoDelta`.
    pub waiting: bool,
    archived: usize,
}

impl Recording {
    /// Opens on [`gamestate_message`] written under `sequence`
    /// (`clc.serverMessageSequence`), then every archive row, as
    /// `CL_Record_f` ends (0x468db3).
    pub fn start(
        out: Box<dyn Write + Send>,
        name: &str,
        sequence: i32,
        gamestate: &[u8],
        archive: &ArchiveRing,
    ) -> io::Result<Self> {
        let mut r = Recording {
            out,
            written: 0,
            name: name.to_string(),
            waiting: true,
            archived: archive.index,
        };
        r.message(sequence, gamestate)?;
        for i in 0..ARCHIVE_ROWS {
            r.archive_row(i, &archive.rows[i])?;
        }
        Ok(r)
    }

    fn put(&mut self, b: &[u8]) -> io::Result<()> {
        self.out.write_all(b)?;
        self.written += b.len() as u64;
        Ok(())
    }

    fn message(&mut self, sequence: i32, msg: &[u8]) -> io::Result<()> {
        self.put(&[REC_MESSAGE])?;
        self.put(&sequence.to_le_bytes())?;
        self.put(&(msg.len() as i32).to_le_bytes())?;
        self.put(msg)
    }

    /// `CL_WriteDemoClientArchive` (0x468750), the names INFERRED.
    fn archive_row(&mut self, index: usize, a: &Archive) -> io::Result<()> {
        let mut b = vec![REC_ARCHIVE];
        b.extend((index as i32).to_le_bytes());
        let f3 = |b: &mut Vec<u8>, v: &[f32; 3]| v.iter().for_each(|x| b.extend(x.to_le_bytes()));
        f3(&mut b, &a.origin);
        f3(&mut b, &a.velocity);
        b.extend(a.movement_dir.to_le_bytes());
        b.extend(a.bob_cycle.to_le_bytes());
        b.extend(a.server_time.to_le_bytes());
        f3(&mut b, &a.view_angles);
        self.put(&b)
    }

    /// One parsed server message, as `CL_PacketEvent` writes it once a full
    /// frame came (0x46c565): the archive rows added since the last, then
    /// the message. Nothing while [`Self::waiting`].
    pub fn write(&mut self, sequence: i32, msg: &[u8], archive: &ArchiveRing) -> io::Result<()> {
        if self.waiting {
            return Ok(());
        }
        while self.archived != archive.index {
            self.archive_row(self.archived, &archive.rows[self.archived])?;
            self.archived = (self.archived + 1) % ARCHIVE_ROWS;
        }
        self.message(sequence, msg)
    }

    /// Bytes written, for the `RECORDING` line.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// `CL_StopRecord_f`'s trailer (0x4688c0): a type 0 record of -1 and
    /// -1. Returns the bytes written.
    pub fn finish(mut self) -> io::Result<u64> {
        self.put(&[REC_MESSAGE])?;
        self.put(&(-1i32).to_le_bytes())?;
        self.put(&(-1i32).to_le_bytes())?;
        self.out.flush()?;
        Ok(self.written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Write for Shared {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_name_takes_the_extension_once() {
        assert_eq!(file_name("demo0003"), "demo0003.dm_1");
        assert_eq!(file_name("a.DM_1"), "a.DM_1");
        assert_eq!(file_name("a.dm_2"), "a.dm_2.dm_1");
    }

    #[test]
    fn records_round_trip_through_the_reader() {
        let out = Shared::default();
        let mut ring = ArchiveRing::default();
        let row = Archive {
            server_time: 1234,
            origin: [1.0, -2.0, 3.5],
            velocity: [0.0, 190.0, 0.0],
            bob_cycle: 7,
            movement_dir: -12,
            view_angles: [10.0, 90.0, 0.0],
        };
        ring.add(row);
        let mut r =
            Recording::start(Box::new(out.clone()), "t", 5, &[9, 9, 9, 9, 1], &ring).unwrap();
        let opening = ring.rows.clone();
        assert!(r.waiting);
        r.write(6, &[1, 2, 3, 4], &ring).unwrap();
        r.waiting = false;
        ring.add(Archive {
            server_time: 1250,
            ..row
        });
        // Same time: no new row.
        ring.add(Archive {
            server_time: 1250,
            ..row
        });
        r.write(7, &[5, 6, 7, 8, 9], &ring).unwrap();
        let n = r.finish().unwrap();
        let bytes = out.0.lock().unwrap().clone();
        assert_eq!(bytes.len() as u64, n);
        // The gamestate record: 1 + 4 + 4 + 5.
        assert_eq!(&bytes[..9], &[0, 5, 0, 0, 0, 5, 0, 0, 0]);
        assert_eq!(
            &bytes[bytes.len() - 9..],
            &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
        );
        let mut rd = DemoReader::new(bytes);
        assert_eq!(
            rd.next_record(),
            Ok(Record::Message(5, vec![9, 9, 9, 9, 1]))
        );
        for (i, row) in opening.iter().enumerate() {
            let Ok(Record::Archive(j, a)) = rd.next_record() else {
                panic!("row {i}");
            };
            assert_eq!((j, a), (i, *row));
        }
        assert_eq!(
            rd.next_record(),
            Ok(Record::Archive(
                1,
                Archive {
                    server_time: 1250,
                    ..row
                }
            ))
        );
        assert_eq!(
            rd.next_record(),
            Ok(Record::Message(7, vec![5, 6, 7, 8, 9]))
        );
        assert_eq!(rd.next_record(), Err(DemoEnd::Completed));
    }

    #[test]
    fn a_short_record_is_truncated_and_a_bad_row_corrupt() {
        let mut b = vec![0];
        b.extend(3i32.to_le_bytes());
        b.extend(10i32.to_le_bytes());
        b.extend([0; 6]);
        assert_eq!(DemoReader::new(b).next_record(), Err(DemoEnd::Truncated));
        let mut b = vec![1];
        b.extend(256i32.to_le_bytes());
        assert_eq!(DemoReader::new(b).next_record(), Err(DemoEnd::Corrupt));
        let mut b = vec![0];
        b.extend(3i32.to_le_bytes());
        b.extend((MAX_MSGLEN as i32 + 1).to_le_bytes());
        assert_eq!(
            DemoReader::new(b).next_record(),
            Err(DemoEnd::TooLong(MAX_MSGLEN + 1))
        );
    }
}
