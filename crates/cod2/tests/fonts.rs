//! Every stock CoD2 font parses and names a material the install holds.

use vcod_cod2::font;

#[test]
fn stock_fonts_parse() {
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    for size in [12, 16, 18, 24, 30, 32] {
        let path = font::name_for_size(size).unwrap();
        let bytes = fs.read(path).unwrap_or_else(|| panic!("{path} missing"));
        let f = font::parse(&bytes).unwrap();
        assert!(
            f.name.eq_ignore_ascii_case(path),
            "{path} names itself {}",
            f.name
        );
        assert_eq!(f.material, "fonts/gamefonts", "{path}");
        assert!(
            fs.read(&format!("materials/{}", f.material)).is_some(),
            "{path}"
        );
        let w = f.glyph('W' as u32);
        assert!(w.dx > 0 && w.pixel_height > 0 && w.y0 < 0, "{path}: {w:?}");
        assert!(w.s1 > w.s0 && w.t1 > w.t0, "{path}: {w:?}");
    }
    let normal = font::parse(&fs.read("fonts/normalFont").unwrap()).unwrap();
    assert_eq!(normal.pixel_height, 16);
    assert_eq!(normal.glyphs.len(), 0xfe);
}
