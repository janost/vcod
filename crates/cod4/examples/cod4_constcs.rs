//! Dumps CoD4's `constantConfigStrings` table from a binary: 16-byte rows
//! `{int configStringNum; char *configString; int hash; int lowercaseHash}`,
//! ascending by slot, ended by a row of slot 0. The table is found as the
//! row whose string pointer names `bg_aimSpreadMoveSpeedThreshold`, its
//! first entry (docs/research/cod4-handshake.md, "Configstring defaults").
//!
//!     cargo run -p vcod-cod4 --example cod4_constcs -- <cod4_lnxded> [<iw3mp.exe>] [--rust]
//!
//! With two binaries it diffs them; `--rust` prints the first one's table
//! as `src/net/constant_cs.rs`'s `CONSTANT_CONFIGSTRINGS`.

use anyhow::{Context, Result, bail};

/// A mapped section: virtual address, file offset, size.
struct Image {
    data: Vec<u8>,
    sections: Vec<(u32, usize, usize)>,
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(d[o..o + 2].try_into().unwrap())
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

impl Image {
    /// An i386 ELF's `PROGBITS` sections, or a PE's sections at its image
    /// base.
    fn load(path: &str) -> Result<Image> {
        let d = std::fs::read(path).with_context(|| path.to_string())?;
        let mut sections = Vec::new();
        if d.starts_with(b"\x7fELF") {
            let shoff = u32_at(&d, 0x20) as usize;
            let (es, n) = (u16_at(&d, 0x2e) as usize, u16_at(&d, 0x30) as usize);
            for i in 0..n {
                let h = shoff + i * es;
                let (typ, addr) = (u32_at(&d, h + 4), u32_at(&d, h + 12));
                if typ == 1 && addr != 0 {
                    sections.push((
                        addr,
                        u32_at(&d, h + 16) as usize,
                        u32_at(&d, h + 20) as usize,
                    ));
                }
            }
        } else if d.starts_with(b"MZ") {
            let pe = u32_at(&d, 0x3c) as usize;
            let n = u16_at(&d, pe + 6) as usize;
            let opt = u16_at(&d, pe + 20) as usize;
            let base = u32_at(&d, pe + 24 + 28);
            for i in 0..n {
                let s = pe + 24 + opt + 40 * i;
                let (vsize, va) = (u32_at(&d, s + 8), u32_at(&d, s + 12));
                let (rsize, raw) = (u32_at(&d, s + 16), u32_at(&d, s + 20));
                sections.push((base + va, raw as usize, vsize.min(rsize) as usize));
            }
        } else {
            bail!("{path}: neither ELF nor PE");
        }
        Ok(Image { data: d, sections })
    }

    fn offset(&self, va: u32) -> Option<usize> {
        self.sections
            .iter()
            .find(|&&(a, _, s)| va >= a && ((va - a) as usize) < s)
            .map(|&(a, o, _)| o + (va - a) as usize)
    }

    fn va(&self, off: usize) -> Option<u32> {
        self.sections
            .iter()
            .find(|&&(_, o, s)| off >= o && off - o < s)
            .map(|&(a, o, _)| a + (off - o) as u32)
    }

    fn cstr(&self, va: u32) -> Option<String> {
        let o = self.offset(va)?;
        let end = o + self.data[o..].iter().position(|&c| c == 0)?;
        Some(String::from_utf8_lossy(&self.data[o..end]).into_owned())
    }

    /// The table's address and rows.
    fn constant_configstrings(&self) -> Result<(u32, Vec<(u32, String)>)> {
        let first = b"\0bg_aimSpreadMoveSpeedThreshold\0";
        let at = self
            .data
            .windows(first.len())
            .position(|w| w == first)
            .context("first entry's string")?
            + 1;
        let sva = self.va(at).context("string outside a section")?;
        let row = (0..self.data.len() - 16)
            .step_by(4)
            .find(|&o| {
                u32_at(&self.data, o) == 20
                    && u32_at(&self.data, o + 4) == sva
                    && u32_at(&self.data, o + 8) == 0
            })
            .context("no row points at the first entry")?;
        let mut rows = Vec::new();
        let mut o = row;
        loop {
            let num = u32_at(&self.data, o);
            if num == 0 {
                break;
            }
            let p = u32_at(&self.data, o + 4);
            let s = self
                .cstr(p)
                .with_context(|| format!("row {num}: string {p:#x}"))?;
            rows.push((num, s));
            o += 16;
        }
        Ok((self.va(row).unwrap(), rows))
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rust = args.iter().any(|a| a == "--rust");
    let paths: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if paths.is_empty() {
        bail!("usage: cod4_constcs <binary> [<binary>] [--rust]");
    }
    let mut tables = Vec::new();
    for p in &paths {
        let (va, rows) = Image::load(p)?.constant_configstrings()?;
        eprintln!(
            "{p}: table at {va:#x}, {} rows, slots {}..{}",
            rows.len(),
            rows[0].0,
            rows.last().unwrap().0
        );
        tables.push(rows);
    }
    if let [a, b] = &tables[..] {
        if a == b {
            eprintln!("the two tables are equal, row for row");
        } else {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if x != y {
                    eprintln!("row {i}: {x:?} vs {y:?}");
                }
            }
            if a.len() != b.len() {
                eprintln!("lengths {} vs {}", a.len(), b.len());
            }
        }
    }
    let rows = &tables[0];
    if rust {
        println!("pub const CONSTANT_CONFIGSTRINGS: &[(u16, &str)] = &[");
        for (n, s) in rows {
            println!("    ({n}, {s:?}),");
        }
        println!("];");
    } else {
        for (n, s) in rows {
            println!("{n} {s}");
        }
    }
    Ok(())
}
