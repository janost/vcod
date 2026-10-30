//! CoD2 demo files (docs/research/cod2-demos.md): `demos/<name>.dm_1`, the
//! records of CoD1's format (`vcod_common::net::demo`: the sequence and the
//! length as little-endian longs ahead of each server message past the
//! netchan, two -1 longs to close) with CoD2's ops, a 0x20000-byte
//! `MAX_MSGLEN` and a gamestate of the client's own to open.

use std::collections::BTreeMap;
use std::io::{self, Write};

use vcod_common::net::codec::MsgWriter;
use vcod_common::net::demo::{DemoReader, DemoWriter};
use vcod_common::net::huffman::Huffman;

use super::delta::EntityState2;
use super::ops::Svc;
use super::parse::Gamestate2;
use super::write::write_gamestate;

/// The number after `.dm_`: `CL_Record_f` and `CL_PlayDemo_f` push a
/// literal 1 (Mac 0x149856, 0x14c288), as CoD1 1.1 does.
pub const EXTENSION: u32 = 1;

/// `CL_ReadDemoMessage`'s buffer (Mac 0x14c132): a longer record is fatal.
pub const MAX_MSGLEN: usize = 0x20000;

/// `demos/<name>`'s file for `demo` / `timedemo`: a name already ending in
/// `.dm_1` (case aside) as it is, any other with `.dm_1` added
/// (`CL_PlayDemo_f`, Mac 0x14c288).
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

/// A reader over a whole `.dm_1` file.
pub fn reader(data: Vec<u8>) -> DemoReader {
    DemoReader::with_max(data, MAX_MSGLEN)
}

/// What `CL_Record_f` (Mac 0x149856) writes ahead of any server message:
/// `clc.reliableSequence` where a message has its acknowledge, then,
/// compressed, `svc_gamestate`, `serverCommandSequence`, every non-empty
/// configstring of the live table, every baseline but those numbered 0
/// (the null state's), `svc_EOF`, `clientNum`, `checksumFeed` and the
/// `svc_EOF` that ends a message.
pub fn gamestate_message(
    huff: &Huffman,
    reliable_sequence: i32,
    gs: &Gamestate2,
    configstrings: &[String],
) -> Vec<u8> {
    let now = Gamestate2 {
        configstrings: configstrings
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.is_empty())
            .map(|(i, s)| (i as u16, s.clone()))
            .collect(),
        baselines: gs
            .baselines
            .iter()
            .filter(|&(&n, _)| n != 0)
            .map(|(&n, es)| (n, *es))
            .collect::<BTreeMap<u32, EntityState2>>(),
        ..gs.clone()
    };
    let mut w = MsgWriter::new(huff);
    write_gamestate(&mut w, &now);
    w.write_byte(Svc::Eof as u8);
    let mut out = reliable_sequence.to_le_bytes().to_vec();
    out.extend(w.finish());
    out
}

/// `record`'s state on the client (`clc.demorecording`, `demowaiting`,
/// `demoName`, the file): Mac `clc` +0x40184, +0x40190, +0x40144, +0x40198.
pub struct Recording {
    writer: DemoWriter<Box<dyn Write + Send>>,
    pub name: String,
    /// No full snapshot since the record began: messages are not written
    /// and the client sends `clc_moveNoDelta`.
    pub waiting: bool,
}

impl Recording {
    /// Opens on [`gamestate_message`] written under `sequence`
    /// (`clc.serverMessageSequence`).
    pub fn start(
        out: Box<dyn Write + Send>,
        name: &str,
        sequence: i32,
        gamestate: &[u8],
    ) -> io::Result<Self> {
        let mut writer = DemoWriter::new(out);
        writer.write_message(sequence as u32, gamestate)?;
        Ok(Recording {
            writer,
            name: name.to_string(),
            waiting: true,
        })
    }

    /// `CL_WriteDemoMessage` (Mac 0x1497d4).
    pub fn write(&mut self, sequence: i32, msg: &[u8]) -> io::Result<()> {
        self.writer.write_message(sequence as u32, msg)
    }

    /// Bytes written, for the `RECORDING` line.
    pub fn written(&self) -> u64 {
        self.writer.written()
    }

    /// `CL_StopRecord_f`'s trailer (Mac 0x14724e); the bytes written.
    pub fn finish(self) -> io::Result<u64> {
        let n = self.writer.written() + 8;
        self.writer.finish()?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_takes_the_extension_once() {
        assert_eq!(file_name("demo0003"), "demo0003.dm_1");
        assert_eq!(file_name("a.DM_1"), "a.DM_1");
        assert_eq!(file_name("a.dm_2"), "a.dm_2.dm_1");
    }
}
