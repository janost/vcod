//! The console's command layer: retail's tokenizer, a cvar table, key binds
//! and the client commands vcod implements (docs/research/cod11-console.md).
//! Commands that act on the window or the connection come back as
//! [`Effect`]s for `main.rs` to carry out; the rest run here.

use std::collections::BTreeMap;

use vcod_common::localize::Localized;
use vcod_common::net::Userinfo;

use crate::play::input::Action;

use super::keys;

/// Something a command asks of the app, in the order the commands ran.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Print(String),
    Clear,
    Connect(String),
    Disconnect,
    Reconnect,
    Quit,
    /// A client command for the server: `say`, `say_team`, `cmd <text>`.
    Forward(String),
    /// A command the client does not know. Retail forwards it to the server
    /// while connected (`CL_ForwardCommandToServer`), except `+`/`-` ones.
    Unknown {
        word: String,
        line: String,
    },
    /// A `+button` bind's press or release.
    Button(Action, bool),
    /// A one-shot bind (`gocrouch`, `weaponslot`, `weapnext`).
    Impulse(Action),
    Scores(bool),
    MessageMode {
        team: bool,
    },
    ToggleConsole,
    /// A userinfo cvar (`name`) changed.
    Userinfo,
    /// A bind or an archived cvar changed.
    SaveConfig,
    /// `exec <file>`: run a config file from the game's search path.
    Exec(String),
    /// `vid_restart`: apply `r_mode` and `r_fullscreen` to the window.
    VidRestart,
}

/// One cvar. `archive` is retail's `CVAR_ARCHIVE`: written to the config.
/// `cheat` is `CVAR_CHEAT`: only `sv_cheats 1` lets it move off its default.
#[derive(Debug, Clone)]
pub struct Cvar {
    pub name: String,
    pub value: String,
    pub default: String,
    pub archive: bool,
    pub userinfo: bool,
    pub cheat: bool,
}

/// Retail's cvar flag bits (docs/research/cod11-console.md, section 5).
const ARCHIVE: u32 = 0x1;
const USERINFO: u32 = 0x2;
const CHEAT: u32 = 0x200;

/// The client commands, for `cmdlist` and completion.
const COMMANDS: &[&str] = &[
    "+activate",
    "+attack",
    "+back",
    "+forward",
    "+gostand",
    "+leanleft",
    "+leanright",
    "+melee",
    "+moveleft",
    "+moveright",
    "+reload",
    "+scores",
    "+speed",
    "-activate",
    "-attack",
    "-back",
    "-forward",
    "-gostand",
    "-leanleft",
    "-leanright",
    "-melee",
    "-moveleft",
    "-moveright",
    "-reload",
    "-scores",
    "-speed",
    "bind",
    "bindlist",
    "clear",
    "cmd",
    "cmdlist",
    "connect",
    "cvarlist",
    "disconnect",
    "echo",
    "exec",
    "gocrouch",
    "goprone",
    "messagemode",
    "messagemode2",
    "quit",
    "reconnect",
    "say",
    "say_team",
    "set",
    "seta",
    "setfromcvar",
    "toggle",
    "toggleconsole",
    "unbind",
    "unbindall",
    "vid_restart",
    "weapnext",
    "weaponslot",
    "weapprev",
];

/// The `+button` commands and the input each holds.
fn button(word: &str) -> Option<Action> {
    Some(match word {
        "forward" => Action::Forward,
        "back" => Action::Back,
        "moveleft" => Action::Left,
        "moveright" => Action::Right,
        "gostand" => Action::Jump,
        "speed" => Action::Ads,
        "attack" => Action::Attack,
        "melee" => Action::Melee,
        "activate" => Action::Use,
        "reload" => Action::Reload,
        "leanleft" => Action::LeanLeft,
        "leanright" => Action::LeanRight,
        _ => return None,
    })
}

/// `weaponslot`'s argument and the slot it selects.
fn weapon_slot(name: &str) -> Option<usize> {
    Some(match name.to_ascii_lowercase().as_str() {
        "primary" => 1,
        "primaryb" => 2,
        "pistol" => 3,
        "grenade" => 4,
        _ => return None,
    })
}

