//! The in-game menu's controls on vcod's console: the bind rows, the
//! yes/no, multi and slider option controls, the list boxes and their
//! feeders (the call vote's maps and players, the mute list), the
//! `uiScript`s their buttons run, and the mute. The behaviour and its
//! labels are cod4-hud.md 12.6's; most of it is INFERRED, not yet
//! confirmed on 1.7. An option whose dvar vcod has no cvar for draws
//! greyed and takes no key.

use vcod_cod4::assets::ui::{ListBox, MultiDef, TypeData};
use vcod_cod4::menuexp::{atof, atoi};
use vcod_cod4::net::parse::Gamestate;
use vcod_cod4::scriptmenu::{
    Arena, ITEM_TYPE_BIND, ITEM_TYPE_DECIMALFIELD, ITEM_TYPE_DVARENUM, ITEM_TYPE_LISTBOX,
    ITEM_TYPE_MULTI, ITEM_TYPE_NUMERICFIELD, ITEM_TYPE_OWNERDRAW, ITEM_TYPE_SLIDER,
    ITEM_TYPE_UPREDITFIELD, ITEM_TYPE_VALIDFILEFIELD, ITEM_TYPE_YESNO, LayoutItem,
    UI_KEYBINDSTATUS, WINDOW_VISIBLE, is_text_field,
};

use super::{ScriptMenus, placed};
use crate::console::keys;
use crate::hud::cod2elem::ScreenPlacement;

/// The CoD4 dvars an option control sets and the vcod cvar each is: the
/// mouse's (`main.rs`'s `mouse_frame`), `r_gamma` (the gamma pass),
/// `fx_marks` as `cg_marks` (the marks), `cg_drawCrosshair`
/// (`cod4_crosshair`), `sm_enable` (the sun shadows), `cl_allowDownload`
/// (the join's downloads), `name` (Multiplayer's name field copies
/// `ui_name` into it on close), and `snd_volume`, the master volume
/// ([`ScriptMenus::host_cvar`]). `m_pitch` is the invert mouse row's,
/// through `uiScript update ui_mousePitch`. `ui_browserShowCoD4x` is
/// vcod's own browser filter (`browser::SHOW_COD4X`). The graphics and
/// game pages' (cod4-front-end.md 9): the video mode, refresh rate and
/// anti-aliasing (`vid_restart`), `r_vsync` as `r_swapInterval`, glow,
/// specular, the dynamic light limit, model detail, water animation, the
/// mip filter, `cg_blood`, `monkeytoy` (the console key), depth of field
/// (`r_dof_enable`) and `r_zFeather`, which Soften Smoke Edges'
/// `ui_zfeather` copy sets as the page closes; the aspect ratio
/// (`vid_restart`), the texture page's picmip dvars (`r_applyPicmip`) and
/// its anisotropy (`r_texFilterAnisoMin`, live), and `ragdoll_enable`.
/// The voice page's record level slider, `winvoice_mic_reclevel`.
/// Create a Class's class names, `customclass1`..`5`, which its rename
/// field sets.
const VCOD_CVARS: [(&str, &str); 39] = [
    ("customclass1", "customclass1"),
    ("customclass2", "customclass2"),
    ("customclass3", "customclass3"),
    ("customclass4", "customclass4"),
    ("customclass5", "customclass5"),
    ("sensitivity", "sensitivity"),
    ("cl_freelook", "cl_freelook"),
    ("m_filter", "m_filter"),
    ("m_pitch", "m_pitch"),
    ("r_gamma", "r_gamma"),
    ("fx_marks", "cg_marks"),
    ("cg_drawCrosshair", "cg_drawcrosshair"),
    ("sm_enable", "sm_enable"),
    ("cl_allowDownload", "cl_allowdownload"),
    ("name", "name"),
    (super::browser::SHOW_COD4X, super::browser::SHOW_COD4X),
    ("r_mode", "r_mode"),
    ("r_displayRefresh", "r_displayrefresh"),
    ("r_aaSamples", "r_aasamples"),
    ("r_vsync", "r_swapinterval"),
    ("r_glow_allowed", "r_glow_allowed"),
    ("r_zFeather", "r_zfeather"),
    ("r_dof_enable", "r_dof_enable"),
    ("r_specular", "r_specular"),
    ("r_dlightLimit", "r_dlightlimit"),
    ("r_lodScaleRigid", "r_lodscalerigid"),
    ("r_lodBiasRigid", "r_lodbiasrigid"),
    ("r_drawWater", "r_drawwater"),
    ("r_texFilterMipMode", "r_texfiltermipmode"),
    ("cg_blood", "cg_blood"),
    ("monkeytoy", "monkeytoy"),
    ("r_aspectRatio", "r_aspectratio"),
    ("r_picmip_manual", "r_picmip_manual"),
    ("r_picmip", "r_picmip"),
    ("r_picmip_bump", "r_picmip_bump"),
    ("r_picmip_spec", "r_picmip_spec"),
    ("r_texFilterAnisoMin", "r_texfilteranisomin"),
    ("ragdoll_enable", "ragdoll_enable"),
    ("winvoice_mic_reclevel", "winvoice_mic_reclevel"),
];

/// The graphics pages' `ui_r_*` copies whose dvar vcod has
/// (`options_graphics.cfg`, `options_graphics_texture`'s `onOpen`): they
/// live in the UI's table until the video restart popup or
/// `apply_picmip_popmenu` copies them back.
const UI_VIDEO_COPIES: [&str; 10] = [
    "ui_r_mode",
    "ui_r_displayRefresh",
    "ui_r_aasamples",
    "ui_r_vsync",
    "ui_zfeather",
    "ui_r_aspectratio",
    "ui_r_picmip_manual",
    "ui_r_picmip",
    "ui_r_picmip_bump",
    "ui_r_picmip_spec",
];

