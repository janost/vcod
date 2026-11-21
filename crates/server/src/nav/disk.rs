//! The bots' graphs kept on disk between runs (bot-navigation.md 2.5, "Build
//! times"): the next start of the same server binary on the same map loads
//! the graph its build proved instead of building it again. A file is read
//! as hostile input: a wrong key, length or hash, an index past the nodes
//! or a float that is not finite is a miss, and the graph is built.

use super::{CacheKey, Edge, NavGraph, SPACING_MAX, SPACING_MIN};
use glam::{Vec2, Vec3};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use vcod_common::pmove::MoveRules;

/// Bumped with any change to what a file holds or how it lays it out.
const FORMAT: u32 = 1;
const MAGIC: [u8; 8] = *b"vcodnav\0";
/// The largest file read: a stock map's graph is a few MB.
const MAX_BYTES: u64 = 256 << 20;

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Keeps graphs under `dir` from now on; `None`, the default, keeps none.
pub fn set_dir(dir: Option<PathBuf>) {
    *DIR.lock().unwrap_or_else(|e| e.into_inner()) = dir;
}

fn dir() -> Option<PathBuf> {
    DIR.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
}

const FNV_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The running binary's bytes, hashed once a process: a graph loads only
/// into the build that proved it, so a change to the walks or the rules
/// never meets a graph an older binary wrote. The binary compares only its
/// own hashes, so `DefaultHasher::new()`'s fixed keys do.
fn exe_hash() -> Option<u64> {
    static HASH: OnceLock<Option<u64>> = OnceLock::new();
    *HASH.get_or_init(|| {
        use std::hash::Hasher;
        let bytes = std::fs::read(std::env::current_exe().ok()?).ok()?;
        let mut h = std::hash::DefaultHasher::new();
        h.write(&bytes);
        Some(h.finish())
    })
}

/// Where one graph's file lives and the key it must carry.
pub(super) struct DiskKey {
    bytes: Vec<u8>,
    path: PathBuf,
}

/// `key`'s file for a world built from the map file `source`
/// (`CollisionWorld::source`); `None` with no cache directory, no source
/// or no readable binary.
pub(super) fn key(key: &CacheKey, source: u64) -> Option<DiskKey> {
    key_in(&dir()?, key, source)
}

fn key_in(dir: &Path, key: &CacheKey, source: u64) -> Option<DiskKey> {
    if source == 0 {
        return None;
    }
    let mut w = Writer::default();
    w.u32(FORMAT);
    w.u64(exe_hash()?);
    w.u64(source);
    let (map, rules, step, brushes, tris, model_tris, hazards, spawns, cover, linked, placed) = key;
    w.str(map);
    w.u32(rules_id(*rules));
    match step {
        Some(s) => {
            w.u32(1);
            w.str(s);
        }
        None => w.u32(0),
    }
    for n in [brushes, tris, model_tris, hazards] {
        w.u64(*n as u64);
    }
    for points in [spawns, cover] {
        w.len(points.len());
        points.iter().flatten().for_each(|&c| w.u32(c));
    }
    w.len(linked.len());
    linked.iter().for_each(|&l| w.u32(u32::from(l)));
    w.len(placed.len());
    placed.iter().for_each(|&p| w.u32(p));
    let stem: String = map
        .chars()
        .take(48)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let path = dir.join(format!("{stem}-{:016x}.nav", fnv(FNV_BASIS, &w.0)));
    Some(DiskKey { bytes: w.0, path })
}

fn rules_id(r: MoveRules) -> u32 {
    match r {
        MoveRules::V1_1 => 0,
        MoveRules::V1_5 => 1,
        MoveRules::Uo => 2,
    }
}

/// The graph `k`'s file holds, walked by `rules` and `step` as the build
/// would have; `None` on any miss.
pub(super) fn load(
    k: &DiskKey,
    rules: MoveRules,
    step: Option<Arc<dyn super::WalkStep>>,
) -> Option<NavGraph> {
    let len = std::fs::metadata(&k.path).ok()?.len();
    if len > MAX_BYTES {
        log::warn!(
            "{}: {len} bytes, past the cap; rebuilding",
            k.path.display()
        );
        return None;
    }
    let bytes = std::fs::read(&k.path).ok()?;
    let g = parse(&bytes, &k.bytes, rules, step);
    if g.is_none() {
        log::warn!(
            "{}: not a graph for this map and build; rebuilding",
            k.path.display()
        );
    }
    g
}

/// The most graph files a directory keeps: each new binary writes its own,
/// and the oldest go.
const MAX_FILES: usize = 64;

