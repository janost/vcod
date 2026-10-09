//! Pak checksums, the numbers `sv_paks` and `sv_referencedPaks` carry: an MD4
//! over the CRC-32s of a pk3's non-empty entries, folded to one word
//! (docs/research/cod11-server-handshake.md, "Pak checksums").

use anyhow::{Context, Result, bail};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// One pak's two checksums. `checksum` is what the pak lists carry;
/// `pure` is the same MD4 with the map load's `checksumFeed` prepended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PakChecksums {
    pub checksum: i32,
    pub pure: i32,
}

/// The central directory's CRC-32 of every entry whose uncompressed size is
/// not 0, in directory order: what `FS_LoadZipFile` feeds the checksum.
pub fn entry_crcs(mut f: impl Read + Seek) -> Result<Vec<u32>> {
    // End of central directory: 22 bytes plus a comment of at most 64 KiB.
    let len = f.seek(SeekFrom::End(0))?;
    let tail = len.min(22 + 0xffff);
    f.seek(SeekFrom::Start(len - tail))?;
    let mut buf = vec![0u8; tail as usize];
    f.read_exact(&mut buf)?;
    let eocd = (0..buf.len().saturating_sub(21))
        .rev()
        .find(|&i| buf[i..i + 4] == *b"PK\x05\x06")
        .context("no end of central directory record")?;
    let u16_at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as usize;
    let u32_at = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let count = u16_at(&buf, eocd + 10);
    let cd_size = u32_at(&buf, eocd + 12) as usize;
    let cd_off = u32_at(&buf, eocd + 16) as u64;
    if cd_off + cd_size as u64 > len {
        bail!("central directory past the end of the file");
    }
    f.seek(SeekFrom::Start(cd_off))?;
    let mut cd = vec![0u8; cd_size];
    f.read_exact(&mut cd)?;
    let mut crcs = Vec::with_capacity(count);
    let mut at = 0;
    for _ in 0..count {
        if at + 46 > cd.len() || cd[at..at + 4] != *b"PK\x01\x02" {
            bail!("bad central directory entry at {at}");
        }
        let crc = u32_at(&cd, at + 16);
        let size = u32_at(&cd, at + 24);
        if size > 0 {
            crcs.push(crc);
        }
        at += 46 + u16_at(&cd, at + 28) + u16_at(&cd, at + 30) + u16_at(&cd, at + 32);
    }
    Ok(crcs)
}

/// Both checksums of the pk3 at `path` for a map load's `checksum_feed`.
pub fn pak_checksums(path: &Path, checksum_feed: i32) -> Result<PakChecksums> {
    let f = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    let crcs = entry_crcs(std::io::BufReader::new(f))
        .with_context(|| format!("cannot read {}", path.display()))?;
    Ok(checksums_of(&crcs, checksum_feed))
}

/// `FS_LoadZipFile`'s two `Com_BlockChecksum`s over the header longs.
pub fn checksums_of(crcs: &[u32], checksum_feed: i32) -> PakChecksums {
    let mut longs = Vec::with_capacity(4 * (crcs.len() + 1));
    longs.extend_from_slice(&checksum_feed.to_le_bytes());
    for c in crcs {
        longs.extend_from_slice(&c.to_le_bytes());
    }
    PakChecksums {
        checksum: block_checksum(&longs[4..]),
        pure: block_checksum(&longs),
    }
}

/// `CL_SendPureChecksums` (CoDMP.exe 0x40fac0): `cp`, the pure checksums of
/// the pak holding `cgame_mp_x86.dll` and of the one holding
/// `ui_mp_x86.dll`, `@`, the pure checksum of every pak a file was read
/// from, then `checksumFeed` XOR all of those XOR their count. Each number
/// is followed by a space. A missing DLL pak contributes nothing.
pub fn pure_command(cgame: Option<i32>, ui: Option<i32>, general: &[i32], feed: i32) -> String {
    let mut s = String::from("cp ");
    for c in [cgame, ui].into_iter().flatten() {
        s.push_str(&format!("{c} "));
    }
    s.push_str("@ ");
    let mut key = feed;
    for &g in general {
        s.push_str(&format!("{g} "));
        key ^= g;
    }
    s.push_str(&format!("{} ", key ^ general.len() as i32));
    s
}