/// The binds `vcod` starts with when it has no config: the stock keys
/// `main.rs` hard-coded before the console existed.
pub const DEFAULT_BINDS: &[(&str, &str)] = &[
    ("TAB", "+scores"),
    ("SPACE", "+gostand"),
    ("1", "weaponslot primary"),
    ("2", "weaponslot primaryb"),
    ("3", "weaponslot pistol"),
    ("4", "weaponslot grenade"),
    ("`", "toggleconsole"),
    ("A", "+moveleft"),
    ("C", "gocrouch"),
    ("D", "+moveright"),
    ("E", "+leanright"),
    ("F", "+activate"),
    ("Q", "+leanleft"),
    ("R", "+reload"),
    ("S", "+back"),
    ("T", "messagemode"),
    ("W", "+forward"),
    ("Y", "messagemode2"),
    ("CTRL", "goprone"),
    ("SHIFT", "+melee"),
    ("MOUSE1", "+attack"),
    ("MOUSE2", "+speed"),
    ("MWHEELDOWN", "weapnext"),
    ("MWHEELUP", "weapprev"),
];

/// The archived cvars the stock options screens set beyond the ones above,
/// at the defaults CoDMP.exe and cgame register them with
/// (docs/research/cod11-front-end.md, section 14), so a choice made there
/// survives a restart. Only `mss_volume`, `r_mode` and `r_fullscreen` drive
/// anything; vcod's window starts at its own size and windowed, so `r_mode`
/// -1 and `r_fullscreen` 0 stand in for retail's 3 and 1.
const MENU_CVARS: &[(&str, &str)] = &[
    ("mss_volume", "0.8"),
    ("mss_khz", "44"),
    ("mss_3d_provider", "Miles Fast 2D Positional Audio"),
    ("r_mode", "-1"),
    ("r_fullscreen", "0"),
    ("r_picmip", "1"),
    ("r_picmip2", "2"),
    ("r_textureMode", "GL_LINEAR_MIPMAP_NEAREST"),
    ("r_texturebits", "0"),
    ("r_gamma", "1.0"),
    ("r_ignorehwgamma", "0"),
    ("r_lodscale", "1"),
    ("r_lodbias", "0"),
    ("r_dynamiclight", "1"),
    ("r_dlightQuality", "1"),
    ("r_swapInterval", "0"),
    ("r_nv_fog_dist", "1"),
    ("cl_freelook", "1"),
    ("m_filter", "0"),
    ("cg_drawCrosshair", "1"),
    ("cg_drawStatus", "1"),
    ("cg_marks", "1"),
    ("cg_brass", "1"),
    ("cg_blood", "1"),
];

pub struct Shell {
    /// Keyed by lowercase name; `Cvar::name` keeps the registered spelling.
    cvars: BTreeMap<String, Cvar>,
    /// Keyed by canonical key name ([`keys::canonical`]).
    binds: BTreeMap<String, String>,
    /// The server's `sv_cheats`; off until a systeminfo says otherwise, as
    /// CoDMP.exe registers it `"0"`.
    cheats: bool,
}

impl Default for Shell {
    fn default() -> Self {
        Self::new()
    }
}