/// Writes `g` to `k`'s file, through a temporary file renamed over it so a
/// reader never sees half a graph, then drops the oldest files past
/// [`MAX_FILES`]. A failure only logs.
pub(super) fn store(k: &DiskKey, g: &NavGraph) {
    if let Err(e) = write(&k.path, &file_bytes(&k.bytes, g)) {
        log::warn!("keeping the nav graph in {}: {e}", k.path.display());
    }
    if let Some(dir) = k.path.parent() {
        prune(dir, MAX_FILES);
    }
}

fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "nav"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort();
    let extra = files.len().saturating_sub(keep);
    for (_, p) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

fn write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// `MAGIC`, the key, then the graph and an FNV-1a 64 over it.
fn file_bytes(key: &[u8], g: &NavGraph) -> Vec<u8> {
    let body = encode(g);
    let mut w = Writer::default();
    w.0.extend_from_slice(&MAGIC);
    w.len(key.len());
    w.0.extend_from_slice(key);
    w.u64(body.len() as u64);
    w.0.extend_from_slice(&body);
    w.u64(fnv(FNV_BASIS, &body));
    w.0
}

fn parse(
    bytes: &[u8],
    key: &[u8],
    rules: MoveRules,
    step: Option<Arc<dyn super::WalkStep>>,
) -> Option<NavGraph> {
    let mut r = Reader(bytes);
    if r.take(MAGIC.len())? != MAGIC {
        return None;
    }
    let n = r.len(1)?;
    if r.take(n)? != key {
        return None;
    }
    let n = usize::try_from(r.u64()?).ok()?;
    let body = r.take(n)?;
    if r.u64()? != fnv(FNV_BASIS, body) || !r.0.is_empty() {
        return None;
    }
    decode(body, rules, step)
}

fn encode(g: &NavGraph) -> Vec<u8> {
    let mut w = Writer::default();
    w.f32(g.spacing);
    w.len(g.nodes.len());
    g.nodes.iter().for_each(|&p| w.vec3(p));
    for out in &g.edges {
        w.len(out.len());
        out.iter().for_each(|e| w.u32(e.to));
    }
    let kinds: [fn(&Edge) -> bool; 5] = [
        |e| e.jump,
        |e| e.leap,
        |e| e.crouched,
        |e| e.risky,
        |e| e.mantle,
    ];
    for kind in kinds {
        w.pairs(sorted(g.edges_where(kind)));
    }
    w.ids(sorted(g.perches.iter().copied()));
    let all: Vec<((u32, u32), Edge)> = (g.edges.iter().enumerate())
        .flat_map(|(a, out)| out.iter().map(move |e| ((a as u32, e.to), *e)))
        .collect();
    let aims = sorted(
        all.iter()
            .filter_map(|(k, e)| Some((*k, e.aim?.to_array().map(f32::to_bits)))),
    );
    w.len(aims.len());
    for (e, a) in aims {
        w.pair(e);
        a.iter().for_each(|&c| w.u32(c));
    }
    let lands = sorted(
        all.iter()
            .filter_map(|(k, e)| Some((*k, e.land?.to_array().map(f32::to_bits)))),
    );
    w.len(lands.len());
    for (e, p) in lands {
        w.pair(e);
        p.iter().for_each(|&c| w.u32(c));
    }
    w.ids(g.seeds.clone());
    // A column's order is the order `find` tries its nodes in.
    let columns = sorted(g.columns.iter().map(|(&c, ids)| (c, ids.clone())));
    w.len(columns.len());
    for ((x, y), ids) in columns {
        w.u32(x as u32);
        w.u32(y as u32);
        w.ids(ids);
    }
    w.len(g.ladder_boxes.len());
    for &(lo, hi) in &g.ladder_boxes {
        w.vec3(lo);
        w.vec3(hi);
    }
    w.0
}

