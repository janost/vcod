//! Prints the menus of the named zones (`COD4_DIR`), item by item: name,
//! type, rect and alignment, owner draw, text, font, scale, style and
//! background, then its expressions, scripts and key handlers, the shape
//! cod4-hud.md reads them in.
//! `cargo run -p vcod-cod4 --example menu_dump -- common_mp [menu...]`

use vcod_cod4::walk::{WalkOptions, walk};
use vcod_cod4::zone::Zone;

fn main() -> anyhow::Result<()> {
    let install = vcod_cod4::testing::cod4_install().expect("COD4_DIR");
    let mut args = std::env::args().skip(1);
    let zone = args.next().unwrap_or_else(|| "common_mp".into());
    let only: Vec<String> = args.collect();
    let z = Zone::open_path(&install.zone("english", &zone))?;
    let a = walk(z, WalkOptions { keep_bulk: false })?.assets;
    let mat = |m: Option<vcod_cod4::assets::Id<vcod_cod4::assets::material::Material>>| {
        m.map(|m| a.materials[m.index()].name.clone())
    };
    for m in &a.menus {
        let name = m.window.name.clone().unwrap_or_default();
        if !only.is_empty() && !only.iter().any(|o| o.eq_ignore_ascii_case(&name)) {
            println!("menu {name}");
            continue;
        }
        let r = &m.raw;
        let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        let i = |o: usize| i32::from_le_bytes(r[o..o + 4].try_into().unwrap());
        println!(
            "menu {name} rect {} {} {} {} align {} {} items {}",
            f(4),
            f(8),
            f(12),
            f(16),
            i(20),
            i(24),
            m.items.len()
        );
        for (k, v) in [
            ("onOpen", &m.on_open),
            ("onClose", &m.on_close),
            ("onEsc", &m.on_esc),
            ("soundName", &m.sound_name),
        ] {
            if let Some(v) = v {
                println!("  {k} {v:?}");
            }
        }
        for (k, a) in &m.on_key {
            println!("  key {k} {a:?}");
        }
        if !m.visible_exp.is_empty() {
            println!("  visible {:?}", m.visible_exp);
        }
        for it in m.items.iter().flatten() {
            let r = &it.raw;
            let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            let i = |o: usize| i32::from_le_bytes(r[o..o + 4].try_into().unwrap());
            println!(
                "  {:?} type {} rect {} {} {} {} align {} {} od {} odflags {:#x} style {} text {:?} font {} talign {} tx {} ty {} scale {} tstyle {} fore {:.2} {:.2} {:.2} {:.2} bg {:?} dvar {:?} flags {:#x}",
                it.window.name.as_deref().unwrap_or(""),
                it.kind,
                f(4),
                f(8),
                f(12),
                f(16),
                i(20),
                i(24),
                i(64),
                i(68),
                i(56),
                it.text,
                i(192),
                i(196),
                f(200),
                f(204),
                f(208),
                i(212),
                f(88),
                f(92),
                f(96),
                f(100),
                mat(it.window.background),
                it.dvar,
                i(80),
            );
            let e = &it.exps;
            for (k, st) in [
                ("visible", &e.visible),
                ("text", &e.text),
                ("material", &e.material),
                ("rectX", &e.rect_x),
                ("rectY", &e.rect_y),
                ("rectW", &e.rect_w),
                ("rectH", &e.rect_h),
                ("foreA", &e.forecolor_a),
            ] {
                if !st.is_empty() {
                    println!("    exp {k} {st:?}");
                }
            }
            let sc = &it.scripts;
            for (k, v) in [
                ("action", &it.action),
                ("mouseEnter", &sc.mouse_enter),
                ("mouseExit", &sc.mouse_exit),
                ("onFocus", &sc.on_focus),
                ("leaveFocus", &sc.leave_focus),
                ("onAccept", &sc.on_accept),
                ("dvarTest", &it.dvar_test),
                ("enableDvar", &it.enable_dvar),
            ] {
                if let Some(v) = v {
                    println!("    {k} {v:?}");
                }
            }
            for (k, a) in &it.on_key {
                println!("    key {k} {a:?}");
            }
            if it.type_data != vcod_cod4::assets::ui::TypeData::None {
                println!("    data {:?}", it.type_data);
            }
            if f(292) != 0.0 {
                println!("    feeder {}", f(292));
            }
        }
    }
    Ok(())
}
