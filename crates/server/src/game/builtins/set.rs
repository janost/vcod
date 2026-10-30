//! The builtins one game version answers (design step 8). Retail compiles a
//! call to a name its tables lack as `unknown (builtin) function '%s'` or
//! `unknown (builtin) method '%s'` (1.1d `cod_lnxded` strings 0x80d6a40 and
//! 0x80d6a60, raised by the call emitters 0x8094be4 and 0x8095154), a script
//! compile error that drops the level (0x809bb04 calls `Com_Error` with 1):
//! VERIFIED, read off the binary. So a version's set is its retail tables,
//! each name carrying vcod's implementation where there is one, and
//! `ScriptRuntime::load_from` refuses a closure that calls outside them.
//!
//! The name lists are `tools/re/dump_builtins.py` over the three game modules
//! (cod11-gsc-object-model.md section 9 has the addresses): VERIFIED. 1.5b
//! drops no 1.1d name and UO 1.51b none of 1.5b's, so each later list holds
//! only what that version adds.

use std::collections::HashMap;
use std::sync::OnceLock;

use vcod_common::net::protocol::Protocol;
use vcod_gsc::Vm;
use vcod_gsc::bytecode::Op;

use super::score::Builtin;

/// One version's builtins by folded name: whether retail lists it as a free
/// function, as a method (any of the per-class tables), and vcod's handler.
/// A name retail lists that vcod has no handler for compiles and fails when
/// a thread reaches it, as before.
pub struct BuiltinSet {
    names: HashMap<&'static str, Entry>,
}

#[derive(Clone, Copy)]
struct Entry {
    function: bool,
    method: bool,
    imp: Option<Builtin>,
}

