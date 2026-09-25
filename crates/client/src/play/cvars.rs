//! Client cvars set by the server's `v` command, plus the 140/204
//! configstring mirror those same names may fall back to
//! (docs/research/cod11-hud-protocol.md, section 0; `crates/server/src/cvars.rs`,
//! the "140/204 mirror").

use std::collections::HashMap;

/// Configstrings 140..204 name the mirrored server cvars, 204..268 their
/// values, paired by this offset.
const MIRROR_NAMES: std::ops::Range<usize> = 140..204;
const MIRROR_VALUE_OFFSET: usize = 64;

/// Cvars a `v` command set, keyed lowercase. A `v` wins over the 140/204
/// mirror `get` also checks.
#[derive(Default)]
pub struct ClientCvars {
    set: HashMap<String, String>,
}

impl ClientCvars {
    /// `v <name> "<value>"`; returns whether the command was a `v` this
    /// consumed.
    pub fn on_server_command(&mut self, tokens: &[String]) -> bool {
        if tokens.first().map(String::as_str) != Some("v") {
            return false;
        }
        let Some(name) = tokens.get(1) else {
            return false;
        };
        let value = tokens.get(2).cloned().unwrap_or_default();
        self.set.insert(name.to_lowercase(), value);
        true
    }

    /// A `v`-set value, else the 140/204 mirror entry under a
    /// case-insensitive name match.
    pub fn get(&self, name: &str, configstrings: &[String]) -> Option<String> {
        if let Some(v) = self.set.get(&name.to_lowercase()) {
            return Some(v.clone());
        }
        let i = MIRROR_NAMES.clone().find(|&i| {
            configstrings
                .get(i)
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })?;
        configstrings.get(i + MIRROR_VALUE_OFFSET).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v_sets_a_client_cvar() {
        let mut c = ClientCvars::default();
        assert!(c.on_server_command(&["v".into(), "scr_showweapontab".into(), "1".into()]));
        assert!(!c.on_server_command(&["t".into(), "0".into()]));
        assert_eq!(c.get("SCR_SHOWWEAPONTAB", &[]).as_deref(), Some("1"));
    }

    #[test]
    fn mirror_lookup() {
        let mut cs = vec![String::new(); 300];
        cs[167] = "scr_allow_m1carbine".into();
        cs[167 + 64] = "0".into();
        let c = ClientCvars::default();
        assert_eq!(c.get("scr_allow_m1carbine", &cs).as_deref(), Some("0"));
        assert_eq!(c.get("scr_allow_nope", &cs), None);
    }
}