/// `SV_VerifyPaks_f` (cod_lnxded 0x808674c) on a `cp` command's arguments
/// (`args[0]` is `cp`). `cgame` and `ui` are the server's pure checksums of
/// the paks holding the two DLLs, `loaded` every pure checksum `sv_paks`
/// lists. False is retail's `pureAuthentic` 2.
pub fn verify_pure(
    args: &[&str],
    cgame: Option<i32>,
    ui: Option<i32>,
    loaded: &[i32],
    feed: i32,
) -> bool {
    let (Some(cgame), Some(ui)) = (cgame, ui) else {
        return false;
    };
    // `strtol` reads a leading number; anything else reads 0.
    let num = |s: &str| -> i32 {
        let t = s.trim_start();
        let end = t
            .char_indices()
            .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
            .map_or(t.len(), |(i, _)| i);
        t[..end].parse::<i64>().map_or(0, |v| v as i32)
    };
    if args.len() <= 5
        || args[1].starts_with('@')
        || num(args[1]) != cgame
        || args[2].starts_with('@')
        || num(args[2]) != ui
        || !args[3].starts_with('@')
    {
        return false;
    }
    let tail: Vec<i32> = args[4..].iter().map(|a| num(a)).collect();
    let (general, last) = tail.split_at(tail.len() - 1);
    for (i, g) in general.iter().enumerate() {
        if general[..i].contains(g) || !loaded.contains(g) {
            return false;
        }
    }
    let key = general.iter().fold(feed, |k, g| k ^ g);
    (general.len() as i32 ^ key) == last[0]
}

/// `Com_BlockChecksum`: the MD4 digest's four little-endian words XORed.
pub fn block_checksum(data: &[u8]) -> i32 {
    let [a, b, c, d] = md4(data);
    (a ^ b ^ c ^ d) as i32
}

