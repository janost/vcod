//! The area-tree order a brush model's push leaves two players in, against
//! the retail capture `movers/mp_carentan-dm-pushorder.txt` (its header
//! carries the recipe): `client-probes/probe_pushorder.gsc` sets both down in
//! the slab's path, pushes them four times and runs a flat `radiusDamage`
//! after each, whose `PROBE cb` lines read the walk, so the tree's order
//! (combat doc 14.7). `G_MoverPush` unlinks every player it lists and
//! relinks each on its turn, which turns the pair round on every frame that
//! pushes both (docs/research/cod11-movers.md, section 12). Ours runs the
//! same probe with two clients standing still.
//!
//! Needs `COD_DIR`; without the paks the test returns early.

mod common;

use common::{Queues, holding, join_pair, step_pair};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const MAP: &str = "mp_carentan";
const FIXTURE: &str = "tests/fixtures/movers/mp_carentan-dm-pushorder.txt";

/// The lines the gate compares: every blast, its callbacks in order, and
/// where each player stood when it went off.
fn order<'a>(lines: impl Iterator<Item = &'a str>) -> Vec<String> {
    lines
        .filter_map(|l| l.find("PROBE ").map(|i| &l[i + 6..]))
        .filter(|l| {
            let tag = l.split_whitespace().next().unwrap_or("");
            ["cb", "blast", "placed", "one", "two", "three", "back"].contains(&tag)
        })
        .map(str::to_string)
        .collect()
}

#[test]
fn a_push_leaves_its_players_in_retails_tree_order() {
    let Some(fs) = vcod_common::testing::game_fs() else {
        eprintln!("COD_DIR unset or has no main/: skipping");
        return;
    };
    let bsp_path = fs.resolve_map(MAP).expect("map in the mounted paks");
    let bsp = vcod_common::bsp::parse(&fs.read(&bsp_path).unwrap()).unwrap();
    let mut now = Instant::now();
    let mut sv = vcod_server::Server::new(common::cfg(MAP, "probe_pushorder"), now);
    sv.overlay_script(
        "maps/mp/gametypes/probe_pushorder",
        include_str!("../../gsc/tests/fixtures/semantics/client-probes/probe_pushorder.gsc"),
    );
    sv.load_world(vcod_server::world::World::from_bsp(&bsp, Some(&fs)));
    sv.load_scripts(Rc::new(fs)).expect("load the scripts");
    let (qa, qb) = (
        Rc::new(RefCell::new(Queues::default())),
        Rc::new(RefCell::new(Queues::default())),
    );
    let (mut ca, mut cb) = join_pair(
        &mut sv,
        &qa,
        &qb,
        &mut now,
        ("allies", "m1carbine_mp"),
        ("allies", "m1carbine_mp"),
    );
    for _ in 0..1200 {
        now += Duration::from_millis(50);
        ca.send_frame(&holding(&ca));
        cb.send_frame(&holding(&cb));
        step_pair(&mut sv, (&qa, &mut ca), (&qb, &mut cb), now);
        if sv.script_log().iter().any(|l| l.contains("PROBE done")) {
            break;
        }
    }
    assert_eq!(sv.script_aborts(), Vec::<String>::new());
    let ours = order(sv.script_log().iter().map(String::as_str));
    let retail_text = std::fs::read_to_string(FIXTURE).unwrap();
    let retail = order(retail_text.lines());
    assert!(retail.len() > 20, "the fixture has no blasts");
    assert_eq!(ours, retail);
}
