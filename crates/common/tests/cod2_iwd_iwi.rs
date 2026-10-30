//! A CoD2 1.3 install's search path and every stock `.iwi`
//! (docs/research/cod2-iwi-iwd.md). Needs `COD2_DIR`; passes vacuously
//! without it.

use std::collections::BTreeMap;
use vcod_common::assets::ImageData;
use vcod_common::iwi::{Iwi, IwiHeader};
use vcod_common::testing::cod2_fs;

#[test]
fn cod2_search_path_reads_and_overrides() {
    let Some(fs) = cod2_fs() else { return };
    let first: Vec<&str> = fs.paks().map(|p| p.name.as_str()).take(3).collect();
    assert_eq!(first, ["iw_15", "iw_14", "iw_13"]);
    assert!(fs.paks().last().unwrap().localized);

    let bsp = fs.read("maps/mp/mp_toujane.d3dbsp").unwrap();
    assert_eq!(&bsp[..8], b"IBSP\x04\0\0\0");

    // In iw_07 and iw_15; 1.3's copy (iw_15) is the longer one.
    let gsc = fs.read("maps/mp/mp_toujane.gsc").unwrap();
    assert!(
        fs.source_archive("maps/mp/mp_toujane.gsc")
            .unwrap()
            .ends_with("iw_15.iwd")
    );
    assert_eq!(gsc.len(), 3354);
    assert!(String::from_utf8_lossy(&gsc).contains("level.radio"));

    // In iw_06 and iw_15.
    assert!(
        fs.source_archive("ui_mp/menus.txt")
            .unwrap()
            .ends_with("iw_15.iwd")
    );
    assert_eq!(fs.read("ui_mp/menus.txt").unwrap().len(), 1786);
}

/// Every stock image decodes, wavelet ones included, and the header's level
/// ends match where the loader stopped reading each level.
#[test]
fn cod2_iwi_census() {
    let Some(fs) = cod2_fs() else { return };
    let names = fs.names_with_suffix(".iwi");
    let mut per_format: BTreeMap<String, usize> = BTreeMap::new();
    let mut cubes = 0;
    for name in &names {
        let data = fs.read(name).unwrap();
        let header = IwiHeader::parse(&data).unwrap_or_else(|e| panic!("{name}: {e}"));
        *per_format
            .entry(format!("{:?}", header.format))
            .or_default() += 1;
        let iwi = Iwi::parse(&data).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        assert_eq!(iwi.read_ends[0], data.len(), "{name}: trailing bytes");
        for (i, &want) in header.level_ends.iter().enumerate() {
            // A level the file does not hold repeats the smallest one's end.
            let ends = &iwi.read_ends;
            let got = ends.get(i).copied().unwrap_or(*ends.last().unwrap());
            assert_eq!(want as usize, got, "{name}: level {i} end");
        }
        if header.is_cube() {
            cubes += 1;
            assert!(iwi.levels.iter().all(|l| l.faces.len() == 6), "{name}");
        }
        let img = iwi.to_image();
        assert_eq!(
            (img.width, img.height),
            (u32::from(header.width), u32::from(header.height))
        );
        match img.data {
            ImageData::Bc { mips, .. } => assert_eq!(mips.len(), iwi.levels.len()),
            ImageData::Rgba8(px) => {
                assert_eq!(px.len(), img.width as usize * img.height as usize * 4)
            }
        }
    }
    eprintln!("{} iwi, {cubes} cubes; {per_format:?}", names.len());
    assert_eq!(per_format["Wavelet(1)"], 13);
    assert_eq!(per_format["Wavelet(2)"], 12);
    assert!(names.len() > 4000, "{} images", names.len());
}
