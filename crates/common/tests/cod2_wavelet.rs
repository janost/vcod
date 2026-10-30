//! CoD2's wavelet-compressed `.iwi` images, decoded through the tables in
//! the install's render DLL (docs/research/cod2-iwi-iwd.md section 4).
//! Needs `COD2_DIR`. `WAVELET_DUMP=<dir>` writes each image's largest level
//! as `<name>-<w>x<h>.rgba`.

use vcod_common::iwi::{self, Iwi, IwiFormat, IwiHeader};
use vcod_common::testing::cod2_fs;

/// All 25 stock wavelet images: every level the header implies, each a
/// picture rather than a flat fill, alpha opaque where the format has none.
#[test]
fn cod2_wavelet_images_decode() {
    let Some(fs) = cod2_fs() else { return };
    let dir = std::path::PathBuf::from(std::env::var_os("COD2_DIR").unwrap());
    assert!(iwi::WaveletTables::from_install(&dir).is_some());
    let dump = std::env::var_os("WAVELET_DUMP").map(std::path::PathBuf::from);
    let mut n = 0;
    for name in fs.names_with_suffix(".iwi") {
        let data = fs.read(&name).unwrap();
        let header = IwiHeader::parse(&data).unwrap();
        let IwiFormat::Wavelet(bitmap) = header.format else {
            continue;
        };
        let iwi = Iwi::parse(&data).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(iwi.levels.len() as u32, header.level_count(), "{name}");
        for (i, level) in iwi.levels.iter().enumerate() {
            let (w, h) = (u32::from(header.width) >> i, u32::from(header.height) >> i);
            assert_eq!((level.width, level.height), (w.max(1), h.max(1)), "{name}");
            assert_eq!(
                level.faces[0].len(),
                (level.width * level.height * 4) as usize
            );
        }
        let px = &iwi.levels[0].faces[0];
        let (mut lo, mut hi, mut sum) = ([255u8; 4], [0u8; 4], [0u64; 4]);
        for p in px.as_chunks::<4>().0 {
            for c in 0..4 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
                sum[c] += u64::from(p[c]);
            }
        }
        let mean = sum.map(|s| s / (px.len() / 4) as u64);
        eprintln!(
            "{name}: format {} min {lo:?} max {hi:?} mean {mean:?}",
            bitmap + 5
        );
        // stored B G R A; popups_alpha is black, its picture all alpha
        assert!((0..4).any(|c| hi[c] - lo[c] > 64), "{name}: flat");
        if bitmap == 2 {
            assert_eq!(lo[3], 255, "{name}: format 7 alpha");
        }
        if let Some(dir) = &dump {
            let base = name.rsplit('/').next().unwrap().trim_end_matches(".iwi");
            let img = iwi.to_image();
            let vcod_common::assets::ImageData::Rgba8(rgba) = img.data else {
                unreachable!()
            };
            let file = format!("{base}-{}x{}.rgba", img.width, img.height);
            std::fs::write(dir.join(file), rgba).unwrap();
        }
        n += 1;
    }
    assert_eq!(n, 25);
}