/// Whether `d` is one of the Options pages' `ui_*` copies of a video or
/// sound dvar (`ui_r_*`, `ui_snd_*`, `ui_zfeather`, `ui_outputConfig`;
/// `options_graphics.cfg`, `options_graphics_texture`, `options_sound`).
fn options_copy(d: &str) -> bool {
    let d = d.to_ascii_lowercase();
    d.starts_with("ui_r_")
        || d.starts_with("ui_snd_")
        || d == "ui_zfeather"
        || d == "ui_outputconfig"
}

/// The owner draw of the game type a call vote's typemap picks
/// (`UI_NETGAMETYPE`): it draws `ui_netGameType`'s display name and a
/// click steps it.
pub const UI_NETGAMETYPE: i32 = 245;
/// The owner draw of Start New Server's map preview (`UI_DrawMapPreview`).
pub const UI_MAPPREVIEW: i32 = 254;
/// The feeders: the maps of `ui_netGameType` (`FEEDER_ALLMAPS`), the
/// players (`FEEDER_PLAYER_LIST`) and the mute list (`FEEDER_MUTELIST`).
const FEEDER_MAPS: f32 = 4.0;
const FEEDER_PLAYERS: f32 = 7.0;
const FEEDER_MUTELIST: f32 = 20.0;

/// The keys a control takes (`Item_HandleKey`'s 200, 201, 13, 156, 157).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKey {
    Mouse1,
    Enter,
    Left,
    Right,
}

/// The call vote's lists: `mp/cod2maps.arena`'s maps and the game types
/// (name, display name key) in `_gametypes.txt`'s order.
#[derive(Clone, Debug, Default)]
pub struct Lists {
    pub maps: Vec<Arena>,
    pub gametypes: Vec<(String, String)>,
}

/// A control's slider in grid units: the bar's left and top, the thumb's
/// centre x (`Item_Slider_Paint`, `Item_Slider_ThumbPosition`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    pub x: f32,
    pub y: f32,
    pub thumb: f32,
}

/// The bar's size, the thumb's travel and inset, and the thumb's size.
pub const SLIDER_W: f32 = 96.0;
pub const SLIDER_H: f32 = 16.0;
const THUMB_TRAVEL: f32 = 84.0;
const THUMB_INSET: f32 = 6.0;
pub const THUMB_W: f32 = 10.0;
pub const THUMB_H: f32 = 20.0;
/// A list box scroll bar's width, and each arrow's and the thumb's side.
pub const SCROLL: f32 = 16.0;

/// A list box's scroll bar ([`ScriptMenus::scroll_bar`]), grid units:
/// the column's left edge, the thumb's top, and the largest first row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollBar {
    pub x: f32,
    pub thumb: f32,
    pub max: usize,
}

/// The thumb's path down a vertical list box of grid rect `r` whose first
/// row runs 0..=`max` (cod4-hud.md 12.6): its top starts a unit under the
/// up arrow, each first row moves it an equal share of the travel, and its
/// bottom stays two units clear of the down arrow.
struct ThumbPath {
    top: f32,
    travel: f32,
    lowest: f32,
    max: usize,
}

impl ThumbPath {
    fn new(r: [f32; 4], max: usize) -> ThumbPath {
        let (y, h) = (r[1], r[3]);
        ThumbPath {
            top: y + SCROLL + 1.0,
            travel: h - 3.0 * SCROLL - 2.0,
            lowest: y + h - 2.0 * SCROLL - 2.0,
            max,
        }
    }

    /// The thumb's top with `first` as the first row shown.
    fn thumb_at(&self, first: usize) -> f32 {
        let share = if self.max == 0 {
            0.0
        } else {
            self.travel * first as f32 / self.max as f32
        };
        self.lowest.min(self.top + share)
    }

    /// The first row a held thumb shows with its centre at grid `y`.
    fn first_under(&self, y: f32) -> usize {
        if self.travel <= 0.0 {
            return 0;
        }
        let along = (y - 0.5 * SCROLL - self.top) / self.travel;
        ((along * self.max as f32).max(0.0) as usize).min(self.max)
    }
}

impl ScriptMenus {
    /// The call vote's lists, once the zones load.
    pub fn set_lists(&mut self, lists: Lists) {
        self.lists = lists;
    }