impl Shell {
    /// The client's own cvars at their retail defaults, and [`DEFAULT_BINDS`].
    pub fn new() -> Shell {
        let mut s = Shell {
            cvars: BTreeMap::new(),
            binds: BTreeMap::new(),
            cheats: false,
        };
        s.register("name", "vcod", ARCHIVE | USERINFO);
        // CoDMP.exe registers both with these defaults
        // (docs/research/cod11-console.md, section 4).
        s.register("cl_run", "1", ARCHIVE);
        s.register("scr_conspeed", "3", 0);
        // CL_Init and cgame's cvar table (section 5).
        s.register("sensitivity", "5", ARCHIVE);
        s.register("m_yaw", "0.022", ARCHIVE);
        s.register("m_pitch", "0.022", ARCHIVE);
        // Retail's first-run rate is 5000, which starves snapshots on a busy server.
        s.register("rate", "25000", ARCHIVE | USERINFO);
        s.register("snaps", "20", ARCHIVE | USERINFO);
        s.register("cg_fov", "80", ARCHIVE | CHEAT);
        // CoDMP.exe registers `password` CVAR_USERINFO, not archived
        // (0x4123ea); the browser's password popup edits it.
        s.register("password", "", USERINFO);
        // The browser's cvars, archived, with ui_mp_x86.dll's defaults
        // (cvar table at 0x40036c8c..0x40036dfc).
        s.register("ui_netSource", "0", ARCHIVE);
        for name in [
            "ui_browserShowFull",
            "ui_browserShowEmpty",
            "ui_browserShowPassword",
            "ui_browserShowNoPassword",
        ] {
            s.register(name, "1", ARCHIVE);
        }
        for (name, value) in MENU_CVARS {
            s.register(name, value, ARCHIVE);
        }
        for (key, cmd) in DEFAULT_BINDS {
            s.binds.insert(key.to_string(), cmd.to_string());
        }
        s
    }

    fn register(&mut self, name: &str, value: &str, flags: u32) {
        self.cvars.insert(
            name.to_ascii_lowercase(),
            Cvar {
                name: name.to_string(),
                value: value.to_string(),
                default: value.to_string(),
                archive: flags & ARCHIVE != 0,
                userinfo: flags & USERINFO != 0,
                cheat: flags & CHEAT != 0,
            },
        );
    }

    /// The server's `sv_cheats` changed. Turning it off puts every cheat
    /// cvar back to its default, as `Cvar_SetCheatState` does.
    pub fn set_cheats(&mut self, on: bool) {
        self.cheats = on;
        if !on {
            for c in self.cvars.values_mut().filter(|c| c.cheat) {
                c.value.clone_from(&c.default);
            }
        }
    }

    /// The userinfo the cvars make.
    pub fn userinfo(&self) -> Userinfo {
        let get = |n: &str| self.cvar(n).unwrap_or_default().to_string();
        Userinfo {
            name: get("name"),
            rate: get("rate"),
            snaps: get("snaps"),
            password: get("password"),
        }
    }

    pub fn cvar(&self, name: &str) -> Option<&str> {
        self.cvars
            .get(&name.to_ascii_lowercase())
            .map(|c| c.value.as_str())
    }

