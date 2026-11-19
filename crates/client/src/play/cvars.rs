//! Client cvars the server sets with `v`, `CG_MapRestart` and the 140/204
//! configstring mirror, all of which the cgame writes into the console's
//! cvar store, where the last write wins
//! (docs/research/cod11-hud-protocol.md, section 0; `crates/server/src/cvars.rs`,
//! the "140/204 mirror").

use std::collections::{HashMap, HashSet};

use crate::console::shell::{Effect, Shell};
use vcod_common::net::protocol::{CS_LAYOUT_V1, CsLayout};

/// Retail's cvar table size: `Cvar_Get` stops at 1024 (CoDMP.exe 0x4394bb,
/// `MAX_CVARS` 0x562368). The server makes at most this many new names.
const MAX_CVARS: usize = 1024;

/// The names the server set this connection, keyed lowercase; their
/// values are the console's. A name not yet written reads the 140/204
/// mirror, which CoD2's menus look through.
pub struct ClientCvars {
    set: HashSet<String>,
    /// Where the mirror sits: configstrings 140..=203 name the cvars and
    /// 204..=267 hold their values on 1.1, United Offensive's 149 and 277
    /// blocks hold 128 each (`CsLayout`).
    layout: CsLayout,
}

impl Default for ClientCvars {
    fn default() -> Self {
        Self {
            set: HashSet::new(),
            layout: CS_LAYOUT_V1,
        }
    }
}

impl ClientCvars {
    /// The connection's configstring layout.
    pub fn set_layout(&mut self, layout: CsLayout) {
        self.layout = layout;
    }

    /// The mirror's name slots and the offset to each one's value.
    fn mirror(&self) -> (std::ops::RangeInclusive<usize>, usize) {
        let (first, last) = self.layout.cvar_names;
        (first..=last, self.layout.cvar_values - first)
    }

    /// `trap_Cvar_Set` from the cgame: CoDMP.exe's trap 9 (0x402331) is
    /// `Cvar_Set2` with `force` (`Shell::force`), so a server-set cvar
    /// lands in the console's store whatever its flags. Returns what the
    /// set asks of the caller (a userinfo push, a config save).
    /// A name the console lacks, once the server set [`MAX_CVARS`] names,
    /// is dropped: retail's table would be full (it errors), and the store
    /// stays bounded.
    fn server_set(&mut self, shell: &mut Shell, name: &str, value: &str) -> Vec<Effect> {
        let key = name.to_lowercase();
        if !self.set.contains(&key) && self.set.len() >= MAX_CVARS && shell.cvar(name).is_none() {
            log::warn!("cvars: the server's cvar {name:?} is past {MAX_CVARS}, dropped");
            return Vec::new();
        }
        self.set.insert(key);
        shell.force(name, value)
    }

    /// `v <name> "<value>"` (cgame 0x30030550, UO 0x3003d570): the cgame
    /// sets the cvar. `None` when the command is not a `v`.
    pub fn on_server_command(
        &mut self,
        tokens: &[String],
        shell: &mut Shell,
    ) -> Option<Vec<Effect>> {
        if tokens.first().map(String::as_str) != Some("v") {
            return None;
        }
        let name = tokens.get(1)?;
        let value = tokens.get(2).map_or("", String::as_str);
        Some(self.server_set(shell, name, value))
    }

    /// `CG_MapRestart`'s `trap_Cvar_Set`s (1.1 cgame 0x3002ca60, uo
    /// 0x30039500; events doc 9.8): the objective text empties, the
    /// view goes back to first person and the run state to running.
    /// `cl_stance`'s reset is the caller's: vcod keeps the stance in its
    /// input, not a cvar.
    pub fn on_map_restart(&mut self, shell: &mut Shell) -> Vec<Effect> {
        let mut out = Vec::new();
        for (name, value) in [
            ("cg_thirdperson", "0"),
            ("cl_run", "1"),
            ("cg_objectivetext", ""),
        ] {
            out.extend(self.server_set(shell, name, value));
        }
        out
    }

    /// The cgame's mirror writer (1.1 0x3002bc60, 1.5 0x3002d0f0, UO
    /// 0x30038430): every pair set into the console's store through trap 9,
    /// as a `v` is. `CG_Init` runs it (1.1 0x30023106, 1.5 0x300244a6, UO
    /// 0x3002e0a6), and so does every change to a slot of either block
    /// ([`Self::in_mirror`]), all pairs again. A console `set` between two
    /// writes holds until the next.
    pub fn apply_mirror(&mut self, configstrings: &[String], shell: &mut Shell) -> Vec<Effect> {
        let pairs: Vec<(String, String)> = self
            .mirrored(configstrings)
            .map(|(n, v)| (n.to_owned(), v.to_owned()))
            .collect();
        let mut out = Vec::new();
        for (name, value) in pairs {
            out.extend(self.server_set(shell, &name, &value));
        }
        out
    }