    /// The vcod cvar a CoD4 dvar is, `None` for one vcod has none for.
    /// `snd_volume` is the shell's master volume (`audio::volume_cvar`).
    pub(super) fn host_cvar(&self, dvar: &str) -> Option<&'static str> {
        if dvar.eq_ignore_ascii_case("snd_volume") {
            return Some(if self.host.cvars.contains_key("snd_volume") {
                "snd_volume"
            } else {
                "mss_volume"
            });
        }
        VCOD_CVARS
            .iter()
            .find(|(d, _)| d.eq_ignore_ascii_case(dvar))
            .map(|&(_, c)| c)
    }

    /// Sets a dvar: a vcod cvar through the console (archived, so the
    /// config keeps it), any other in the UI's table.
    pub(super) fn set_dvar(&mut self, name: &str, value: &str) {
        match self.host_cvar(name) {
            Some(c) => {
                self.console.push(format!("set {c} \"{value}\""));
                self.host.cvars.insert(c.to_string(), value.to_string());
            }
            None => {
                self.ui
                    .dvars
                    .insert(name.to_ascii_lowercase(), value.to_string());
            }
        }
    }

    /// `Item_EnableShowViaDvar`: whether `dvarTest`'s value is
    /// in `enableDvar`'s list, read through `flag` (1 enable, 4 show: the
    /// flag set, listed; else not listed); true with no test.
    pub fn dvar_enabled(&self, it: &LayoutItem, flag: i32) -> bool {
        let (Some(list), Some(test)) = (
            it.enable_dvar.as_deref().filter(|s| !s.is_empty()),
            it.dvar_test.as_deref().filter(|s| !s.is_empty()),
        ) else {
            return true;
        };
        let v = self.dvar(test).unwrap_or_default();
        let listed = vcod_cod4::hud::tokenize(list)
            .iter()
            .any(|t| t != ";" && t.eq_ignore_ascii_case(&v));
        listed == (it.dvar_flags & flag != 0)
    }

    /// Whether `it` is an option control vcod has no cvar for, or an item
    /// its `dvarTest` disables: drawn in the menu's `disableColor`, never
    /// focused.
    pub fn greyed(&self, it: &LayoutItem) -> bool {
        if it.dvar_flags & 3 != 0 && !self.dvar_enabled(it, 1) {
            return true;
        }
        // A `dvarEnumList` steps through its enum dvar's domain (the video
        // modes and refresh rates of `r_mode`, `r_displayRefresh`), greyed
        // while the window has listed none.
        if it.kind == ITEM_TYPE_DVARENUM {
            return self.enum_entries(it).is_empty();
        }
        let option = is_text_field(it.kind)
            || matches!(
                it.kind,
                ITEM_TYPE_SLIDER | ITEM_TYPE_YESNO | ITEM_TYPE_MULTI
            );
        // The front end's own `ui_*` dvars (Start New Server's
        // `ui_dedicated`) and the server settings it hands the server
        // (`sv_hostname`, `g_password`) live in the UI's table.
        // The Options pages' `ui_*` copies of video and sound dvars
        // apply through a restart: greyed unless vcod has the dvar.
        if option && let Some(d) = it.dvar.as_deref().filter(|d| options_copy(d)) {
            return !UI_VIDEO_COPIES.iter().any(|c| c.eq_ignore_ascii_case(d));
        }
        option
            && it.dvar.as_deref().is_none_or(|d| {
                self.host_cvar(d).is_none()
                    && !d.eq_ignore_ascii_case("ui_mousePitch")
                    && !d.eq_ignore_ascii_case("ui_name")
                    && !(self.is_front && super::front::ui_table_dvar(d))
                    && super::browser::ui_dvar(d).is_none()
            })
    }

    /// A menu just opened: each greyed option logged once, and each list
    /// box's first row picked (`UI_FeederSelection`).
    pub(super) fn opened_controls(&mut self, gs: &Gamestate) {
        let _ = gs;
        let Some(open) = &self.open else {
            return;
        };
        let Some(layout) = open.layout.clone() else {
            return;
        };
        let name = open.name.clone();
        for (i, it) in layout.items.iter().enumerate() {
            if self.greyed(it) {
                let d = it.dvar.clone().unwrap_or_default();
                if self.greyed.insert(d.to_ascii_lowercase()) {
                    log::info!("cod4 menu {name}: {d:?} has no vcod equivalent; drawn greyed");
                }
            }
            if it.kind == ITEM_TYPE_LISTBOX {
                if let Some(o) = self.open.as_mut() {
                    o.lists.insert(i, (0, 0));
                }
                if self.feeder_count(it.feeder) > 0 {
                    self.feeder_select(it.feeder, 0);
                }
            }
        }
    }

    /// `show` / `hide` of item `name` in `menu`.
    pub(super) fn show_item(&mut self, menu: &str, name: &str, show: bool) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        if !open.name.eq_ignore_ascii_case(menu) {
            return;
        }
        let Some(layout) = open.layout.as_mut() else {
            return;
        };
        for it in layout
            .items
            .iter_mut()
            .filter(|it| it.name.eq_ignore_ascii_case(name))
        {
            if show {
                it.dynamic_flags |= WINDOW_VISIBLE;
            } else {
                it.dynamic_flags &= !WINDOW_VISIBLE;
            }
        }
    }

    fn item(&self, i: usize) -> Option<&LayoutItem> {
        self.open.as_ref()?.layout.as_ref()?.items.get(i)
    }

    /// `Item_HandleKey` for the focused item `i`; true when it took the
    /// key. MOUSE1 reaches it only with the cursor on the item.
    pub(super) fn item_key(&mut self, i: usize, key: ItemKey, gs: &Gamestate) -> bool {
        let _ = gs;
        let Some(it) = self.item(i).cloned() else {
            return false;
        };
        if self.greyed(&it) {
            return false;
        }
        let dvar = it.dvar.clone().unwrap_or_default();
        match it.kind {
            // `Item_YesNo_HandleKey`: `va("%i", atoi(value) == 0)`.
            ITEM_TYPE_YESNO => {
                let on = atoi(&self.dvar(&dvar).unwrap_or_default()) != 0;
                self.set_dvar(&dvar, if on { "0" } else { "1" });
                true
            }
            // `Item_DvarEnum_HandleKey`: the entry after (Left:
            // before) the one the dvar names, wrapping, as `%i`.
            ITEM_TYPE_DVARENUM => {
                let n = self.enum_entries(&it).len();
                let cur = self.enum_index(&it);
                let next = match key {
                    _ if n == 0 => return false,
                    ItemKey::Left => (cur + n - 1) % n,
                    _ => (cur + 1) % n,
                };
                if next == cur {
                    return false;
                }
                self.set_dvar(&dvar, &next.to_string());
                true
            }
            ITEM_TYPE_MULTI => {
                let TypeData::Multi(m) = &it.type_data else {
                    return false;
                };
                let n = m.labels.len();
                let cur = self.multi_index(&dvar, m);
                let next = match key {
                    _ if n == 0 => return false,
                    ItemKey::Left => (cur + n - 1) % n,
                    _ => (cur + 1) % n,
                };
                if next == cur {
                    return false;
                }
                let v = if m.str_def {
                    m.strings[next].clone()
                } else {
                    format_g(m.values[next])
                };
                self.set_dvar(&dvar, &v);
                true
            }
            // `Item_Slider_HandleKey`: the arrows step 5% of the range.
            ITEM_TYPE_SLIDER => {
                let TypeData::Edit(e) = &it.type_data else {
                    return false;
                };
                let v = atof(&self.dvar(&dvar).unwrap_or_default()) as f32;
                let step = (e.max - e.min) * 0.05;
                let v = match key {
                    ItemKey::Left => (v - step).max(e.min),
                    ItemKey::Right => (v + step).min(e.max),
                    _ => return false,
                };
                self.set_dvar(&dvar, &format_g(v));
                true
            }
            // `Item_Bind_HandleKey`: Enter or a click waits for a key.
            ITEM_TYPE_BIND => {
                if matches!(key, ItemKey::Enter | ItemKey::Mouse1) {
                    self.waiting = Some(i);
                    true
                } else {
                    false
                }
            }
            k if is_text_field(k) && matches!(key, ItemKey::Enter | ItemKey::Mouse1) => {
                self.begin_edit(i);
                true
            }
            ITEM_TYPE_OWNERDRAW if it.owner_draw != UI_NETGAMETYPE => {
                self.browser_owner_key(it.owner_draw, key != ItemKey::Left)
            }
            ITEM_TYPE_OWNERDRAW if it.owner_draw == UI_NETGAMETYPE => {
                // `UI_NetGameType_HandleKey`: a click or Enter steps on.
                let n = self.lists.gametypes.len();
                if n == 0 || !matches!(key, ItemKey::Mouse1 | ItemKey::Enter) {
                    return false;
                }
                let cur = usize::try_from(atoi(&self.dvar("ui_netGameType").unwrap_or_default()))
                    .unwrap_or(0)
                    .min(n - 1);
                let next = (cur + 1) % n;
                self.set_dvar("ui_netGameType", &next.to_string());
                self.select_first_map();
                true
            }
            _ => false,
        }
    }

    /// A click on the focused item `i` (the cursor is on it): a slider
    /// takes the cursor's place (`Scroll_Slider_SetThumbPos`) and holds
    /// the mouse for a drag; a list box's scroll bar steps a row on its
    /// arrows, a page on its track, and holds the mouse on its thumb
    /// (`Item_ListBox_OverLB`, `Item_StartCapture`); else the row under
    /// the cursor is picked, and a second click on it inside 300 ms runs
    /// the list's `doubleClick` (`Item_ListBox_HandleKey`); any other
    /// control takes MOUSE1.
    pub(super) fn click_item(
        &mut self,
        i: usize,
        size: (f32, f32),
        gs: &Gamestate,
        out: &mut Vec<String>,
    ) {
        let Some(it) = self.item(i).cloned() else {
            return;
        };
        let Some(layout) = self.open.as_ref().and_then(|o| o.layout.clone()) else {
            return;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let c = self.cursor(size);
        match it.kind {
            ITEM_TYPE_SLIDER if !self.greyed(&it) => {
                self.drag(i, size);
                self.capture = Some(i);
            }
            ITEM_TYPE_LISTBOX => {
                let TypeData::ListBox(lb) = &it.type_data else {
                    return;
                };
                let (r, align) = self.rect(&it, &layout);
                if let Some(bar) = self.scroll_bar(i) {
                    let hit = |y: f32, h: f32| {
                        super::contains(placed(&sp, ([bar.x, y, SCROLL, h], align)), c)
                    };
                    let (start, cur) = self.list_state(i);
                    let page = list_rows(lb, r[3]);
                    let to = if hit(r[1], SCROLL) {
                        Some(start.saturating_sub(1))
                    } else if hit(r[1] + r[3] - SCROLL, SCROLL) {
                        Some(start + 1)
                    } else if hit(bar.thumb, SCROLL) {
                        self.capture = Some(i);
                        None
                    } else if hit(r[1] + SCROLL, bar.thumb - r[1] - SCROLL) {
                        Some(start.saturating_sub(page))
                    } else if hit(bar.thumb + SCROLL, r[1] + r[3] - 2.0 * SCROLL - bar.thumb) {
                        Some(start + page)
                    } else {
                        None
                    };
                    if let Some(to) = to {
                        let to = to.min(bar.max);
                        if let Some(o) = self.open.as_mut() {
                            o.lists.insert(i, (to, cur));
                        }
                    }
                    if to.is_some() || self.capture == Some(i) {
                        return;
                    }
                }
                let [_, y, _, h] =
                    placed(&sp, ([r[0], r[1] + 1.0, r[2], lb.element_height], align));
                if h <= 0.0 {
                    return;
                }
                let start = self.list_state(i).0;
                let row = start + ((c[1] - y) / h).max(0.0) as usize;
                if row < self.feeder_count(it.feeder) {
                    let now = std::time::Instant::now();
                    let again = self.last_list_click.is_some_and(|(t, li, lr)| {
                        li == i && lr == row && now.duration_since(t).as_millis() < 300
                    });
                    self.last_list_click = Some((now, i, row));
                    if let Some(o) = self.open.as_mut() {
                        o.lists.insert(i, (start, row));
                    }
                    self.feeder_select(it.feeder, row);
                    if again && let Some(script) = &lb.double_click {
                        let name = self
                            .open
                            .as_ref()
                            .map(|o| o.name.clone())
                            .unwrap_or_default();
                        let cmds = vcod_cod4::scriptmenu::Cmd::parse(script);
                        self.run(&name, Some(i), &cmds, gs, out, 0);
                    }
                }
            }
            _ => {
                self.item_key(i, ItemKey::Mouse1, gs);
            }
        }
    }

    /// The held item follows the cursor: a slider's value
    /// (`Scroll_Slider_SetThumbPos`), a list's first row (the thumb's
    /// centre under the cursor, cod4-hud.md 12.6).
    pub(super) fn drag(&mut self, i: usize, size: (f32, f32)) {
        let Some(it) = self.item(i).cloned() else {
            return;
        };
        let Some(layout) = self.open.as_ref().and_then(|o| o.layout.clone()) else {
            return;
        };
        let sp = ScreenPlacement::cod4(size.0, size.1);
        let c = self.cursor(size);
        let (r, align) = self.rect(&it, &layout);
        match it.kind {
            ITEM_TYPE_SLIDER => {
                let (TypeData::Edit(e), Some(s), Some(d)) =
                    (&it.type_data, self.slider(&it, &layout), it.dvar.as_deref())
                else {
                    return;
                };
                let [x, _, w, _] = placed(
                    &sp,
                    ([s.x + THUMB_INSET, s.y, THUMB_TRAVEL, SLIDER_H], align),
                );
                let frac = ((c[0] - x) / w).clamp(0.0, 1.0);
                self.set_dvar(d, &format_g(frac * (e.max - e.min) + e.min));
            }
            ITEM_TYPE_LISTBOX => {
                let Some(bar) = self.scroll_bar(i) else {
                    return;
                };
                let [_, screen_y, _, screen_h] = placed(&sp, (r, align));
                if screen_h <= 0.0 {
                    return;
                }
                // The cursor back in the box's grid units.
                let grid_y = r[1] + r[3] * (c[1] - screen_y) / screen_h;
                let first = ThumbPath::new(r, bar.max).first_under(grid_y);
                let picked = self.list_state(i).1;
                if let Some(o) = self.open.as_mut() {
                    o.lists.insert(i, (first, picked));
                }
            }
            _ => {}
        }
    }

    /// List box `i`'s scroll bar, `None` for one with `noScrollBars`: a
    /// `SCROLL`-wide column at the box's right edge, the thumb placed for
    /// the first row shown (cod4-hud.md 12.6). vcod's largest first row is
    /// the row count less the rows that fit (that section's vcod note).
    pub fn scroll_bar(&self, i: usize) -> Option<ScrollBar> {
        let it = self.item(i)?;
        let TypeData::ListBox(lb) = &it.type_data else {
            return None;
        };
        if lb.no_scroll_bars {
            return None;
        }
        let (r, _) = self.rect(it, self.open.as_ref()?.layout.as_ref()?);
        let max = self
            .feeder_count(it.feeder)
            .saturating_sub(list_rows(lb, r[3]));
        let path = ThumbPath::new(r, max);
        Some(ScrollBar {
            x: r[0] + r[2] - SCROLL,
            thumb: path.thumb_at(self.list_state(i).0.min(max)),
            max,
        })
    }

    /// The wheel over the open menu: a focused list box scrolls a row
    /// (`Item_ListBox_HandleKey` 205, 206).
    pub fn wheel(&mut self, up: bool) {
        let Some(open) = &self.open else {
            return;
        };
        let (Some(i), Some(layout)) = (open.focus, &open.layout) else {
            return;
        };
        let Some(it) = layout.items.get(i) else {
            return;
        };
        let TypeData::ListBox(lb) = &it.type_data else {
            return;
        };
        let max = self
            .feeder_count(it.feeder)
            .saturating_sub(list_rows(lb, it.rect[3]));
        let (start, cur) = self.list_state(i);
        let start = if up {
            start.saturating_sub(1)
        } else {
            (start + 1).min(max)
        };
        if let Some(o) = self.open.as_mut() {
            o.lists.insert(i, (start, cur));
        }
    }

    /// A list box's first drawn row and picked row.
    pub fn list_state(&self, i: usize) -> (usize, usize) {
        self.open
            .as_ref()
            .and_then(|o| o.lists.get(&i).copied())
            .unwrap_or((0, 0))
    }

    /// `Item_TextField_BeginEdit`: an edit field of the open menu takes
    /// the typed text.
    pub(super) fn begin_edit(&mut self, i: usize) {
        if self
            .item(i)
            .is_some_and(|it| is_text_field(it.kind) && !self.greyed(it))
        {
            self.editing = Some(i);
        }
    }

    /// Whether an edit field takes the typed text.
    pub fn editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Typed text into the field being edited (`Item_TextField_HandleKey`'s
    /// characters), up to its `maxChars` (255 with none): a numeric field
    /// takes digits, a decimal one digits and `.`, a file name field
    /// letters, digits, `_` and `-`; an upper case field shouts.
    pub fn type_text(&mut self, text: &str) {
        let Some(it) = self.editing.and_then(|i| self.item(i)).cloned() else {
            return;
        };
        let Some(d) = it.dvar.as_deref() else {
            return;
        };
        let max = match &it.type_data {
            TypeData::Edit(e) if e.max_chars > 0 => e.max_chars as usize,
            _ => 255,
        };
        let mut v = self.dvar(d).unwrap_or_default();
        let takes = |c: char| match it.kind {
            ITEM_TYPE_NUMERICFIELD => c.is_ascii_digit(),
            ITEM_TYPE_DECIMALFIELD => c.is_ascii_digit() || c == '.',
            ITEM_TYPE_VALIDFILEFIELD => c.is_ascii_alphanumeric() || matches!(c, '_' | '-'),
            _ => c != '@',
        };
        for c in text
            .chars()
            .filter(|&c| (' '..='~').contains(&c) && takes(c))
        {
            if v.chars().count() >= max {
                break;
            }
            v.push(if it.kind == ITEM_TYPE_UPREDITFIELD {
                c.to_ascii_uppercase()
            } else {
                c
            });
        }
        self.set_dvar(d, &v);
    }

    /// A key while an edit field takes text: Backspace (8 or 127) drops
    /// the last character, Enter runs the field's `onAccept` and ends the
    /// edit, Escape ends it and goes on to the menu, other characters
    /// come through [`Self::type_text`]. True when the field took it.
    pub(super) fn edit_key(
        &mut self,
        key: super::MenuKey,
        gs: &Gamestate,
        out: &mut Vec<String>,
    ) -> bool {
        use super::MenuKey;
        let Some(i) = self.editing else {
            return false;
        };
        let Some(it) = self.item(i).cloned() else {
            self.editing = None;
            return false;
        };
        match key {
            MenuKey::Char(8 | 127) => {
                if let Some(d) = it.dvar.as_deref() {
                    let mut v = self.dvar(d).unwrap_or_default();
                    v.pop();
                    self.set_dvar(d, &v);
                }
                true
            }
            MenuKey::Char(_) => true,
            MenuKey::Enter => {
                self.editing = None;
                let name = self
                    .open
                    .as_ref()
                    .map(|o| o.name.clone())
                    .unwrap_or_default();
                self.run(&name, Some(i), &it.on_accept, gs, out, 0);
                true
            }
            _ => {
                self.editing = None;
                false
            }
        }
    }

    /// Whether a bind row waits for its key.
    pub fn waiting_for_key(&self) -> bool {
        self.waiting.is_some()
    }

    /// `Item_Bind_HandleKey` while waiting, `key` vcod's key name: Escape
    /// cancels; Backspace, or a command that already has two keys, unbinds
    /// them; any key but Backspace then takes the command.
    pub fn bind_key(&mut self, key: &str) {
        let Some(i) = self.waiting.take() else {
            return;
        };
        let Some(cmd) = self.item(i).and_then(|it| it.dvar.clone()) else {
            return;
        };
        if key == "ESCAPE" {
            return;
        }
        let bound = self.bound_keys(&cmd);
        if key == "BACKSPACE" || bound.len() == 2 {
            for k in bound {
                self.console.push(format!("unbind {k}"));
                self.host.binds.retain(|(b, _)| *b != k);
            }
        }
        if key != "BACKSPACE" {
            self.console.push(format!("bind {key} \"{cmd}\""));
            self.host.binds.retain(|(b, _)| b != key);
            self.host.binds.push((key.to_string(), cmd));
        }
    }

    /// `Key_GetCommandAssignment`: the first two keys bound to `cmd` in key
    /// number order.
    pub fn bound_keys(&self, cmd: &str) -> Vec<String> {
        let mut k: Vec<&str> = self
            .host
            .binds
            .iter()
            .filter(|(_, c)| c.eq_ignore_ascii_case(cmd))
            .map(|(k, _)| k.as_str())
            .collect();
        k.sort_by_key(|k| keys::number(k));
        k.into_iter().take(2).map(str::to_string).collect()
    }

    /// The text a control draws in place of its own (`Item_YesNo_Paint`,
    /// `Item_Multi_Setting`, `Item_Bind_Paint`, the key bind status and
    /// game type owner draws), `@`-keys for the caller to localize; a bind
    /// row's keys come as `KEY_*` names joined by `@KEY_OR`, or
    /// `@KEY_UNBOUND`.
    pub fn control_text(&self, i: usize, it: &LayoutItem) -> Option<String> {
        let value = || {
            it.dvar
                .as_deref()
                .and_then(|d| self.dvar(d))
                .unwrap_or_default()
        };
        Some(match it.kind {
            ITEM_TYPE_YESNO => {
                if atof(&value()) == 0.0 {
                    "@EXE_NO".into()
                } else {
                    "@EXE_YES".into()
                }
            }
            ITEM_TYPE_MULTI => match &it.type_data {
                TypeData::Multi(m) if !m.labels.is_empty() => {
                    m.labels[self.multi_index(it.dvar.as_deref()?, m)].clone()
                }
                _ => String::new(),
            },
            // `Item_DvarEnum_Setting`.
            ITEM_TYPE_DVARENUM => self
                .enum_entries(it)
                .get(self.enum_index(it))
                .cloned()
                .unwrap_or_default(),
            // `Item_TextField_Paint`: a decimal field's float to two places.
            ITEM_TYPE_DECIMALFIELD => {
                let v = value();
                match v.trim().parse::<f32>() {
                    Ok(f) => format!("{f:.2}"),
                    Err(_) => v,
                }
            }
            k if is_text_field(k) => value(),
            ITEM_TYPE_BIND => {
                if self.waiting == Some(i) {
                    return Some("@MENU_BIND_KEY_PENDING".into());
                }
                match self.bound_keys(it.dvar.as_deref()?).as_slice() {
                    [] => "@KEY_UNBOUND".into(),
                    [a] => format!("@KEY_{a}"),
                    [a, b, ..] => format!("@KEY_{a}\u{1}@KEY_OR\u{1}@KEY_{b}"),
                }
            }
            ITEM_TYPE_OWNERDRAW if it.owner_draw == UI_KEYBINDSTATUS => {
                if self.waiting.is_some() {
                    "@EXE_KEYWAIT".into()
                } else {
                    "@EXE_KEYCHANGE".into()
                }
            }
            ITEM_TYPE_OWNERDRAW if it.owner_draw != UI_NETGAMETYPE => {
                return self.browser_owner_text(it.owner_draw);
            }
            ITEM_TYPE_OWNERDRAW if it.owner_draw == UI_NETGAMETYPE => {
                let at = atoi(&self.dvar("ui_netGameType").unwrap_or_default());
                let (_, key) = self.lists.gametypes.get(usize::try_from(at).ok()?)?;
                format!("@{key}")
            }
            _ => return None,
        })
    }

    /// The entries of an `ITEM_TYPE_DVARENUM`'s enum dvar, as the client
    /// listed them; none for an item of another type.
    fn enum_entries(&self, it: &LayoutItem) -> &[String] {
        match &it.type_data {
            TypeData::Enum(name) => self
                .host
                .enums
                .get(&name.to_ascii_lowercase())
                .map_or(&[], Vec::as_slice),
            _ => &[],
        }
    }

    /// `Item_DvarEnum_EnumIndex`: the item's dvar as an index
    /// in range, else the entry it names in any case, else 0.
    fn enum_index(&self, it: &LayoutItem) -> usize {
        let entries = self.enum_entries(it);
        let v = it
            .dvar
            .as_deref()
            .and_then(|d| self.dvar(d))
            .unwrap_or_default();
        match usize::try_from(atoi(&v)) {
            Ok(i) if i < entries.len() => i,
            _ => entries
                .iter()
                .position(|e| e.eq_ignore_ascii_case(&v))
                .unwrap_or(0),
        }
    }

    /// `Item_Multi_FindDvarByValue`: the entry the dvar holds, else 0.
    fn multi_index(&self, dvar: &str, m: &MultiDef) -> usize {
        let v = self.dvar(dvar).unwrap_or_default();
        if m.str_def {
            m.strings.iter().position(|s| s.eq_ignore_ascii_case(&v))
        } else {
            let f = atof(&v) as f32;
            m.values.iter().position(|x| *x == f)
        }
        .unwrap_or(0)
    }

    /// A slider control's bar and thumb on the grid: the bar 96 x 16
    /// placed in the rect by `textAlignMode` off `textalignx` / `y`, the
    /// thumb 6 in plus 84 times the clamped value's fraction.
    pub fn slider(
        &self,
        it: &LayoutItem,
        layout: &vcod_cod4::scriptmenu::Layout,
    ) -> Option<Slider> {
        let TypeData::Edit(e) = &it.type_data else {
            return None;
        };
        let (r, _) = self.rect(it, layout);
        let x0 = r[0] + it.text_offset[0];
        let x = match it.text_align & 3 {
            1 => x0 + 0.5 * (r[2] - SLIDER_W),
            2 => x0 + r[2] - SLIDER_W,
            _ => x0,
        };
        let y0 = r[1] + it.text_offset[1];
        let y = match it.text_align & 0xc {
            8 => y0 + 0.5 * (r[3] - SLIDER_H),
            12 => y0 + r[3] - SLIDER_H,
            _ => y0,
        };
        let v = it
            .dvar
            .as_deref()
            .and_then(|d| self.dvar(d))
            .map_or(0.0, |v| atof(&v) as f32)
            .clamp(e.min.min(e.max), e.max.max(e.min));
        let span = e.max - e.min;
        let frac = if span != 0.0 { (v - e.min) / span } else { 0.0 };
        Some(Slider {
            x,
            y,
            thumb: x + THUMB_INSET + frac * THUMB_TRAVEL,
        })
    }

    /// `UI_DrawMapPreview`'s level shot: `loadscreen_<map>` of
    /// `ui_currentNetMap` (`UI_LoadArenas`, `iw3mp.exe` 0x5412b4).
    pub fn map_preview(&self) -> Option<String> {
        let at = atoi(&self.dvar("ui_currentNetMap").unwrap_or_default());
        let maps = &self.lists.maps;
        let m = usize::try_from(at)
            .ok()
            .filter(|&m| m < maps.len())
            .unwrap_or(0);
        Some(format!("loadscreen_{}", maps.get(m)?.map))
    }

    /// `UI_FeederCount`.
    pub fn feeder_count(&self, feeder: f32) -> usize {
        if let Some(n) = self
            .front_feeder_count(feeder)
            .or_else(|| self.browser_feeder_count(feeder))
        {
            return n;
        }
        if feeder == FEEDER_MAPS {
            self.active_maps().len()
        } else if feeder == FEEDER_PLAYERS || feeder == FEEDER_MUTELIST {
            self.host.players.len()
        } else {
            0
        }
    }

    /// `UI_FeederItemText`: a map's long name key (`@`-prefixed), a
    /// player's cleaned name; the mute list's first column `@MP_MUTED`
    /// for a muted player.
    pub fn feeder_text(&self, feeder: f32, row: usize, column: usize) -> String {
        if let Some(t) = self
            .front_feeder_text(feeder, row)
            .or_else(|| self.browser_feeder_text(feeder, row, column))
        {
            return t;
        }
        if feeder == FEEDER_MAPS {
            return self.active_maps().get(row).map_or(String::new(), |&m| {
                format!("@{}", self.lists.maps[m].longname)
            });
        }
        let Some((client, name)) = self.host.players.get(row) else {
            return String::new();
        };
        if feeder == FEEDER_MUTELIST && column == 0 {
            if self.muted.contains(client) {
                "@MP_MUTED".into()
            } else {
                String::new()
            }
        } else if feeder == FEEDER_PLAYERS || feeder == FEEDER_MUTELIST {
            clean(name)
        } else {
            String::new()
        }
    }

    /// `UI_FeederSelection`: a map sets `ui_currentNetMap` to its index in
    /// the arena list, a player the player pick.
    fn feeder_select(&mut self, feeder: f32, row: usize) {
        if self.front_feeder_select(feeder, row) || self.browser_feeder_select(feeder, row) {
            return;
        }
        if feeder == FEEDER_MAPS {
            if let Some(&m) = self.active_maps().get(row) {
                self.set_dvar("ui_currentNetMap", &m.to_string());
            }
        } else if feeder == FEEDER_PLAYERS || feeder == FEEDER_MUTELIST {
            self.player_index = Some(row);
        }
    }

    /// The maps whose game types hold `ui_netGameType`'s
    /// (`UI_MapCountByGameType`), as indexes into the arena list.
    fn active_maps(&self) -> Vec<usize> {
        let at = atoi(&self.dvar("ui_netGameType").unwrap_or_default());
        let gt = usize::try_from(at)
            .ok()
            .and_then(|i| self.lists.gametypes.get(i));
        (0..self.lists.maps.len())
            .filter(|&m| {
                let a = &self.lists.maps[m];
                a.gametypes.is_empty()
                    || gt.is_none_or(|(g, _)| a.gametypes.iter().any(|x| x.eq_ignore_ascii_case(g)))
            })
            .collect()
    }

    /// After a game type step: the first map of the new list.
    fn select_first_map(&mut self) {
        let Some(open) = &self.open else {
            return;
        };
        let lists: Vec<usize> = open.lists.keys().copied().collect();
        for i in lists {
            if self.item(i).is_some_and(|it| it.feeder == FEEDER_MAPS) {
                if let Some(o) = self.open.as_mut() {
                    o.lists.insert(i, (0, 0));
                }
                self.feeder_select(FEEDER_MAPS, 0);
            }
        }
    }

    /// The `uiScript`s the in-game pages run: `update ui_mousePitch`
    /// (`m_pitch` to -0.022 when inverted, else 0.022), the votes
    /// (`iw3mp.exe` strings `callvote map %s` 0x6eaffc, `callvote typemap
    /// %s %s` 0x6eafdc, `callvote g_gametype %s` 0x6eb01c, `callvote kick
    /// "%s"` 0x6eb1d8, `callvote tempBanUser "%s"` 0x6eb1f8), and
    /// `mutePlayer`.
    pub(super) fn ui_script(&mut self, name: &str, args: &[String], gs: &Gamestate) {
        let _ = gs;
        let map = || {
            let at = atoi(&self.dvar("ui_currentNetMap").unwrap_or_default());
            usize::try_from(at)
                .ok()
                .and_then(|m| self.lists.maps.get(m))
                .map(|a| a.map.clone())
        };
        let gametype = || {
            let at = atoi(&self.dvar("ui_netGameType").unwrap_or_default());
            usize::try_from(at)
                .ok()
                .and_then(|g| self.lists.gametypes.get(g))
                .map(|(g, _)| g.clone())
        };
        let player = || {
            self.player_index
                .and_then(|i| self.host.players.get(i))
                .map(|(n, name)| (*n, clean(name)))
        };
        let line = match name.to_ascii_lowercase().as_str() {
            "update" => {
                if args
                    .first()
                    .is_some_and(|a| a.eq_ignore_ascii_case("ui_mousePitch"))
                {
                    let inverted = atoi(&self.dvar("ui_mousePitch").unwrap_or_default()) != 0;
                    self.set_dvar("m_pitch", if inverted { "-0.022" } else { "0.022" });
                }
                None
            }
            "votemap" => map().map(|m| format!("callvote map {m}")),
            "votetypemap" => gametype()
                .zip(map())
                .map(|(g, m)| format!("callvote typemap {g} {m}")),
            "votegame" => gametype().map(|g| format!("callvote g_gametype {g}")),
            "votekick" => player().map(|(_, n)| format!("callvote kick \"{n}\"")),
            "votetempban" => player().map(|(_, n)| format!("callvote tempBanUser \"{n}\"")),
            // `CL_MutePlayer`: the toggle, and the server told.
            "muteplayer" => player().map(|(n, _)| {
                if self.muted.insert(n) {
                    format!("muteplayer {n}")
                } else {
                    self.muted.remove(&n);
                    format!("unmuteplayer {n}")
                }
            }),
            _ => {
                log::debug!("cod4 menu uiScript {name} {args:?}: not run");
                None
            }
        };
        self.console.extend(line);
    }

    /// Whether a chat line is a muted player's: its speaker, the text
    /// before the first `": "` with the colour codes and the message
    /// bytes out, ends with a muted player's cleaned name. vcod's own
    /// rule: retail's mute holds the voice back only.
    pub fn chat_muted(&self, line: &str) -> bool {
        let line = clean(line);
        let Some((speaker, _)) = line.split_once(": ") else {
            return false;
        };
        self.host.players.iter().any(|(n, name)| {
            let name = clean(name);
            self.muted.contains(n) && !name.is_empty() && speaker.ends_with(&name)
        })
    }
}

/// How many rows a list box of height `h` draws (`Item_ListBox_Paint`:
/// `h - 2` less a row each until a row no longer fits).
pub fn list_rows(lb: &ListBox, h: f32) -> usize {
    if lb.element_height <= 0.0 {
        return 0;
    }
    let mut size = h - 2.0;
    let mut n = 0;
    loop {
        n += 1;
        size -= lb.element_height;
        if lb.element_height > size {
            return n;
        }
    }
}

/// `I_CleanStr`: the colour codes and the message bytes (0x14, 0x15) out.
pub fn clean(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '^' if chars.peek().is_some_and(|n| *n != '^') => {
                chars.next();
            }
            '\u{14}' | '\u{15}' => {}
            c => out.push(c),
        }
    }
    out
}