impl BuiltinSet {
    /// The set for `proto`'s game version, built on first use.
    pub fn of(proto: &Protocol) -> &'static BuiltinSet {
        if proto.is_uo() {
            Self::uo()
        } else if proto.is_v1() {
            Self::v11()
        } else {
            Self::v15()
        }
    }

    /// 1.1's set.
    pub fn v11() -> &'static BuiltinSet {
        static V1: OnceLock<BuiltinSet> = OnceLock::new();
        V1.get_or_init(|| Self::build(&[FUNCTIONS_11], &[METHODS_11]))
    }

    /// 1.5's: 1.1's names and what 1.5b adds.
    pub fn v15() -> &'static BuiltinSet {
        static V15: OnceLock<BuiltinSet> = OnceLock::new();
        V15.get_or_init(|| Self::build(&[FUNCTIONS_11, FUNCTIONS_15], &[METHODS_11, METHODS_15]))
    }

    /// United Offensive 1.51's: 1.5's names and what UO adds.
    pub fn uo() -> &'static BuiltinSet {
        static UO: OnceLock<BuiltinSet> = OnceLock::new();
        UO.get_or_init(|| {
            Self::build(
                &[FUNCTIONS_11, FUNCTIONS_15, FUNCTIONS_UO],
                &[METHODS_11, METHODS_15, METHODS_UO],
            )
        })
    }

    /// CoD2 1.3's set: `cod2_lnxded`'s own tables, which drop four 1.1
    /// names and add 82 (cod2-gsc-diff.md 1.2), so it builds on no CoD1
    /// list.
    pub fn cod2() -> &'static BuiltinSet {
        static COD2: OnceLock<BuiltinSet> = OnceLock::new();
        COD2.get_or_init(|| {
            let mut set = Self::build(&[FUNCTIONS_COD2], &[METHODS_COD2]);
            for &(n, imp) in super::cod2::NAMES
                .iter()
                .chain(super::cod2_fx::NAMES)
                .chain(super::cod2_trigger::NAMES)
            {
                let e = set
                    .names
                    .get_mut(n)
                    .expect("a CoD2 handler for a name CoD2 lists");
                e.imp = Some(imp);
            }
            set
        })
    }

    /// CoD4 1.7's set: `cod4_lnxded`'s six tables (`cod4_names`, generated
    /// by `tools/re/cod4_builtins.py`; cod4-gsc.md 2), the shared handlers
    /// bound by name and CoD4's own (`builtins::cod4`) over them.
    pub fn cod4() -> &'static BuiltinSet {
        use super::cod4_names as t;
        static COD4: OnceLock<BuiltinSet> = OnceLock::new();
        COD4.get_or_init(|| {
            let names = |rows: &[&[(&'static str, bool)]]| -> Vec<&'static str> {
                rows.iter()
                    .flat_map(|r| r.iter().map(|&(n, _)| n))
                    .collect()
            };
            let functions = names(&[t::FUNCTIONS]);
            let methods = names(&[
                t::PLAYER_METHODS,
                t::SCRIPTENT_METHODS,
                t::VEHICLE_METHODS,
                t::ENTITY_METHODS,
                t::HUDELEM_METHODS,
            ]);
            let mut set = Self::build(&[&functions], &[&methods]);
            for &(n, imp) in super::cod4::NAMES
                .iter()
                .chain(super::cod4_client::NAMES)
                .chain(super::cod4_combat::NAMES)
                .chain(super::cod4_script::NAMES)
                .chain(super::cod4_vehicle::NAMES)
                .chain(super::cod4_trace::NAMES)
            {
                let e = set
                    .names
                    .get_mut(n)
                    .expect("a CoD4 handler for a name CoD4 lists");
                e.imp = Some(imp);
            }
            set
        })
    }

    /// Every name this set lists, function or method, for
    /// `Loader::set_builtins`.
    pub fn table(&'static self) -> vcod_gsc::load::BuiltinTable {
        Box::new(move |n| self.names.contains_key(n))
    }

    fn build(functions: &[&[&'static str]], methods: &[&[&'static str]]) -> BuiltinSet {
        let imps: HashMap<&'static str, Builtin> = implementations().collect();
        let mut names: HashMap<&'static str, Entry> = HashMap::new();
        let mut add = |n: &'static str, method: bool| {
            let e = names.entry(n).or_insert(Entry {
                function: false,
                method: false,
                imp: imps.get(n).copied(),
            });
            if method {
                e.method = true;
            } else {
                e.function = true;
            }
        };
        for &n in functions.iter().flat_map(|t| t.iter()) {
            add(n, false);
        }
        for &n in methods.iter().flat_map(|t| t.iter()) {
            add(n, true);
        }
        BuiltinSet { names }
    }

    /// vcod's handler for a folded name, if this version lists it.
    pub fn get(&self, folded: &str) -> Option<Builtin> {
        self.names.get(folded).and_then(|e| e.imp)
    }

    /// Whether this version lists the name, as a method when the call has a
    /// receiver and as a function when it has none.
    pub fn knows(&self, folded: &str, method: bool) -> bool {
        self.names
            .get(folded)
            .is_some_and(|e| if method { e.method } else { e.function })
    }

    /// Every builtin call in `vm`'s closure this version's compiler refuses,
    /// as retail words it (`function 'x'`, `method 'x'`), sorted.
    pub fn unknown_calls(&self, vm: &Vm) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in vm.functions() {
            for op in &f.code {
                if let Op::CallBuiltin { name, has_recv, .. } = *op {
                    let n = vm.interner().resolve_folded(name);
                    if !self.knows(n, has_recv) {
                        let kind = if has_recv { "method" } else { "function" };
                        out.push(format!("{kind} '{n}'"));
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// The names this version lists that vcod has no handler for, sorted.
    pub fn unimplemented_calls(&self, vm: &Vm) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in vm.functions() {
            for op in &f.code {
                if let Op::CallBuiltin { name, .. } = *op {
                    let n = vm.interner().resolve_folded(name);
                    if self.names.get(n).is_some_and(|e| e.imp.is_none()) {
                        out.push(n.to_string());
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Every handler vcod has, across all versions: the family tables and the
/// host's own. A name appears once (`implementations_are_unique`).
pub fn implementations() -> impl Iterator<Item = (&'static str, Builtin)> {
    use super::*;
    [
        entity::NAMES,
        math::NAMES,
        fx::NAMES,
        hud::NAMES,
        sound::NAMES,
        attach::NAMES,
        client::NAMES,
        combat::NAMES,
        mover::NAMES,
        cvar::NAMES,
        precache::NAMES,
        score::NAMES,
        objective::NAMES,
        uo::NAMES,
        vehicle::NAMES,
        crate::game::host::INLINE,
    ]
    .into_iter()
    .flat_map(|t| t.iter().copied())
}

/// 1.1d `functions` (0x7e508, `Scr_GetFunction` 0x5c15c), 106 rows.
const FUNCTIONS_11: &[&str] = &[
    "print",
    "println",
    "iprintln",
    "iprintlnbold",
    "print3d",
    "line",
    "getent",
    "getentarray",
    "spawn",
    "spawnstruct",
    "assert",
    "isdefined",
    "isalive",
    "getcvar",
    "getcvarint",
    "getcvarfloat",
    "setcvar",
    "gettime",
    "getentbynum",
    "getweaponmodel",
    "getweaponclassname",
    "getanimlength",
    "animhasnotetrack",
    "getbrushmodelcenter",
    "objective_add",
    "objective_delete",
    "objective_state",
    "objective_icon",
    "objective_position",
    "objective_onentity",
    "objective_current",
    "bullettrace",
    "getmovedelta",
    "getangledelta",
    "randomint",
    "randomfloat",
    "randomintrange",
    "randomfloatrange",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "distance",
    "distancesquared",
    "length",
    "lengthsquared",
    "closer",
    "vectordot",
    "vectornormalize",
    "vectortoangles",
    "anglestoup",
    "anglestoright",
    "anglestoforward",
    "musicplay",
    "musicstop",
    "soundfade",
    "ambientplay",
    "ambientstop",
    "precachemodel",
    "precacheshellshock",
    "precacheitem",
    "precacheshader",
    "precachestring",
    "loadfx",
    "playfx",
    "playfxontag",
    "playloopedfx",
    "setcullfog",
    "setexpfog",
    "grenadeexplosioneffect",
    "radiusdamage",
    "setplayerignoreradiusdamage",
    "getnumparts",
    "getpartname",
    "earthquake",
    "newhudelem",
    "newclienthudelem",
    "newteamhudelem",
    "resettimeout",
    "isplayer",
    "isplayernumber",
    "setwinningplayer",
    "setwinningteam",
    "announcement",
    "clientannouncement",
    "getteamscore",
    "setteamscore",
    "setclientnamemode",
    "updateclientnames",
    "getteamplayersalive",
    "objective_team",
    "logprint",
    "worldentnumber",
    "obituary",
    "positionwouldtelefrag",
    "getstarttime",
    "precachemenu",
    "precachestatusicon",
    "precacheheadicon",
    "map_restart",
    "exitlevel",
    "addtestclient",
    "makecvarserverinfo",
    "setarchive",
];

/// 1.1d's four method tables, in `Scr_GetMethod`'s (0x5f724) order: player
/// (0x733dc, 46), scriptent (0x78d40, 12), hudelem (0x749b4, 14), entity (0x7ea00, 38).
const METHODS_11: &[&str] = &[
    "giveweapon",
    "takeweapon",
    "takeallweapons",
    "getcurrentweapon",
    "hasweapon",
    "switchtoweapon",
    "givestartammo",
    "givemaxammo",
    "getfractionstartammo",
    "getfractionmaxammo",
    "setorigin",
    "setplayerangles",
    "usebuttonpressed",
    "attackbuttonpressed",
    "meleebuttonpressed",
    "isonground",
    "pingplayer",
    "setviewmodel",
    "getviewmodel",
    "sayall",
    "sayteam",
    "allowcomplaint",
    "showscoreboard",
    "setspawnweapon",
    "dropitem",
    "finishplayerdamage",
    "suicide",
    "openmenu",
    "openmenunomouse",
    "closemenu",
    "freezecontrols",
    "setreverb",
    "getweaponslotweapon",
    "setweaponslotweapon",
    "getweaponslotammo",
    "setweaponslotammo",
    "getweaponslotclipammo",
    "setweaponslotclipammo",
    "iprintln",
    "iprintlnbold",
    "spawn",
    "setentertime",
    "cloneplayer",
    "setclientcvar",
    "islookingat",
    "playlocalsound",
    "moveto",
    "movex",
    "movey",
    "movez",
    "movegravity",
    "rotateto",
    "rotatepitch",
    "rotateyaw",
    "rotateroll",
    "rotatevelocity",
    "solid",
    "notsolid",
    "settext",
    "setshader",
    "settimer",
    "settimerup",
    "settenthstimer",
    "settenthstimerup",
    "setclock",
    "setclockup",
    "setvalue",
    "fadeovertime",
    "scaleovertime",
    "moveovertime",
    "reset",
    "destroy",
    "attach",
    "detach",
    "detachall",
    "getattachsize",
    "getattachmodelname",
    "getattachtagname",
    "linkto",
    "unlink",
    "enablelinkto",
    "getorigin",
    "geteye",
    "useby",
    "istouching",
    "lockdoor",
    "unlockdoor",
    "isdoorlocked",
    "playsound",
    "playloopsound",
    "stoploopsound",
    "delete",
    "setmodel",
    "getnormalhealth",
    "setnormalhealth",
    "show",
    "hide",
    "setcontents",
    "setcursorhint",
    "sethintstring",
    "shellshock",
    "stopshellshock",
    "viewkick",
    "localtoworldcoords",
    "getentitynumber",
    "enablegrenadetouchdamage",
    "disablegrenadetouchdamage",
    "enablegrenadebounce",
    "disablegrenadebounce",
    "placespawnpoint",
];

/// What 1.5b's `functions` (0x83c40, 110 rows) adds to 1.1d's; it drops none.
const FUNCTIONS_15: &[&str] = &["spawnturret", "precacheturret", "prof_begin", "prof_end"];

/// What 1.5b's method tables add to 1.1d's: player (0x82ca0, 54) and entity
/// (0x84180, 44); scriptent and hudelem are unchanged and none drops a name.
const METHODS_15: &[&str] = &[
    "saysquad",
    "disableweapon",
    "enableweapon",
    "allowspectateteam",
    "getguid",
    "autodemostart",
    "autodemostop",
    "autoscreenshot",
    "getattachignorecollision",
    "getstance",
    "setrightarc",
    "setleftarc",
    "settoparc",
    "setbottomarc",
];

/// What UO 1.51b's `functions` (0xadde0, 120 rows, `precacheturret` twice) adds
/// to 1.5b's; it drops none.
const FUNCTIONS_UO: &[&str] = &[
    "spawntriggermount",
    "isvalidplayer",
    "getfullclipammo",
    "precachevehicle",
    "getnumvehicles",
    "rewindfx",
    "playfxonplayer",
    "setwind",
    "exec",
];

/// What UO 1.51b's method tables add to 1.5b's: player (0xacc80, 60), the
/// vehicle table (0xaec80, 31) and entity (0xae380, 56 rows, `getentitynumber`
/// twice); none drops a name.
const METHODS_UO: &[&str] = &[
    "giveclipammo",
    "isads",
    "isinvehicle",
    "playlocalannouncersound",
    "getfatigue",
    "setfatigue",
    "attachpath",
    "startpath",
    "setswitchnode",
    "setwaitnode",
    "setwaitspeed",
    "setspeed",
    "resumespeed",
    "joltbody",
    "freevehicle",
    "getwheelsurface",
    "getspeedmph",
    "getvehicleowner",
    "startenginesound",
    "stopenginesound",
    "makevehicleusable",
    "makevehicleunusable",
    "addvehicletocompass",
    "removevehiclefromcompass",
    "setturrettargetvec",
    "setturrettargetent",
    "clearturrettarget",
    "fireturret",
    "firealtturret",
    "firegunner",
    "isturretready",
    "get_fire_time",
    "getaltheat",
    "getgunnerheat",
    "getaltoverheating",
    "getgunneroverheating",
    "getdismountspot",
    "dodamage",
    "dodamagemod",
    "settakedamage",
    "unlinkfromworld",
    "linkintoworld",
    "verifyposition",
    "clearvehicleposition",
    "setbounds",
    "spawnduplicate",
    "getturretheat",
    "getturretoverheating",
];

/// CoD2 1.3 `functions` (cod2_lnxded 0x8188040, `Scr_GetFunction` 0x8117cb2), 145 rows.
const FUNCTIONS_COD2: &[&str] = &[
    "print",
    "println",
    "iprintln",
    "iprintlnbold",
    "print3d",
    "line",
    "getent",
    "getentarray",
    "spawn",
    "spawnturret",
    "precacheturret",
    "spawnstruct",
    "assert",
    "assertex",
    "assertmsg",
    "isdefined",
    "isstring",
    "isalive",
    "getcvar",
    "getcvarint",
    "getcvarfloat",
    "setcvar",
    "gettime",
    "getentbynum",
    "getweaponmodel",
    "getanimlength",
    "animhasnotetrack",
    "getbrushmodelcenter",
    "objective_add",
    "objective_delete",
    "objective_state",
    "objective_icon",
    "objective_position",
    "objective_onentity",
    "objective_current",
    "bullettrace",
    "bullettracepassed",
    "sighttracepassed",
    "physicstrace",
    "getmovedelta",
    "getangledelta",
    "getnorthyaw",
    "randomint",
    "randomfloat",
    "randomintrange",
    "randomfloatrange",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "int",
    "distance",
    "distancesquared",
    "length",
    "lengthsquared",
    "closer",
    "vectordot",
    "vectornormalize",
    "vectortoangles",
    "anglestoup",
    "anglestoright",
    "anglestoforward",
    "issubstr",
    "getsubstr",
    "tolower",
    "strtok",
    "musicplay",
    "musicstop",
    "soundfade",
    "ambientplay",
    "ambientstop",
    "precachemodel",
    "precacheshellshock",
    "precacheitem",
    "precacheshader",
    "precachestring",
    "precacherumble",
    "loadfx",
    "playfx",
    "playfxontag",
    "playloopedfx",
    "setcullfog",
    "setexpfog",
    "grenadeexplosioneffect",
    "radiusdamage",
    "setplayerignoreradiusdamage",
    "getnumparts",
    "getpartname",
    "earthquake",
    "newhudelem",
    "newclienthudelem",
    "newteamhudelem",
    "resettimeout",
    "isplayer",
    "isplayernumber",
    "setwinningplayer",
    "setwinningteam",
    "announcement",
    "clientannouncement",
    "getteamscore",
    "setteamscore",
    "setclientnamemode",
    "updateclientnames",
    "getteamplayersalive",
    "objective_team",
    "logprint",
    "worldentnumber",
    "obituary",
    "positionwouldtelefrag",
    "getstarttime",
    "precachemenu",
    "precachestatusicon",
    "precacheheadicon",
    "map_restart",
    "exitlevel",
    "addtestclient",
    "makecvarserverinfo",
    "setarchive",
    "allclientsprint",
    "clientprint",
    "mapexists",
    "isvalidgametype",
    "matchend",
    "setplayerteamrank",
    "sendranks",
    "endparty",
    "setvotestring",
    "setvotetime",
    "setvoteyescount",
    "setvotenocount",
    "openfile",
    "closefile",
    "fprintln",
    "freadln",
    "fgetarg",
    "kick",
    "ban",
    "map",
    "playrumbleonpos",
    "playlooprumbleonpos",
    "stopallrumbles",
    "soundexists",
    "issplitscreen",
];

/// CoD2 1.3's four method tables in table order: player (0x8158a40, 59), scriptent
/// (0x815e3a0, 12), generic entity (0x8188720, 59) and hudelem (0x815a180, 18).
const METHODS_COD2: &[&str] = &[
    "giveweapon",
    "takeweapon",
    "takeallweapons",
    "getcurrentweapon",
    "getcurrentoffhand",
    "hasweapon",
    "switchtoweapon",
    "switchtooffhand",
    "givestartammo",
    "givemaxammo",
    "getfractionstartammo",
    "getfractionmaxammo",
    "setorigin",
    "setplayerangles",
    "getplayerangles",
    "usebuttonpressed",
    "attackbuttonpressed",
    "meleebuttonpressed",
    "playerads",
    "isonground",
    "pingplayer",
    "setviewmodel",
    "getviewmodel",
    "sayall",
    "sayteam",
    "showscoreboard",
    "setspawnweapon",
    "dropitem",
    "finishplayerdamage",
    "suicide",
    "openmenu",
    "openmenunomouse",
    "closemenu",
    "closeingamemenu",
    "freezecontrols",
    "disableweapon",
    "enableweapon",
    "setreverb",
    "deactivatereverb",
    "setchannelvolumes",
    "deactivatechannelvolumes",
    "getweaponslotweapon",
    "setweaponslotweapon",
    "getweaponslotammo",
    "setweaponslotammo",
    "getweaponslotclipammo",
    "setweaponslotclipammo",
    "setweaponclipammo",
    "iprintln",
    "iprintlnbold",
    "spawn",
    "setentertime",
    "cloneplayer",
    "setclientcvar",
    "islookingat",
    "playlocalsound",
    "istalking",
    "allowspectateteam",
    "getguid",
    "moveto",
    "movex",
    "movey",
    "movez",
    "movegravity",
    "rotateto",
    "rotatepitch",
    "rotateyaw",
    "rotateroll",
    "rotatevelocity",
    "solid",
    "notsolid",
    "attach",
    "detach",
    "detachall",
    "getattachsize",
    "getattachmodelname",
    "getattachtagname",
    "getattachignorecollision",
    "getammocount",
    "getclanid",
    "getclanname",
    "getclandescription",
    "getclanmotto",
    "getclanurl",
    "linkto",
    "unlink",
    "enablelinkto",
    "getorigin",
    "geteye",
    "useby",
    "setstablemissile",
    "istouching",
    "playsound",
    "playsoundasmaster",
    "playloopsound",
    "stoploopsound",
    "playrumble",
    "playlooprumble",
    "stoprumble",
    "delete",
    "setmodel",
    "getnormalhealth",
    "setnormalhealth",
    "show",
    "hide",
    "showtoplayer",
    "setcontents",
    "setcursorhint",
    "sethintstring",
    "shellshock",
    "stopshellshock",
    "viewkick",
    "localtoworldcoords",
    "setrightarc",
    "setleftarc",
    "settoparc",
    "setbottomarc",
    "getentitynumber",
    "enablegrenadetouchdamage",
    "disablegrenadetouchdamage",
    "enablegrenadebounce",
    "disablegrenadebounce",
    "enableaimassist",
    "disableaimassist",
    "placespawnpoint",
    "updatescores",
    "setteamfortrigger",
    "clientclaimtrigger",
    "clientreleasetrigger",
    "releaseclaimedtrigger",
    "settext",
    "setplayernamestring",
    "setmapnamestring",
    "setgametypestring",
    "setshader",
    "settimer",
    "settimerup",
    "settenthstimer",
    "settenthstimerup",
    "setclock",
    "setclockup",
    "setvalue",
    "setwaypoint",
    "fadeovertime",
    "scaleovertime",
    "moveovertime",
    "reset",
    "destroy",
];

#[cfg(test)]
mod tests {
    use super::*;
    use vcod_common::net::protocol::{PROTOCOL_V1, PROTOCOL_V6, PROTOCOL_V22};

    #[test]
    fn implementations_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (n, _) in implementations() {
            assert!(seen.insert(n), "{n} has two handlers");
        }
    }

    /// Every handler answers a name some version lists, so UO's set, which
    /// holds them all, reaches every one.
    #[test]
    fn every_handler_is_a_retail_name() {
        let uo = BuiltinSet::of(&PROTOCOL_V22);
        for (n, _) in implementations() {
            assert!(uo.get(n).is_some(), "{n} is in no retail table");
        }
    }

    #[test]
    fn each_version_lists_its_own_names() {
        let v1 = BuiltinSet::of(&PROTOCOL_V1);
        let v15 = BuiltinSet::of(&PROTOCOL_V6);
        let uo = BuiltinSet::of(&PROTOCOL_V22);
        // 1.1d: 106 functions and 110 methods (46 + 12 + 14 + 38).
        let count = |s: &BuiltinSet, m: bool| {
            s.names
                .values()
                .filter(|e| e.function && !m || e.method && m)
                .count()
        };
        assert_eq!((count(v1, false), count(v1, true)), (106, 110));
        assert_eq!((count(v15, false), count(v15, true)), (110, 124));
        assert!(v1.get("getguid").is_none() && v15.get("getguid").is_some());
        assert!(v15.get("getnumvehicles").is_none() && uo.get("getnumvehicles").is_some());
        assert!(!v1.knows("setspeed", true) && uo.knows("setspeed", true));
        // `iprintln` is both a function and a player method.
        assert!(v1.knows("iprintln", false) && v1.knows("iprintln", true));
        assert!(!v1.knows("delete", false) && v1.knows("delete", true));
    }

    /// cod2_lnxded's 145 functions and 148 methods, and the four 1.1 names
    /// it drops (cod2-gsc-diff.md 1.2).
    #[test]
    fn cod2_lists_its_own_tables() {
        let c = BuiltinSet::cod2();
        let count = |m: bool| {
            c.names
                .values()
                .filter(|e| e.function && !m || e.method && m)
                .count()
        };
        assert_eq!((count(false), count(true)), (145, 148));
        assert!(!c.knows("getweaponclassname", false));
        assert!(!c.knows("lockdoor", true) && !c.knows("allowcomplaint", true));
        assert!(c.knows("strtok", false) && c.knows("setwaypoint", true));
        assert!(!BuiltinSet::of(&PROTOCOL_V22).knows("strtok", false));
        // A CoD2 handler answers only CoD2's set.
        assert!(c.get("issplitscreen").is_some());
        assert!(BuiltinSet::of(&PROTOCOL_V22).get("issplitscreen").is_none());
        let mut seen = std::collections::HashSet::new();
        for (n, _) in super::super::cod2::NAMES {
            assert!(seen.insert(n), "{n} has two CoD2 handlers");
        }
    }

    /// cod4_lnxded's 205 function rows (204 names: `weaponfiretime` is
    /// listed twice) and five method tables' 230 names; a CoD4 handler
    /// answers only CoD4's set.
    #[test]
    fn cod4_lists_its_own_tables() {
        let c = BuiltinSet::cod4();
        let count = |m: bool| {
            c.names
                .values()
                .filter(|e| e.function && !m || e.method && m)
                .count()
        };
        assert_eq!((count(false), count(true)), (204, 230));
        assert!(c.knows("tablelookup", false) && c.knows("setperk", true));
        assert!(!c.knows("getcvar", false) && c.knows("getdvar", false));
        assert!(c.get("tablelookup").is_some());
        assert!(BuiltinSet::cod2().get("tablelookup").is_none());
        let mut seen = std::collections::HashSet::new();
        for (n, _) in super::super::cod4::NAMES {
            assert!(seen.insert(n), "{n} has two CoD4 handlers");
        }
    }

    /// CoD2's stock MP gametype and map scripts compile under
    /// `Dialect::Cod2` against its set. Needs `COD2_DIR`.
    #[test]
    fn cod2_stock_scripts_call_only_cod2_builtins() {
        use crate::game::script::PakScripts;
        use std::rc::Rc;
        let Some(fs) = vcod_common::testing::cod2_fs() else {
            return;
        };
        let entries: Vec<String> = fs
            .list_prefix("maps/mp/")
            .into_iter()
            .filter(|p| {
                p.ends_with(".gsc")
                    && (p.starts_with("maps/mp/mp_") || p.starts_with("maps/mp/gametypes/"))
            })
            .map(|p| p.trim_end_matches(".gsc").to_string())
            .collect();
        assert!(entries.len() > 20, "{entries:?}");
        let set = BuiltinSet::cod2();
        let mut vm = Vm::new();
        let mut loader = vcod_gsc::Loader::with_dialect(
            Box::new(PakScripts::new(Rc::new(fs), None)),
            vcod_gsc::Dialect::Cod2,
        );
        loader.set_builtins(set.table());
        for e in &entries {
            loader
                .load(&mut vm, e)
                .unwrap_or_else(|err| panic!("{e}: {err:?}"));
        }
        let unknown = set.unknown_calls(&vm);
        assert!(unknown.is_empty(), "{unknown:?}");
        // 1.1's set refuses CoD2's own calls.
        let v1 = BuiltinSet::of(&PROTOCOL_V1).unknown_calls(&vm);
        assert!(v1.iter().any(|n| n == "function 'issplitscreen'"), "{v1:?}");
    }

    /// Every stock gametype and map script compiles under its own version's
    /// set: what retail's compiler accepts, ours must. Runs on the install
    /// `COD_DIR` names (1.1 or 1.5's `main/`) and, where it has one, on
    /// UO's `uo/`; passes vacuously without one.
    #[test]
    fn stock_scripts_call_only_their_versions_builtins() {
        use crate::game::script::PakScripts;
        use std::rc::Rc;
        use vcod_common::net::protocol::install_protocol;
        use vcod_common::pk3::Pk3Fs;
        use vcod_common::testing;
        let uo = testing::game_dir().join("uo");
        let installs = [
            testing::stock_fs(),
            uo.is_dir()
                .then(|| Pk3Fs::open_search(&uo, None, None, true).ok())
                .flatten(),
        ];
        for fs in installs.into_iter().flatten() {
            let proto = install_protocol(&fs);
            let entries: Vec<String> = fs
                .list_prefix("maps/mp/")
                .into_iter()
                .filter(|p| {
                    p.ends_with(".gsc")
                        && (p.starts_with("maps/mp/mp_") || p.starts_with("maps/mp/gametypes/"))
                })
                .map(|p| p.trim_end_matches(".gsc").to_string())
                .collect();
            assert!(!entries.is_empty());
            let fs = Rc::new(fs);
            let mut vm = Vm::new();
            let mut loader = vcod_gsc::Loader::new(Box::new(PakScripts::new(fs, None)));
            for e in &entries {
                loader
                    .load(&mut vm, e)
                    .unwrap_or_else(|err| panic!("{e}: {err:?}"));
            }
            let unknown = BuiltinSet::of(proto).unknown_calls(&vm);
            assert!(
                unknown.is_empty(),
                "protocol {}: {unknown:?}",
                proto.transport.version
            );
            // And one version down refuses UO's or 1.5's own calls.
            if proto.is_uo() {
                let v15 = BuiltinSet::of(&PROTOCOL_V6).unknown_calls(&vm);
                assert!(
                    v15.iter().any(|n| n == "function 'precachevehicle'"),
                    "{v15:?}"
                );
            }
        }
    }
}