    /// Whether configstring `index` is a mirror name or value slot, whose
    /// change reruns the writer (1.1 and 1.5 140..=267, cgame 0x3002c72a;
    /// UO 149..=404, 0x30038f35).
    pub fn in_mirror(&self, index: usize) -> bool {
        let (names, offset) = self.mirror();
        (*names.start()..=*names.end() + offset).contains(&index)
    }

    /// The mirror's `(name, value)` pairs as the cgame sets them (1.1
    /// 0x3002bc60, UO 0x30038430): in slot order, stopping at the first
    /// empty name, so a name past a hole is never set.
    fn mirrored<'a>(
        &self,
        configstrings: &'a [String],
    ) -> impl Iterator<Item = (&'a str, &'a str)> {
        let (names, offset) = self.mirror();
        names
            .map(move |i| {
                let name = configstrings.get(i).map_or("", String::as_str);
                let value = configstrings.get(i + offset).map_or("", String::as_str);
                (name, value)
            })
            .take_while(|(name, _)| !name.is_empty())
    }

    /// A server-set cvar's console value, else the 140/204 mirror entry
    /// under a case-insensitive name match, else the console's. Once
    /// [`Self::apply_mirror`] ran every mirrored name is server-set.
    pub fn get(&self, name: &str, configstrings: &[String], shell: &Shell) -> Option<String> {
        let key = name.to_lowercase();
        if !self.set.contains(&key)
            && let Some((_, v)) = self
                .mirrored(configstrings)
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            return Some(v.to_owned());
        }
        shell.cvar(name).map(str::to_owned)
    }

    /// The mirror's values with the server-set cvars' console values over
    /// them, keyed lowercase. The UI falls back to the console for the
    /// rest.
    pub fn all(&self, configstrings: &[String], shell: &Shell) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = self
            .mirrored(configstrings)
            .map(|(n, v)| (n.to_lowercase(), v.to_owned()))
            .collect();
        out.extend(
            self.set
                .iter()
                .filter_map(|k| Some((k.clone(), shell.cvar(k)?.to_owned()))),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<String> {
        s.split(' ').map(str::to_string).collect()
    }

    /// A server making cvar after cvar stops at retail's table size; the
    /// names it made, and the console's own, still take a `v` (SEC-111).
    #[test]
    fn the_server_makes_at_most_max_cvars_names() {
        let mut c = ClientCvars::default();
        let mut shell = Shell::new();
        for i in 0..MAX_CVARS + 50 {
            c.on_server_command(&toks(&format!("v junk{i} 1")), &mut shell);
        }
        assert_eq!(shell.cvar(&format!("junk{}", MAX_CVARS - 1)), Some("1"));
        assert_eq!(shell.cvar(&format!("junk{MAX_CVARS}")), None);
        c.on_server_command(&toks("v junk0 2"), &mut shell);
        c.on_server_command(&toks("v cg_drawGun 0"), &mut shell);
        assert_eq!(shell.cvar("junk0"), Some("2"));
        assert_eq!(shell.cvar("cg_drawGun"), Some("0"));
    }

    #[test]
    fn v_sets_the_console_cvar() {
        let mut c = ClientCvars::default();
        let mut shell = Shell::new();
        assert_eq!(
            c.on_server_command(&toks("v scr_showweapontab 1"), &mut shell),
            Some(vec![])
        );
        assert_eq!(c.on_server_command(&toks("t 0"), &mut shell), None);
        assert_eq!(shell.cvar("SCR_SHOWWEAPONTAB"), Some("1"));
        assert_eq!(
            c.get("SCR_SHOWWEAPONTAB", &[], &shell).as_deref(),
            Some("1")
        );
    }

    /// The chat cvars the HUD reads off the console, and `vc_widescreen`
    /// through the HUD's lookup, both take a `v`; an archived one asks
    /// for a save, as `cvar_modifiedFlags` does.
    #[test]
    fn v_reaches_the_cvars_the_client_reads() {
        let mut c = ClientCvars::default();
        let mut shell = Shell::new();
        assert_eq!(c.get("vc_widescreen", &[], &shell).as_deref(), Some("1"));
        let out = c.on_server_command(&toks("v cg_chatHeight 2"), &mut shell);
        assert_eq!(out, Some(vec![Effect::SaveConfig]));
        assert_eq!(shell.cvar("cg_chatheight"), Some("2"));
        c.on_server_command(&toks("v cg_teamChatsOnly 1"), &mut shell);
        assert_eq!(shell.cvar("cg_teamChatsOnly"), Some("1"));
        // Forced: a read-only cvar takes it too.
        c.on_server_command(&toks("v vc_vanilla 1"), &mut shell);
        assert_eq!(shell.cvar("vc_vanilla"), Some("1"));
        // A console set after the `v` is what the lookup reads.
        shell.execute("set scr_showweapontab 0");
        c.on_server_command(&toks("v scr_showweapontab 1"), &mut shell);
        shell.execute("set scr_showweapontab 2");
        assert_eq!(
            c.get("scr_showweapontab", &[], &shell).as_deref(),
            Some("2")
        );
    }

    #[test]
    fn v_of_a_userinfo_cvar_asks_for_a_userinfo_push() {
        let mut c = ClientCvars::default();
        let mut shell = Shell::new();
        let out = c
            .on_server_command(&toks("v rate 5000"), &mut shell)
            .unwrap();
        assert!(out.contains(&Effect::Userinfo), "{out:?}");
        assert_eq!(shell.userinfo().rate, "5000");
    }

    #[test]
    fn map_restart_resets_through_the_console() {
        let mut c = ClientCvars::default();
        let mut shell = Shell::new();
        shell.execute("set cl_run 0");
        c.on_server_command(&toks("v cg_objectiveText DM_KILL"), &mut shell);
        c.on_map_restart(&mut shell);
        assert_eq!(shell.cvar("cl_run"), Some("1"));
        assert_eq!(c.get("cg_objectiveText", &[], &shell).as_deref(), Some(""));
        assert_eq!(shell.cvar("cg_thirdperson"), Some("0"));
    }

    #[test]
    fn mirror_lookup() {
        let mut cs = vec![String::new(); 300];
        for (i, name) in cs[140..167].iter_mut().enumerate() {
            *name = format!("scr_pad{i}");
        }
        cs[167] = "scr_allow_m1carbine".into();
        cs[167 + 64] = "0".into();
        let mut shell = Shell::new();
        let mut c = ClientCvars::default();
        assert_eq!(
            c.get("scr_allow_m1carbine", &cs, &shell).as_deref(),
            Some("0")
        );
        assert_eq!(c.get("scr_allow_nope", &cs, &shell), None);
        c.on_server_command(&toks("v SCR_allow_m1carbine 1"), &mut shell);
        assert_eq!(c.all(&cs, &shell)["scr_allow_m1carbine"], "1");
        assert_eq!(
            c.get("scr_allow_m1carbine", &cs, &shell).as_deref(),
            Some("1")
        );
    }

    /// The mirror goes into the console's store, where the last write
    /// wins: a console `set` after it, a mirror change after a `v`.
    #[test]
    fn the_mirror_and_the_console_share_one_store() {
        let mut cs = vec![String::new(); 300];
        cs[140] = "scr_friendlyfire".into();
        cs[204] = "1".into();
        let mut shell = Shell::new();
        let mut c = ClientCvars::default();
        c.apply_mirror(&cs, &mut shell);
        assert_eq!(shell.cvar("scr_friendlyfire"), Some("1"));
        shell.execute("set scr_friendlyfire 2");
        assert_eq!(c.get("scr_friendlyfire", &cs, &shell).as_deref(), Some("2"));
        c.on_server_command(&toks("v scr_friendlyfire 3"), &mut shell);
        cs[204] = "0".into();
        assert!(c.in_mirror(204) && c.in_mirror(140) && c.in_mirror(267));
        assert!(!c.in_mirror(139) && !c.in_mirror(268));
        c.apply_mirror(&cs, &mut shell);
        assert_eq!(c.get("scr_friendlyfire", &cs, &shell).as_deref(), Some("0"));
        assert_eq!(c.all(&cs, &shell)["scr_friendlyfire"], "0");
    }

    /// The cgame stops at the first empty name: a name past a hole is
    /// never mirrored.
    #[test]
    fn mirror_stops_at_the_first_empty_name() {
        let mut cs = vec![String::new(); 300];
        cs[140] = "scr_first".into();
        cs[204] = "1".into();
        cs[142] = "scr_past_hole".into();
        cs[206] = "2".into();
        let shell = Shell::new();
        let c = ClientCvars::default();
        assert_eq!(c.get("scr_first", &cs, &shell).as_deref(), Some("1"));
        assert_eq!(c.get("scr_past_hole", &cs, &shell), None);
        let all = c.all(&cs, &shell);
        assert_eq!(all.len(), 1, "{all:?}");
    }
}