    /// Q3's `atof`: a non-number reads 0.
    pub fn cvar_f32(&self, name: &str) -> f32 {
        self.cvar(name)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0.0)
    }

    #[cfg(test)]
    pub fn bind(&self, key: &str) -> Option<&str> {
        self.binds.get(key).map(String::as_str)
    }

    /// Every key bound to exactly `cmd` (case folded), in retail's key-number
    /// order, which is the order the options screens list them in.
    pub fn keys_bound_to(&self, cmd: &str) -> Vec<&str> {
        let mut keys: Vec<&str> = self
            .binds
            .iter()
            .filter(|(_, v)| v.eq_ignore_ascii_case(cmd))
            .map(|(k, _)| k.as_str())
            .collect();
        keys.sort_by_key(|k| keys::number(k));
        keys
    }

    /// The cgame's key text for `cmd` (0x30046940): its first key's name,
    /// or the first two joined by `KEY_OR`, names through `KEY_*`. `None`
    /// while nothing is bound to it.
    pub fn key_text(&self, cmd: &str, loc: &Localized) -> Option<String> {
        let name = |k: &str| loc.get(&format!("KEY_{k}")).unwrap_or(k).to_string();
        match self.keys_bound_to(cmd).as_slice() {
            [] => None,
            [a] => Some(name(a)),
            [a, b, ..] => Some(format!(
                "{} {} {}",
                name(a),
                loc.translate("@KEY_OR"),
                name(b)
            )),
        }
    }

    /// Runs `text`: commands split on `;` and newlines outside quotes, as
    /// `Cbuf_Execute` does.
    pub fn execute(&mut self, text: &str) -> Vec<Effect> {
        let mut out = Vec::new();
        for line in split_commands(text) {
            self.execute_one(&line, &mut out);
        }
        out
    }

    /// A bound key's edge. A press runs the bind; a release runs `-cmd` for a
    /// `+cmd` bind and nothing otherwise (CoDMP.exe 0x40dc30).
    pub fn key_event(&mut self, key: &str, down: bool) -> Vec<Effect> {
        let Some(cmd) = self.binds.get(key).cloned() else {
            return Vec::new();
        };
        if down {
            self.execute(&cmd)
        } else if let Some(rest) = cmd.strip_prefix('+') {
            self.execute(&format!("-{rest}"))
        } else {
            Vec::new()
        }
    }

    /// The bind of `key` if it holds a [`Action`] button or impulse, which is
    /// what the mouse grab gates (`main.rs`).
    pub fn bind_is_gameplay(&self, key: &str) -> bool {
        let Some(cmd) = self.binds.get(key) else {
            return false;
        };
        let tokens = tokenize(cmd);
        match tokens.first().map(String::as_str) {
            Some(w) if w.starts_with('+') => button(&w[1..]).is_some(),
            Some("gocrouch" | "goprone" | "weaponslot" | "weapnext" | "weapprev") => true,
            _ => false,
        }
    }

    fn execute_one(&mut self, line: &str, out: &mut Vec<Effect>) {
        let tokens = tokenize(line);
        let Some(word) = tokens.first() else { return };
        let word = word.to_ascii_lowercase();
        let arg = |i: usize| tokens.get(i).map(String::as_str);
        let print = |out: &mut Vec<Effect>, s: String| out.push(Effect::Print(s));

        if let Some(rest) = word.strip_prefix('+')
            && let Some(action) = button(rest)
        {
            out.push(Effect::Button(action, true));
            return;
        }
        if let Some(rest) = word.strip_prefix('-')
            && let Some(action) = button(rest)
        {
            out.push(Effect::Button(action, false));
            return;
        }

        match word.as_str() {
            "+scores" => out.push(Effect::Scores(true)),
            "-scores" => out.push(Effect::Scores(false)),
            "gocrouch" => out.push(Effect::Impulse(Action::Crouch)),
            "goprone" => out.push(Effect::Impulse(Action::Prone)),
            "weapnext" => out.push(Effect::Impulse(Action::NextWeapon)),
            "weapprev" => out.push(Effect::Impulse(Action::PrevWeapon)),
            "weaponslot" => match arg(1).and_then(weapon_slot) {
                Some(n) => out.push(Effect::Impulse(Action::Slot(n))),
                None => print(
                    out,
                    "usage: weaponslot <primary|primaryb|pistol|grenade>".into(),
                ),
            },
            "messagemode" => out.push(Effect::MessageMode { team: false }),
            "messagemode2" => out.push(Effect::MessageMode { team: true }),
            "toggleconsole" => out.push(Effect::ToggleConsole),
            "clear" => out.push(Effect::Clear),
            "echo" => print(out, args_from(line, 1)),
            "quit" => out.push(Effect::Quit),
            "disconnect" => out.push(Effect::Disconnect),
            "reconnect" => out.push(Effect::Reconnect),
            "connect" => match arg(1) {
                Some(addr) => out.push(Effect::Connect(addr.to_string())),
                None => print(out, "usage: connect [server]".into()),
            },
            "say" | "say_team" => out.push(Effect::Forward(line.trim().to_string())),
            "cmd" => out.push(Effect::Forward(args_from(line, 1))),
            "bind" => self.cmd_bind(&tokens, line, out),
            "unbind" => match arg(1) {
                None => print(out, "unbind <key> : remove commands from a key".into()),
                Some(k) => match keys::canonical(k) {
                    None => print(out, format!("\"{k}\" isn't a valid key")),
                    Some(k) => {
                        self.binds.remove(&k);
                        out.push(Effect::SaveConfig);
                    }
                },
            },
            "unbindall" => {
                self.binds.clear();
                out.push(Effect::SaveConfig);
            }
            "bindlist" => {
                for (k, v) in &self.binds {
                    print(out, format!("{k} \"{v}\""));
                }
            }
            "set" | "seta" => {
                let (Some(name), Some(_)) = (arg(1), arg(2)) else {
                    print(out, format!("usage: {word} <variable> <value>"));
                    return;
                };
                let value = args_from(line, 2);
                self.set(name, &value, word == "seta", out);
            }
            "toggle" => self.cmd_toggle(&tokens, out),
            // `setfromcvar <variable> <variablein>` (CoDMP.exe 0x43a070): a
            // missing source reads "".
            "setfromcvar" => match (arg(1), arg(2)) {
                (Some(dest), Some(src)) => {
                    let value = self.cvar(src).unwrap_or("").to_string();
                    self.set(dest, &value, false, out);
                }
                _ => print(out, "usage: setfromcvar <variable> <variablein>".into()),
            },
            "exec" => match arg(1) {
                Some(file) => out.push(Effect::Exec(file.to_string())),
                None => print(out, "exec <filename> : execute a script file".into()),
            },
            "vid_restart" => out.push(Effect::VidRestart),
            "cvarlist" => {
                let prefix = arg(1).map(str::to_ascii_lowercase);
                let mut n = 0;
                for (key, c) in &self.cvars {
                    if prefix
                        .as_ref()
                        .is_some_and(|p| !key.starts_with(p.as_str()))
                    {
                        continue;
                    }
                    let flags = if c.archive { 'A' } else { ' ' };
                    print(out, format!("{flags} {} \"{}\"", c.name, c.value));
                    n += 1;
                }
                print(out, format!("{n} total cvars"));
            }
            "cmdlist" => {
                let prefix = arg(1).map(str::to_ascii_lowercase);
                let mut n = 0;
                for c in COMMANDS {
                    if prefix.as_ref().is_some_and(|p| !c.starts_with(p.as_str())) {
                        continue;
                    }
                    print(out, c.to_string());
                    n += 1;
                }
                print(out, format!("{n} commands"));
            }
            _ => {
                // `Cvar_Command`: a bare cvar name prints it, a value sets it.
                if let Some(c) = self.cvars.get(&word) {
                    match arg(1) {
                        None => print(
                            out,
                            format!(
                                "\"{}\" is:\"{}^7\" default:\"{}^7\"",
                                c.name, c.value, c.default
                            ),
                        ),
                        Some(_) => {
                            let name = c.name.clone();
                            self.set(&name, &args_from(line, 1), false, out);
                        }
                    }
                    return;
                }
                out.push(Effect::Unknown {
                    word: tokens[0].clone(),
                    line: line.trim().to_string(),
                });
            }
        }
    }

    fn cmd_bind(&mut self, tokens: &[String], line: &str, out: &mut Vec<Effect>) {
        let Some(k) = tokens.get(1) else {
            out.push(Effect::Print(
                "bind <key> [command] : attach a command to a key".into(),
            ));
            return;
        };
        let Some(key) = keys::canonical(k) else {
            out.push(Effect::Print(format!("\"{k}\" isn't a valid key")));
            return;
        };
        if tokens.len() == 2 {
            out.push(Effect::Print(match self.binds.get(&key) {
                Some(cmd) => format!("\"{k}\" = \"{cmd}\""),
                None => format!("\"{k}\" is not bound"),
            }));
            return;
        }
        // `bind k "a; b"` keeps the quoted text whole; `bind k +attack` too.
        let cmd = if tokens.len() == 3 {
            tokens[2].clone()
        } else {
            args_from(line, 2)
        };
        self.binds.insert(key, cmd);
        out.push(Effect::SaveConfig);
    }

    /// `toggle <var>` flips 0/1; `toggle <var> a b c` steps through the list.
    fn cmd_toggle(&mut self, tokens: &[String], out: &mut Vec<Effect>) {
        let Some(name) = tokens.get(1) else {
            out.push(Effect::Print(
                "usage: toggle <variable> <optional value sequence>".into(),
            ));
            return;
        };
        let current = self.cvar(name).unwrap_or("").to_string();
        let next = if tokens.len() <= 3 {
            if current.trim().parse::<f32>().unwrap_or(0.0) != 0.0 {
                "0".to_string()
            } else {
                "1".to_string()
            }
        } else {
            let seq = &tokens[2..];
            match seq.iter().position(|v| *v == current) {
                Some(i) => seq[(i + 1) % seq.len()].clone(),
                None => seq[0].clone(),
            }
        };
        self.set(name, &next, false, out);
    }

    /// `set` creates a cvar the client does not know, as retail does;
    /// `seta` marks it archived as well.
    fn set(&mut self, name: &str, value: &str, archive: bool, out: &mut Vec<Effect>) {
        let key = name.to_ascii_lowercase();
        let c = self.cvars.entry(key).or_insert_with(|| Cvar {
            name: name.to_string(),
            value: String::new(),
            default: value.to_string(),
            archive: false,
            userinfo: false,
            cheat: false,
        });
        if c.cheat && !self.cheats && c.value != value {
            out.push(Effect::Print(format!("{} is cheat protected.", c.name)));
            return;
        }
        let changed = c.value != value;
        let newly_archived = archive && !c.archive;
        c.value = value.to_string();
        c.archive |= archive;
        if changed && c.userinfo {
            out.push(Effect::Userinfo);
        }
        if (changed || newly_archived) && c.archive {
            out.push(Effect::SaveConfig);
        }
    }

    /// Command and cvar names starting with `prefix`, case-insensitively,
    /// sorted.
    pub fn complete(&self, prefix: &str) -> Vec<String> {
        let p = prefix.to_ascii_lowercase();
        let mut names: Vec<String> = COMMANDS
            .iter()
            .filter(|c| c.starts_with(&p))
            .map(|c| c.to_string())
            .chain(
                self.cvars
                    .iter()
                    .filter(|(k, _)| k.starts_with(&p))
                    .map(|(_, c)| c.name.clone()),
            )
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// The config file text, in the shape CoDMP.exe writes `config_mp.cfg`:
    /// `unbindall`, every bind, then every archived cvar as `seta`.
    pub fn config_text(&self) -> String {
        let mut s = String::from("// generated by vcod, do not modify\nunbindall\n");
        for (k, v) in &self.binds {
            s.push_str(&format!("bind {k} \"{v}\"\n"));
        }
        for c in self.cvars.values().filter(|c| c.archive) {
            s.push_str(&format!("seta {} \"{}\"\n", c.name, c.value));
        }
        s
    }
}