fn decode(
    body: &[u8],
    rules: MoveRules,
    step: Option<Arc<dyn super::WalkStep>>,
) -> Option<NavGraph> {
    let mut r = Reader(body);
    let spacing = r.f32()?;
    if !(SPACING_MIN..=SPACING_MAX).contains(&spacing) {
        return None;
    }
    let count = r.len(12)?;
    let nodes: Vec<Vec3> = (0..count).map(|_| r.vec3()).collect::<Option<_>>()?;
    let node = |id: u32| (id as usize) < nodes.len();
    let mut edges: Vec<Vec<Edge>> = Vec::with_capacity(count);
    for _ in 0..count {
        let out = r.ids()?;
        let fresh: HashSet<u32> = out.iter().copied().collect();
        if !out.iter().all(|&b| node(b)) || fresh.len() != out.len() {
            return None;
        }
        edges.push(out.into_iter().map(Edge::to).collect());
    }
    // Each kind, aim and landing names an edge the lists above hold.
    fn edge(edges: &mut [Vec<Edge>], (a, b): (u32, u32)) -> Option<&mut Edge> {
        edges.get_mut(a as usize)?.iter_mut().find(|e| e.to == b)
    }
    let kinds: [fn(&mut Edge); 5] = [
        |e| e.jump = true,
        |e| e.leap = true,
        |e| e.crouched = true,
        |e| e.risky = true,
        |e| e.mantle = true,
    ];
    for set in kinds {
        for pair in r.pairs()? {
            set(edge(&mut edges, pair)?);
        }
    }
    let perches = r.ids()?;
    for _ in 0..r.len(16)? {
        let e = r.pair()?;
        let a = Vec2::new(r.f32()?, r.f32()?);
        edge(&mut edges, e)?.aim = Some(a);
    }
    for _ in 0..r.len(20)? {
        let e = r.pair()?;
        let p = r.vec3()?;
        edge(&mut edges, e)?.land = Some(p);
    }
    let seeds = r.ids()?;
    let mut columns = HashMap::new();
    for _ in 0..r.len(12)? {
        let c = (r.u32()? as i32, r.u32()? as i32);
        let ids = r.ids()?;
        if !ids.iter().all(|&n| node(n)) || columns.insert(c, ids).is_some() {
            return None;
        }
    }
    let mut ladder_boxes = Vec::new();
    for _ in 0..r.len(24)? {
        ladder_boxes.push((r.vec3()?, r.vec3()?));
    }
    if !r.0.is_empty() || !perches.iter().chain(&seeds).all(|&n| node(n)) {
        return None;
    }
    let mut g = NavGraph {
        nodes,
        edges,
        perches: perches.into_iter().collect(),
        seeds,
        spacing,
        columns,
        comp: Vec::new(),
        members: Vec::new(),
        reach: Vec::new(),
        safe: Vec::new(),
        ladder_boxes,
        rules,
        falls: super::falls(),
        step,
    };
    g.index_components();
    Some(g)
}

fn sorted<T: Ord>(it: impl Iterator<Item = T>) -> Vec<T> {
    let mut v: Vec<T> = it.collect();
    v.sort_unstable();
    v
}

/// Little-endian words.
#[derive(Default)]
struct Writer(Vec<u8>);

impl Writer {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn len(&mut self, n: usize) {
        self.u32(n as u32);
    }
    fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }
    fn vec3(&mut self, v: Vec3) {
        v.to_array().iter().for_each(|&c| self.f32(c));
    }
    fn str(&mut self, s: &str) {
        self.len(s.len());
        self.0.extend_from_slice(s.as_bytes());
    }
    fn pair(&mut self, (a, b): (u32, u32)) {
        self.u32(a);
        self.u32(b);
    }
    fn pairs(&mut self, v: Vec<(u32, u32)>) {
        self.len(v.len());
        v.into_iter().for_each(|p| self.pair(p));
    }
    fn ids(&mut self, v: Vec<u32>) {
        self.len(v.len());
        v.into_iter().for_each(|n| self.u32(n));
    }
}

