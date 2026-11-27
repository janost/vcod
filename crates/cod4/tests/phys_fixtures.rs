//! The world 0 and 1 captures (`fixtures/phys/`, cod4-gsc.md 3.6.2) parse:
//! each starts at the map load's seed, names its bodies before stepping
//! them, and every float word is hex.

use std::collections::BTreeSet;

static FIXTURES: vcod_common::manifest::ManifestPath =
    vcod_common::manifest_const!("/tests/fixtures/phys");

/// What one dump holds, as its README's lines give it.
#[derive(Debug, Default)]
struct Dump {
    world: u32,
    seed: u32,
    bodies: BTreeSet<u32>,
    steps: usize,
    contacts: usize,
}

fn hex(w: &str) -> u32 {
    u32::from_str_radix(w, 16).expect("hex word")
}

fn parse(text: &str) -> Dump {
    let mut d = Dump::default();
    let mut ended = false;
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        match w.first().copied() {
            Some("world") => (d.world, d.seed) = (w[1].parse().expect("world"), hex(w[4])),
            Some("new") => {
                let words: Vec<u32> = line
                    .split('|')
                    .skip(1)
                    .flat_map(|p| p.split_whitespace().skip(1))
                    .map(hex)
                    .collect();
                // mass, inv, force, torque, idle, pos, q, R, lvel, avel
                assert_eq!(words.len(), 17 + 13 + 3 + 3 + 4 + 3 + 4 + 12 + 3 + 3);
                d.bodies.insert(w[1].parse().expect("body"));
            }
            Some("step") => d.steps += 1,
            Some("body") => {
                let id = w[1].parse().expect("body");
                assert!(
                    d.bodies.contains(&id),
                    "body {id} stepped before its new line"
                );
                assert_eq!(
                    line.split('|').flat_map(|p| p.split_whitespace()).count(),
                    4 + 3 + 4 + 3 + 3
                );
            }
            Some("c") => {
                let words: Vec<u32> = w[1..].iter().map(|x| hex(x)).collect();
                assert_eq!(words.len(), 12);
                d.contacts += 1;
            }
            Some("end") => ended = true,
            _ => {}
        }
    }
    assert!(ended, "no end line");
    d
}

#[test]
fn the_phys_captures_parse_from_the_map_loads_seed() {
    let mut names: Vec<_> = std::fs::read_dir(&FIXTURES)
        .expect("fixtures/phys")
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".txt"))
        .collect();
    names.sort();
    assert!(!names.is_empty());
    for name in names {
        let d = parse(&std::fs::read_to_string(format!("{FIXTURES}/{name}")).unwrap());
        println!(
            "{name}: world {} {} bodies {} steps {} contacts",
            d.world,
            d.bodies.len(),
            d.steps,
            d.contacts
        );
        assert_eq!(d.seed, 0x89ab_cdef, "{name}");
        assert!(
            d.world <= 1 && !d.bodies.is_empty() && d.steps > 0,
            "{name}"
        );
    }
}