/// `Cmd_TokenizeString`: whitespace splits, a quoted run is one token
/// (quotes dropped), `//` ends the line.
pub fn tokenize(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        while i < b.len() && b[i] <= b' ' {
            i += 1;
        }
        if i >= b.len() || b[i..].starts_with(b"//") {
            return out;
        }
        let start;
        if b[i] == b'"' {
            start = i + 1;
            i = start;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            out.push(line[start..i].to_string());
            i += 1;
        } else {
            start = i;
            while i < b.len() && b[i] > b' ' && b[i] != b'"' && !b[i..].starts_with(b"//") {
                i += 1;
            }
            out.push(line[start..i].to_string());
        }
    }
}

/// The raw text from token `n` on, quotes kept, as `Cmd_ArgsFrom` gives it.
/// Leading and trailing whitespace go.
fn args_from(line: &str, n: usize) -> String {
    let b = line.as_bytes();
    let mut i = 0;
    for _ in 0..n {
        while i < b.len() && b[i] <= b' ' {
            i += 1;
        }
        if i < b.len() && b[i] == b'"' {
            i += 1;
            while i < b.len() && b[i] != b'"' {
                i += 1;
            }
            i += 1;
        } else {
            while i < b.len() && b[i] > b' ' && b[i] != b'"' {
                i += 1;
            }
        }
    }
    let rest = line.get(i.min(line.len())..).unwrap_or("").trim();
    // A lone quoted argument comes back without its quotes.
    if n > 0 {
        let toks = tokenize(rest);
        if toks.len() == 1 && rest.starts_with('"') {
            return toks[0].clone();
        }
    }
    rest.to_string()
}