/// RFC 1320 MD4, as four little-endian state words.
fn md4(data: &[u8]) -> [u32; 4] {
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    let mut s = [0x6745_2301u32, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    for block in msg.as_chunks::<64>().0 {
        let x: Vec<u32> = block
            .as_chunks::<4>()
            .0
            .iter()
            .map(|w| u32::from_le_bytes(*w))
            .collect();
        let [mut a, mut b, mut c, mut d] = s;
        let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
        let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
        let h = |x: u32, y: u32, z: u32| x ^ y ^ z;
        for i in 0..16 {
            let r = [3, 7, 11, 19][i % 4];
            let (v, w, y, z) = rot(i, a, b, c, d);
            let n = v.wrapping_add(f(w, y, z)).wrapping_add(x[i]).rotate_left(r);
            set(i, &mut a, &mut b, &mut c, &mut d, n);
        }
        for (i, k) in [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15]
            .into_iter()
            .enumerate()
        {
            let r = [3, 5, 9, 13][i % 4];
            let (v, w, y, z) = rot(i, a, b, c, d);
            let n = v
                .wrapping_add(g(w, y, z))
                .wrapping_add(x[k])
                .wrapping_add(0x5a82_7999)
                .rotate_left(r);
            set(i, &mut a, &mut b, &mut c, &mut d, n);
        }
        for (i, k) in [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]
            .into_iter()
            .enumerate()
        {
            let r = [3, 9, 11, 15][i % 4];
            let (v, w, y, z) = rot(i, a, b, c, d);
            let n = v
                .wrapping_add(h(w, y, z))
                .wrapping_add(x[k])
                .wrapping_add(0x6ed9_eba1)
                .rotate_left(r);
            set(i, &mut a, &mut b, &mut c, &mut d, n);
        }
        s = [
            s[0].wrapping_add(a),
            s[1].wrapping_add(b),
            s[2].wrapping_add(c),
            s[3].wrapping_add(d),
        ];
    }
    s
}

/// Step `i`'s operands: the word it writes, then the three it mixes, cycling
/// a, d, c, b.
fn rot(i: usize, a: u32, b: u32, c: u32, d: u32) -> (u32, u32, u32, u32) {
    match i % 4 {
        0 => (a, b, c, d),
        1 => (d, a, b, c),
        2 => (c, d, a, b),
        _ => (b, c, d, a),
    }
}

fn set(i: usize, a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32, n: u32) {
    match i % 4 {
        0 => *a = n,
        1 => *d = n,
        2 => *c = n,
        _ => *b = n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: [u32; 4]) -> String {
        s.iter()
            .flat_map(|w| w.to_le_bytes())
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    #[test]
    fn md4_matches_rfc_1320() {
        assert_eq!(hex(md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(
            hex(md4(
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890"
            )),
            "e33b4ddc9c38f2199c3e7b164fcc0536"
        );
    }

    #[test]
    fn entry_crcs_skip_empty_entries() {
        use std::io::Write;
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        w.add_directory("maps/", opts).unwrap();
        w.start_file("maps/a.txt", opts).unwrap();
        w.write_all(b"hello").unwrap();
        w.start_file("empty.txt", opts).unwrap();
        w.set_comment("a trailing comment").unwrap();
        let mut buf = w.finish().unwrap();
        buf.set_position(0);
        assert_eq!(entry_crcs(buf).unwrap(), vec![0x3610_a686]);
    }

    #[test]
    fn a_pure_command_verifies_and_a_tampered_one_does_not() {
        let cmd = pure_command(Some(11), Some(22), &[5, -7, 9], 0x1234);
        assert_eq!(
            cmd,
            format!("cp 11 22 @ 5 -7 9 {} ", 0x1234 ^ 5 ^ -7 ^ 9 ^ 3)
        );
        let args: Vec<&str> = cmd.split_whitespace().collect();
        let loaded = [9, 5, -7, 100];
        assert!(verify_pure(&args, Some(11), Some(22), &loaded, 0x1234));
        assert!(
            !verify_pure(&args, Some(11), Some(22), &loaded, 0x1235),
            "feed"
        );
        assert!(
            !verify_pure(&args, Some(11), Some(23), &loaded, 0x1234),
            "ui pak"
        );
        assert!(
            !verify_pure(&args, Some(11), None, &loaded, 0x1234),
            "no ui pak"
        );
        assert!(
            !verify_pure(&args, Some(11), Some(22), &[5, -7], 0x1234),
            "unknown pak"
        );
        let dup = pure_command(Some(11), Some(22), &[5, 5], 0x1234);
        let dup: Vec<&str> = dup.split_whitespace().collect();
        assert!(
            !verify_pure(&dup, Some(11), Some(22), &loaded, 0x1234),
            "duplicate"
        );
        let none = pure_command(Some(11), Some(22), &[], 0x1234);
        let none: Vec<&str> = none.split_whitespace().collect();
        assert!(
            !verify_pure(&none, Some(11), Some(22), &loaded, 0x1234),
            "argc 5"
        );
    }

    /// The stock paks' checksums against retail's `sv_referencedPaks`
    /// (crates/server/tests/fixtures/configstrings/mp_carentan-dm.txt), when
    /// the install has the same paks.
    #[test]
    fn stock_paks_match_retail() {
        let dir = crate::testing::game_dir().join("main");
        if !dir.join("pak5.pk3").is_file() {
            return;
        }
        for (pak, want) in [
            ("pak1.pk3", 1_265_884_747),
            ("pak2.pk3", 616_334_813),
            ("pak3.pk3", 918_160_098),
            ("pak4.pk3", -1_825_805_837),
            ("pak5.pk3", 77_111_478),
        ] {
            let got = pak_checksums(&dir.join(pak), 0).unwrap();
            assert_eq!(got.checksum, want, "{pak}");
        }
    }
}