/// [`Writer`]'s words back, each read bounded by what is left: a count
/// that claims more than the bytes hold fails before anything is
/// allocated for it.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(n)?;
        self.0 = rest;
        Some(head)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    /// A count of items at least `size` bytes each.
    fn len(&mut self, size: usize) -> Option<usize> {
        let n = self.u32()? as usize;
        (n.checked_mul(size)? <= self.0.len()).then_some(n)
    }
    fn f32(&mut self) -> Option<f32> {
        Some(f32::from_bits(self.u32()?)).filter(|v| v.is_finite())
    }
    fn vec3(&mut self) -> Option<Vec3> {
        Some(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }
    fn pair(&mut self) -> Option<(u32, u32)> {
        Some((self.u32()?, self.u32()?))
    }
    fn pairs(&mut self) -> Option<Vec<(u32, u32)>> {
        (0..self.len(8)?).map(|_| self.pair()).collect()
    }
    fn ids(&mut self) -> Option<Vec<u32>> {
        (0..self.len(4)?).map(|_| self.u32()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_key(map: &str) -> CacheKey {
        let spawns = vec![[1, 2, 3]];
        (
            map.into(),
            MoveRules::Uo,
            None,
            1,
            2,
            3,
            4,
            spawns,
            Vec::new(),
            vec![true],
            vec![7],
        )
    }

    /// Every field the build fills, each set and map non-empty.
    fn graph() -> NavGraph {
        let mut g = NavGraph::from_parts(
            vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(32.0, 0.0, 0.0),
                Vec3::new(64.0, 0.0, 8.0),
                Vec3::new(32.0, 64.0, 0.0),
                Vec3::new(96.0, 0.0, -100.0),
            ],
            vec![vec![1, 3], vec![0, 2], vec![1, 4], vec![0, 2], vec![]],
        );
        g.edge_mut(1, 2).unwrap().jump = true;
        g.edge_mut(1, 2).unwrap().leap = true;
        g.edge_mut(0, 3).unwrap().crouched = true;
        g.edge_mut(1, 2).unwrap().aim = Some(Vec2::new(60.0, 2.0));
        g.edge_mut(2, 4).unwrap().risky = true;
        g.perches.insert(4);
        g.edge_mut(3, 2).unwrap().mantle = true;
        g.edge_mut(3, 2).unwrap().land = Some(Vec3::new(60.0, 4.0, 8.0));
        g.seeds = vec![0, 3];
        g.ladder_boxes = vec![(Vec3::new(1.0, 2.0, 3.0), Vec3::new(4.0, 5.0, 6.0))];
        g.index_components();
        g
    }

    fn parsed(bytes: &[u8], key: &[u8]) -> Option<NavGraph> {
        parse(bytes, key, MoveRules::Uo, None)
    }

    #[test]
    fn a_graph_comes_back_off_disk_as_it_went() {
        let g = graph();
        let bytes = file_bytes(b"key", &g);
        let back = parsed(&bytes, b"key").expect("a good file loads");
        assert_eq!(encode(&back), encode(&g));
        assert_eq!(back.columns, g.columns);
        assert_eq!(back.comp, g.comp);
        assert_eq!(back.members, g.members);
        assert_eq!(back.reach, g.reach);
        assert_eq!(back.safe, g.safe);
        assert_eq!(back.rules, MoveRules::Uo);
    }

    #[test]
    fn another_key_or_a_damaged_file_is_a_miss() {
        let bytes = file_bytes(b"key", &graph());
        assert!(parsed(&bytes, b"kez").is_none());
        assert!(parsed(&bytes[..bytes.len() - 1], b"key").is_none());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(parsed(&longer, b"key").is_none());
        // A byte of the graph's own, past the header: the hash misses it.
        let mut flipped = bytes.clone();
        flipped[MAGIC.len() + 4 + 3 + 8 + 9] ^= 1;
        assert!(parsed(&flipped, b"key").is_none());
    }

    /// The hash only proves the file whole: what it holds is checked too.
    #[test]
    fn a_whole_file_with_a_bad_graph_is_refused() {
        let mut g = graph();
        g.edges[0].push(Edge::to(5));
        assert!(parsed(&file_bytes(b"k", &g), b"k").is_none());
        let mut g = graph();
        g.edge_mut(3, 2).unwrap().land = Some(Vec3::new(f32::NAN, 0.0, 0.0));
        assert!(parsed(&file_bytes(b"k", &g), b"k").is_none());
        let mut g = graph();
        g.spacing = 1.0;
        assert!(parsed(&file_bytes(b"k", &g), b"k").is_none());
        // A node count past what the bytes hold allocates nothing.
        let mut body = encode(&graph());
        body[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&body, MoveRules::Uo, None).is_none());
    }

    #[test]
    fn the_file_moves_with_the_map_file_and_the_key() {
        let dir = Path::new("cache");
        let k = |map, source| key_in(dir, &cache_key(map), source);
        assert!(k("mp_a", 0).is_none(), "no map checksum, no file");
        let a = k("mp_a", 1).unwrap();
        for other in [k("mp_a", 2).unwrap(), k("mp_b", 1).unwrap()] {
            assert_ne!(other.bytes, a.bytes);
            assert_ne!(other.path, a.path);
        }
        assert_eq!(k("mp_a", 1).unwrap().path, a.path);
        assert!(a.path.starts_with(dir));
        let odd = k("../x y", 1).unwrap();
        assert_eq!(odd.path.parent(), Some(dir));
    }

    #[test]
    fn a_stored_graph_loads_and_another_binary_s_does_not() {
        let dir = std::env::temp_dir().join(format!("vcod-navdisk-{}", std::process::id()));
        let k = key_in(&dir, &cache_key("mp_a"), 1).unwrap();
        assert!(load(&k, MoveRules::Uo, None).is_none(), "nothing kept yet");
        store(&k, &graph());
        let back = load(&k, MoveRules::Uo, None).expect("kept");
        assert_eq!(encode(&back), encode(&graph()));
        // Another binary's key under the same name reads as a miss.
        let other = DiskKey {
            bytes: [&k.bytes[..4], &[0; 8], &k.bytes[12..]].concat(),
            path: k.path.clone(),
        };
        assert!(load(&other, MoveRules::Uo, None).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_directory_keeps_the_newest_graphs() {
        let dir = std::env::temp_dir().join(format!("vcod-navprune-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1 << 30);
        for (i, name) in ["a.nav", "b.nav", "c.nav", "d.txt"].iter().enumerate() {
            let f = std::fs::File::create(dir.join(name)).unwrap();
            f.set_modified(t0 + std::time::Duration::from_secs(i as u64))
                .unwrap();
        }
        prune(&dir, 2);
        let mut left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        left.sort();
        assert_eq!(left, ["b.nav", "c.nav", "d.txt"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