/// C's `%g` for the menus' values: six significant digits, trailing zeros
/// dropped.
pub fn format_g(v: f32) -> String {
    if v == 0.0 || !v.is_finite() {
        return "0".into();
    }
    let digits = (v.abs() as f64).log10().floor() as i32 + 1;
    let decimals = (6 - digits).clamp(0, 6) as usize;
    let s = format!("{:.*}", decimals, v);
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_print_as_percent_g() {
        assert_eq!(format_g(5.0), "5");
        assert_eq!(format_g(0.8), "0.8");
        assert_eq!(format_g(-0.022), "-0.022");
        assert_eq!(format_g(15.5), "15.5");
        assert_eq!(format_g(65535.0), "65535");
        assert_eq!(clean("\u{15}\u{15}^9walker^7: ^7hi"), "walker: hi");
    }

    #[test]
    fn a_list_box_draws_the_rows_that_fit() {
        let lb = ListBox {
            element_height: 24.0,
            ..ListBox::default()
        };
        // 268 units: rows while one still fits after it.
        assert_eq!(list_rows(&lb, 270.0), 11);
        assert_eq!(list_rows(&lb, 300.0), 12);
    }

    /// A held thumb's centre picks the first row by its share of the
    /// track, clamped at both ends, and lands where the bar draws it.
    #[test]
    fn a_dragged_list_thumb_picks_the_row_its_centre_is_over() {
        // 250 tall: the thumb's top runs 17..217, a row every 20 units.
        let path = ThumbPath::new([0.0, 0.0, 200.0, 250.0], 10);
        let half = 0.5 * SCROLL;
        assert_eq!(path.first_under(17.0 + half), 0);
        assert_eq!(path.first_under(-50.0), 0);
        assert_eq!(path.first_under(117.0 + half), 5);
        assert_eq!(path.first_under(136.0 + half), 5);
        assert_eq!(path.first_under(217.0 + half), 10);
        assert_eq!(path.first_under(400.0), 10);
        for first in 0..10 {
            assert_eq!(path.first_under(path.thumb_at(first) + half), first);
        }
        // The last row's thumb stops two above the down arrow.
        assert_eq!(path.thumb_at(10), 250.0 - 2.0 * SCROLL - 2.0);
        // A list that fits has nowhere to go.
        assert_eq!(
            ThumbPath::new([0.0, 0.0, 200.0, 250.0], 0).first_under(200.0),
            0
        );
    }
}
