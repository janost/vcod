//! Offscreen frames through the HUD pass, for a look: with `COD2_DIR` set,
//! `VCOD_MENU_SHOT=<file.ppm>` draws CoD2's main menu (after the clicks
//! `VCOD_MENU_SHOT_CLICKS` lists, comma separated item texts or names, and
//! `VCOD_MENU_SHOT_WAIT` ms of UI frames, a LAN refresh's) and
//! `VCOD_HUD_SHOT=<file.ppm>` a made-up player's HUD on a headless adapter
//! (lavapipe will do), each written as a PPM. `VCOD_HUD_QUADS=<a.quads>,...`
//! draws each `vcod --net-probe ADDR --game cod2` HUD dump
//! (`VCOD_COD2_HUD_DUMP`) to `<a.quads>.ppm` at 1280 x 720.

use super::*;
use vcod_common::asset_source::AssetSource;

/// A device off a headless adapter with BC textures, `None` where the
/// machine has none.
fn headless() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::default(),
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let info = adapter.get_info();
    eprintln!("adapter: {} {:?}", info.name, info.backend);
    if !adapter
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("hud shot device"),
        required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        ..Default::default()
    }))
    .ok()
}

#[test]
fn cod2_main_menu_shot() {
    let Some(out) = std::env::var_os("VCOD_MENU_SHOT") else {
        return;
    };
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((device, queue)) = headless() else {
        return;
    };
    let (w, h) = (1280u32, 720u32);

    let mut ui = crate::frontend::Ui::new(&fs, "");
    let mut shell = crate::console::shell::Shell::new();
    // A display's modes, as `set_cod2_mode_lists` sets them in the app.
    let modes = ["640x480", "800x600", "1024x768", "1280x720", "1920x1080"];
    shell.execute("seta r_mode 1280x720");
    shell.set_enum_domain("r_mode", modes.map(String::from).to_vec());
    shell.execute("seta r_displayRefresh \"60 Hz\"");
    shell.set_enum_domain("r_displayRefresh", vec!["60 Hz".into(), "75 Hz".into()]);
    let mut effects = Vec::new();
    ui.open_main(&shell, &mut effects);
    for e in effects {
        if let crate::frontend::UiEffect::Command(c) = e {
            shell.execute(&c);
        }
    }
    ui.set_screen(w as f32, h as f32);
    let clicks = std::env::var("VCOD_MENU_SHOT_CLICKS").unwrap_or_default();
    for what in clicks.split(',').filter(|c| !c.is_empty()) {
        crate::frontend::cod2::click_shown(&mut ui, &mut shell, what, (w as f32, h as f32));
    }
    let wait = std::env::var("VCOD_MENU_SHOT_WAIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let until = std::time::Instant::now() + std::time::Duration::from_millis(wait);
    while std::time::Instant::now() < until {
        ui.frame(std::time::Instant::now(), &shell);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let loc = vcod_common::localize::Localized::load(&fs);
    let quads = ui.build(w as f32, h as f32, &loc, &shell);
    shoot(&device, &queue, &fs, &quads, (w, h), &out);
}

#[test]
fn cod2_dumped_hud_shot() {
    let Some(files) = std::env::var_os("VCOD_HUD_QUADS") else {
        return;
    };
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((device, queue)) = headless() else {
        return;
    };
    for file in files.to_string_lossy().split(',').filter(|f| !f.is_empty()) {
        let quads = crate::cod2_hud::load_quads(&std::fs::read_to_string(file).unwrap());
        let out = format!("{file}.ppm");
        shoot(&device, &queue, &fs, &quads, (1280, 720), out.as_ref());
    }
}

/// Draws `quads` on a black `w` x `h` target and writes it to `out` as a
/// PPM.
fn shoot(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fs: &dyn AssetSource,
    quads: &[HudQuad],
    (w, h): (u32, u32),
    out: &std::ffi::OsStr,
) {
    let mut pass = create_hud_pass(device, SCENE_FORMAT);
    let shaders = assets::load_shaders(fs);
    let lib = material_lib(fs);
    let textures = texture_settings::TextureSettings::default();
    let samplers = TexSamplers::new(device, textures.filter);
    let up = HudUpload {
        device,
        queue,
        samplers: &samplers,
        image_map: shaders.image_map(),
        lib: &lib,
        textures: &textures,
        image_gamma: None,
        identity_light: 1.0,
        size: (w as f32, h as f32),
    };
    up.upload(&mut pass, fs, quads, &mut false);

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hud shot"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: SCENE_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hud shot readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hud shot pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.draw(&mut rp, true);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        target.size(),
    );
    queue.submit([encoder.finish()]);
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = readback.slice(..).get_mapped_range().unwrap();
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for y in 0..h {
        let line = &data[(y * row) as usize..][..(w * 4) as usize];
        for px in line.chunks(4) {
            ppm.extend_from_slice(&px[..3]);
        }
    }
    std::fs::write(out, ppm).unwrap();
    eprintln!("{} quads, {} runs", quads.len(), pass.runs.len());
}

/// A crouched allies player with a Thompson, a frag and a smoke grenade, a
/// third of his health, a use trigger in his sight, a teammate ahead and
/// a hit from his right, a second into the hit.
#[test]
fn cod2_player_hud_shot() {
    use vcod_cod2::net::delta::field_index;
    use vcod_cod2::net::delta::{ClientState2, EntityState2, PlayerState2};
    use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
    use vcod_cod2::net::parse::{Gamestate2, Snapshot2};
    use vcod_cod2::net::{ConfigStrings, Frame, SnapshotView};

    let Some(out) = std::env::var_os("VCOD_HUD_SHOT") else {
        return;
    };
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((device, queue)) = headless() else {
        return;
    };
    let (w, h) = (1280u32, 720u32);
    let mut gs = Gamestate2 {
        server_command_sequence: 0,
        configstrings: Default::default(),
        baselines: Default::default(),
        client_num: 0,
        checksum_feed: 0,
    };
    gs.configstrings.insert(
        vcod_cod2::net::protocol::CS_WEAPONS as u16,
        "thompson_mp frag_grenade_american_mp smoke_grenade_american_mp".into(),
    );
    gs.configstrings
        .insert(0x4fe, "Press &&1 to open the door".into());
    let cs = ConfigStrings::from_gamestate(&gs);
    let mut ps = PlayerState2::default();
    let mut set = |n: &str, v: u32| ps.f[field_index(&PLAYER_STATE_FIELDS, n)] = v;
    set("pm_flags", 2);
    set("weapon", 1);
    set("weapons[0]", 0b1110);
    set("viewHeightCurrent", 40f32.to_bits());
    set("cursorHint", 2);
    set("cursorHintString", 0);
    set("damageCount", 30);
    set("damageYaw", 64);
    set("aimSpreadScale", 128f32.to_bits());
    // The clip slots: 1 the Thompson's, 2 and 3 the grenades'.
    ps.ammoclip[1] = 5;
    ps.ammo[1] = 120;
    ps.ammoclip[2] = 1;
    ps.ammoclip[3] = 1;
    ps.stats[0] = 30;
    ps.stats[2] = 100;
    let mut mate = EntityState2::null(1);
    mate.f[field_index(&ENTITY_STATE_FIELDS, "eType")] = 1;
    mate.f[field_index(&ENTITY_STATE_FIELDS, "pos.trBase[0]")] = 600f32.to_bits();
    mate.f[field_index(&ENTITY_STATE_FIELDS, "pos.trBase[1]")] = 300f32.to_bits();
    let team = field_index(&CLIENT_STATE_FIELDS, "team");
    let clients = [0, 1].map(|n| {
        let mut c = ClientState2::null(n);
        c.f[team] = 2;
        c
    });
    let snap_at = |ps: &PlayerState2, t: i32| {
        SnapshotView::new(
            &Snapshot2 {
                message_num: 1,
                server_time: t,
                delta_num: -1,
                snap_flags: 0,
                valid: true,
                ps_lc: 0,
                ps: ps.clone(),
                entities: vec![mate],
                clients: clients.to_vec(),
            },
            0,
        )
    };
    let fonts = crate::hud::font::UiFonts::load(&fs).unwrap();
    let loc = vcod_common::localize::Localized::load(&fs);
    let mut hud = crate::cod2_player_hud::PlayerHud::default();
    let input = crate::cod2_player_hud::HudInput {
        selected: 1,
        use_key: "F",
        mantle_hint: true,
        stance_keys: None,
        hint_fade_ms: 100,
    };
    let mut quads = Vec::new();
    for (t, event) in [(1000, 0), (1050, 1), (2050, 1)] {
        let mut p = ps.clone();
        p.f[field_index(&PLAYER_STATE_FIELDS, "damageEvent")] = event;
        let snap = snap_at(&p, t);
        let frame = Frame {
            snap: &snap,
            prev: None,
            time_ms: t,
            frac: 1.0,
            configstrings: &cs,
        };
        quads.clear();
        hud.build(
            &frame,
            &fs,
            &loc,
            &fonts,
            (w as f32, h as f32),
            t,
            &input,
            &mut quads,
        );
    }
    shoot(&device, &queue, &fs, &quads, (w, h), &out);
}

/// `VCOD_UI_SHOT=<prefix>` writes `<prefix>-board.ppm` (a tdm scoreboard
/// held: two allies, two axis, a spectator, the viewer's row lit) and
/// `<prefix>-chat.ppm` (chat lines, the open say field, two kill lines and
/// the viewer's death print) through `Cod2Hud`.
#[test]
fn cod2_ui_shot() {
    use vcod_cod2::net::delta::field_index;
    use vcod_cod2::net::delta::{ClientState2, EntityState2, PlayerState2};
    use vcod_cod2::net::fields::{CLIENT_STATE_FIELDS, ENTITY_STATE_FIELDS, PLAYER_STATE_FIELDS};
    use vcod_cod2::net::parse::{Gamestate2, Snapshot2};
    use vcod_cod2::net::{ConfigStrings, Frame, SnapshotView};

    let Some(prefix) = std::env::var_os("VCOD_UI_SHOT") else {
        return;
    };
    let Some(fs) = vcod_common::testing::cod2_fs() else {
        return;
    };
    let Some((device, queue)) = headless() else {
        return;
    };
    let (w, h) = (1280u32, 720u32);
    let mut gs = Gamestate2 {
        server_command_sequence: 0,
        configstrings: Default::default(),
        baselines: Default::default(),
        client_num: 0,
        checksum_feed: 0,
    };
    let mut cs = |n: usize, s: &str| {
        gs.configstrings.insert(n as u16, s.into());
    };
    cs(
        0,
        "\\g_gametype\\tdm\\mapname\\mp_toujane\\sv_hostname\\vcod test server",
    );
    cs(
        vcod_cod2::net::protocol::CS_WEAPONS,
        "kar98k_mp thompson_mp",
    );
    cs(23, "hud_status_dead");
    // The server dvar mirror as retail's mp_toujane tdm sent it.
    for (i, (name, value)) in [
        ("g_ScoresBanner_Allies", "mpflag_british"),
        ("g_ScoresBanner_Axis", "mpflag_german"),
        ("g_ScoresBanner_None", "mpflag_none"),
        ("g_ScoresBanner_Spectators", "mpflag_spectator"),
        ("g_TeamColor_Allies", "0.25098 0.25098 0.74902 0"),
        ("g_TeamColor_Axis", "0.6 0.6 0.6 0"),
        ("g_TeamName_Allies", "MPUI_BRITISH"),
        ("g_TeamName_Axis", "MPUI_GERMAN"),
    ]
    .into_iter()
    .enumerate()
    {
        cs(142 + i, name);
        cs(238 + i, value);
    }
    let cs = ConfigStrings::from_gamestate(&gs);
    let mut ps = PlayerState2::default();
    ps.f[field_index(&PLAYER_STATE_FIELDS, "pm_type")] = 6;
    let team = field_index(&CLIENT_STATE_FIELDS, "team");
    let client = |n: u32, name: &str, t: u32| {
        let mut c = ClientState2::null(n);
        c.f[team] = t;
        let mut bytes = name.as_bytes().to_vec();
        bytes.resize(32, 0);
        for k in 0..8 {
            let i = field_index(&CLIENT_STATE_FIELDS, &format!("name[{}]", k * 4));
            c.f[i] = u32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().unwrap());
        }
        c
    };
    let clients = vec![
        client(0, "vcodplay", 2),
        client(1, "Sgt.Miller", 2),
        client(2, "^1Hans", 1),
        client(3, "Fritz", 1),
        client(4, "watcher", 3),
    ];
    // Two obituary temp entities: Hans kills the viewer with a Kar98k,
    // Miller heads Fritz.
    let obit = |n: u32, attacker: i32| {
        let mut e = EntityState2::null(n);
        e.f[field_index(&ENTITY_STATE_FIELDS, "attackerEntityNum")] = attacker as u32;
        e
    };
    let snap = SnapshotView::new(
        &Snapshot2 {
            message_num: 1,
            server_time: 1000,
            delta_num: -1,
            snap_flags: 0,
            valid: true,
            ps_lc: 0,
            ps,
            entities: vec![obit(100, 2), obit(101, 1)],
            clients,
        },
        0,
    );
    let frame = Frame {
        snap: &snap,
        prev: None,
        time_ms: 1000,
        frac: 1.0,
        configstrings: &cs,
    };
    let loc = vcod_common::localize::Localized::load(&fs);
    let mut hud = crate::cod2_hud::Cod2Hud::default();
    let ev = |entity: u32, victim: i32, parm: i32| crate::play::cod2_events::Event2 {
        entity,
        event: vcod_cod2::net::events::EV_OBITUARY,
        parm,
        etype: 0,
        eflags: 0,
        weapon: 0,
        surf: 0,
        other_entity: victim,
        time: 0,
        scale: 0,
        pos: glam::Vec3::ZERO,
        angles: glam::Vec3::ZERO,
        angles2: [0.0; 2],
        from_ps: false,
    };
    hud.on_events(
        &[ev(100, 0, 1), ev(101, 3, 0x80 | 8)],
        &frame,
        &fs,
        &loc,
        800,
    );
    for (t, line) in [
        (700, "^1Hans^7: ^7nice shot"),
        (750, "Sgt.Miller^7: ^7cover me, going in"),
        (900, "^5(Team) ^7vcodplay^7: ^7on my way"),
    ] {
        hud.chat.push(line, t);
    }
    hud.chat_field = Some(crate::hud::ChatField {
        team: false,
        text: "gg".into(),
    });
    let path = |tag: &str| {
        let mut p = prefix.clone();
        p.push(format!("-{tag}.ppm"));
        p
    };
    let quads = hud.build(&frame, &fs, &loc, (w as f32, h as f32), 1000, 1.0);
    shoot(&device, &queue, &fs, &quads, (w, h), &path("chat"));
    hud.chat_field = None;
    hud.scoreboard.press(0.0);
    let reply = "b 5 3 5 1 3 55 1 0 0 2 40 2 0 2 4 80 3 1 3 2 120 4 0 4 0 30 0 0";
    hud.scoreboard
        .on_server_command(&reply.split(' ').map(str::to_string).collect::<Vec<_>>());
    let quads = hud.build(&frame, &fs, &loc, (w as f32, h as f32), 1000, 1.0);
    shoot(&device, &queue, &fs, &quads, (w, h), &path("board"));
}