/// Splits on `;` and newlines outside quotes, as `Cbuf_Execute` does.
pub fn split_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ';' if !quoted => out.push(std::mem::take(&mut cur)),
            '\n' | '\r' => {
                quoted = false;
                out.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.retain(|l| !l.trim().is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_groups_quotes_and_drops_comments() {
        assert_eq!(
            tokenize("bind  K \"say hi there\" // note"),
            ["bind", "K", "say hi there"]
        );
        assert_eq!(tokenize("a\"b c\"d"), ["a", "b c", "d"]);
        assert!(tokenize("   // all comment").is_empty());
    }

    #[test]
    fn semicolons_split_outside_quotes() {
        assert_eq!(
            split_commands("bind k \"a; b\"; echo x\nquit"),
            ["bind k \"a; b\"", " echo x", "quit"]
        );
    }

    #[test]
    fn bind_and_key_events() {
        let mut s = Shell::new();
        assert_eq!(s.execute("bind k \"+attack\""), [Effect::SaveConfig]);
        assert_eq!(s.bind("K"), Some("+attack"));
        assert_eq!(
            s.key_event("K", true),
            [Effect::Button(Action::Attack, true)]
        );
        assert_eq!(
            s.key_event("K", false),
            [Effect::Button(Action::Attack, false)]
        );
        // A one-shot bind fires on the press only.
        s.execute("bind g weaponslot pistol");
        assert_eq!(s.key_event("G", true), [Effect::Impulse(Action::Slot(3))]);
        assert!(s.key_event("G", false).is_empty());
        assert!(s.bind_is_gameplay("G"));
        assert!(!s.bind_is_gameplay("T"));
    }

    #[test]
    fn bind_queries_and_bad_keys() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("bind w"),
            [Effect::Print("\"w\" = \"+forward\"".into())]
        );
        assert_eq!(
            s.execute("bind nosuchkey quit"),
            [Effect::Print("\"nosuchkey\" isn't a valid key".into())]
        );
        s.execute("unbind w");
        assert_eq!(
            s.execute("bind w"),
            [Effect::Print("\"w\" is not bound".into())]
        );
    }

    #[test]
    fn multi_command_binds_run_every_command() {
        let mut s = Shell::new();
        s.execute("bind h \"say hi; weapnext\"");
        assert_eq!(
            s.key_event("H", true),
            [
                Effect::Forward("say hi".into()),
                Effect::Impulse(Action::NextWeapon)
            ]
        );
    }

    #[test]
    fn cvars_set_print_and_toggle() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("cl_run"),
            [Effect::Print(
                "\"cl_run\" is:\"1^7\" default:\"1^7\"".into()
            )]
        );
        assert_eq!(s.execute("toggle cl_run"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("CL_RUN"), Some("0"));
        assert_eq!(
            s.execute("name \"Big Bob\""),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(s.cvar("name"), Some("Big Bob"));
        // A user cvar: `set` keeps it out of the config, `seta` writes it.
        assert!(s.execute("set foo bar").is_empty());
        assert!(!s.config_text().contains("foo"));
        s.execute("seta foo baz");
        assert!(s.config_text().contains("seta foo \"baz\"\n"));
        s.execute("toggle foo a b c");
        assert_eq!(s.cvar("foo"), Some("a"));
        s.execute("toggle foo a b c");
        assert_eq!(s.cvar("foo"), Some("b"));
    }

    #[test]
    fn unknown_and_forwarded_commands() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("say hello world"),
            [Effect::Forward("say hello world".into())]
        );
        assert_eq!(
            s.execute("callvote map mp_harbor"),
            [Effect::Unknown {
                word: "callvote".into(),
                line: "callvote map mp_harbor".into()
            }]
        );
        assert_eq!(s.execute("cmd score"), [Effect::Forward("score".into())]);
    }

    #[test]
    fn config_round_trips() {
        let mut s = Shell::new();
        s.execute("unbindall; bind mouse3 +attack; bind f1 \"vote yes\"; name Rob");
        let text = s.config_text();
        let mut t = Shell::new();
        t.execute(&text);
        assert_eq!(t.config_text(), text);
        assert_eq!(t.bind("F1"), Some("vote yes"));
        assert_eq!(t.bind("W"), None);
        assert_eq!(t.cvar("name"), Some("Rob"));
    }

    #[test]
    fn completion_covers_commands_and_cvars() {
        let s = Shell::new();
        assert_eq!(s.complete("unb"), ["unbind", "unbindall"]);
        assert_eq!(s.complete("CL_"), ["cl_freelook", "cl_run"]);
        assert!(s.complete("zzz").is_empty());
    }

    #[test]
    fn setfromcvar_exec_and_keys_in_key_order() {
        let mut s = Shell::new();
        s.execute("setfromcvar ui_name name; setfromcvar ui_x nosuchcvar");
        assert_eq!(s.cvar("ui_name"), Some("vcod"));
        assert_eq!(s.cvar("ui_x"), Some(""));
        assert_eq!(
            s.execute("exec default_mp.cfg; vid_restart"),
            [Effect::Exec("default_mp.cfg".into()), Effect::VidRestart]
        );
        // Space (32) before letters, letters before named keys, mouse last.
        s.execute("bind MOUSE2 +forward; bind UPARROW +forward; bind SPACE +forward");
        assert_eq!(
            s.keys_bound_to("+FORWARD"),
            ["SPACE", "W", "UPARROW", "MOUSE2"]
        );
    }

    #[test]
    fn key_text_names_the_first_two_keys() {
        let mut loc = Localized::default();
        loc.parse_into(
            "key",
            "REFERENCE SPACE\nLANG_ENGLISH \"Space\"\nREFERENCE OR\nLANG_ENGLISH \"or\"\n",
        );
        let mut s = Shell::new();
        assert_eq!(s.key_text("+gostand", &loc).as_deref(), Some("Space"));
        assert_eq!(s.key_text("toggleprone", &loc), None);
        s.execute("bind Z +gostand; bind X +gostand");
        assert_eq!(s.key_text("+gostand", &loc).as_deref(), Some("Space or X"));
    }

    #[test]
    fn client_cvars_start_at_defaults() {
        let s = Shell::new();
        for (name, v) in [
            ("sensitivity", "5"),
            ("m_yaw", "0.022"),
            ("m_pitch", "0.022"),
            ("rate", "25000"),
            ("snaps", "20"),
            ("cg_fov", "80"),
        ] {
            assert_eq!(s.cvar(name), Some(v), "{name}");
        }
        assert_eq!(
            s.userinfo(),
            Userinfo {
                name: "vcod".into(),
                rate: "25000".into(),
                snaps: "20".into(),
                password: String::new(),
            }
        );
    }

    #[test]
    fn rate_and_snaps_are_userinfo() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("rate 15000"),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(
            s.execute("seta snaps 30"),
            [Effect::Userinfo, Effect::SaveConfig]
        );
        assert_eq!(s.userinfo().rate, "15000");
        assert_eq!(s.userinfo().snaps, "30");
        assert_eq!(s.execute("sensitivity 3"), [Effect::SaveConfig]);
    }

    #[test]
    fn cg_fov_is_cheat_protected() {
        let mut s = Shell::new();
        assert_eq!(
            s.execute("cg_fov 95"),
            [Effect::Print("cg_fov is cheat protected.".into())]
        );
        assert_eq!(s.cvar("cg_fov"), Some("80"));
        s.set_cheats(true);
        assert_eq!(s.execute("cg_fov 95"), [Effect::SaveConfig]);
        assert_eq!(s.cvar("cg_fov"), Some("95"));
        s.set_cheats(false);
        assert_eq!(s.cvar("cg_fov"), Some("80"), "sv_cheats 0 resets it");
    }
}
