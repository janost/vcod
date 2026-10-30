//! The asset body walk: every body of a zone read as 1.7's `Load_*` stream
//! functions read it, block by block. Layouts and sources:
//! `docs/research/cod4-fastfile.md`, sections 9 and on.
//!
//! The walk keeps each block's position, as `DB_SetStreamIndex` does, so
//! an offset token can be resolved against what landed there and the
//! final positions can be held against the `XFile`'s `blockSize`.

use crate::assets::{AssetRef, Assets};
use crate::zone::{BLOCK_COUNT, Block, Pointer, Zone, ZoneStream};
use anyhow::{Context, Result, bail, ensure};
use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;

/// What the walk keeps beyond names and headers.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOptions {
    /// Keep bulk arrays (vertices, indices, pixels, collision trees) as
    /// raw bytes; without it they are read and dropped.
    pub keep_bulk: bool,
}

/// Where the walk left each block.
#[derive(Clone, Debug, Default)]
pub struct WalkStats {
    /// Each block's position after the last body.
    pub end: [u32; BLOCK_COUNT],
    /// Each block's highest position: [`Block::Temp`] rewinds on every
    /// pop, so its size is this, not its end (and the linker counts the
    /// 16-byte `XAssetList` in it besides).
    pub peak: [u32; BLOCK_COUNT],
    /// Offset tokens that named nothing the walk recorded, by what the
    /// slot held.
    pub unresolved: HashMap<&'static str, u32>,
    /// Offset tokens into the middle of data another struct's array holds
    /// (a leaf brush node's list into the map's leaf brushes): copied
    /// when the walk kept that array, else only found.
    pub into_arrays: u32,
}

/// A walked zone: its assets, listed and inline, and the block positions.
pub struct Walked {
    pub assets: Assets,
    /// One per list entry: the asset its header slot holds, `None` for the
    /// loader-less types.
    pub list: Vec<Option<AssetRef>>,
    pub stats: WalkStats,
}

/// Walks every asset body of `zone`, then inflates the rest and checks
/// that nothing follows the last body.
pub fn walk<R: BufRead>(mut zone: Zone<R>, opts: WalkOptions) -> Result<Walked> {
    let entries = zone.assets.clone();
    let strings = zone.script_strings.clone();
    let vpos = zone.virtual_pos();
    let mut w = W::new(zone.stream(), opts, vpos);
    w.out.script_strings = strings;
    // The asset array sits at the end of the list in the virtual block;
    // each header slot is an alias later tokens may name.
    let array = vpos - 8 * entries.len() as u32;
    w.push(Block::Virtual);
    let mut list = Vec::with_capacity(entries.len());
    for (i, a) in entries.iter().enumerate() {
        let slot = addr(Block::Virtual, array + 8 * i as u32 + 4);
        let r = crate::assets::load_header(&mut w, a.kind, a.header, slot)
            .with_context(|| format!("asset {i} ({})", a.kind.name()))?;
        list.push(r);
    }
    w.pop()?;
    ensure!(w.stack.is_empty(), "block stack left {:?}", w.stack);
    let mut stats = std::mem::take(&mut w.stats);
    stats.end = w.pos;
    stats.peak = w.peak;
    let body_end = w.s.pos();
    let assets = std::mem::take(&mut w.out);
    drop(w);
    let len = zone.finish()?;
    ensure!(
        len == body_end,
        "last body ends at {body_end:#x}, stream at {len:#x}"
    );
    Ok(Walked {
        assets,
        list,
        stats,
    })
}

/// A packed `block << 28 | offset` address, as offset tokens carry them.
pub fn addr(block: Block, offset: u32) -> u32 {
    (block as u32) << 28 | offset
}

/// Little-endian u16s.
pub fn u16s(b: &[u8]) -> Vec<u16> {
    b.as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect()
}

/// A struct as loaded: its bytes and where they landed.
pub struct S {
    pub at: u32,
    pub b: Vec<u8>,
}

impl S {
    pub fn u8(&self, o: usize) -> u8 {
        self.b[o]
    }
    pub fn i8(&self, o: usize) -> i8 {
        self.b[o] as i8
    }
    pub fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes(self.b[o..o + 2].try_into().unwrap())
    }
    pub fn i16(&self, o: usize) -> i16 {
        self.u16(o) as i16
    }
    pub fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.b[o..o + 4].try_into().unwrap())
    }
    pub fn i32(&self, o: usize) -> i32 {
        self.u32(o) as i32
    }
    pub fn f32(&self, o: usize) -> f32 {
        f32::from_bits(self.u32(o))
    }
    pub fn vec3(&self, o: usize) -> [f32; 3] {
        [self.f32(o), self.f32(o + 4), self.f32(o + 8)]
    }
    pub fn ptr(&self, o: usize) -> Pointer {
        Pointer::from_token(self.u32(o))
    }
    /// The address of the field at `o`, for slots later tokens may name.
    pub fn slot(&self, o: usize) -> u32 {
        self.at + o as u32
    }
    /// The `len` bytes at `o` as their own struct (an array element or an
    /// embedded member).
    pub fn sub(&self, o: usize, len: usize) -> S {
        S {
            at: self.at + o as u32,
            b: self.b[o..o + len].to_vec(),
        }
    }
    /// Element `i` of an array of `size`-byte structs.
    pub fn elem(&self, i: usize, size: usize) -> S {
        self.sub(i * size, size)
    }
    pub fn len(&self) -> usize {
        self.b.len()
    }
    pub fn is_empty(&self) -> bool {
        self.b.is_empty()
    }
}

/// How a pointer slot's data is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Follow {
    Null,
    /// The data follows; the current block is already aligned for it.
    Inline,
    /// Data already loaded, at this packed address.
    At(u32),
}

#[derive(Debug)]
struct Frame {
    block: Block,
    saved: u32,
}

/// The walk's state: the stream, the block positions and stack, and what
/// has landed where.
pub struct W<'z, R: BufRead> {
    s: &'z mut ZoneStream<R>,
    pos: [u32; BLOCK_COUNT],
    peak: [u32; BLOCK_COUNT],
    stack: Vec<Frame>,
    opts: WalkOptions,
    /// Pointer slots (and alias slots) holding an asset, by address:
    /// what `DB_ConvertOffsetToAlias` would read there.
    slots: HashMap<u32, AssetRef>,
    /// Assets by the address their struct landed at, for the types whose
    /// handles are plain pointers.
    objects: HashMap<u32, AssetRef>,
    /// Inline strings by address.
    strings: HashMap<u32, String>,
    /// Every loaded struct and array, start to length, so an offset token
    /// can be checked to land inside real data.
    spans: BTreeMap<u32, u32>,
    /// Shared sub-structs (a texture table, a technique) by kind and
    /// address, for offset tokens that name one.
    memo: HashMap<(&'static str, u32), Box<dyn Any>>,
    /// Kept [`Self::shared`] data and [`Self::pool`] arrays by kind and
    /// start, so an offset token into the middle of one copies its bytes.
    pools: HashMap<&'static str, BTreeMap<u32, Vec<u8>>>,
    scratch: Vec<u8>,
    pub out: Assets,
    pub stats: WalkStats,
}

impl<'z, R: BufRead> W<'z, R> {
    fn new(s: &'z mut ZoneStream<R>, opts: WalkOptions, virtual_pos: u32) -> Self {
        // DB_InitStreams starts every block at 0; the list itself went to
        // a static, not the temp block.
        let mut pos = [0; BLOCK_COUNT];
        pos[Block::Virtual as usize] = virtual_pos;
        Self {
            s,
            pos,
            peak: pos,
            stack: Vec::new(),
            opts,
            slots: HashMap::new(),
            objects: HashMap::new(),
            strings: HashMap::new(),
            spans: BTreeMap::new(),
            memo: HashMap::new(),
            pools: HashMap::new(),
            scratch: Vec::new(),
            out: Assets::default(),
            stats: WalkStats::default(),
        }
    }

    pub fn keep_bulk(&self) -> bool {
        self.opts.keep_bulk
    }

    fn block(&self) -> Result<Block> {
        Ok(self.stack.last().context("no block pushed")?.block)
    }

    /// `DB_PushStreamPos`.
    pub fn push(&mut self, block: Block) {
        let saved = self.pos[block as usize];
        self.stack.push(Frame { block, saved });
    }

    /// `DB_PopStreamPos`: only [`Block::Temp`]'s position rewinds.
    pub fn pop(&mut self) -> Result<()> {
        let f = self.stack.pop().context("block stack underflow")?;
        if f.block == Block::Temp {
            self.pos[Block::Temp as usize] = f.saved;
        }
        Ok(())
    }

    /// `DB_AllocStreamPos(a - 1)` in the current block.
    pub fn align(&mut self, a: u32) -> Result<()> {
        let b = self.block()? as usize;
        self.pos[b] = (self.pos[b] + a - 1) & !(a - 1);
        Ok(())
    }

    /// The packed address the current block's next byte lands at.
    pub fn here(&self) -> Result<u32> {
        let b = self.block()?;
        Ok(addr(b, self.pos[b as usize]))
    }

    fn advance(&mut self, n: usize) -> Result<Block> {
        let b = self.block()?;
        let i = b as usize;
        self.pos[i] = self.pos[i]
            .checked_add(u32::try_from(n)?)
            .context("block overflow")?;
        self.peak[i] = self.peak[i].max(self.pos[i]);
        Ok(b)
    }

    /// `Load_Stream(true, ...)`: `n` bytes into the current block. The
    /// runtime block is zero-filled and reads nothing.
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        match self.advance(buf.len())? {
            Block::Runtime => buf.fill(0),
            Block::LargeRuntime | Block::PhysicalRuntime => {
                bail!("deferred block data (no stock zone has any)")
            }
            _ => self.s.read_bytes(buf)?,
        }
        Ok(())
    }

    /// Records the `n` bytes about to land at the current position as a
    /// span an offset token may point into.
    fn note_span(&mut self, n: usize) -> Result<()> {
        if n > 0 {
            let at = self.here()?;
            self.spans.insert(at, n as u32);
        }
        Ok(())
    }

    /// Loads a `size`-byte struct at the current (already aligned)
    /// position.
    pub fn load(&mut self, size: usize) -> Result<S> {
        let at = self.here()?;
        if size > 0 {
            self.spans.insert(at, size as u32);
        }
        let mut b = vec![0; size];
        self.fill(&mut b)?;
        Ok(S { at, b })
    }

    /// Aligns, then loads `size` bytes: `AllocLoad_*` and `Load_Stream`.
    pub fn load_at(&mut self, align: u32, size: usize) -> Result<S> {
        self.align(align)?;
        self.load(size)
    }

    /// Bulk data: aligned, then `n` bytes, kept only under
    /// [`WalkOptions::keep_bulk`].
    pub fn bulk(&mut self, align: u32, n: usize) -> Result<Option<Vec<u8>>> {
        if self.opts.keep_bulk {
            return self.bulk_kept(align, n).map(Some);
        }
        self.align(align)?;
        self.note_span(n)?;
        let mut left = n;
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.resize(n.min(1 << 16), 0);
        while left > 0 {
            let k = left.min(scratch.len());
            self.fill(&mut scratch[..k])?;
            left -= k;
        }
        self.scratch = scratch;
        Ok(None)
    }

    /// [`Self::bulk`]'s bytes kept whatever the options say: what a small
    /// array the server needs from every zone reads (an xmodel's
    /// collision triangles).
    pub fn bulk_kept(&mut self, align: u32, n: usize) -> Result<Vec<u8>> {
        self.align(align)?;
        self.note_span(n)?;
        let mut b = vec![0; n];
        self.fill(&mut b)?;
        Ok(b)
    }

    /// Data a slot may share with an earlier struct: `n` bytes inline at
    /// `align`, pooled by address, or copied from pooled `kind` data an
    /// offset token names. `bulk` data is kept only under
    /// [`WalkOptions::keep_bulk`], and an offset to dropped data reads
    /// `None`.
    pub fn shared(
        &mut self,
        p: Pointer,
        align: u32,
        n: usize,
        kind: &'static str,
        bulk: bool,
    ) -> Result<Option<Vec<u8>>> {
        Ok(self.shared_at(p, align, n, kind, bulk)?.1)
    }

    /// [`Self::shared`], with the packed address the data sits at (0 for a
    /// null slot).
    pub fn shared_at(
        &mut self,
        p: Pointer,
        align: u32,
        n: usize,
        kind: &'static str,
        bulk: bool,
    ) -> Result<(u32, Option<Vec<u8>>)> {
        let keep = !bulk || self.opts.keep_bulk;
        Ok(match self.follow(p, align)? {
            Follow::Null => (0, None),
            Follow::Inline => {
                let at = self.here()?;
                if keep {
                    let b = self.bytes(1, n)?;
                    self.pool(kind, at, b.clone());
                    (at, Some(b))
                } else {
                    (at, self.bulk(1, n)?)
                }
            }
            Follow::At(a) => {
                if let Some(b) = self.pooled(kind, a, n) {
                    return Ok((a, Some(b)));
                }
                // Data another struct's array holds (a leaf's brushes into
                // the map's leaf brush list) that the walk did not keep.
                if self.span_at(a).is_some() {
                    self.stats.into_arrays += 1;
                } else {
                    self.unresolved(kind);
                }
                (a, None)
            }
        })
    }

    /// Records `b`, loaded at `at`, as `kind` data a later offset token
    /// may name the start or the middle of ([`Self::shared`]).
    pub fn pool(&mut self, kind: &'static str, at: u32, b: Vec<u8>) {
        self.pools.entry(kind).or_default().insert(at, b);
    }

    /// The `n` bytes at `a` inside pooled `kind` data, counting a token
    /// into the middle of an array in [`WalkStats::into_arrays`].
    fn pooled(&mut self, kind: &'static str, a: u32, n: usize) -> Option<Vec<u8>> {
        let (&start, b) = self.pools.get(kind)?.range(..=a).next_back()?;
        let o = (a - start) as usize;
        let out = b.get(o..o + n)?.to_vec();
        if o > 0 {
            self.stats.into_arrays += 1;
        }
        Some(out)
    }

    /// The start of the loaded span holding `a`, if any.
    fn span_at(&self, a: u32) -> Option<u32> {
        let (&start, &len) = self.spans.range(..=a).next_back()?;
        (a < start + len).then_some(start)
    }

    /// Bulk data that is always kept (small, or decoded by the caller).
    pub fn bytes(&mut self, align: u32, n: usize) -> Result<Vec<u8>> {
        self.align(align)?;
        self.note_span(n)?;
        let mut b = vec![0; n];
        self.fill(&mut b)?;
        Ok(b)
    }

    /// The slot's data: inline (the block aligned to `align` for it), at
    /// an earlier address, or absent. A -2 token on a slot that is not an
    /// asset handle is refused.
    pub fn follow(&mut self, p: Pointer, align: u32) -> Result<Follow> {
        Ok(match p {
            Pointer::Null => Follow::Null,
            Pointer::Inline => {
                self.align(align)?;
                Follow::Inline
            }
            Pointer::InlineShared => bail!("-2 token on a plain pointer"),
            Pointer::Offset { block, offset } => Follow::At((block as u32) << 28 | offset),
        })
    }

    /// Whether the slot is followed inline; a slot retail only tests for
    /// null (no offset path) reads any other value as inline too.
    pub fn inline_only(&mut self, p: Pointer, align: u32) -> Result<bool> {
        if p == Pointer::Null {
            return Ok(false);
        }
        self.align(align)?;
        Ok(true)
    }

    /// `Load_XString`: a NUL-terminated string at alignment 1, or one
    /// already loaded.
    pub fn xstring(&mut self, p: Pointer) -> Result<Option<String>> {
        match self.follow(p, 1)? {
            Follow::Null => Ok(None),
            Follow::Inline => {
                let at = self.here()?;
                let bytes = self.s.read_cstr()?;
                self.advance(bytes.len() + 1)?;
                let s: String = bytes.iter().map(|&c| c as char).collect();
                self.strings.insert(at, s.clone());
                Ok(Some(s))
            }
            Follow::At(a) => match self.strings.get(&a) {
                Some(s) => Ok(Some(s.clone())),
                None => {
                    self.unresolved("string");
                    Ok(None)
                }
            },
        }
    }

    /// `Load_XStringArray`: `n` string slots loaded at once, then each
    /// string.
    pub fn xstring_array(&mut self, align: u32, n: usize) -> Result<Vec<Option<String>>> {
        let slots = self.load_at(align, 4 * n)?;
        self.xstrings_in(&slots, 0, n)
    }

    /// The `n` string slots at `o` of an already loaded struct.
    pub fn xstrings_in(&mut self, s: &S, o: usize, n: usize) -> Result<Vec<Option<String>>> {
        (0..n).map(|i| self.xstring(s.ptr(o + 4 * i))).collect()
    }

    /// `Load_ScriptString`: a script string index, kept as is.
    pub fn unresolved(&mut self, what: &'static str) {
        *self.stats.unresolved.entry(what).or_default() += 1;
    }

    /// An asset handle (`Load_*Ptr`): `temp` pushes [`Block::Temp`] around
    /// it, as the reusable types' loaders do, and a -2 token reserves the
    /// alias slot first. An offset token reads the asset recorded at that
    /// slot (`DB_ConvertOffsetToAlias`), or, for `alias == false`, at that
    /// struct address. `slot` is where the handle itself sits.
    pub fn handle(
        &mut self,
        p: Pointer,
        slot: u32,
        how: Handle,
        what: &'static str,
        load: impl FnOnce(&mut Self) -> Result<AssetRef>,
    ) -> Result<Option<AssetRef>> {
        if how.temp {
            self.push(Block::Temp);
        }
        let r = match p {
            Pointer::Null => None,
            Pointer::Inline | Pointer::InlineShared => {
                ensure!(
                    p == Pointer::Inline || how.insert,
                    "-2 token on a {what} handle"
                );
                self.align(4)?;
                let alias = (p == Pointer::InlineShared).then(|| self.alias_slot());
                let at = self.here()?;
                let r = load(self)?;
                self.objects.insert(at, r);
                if let Some(a) = alias {
                    self.slots.insert(a, r);
                }
                Some(r)
            }
            Pointer::Offset { block, offset } => {
                let a = (block as u32) << 28 | offset;
                let found = if how.alias {
                    self.slots.get(&a)
                } else {
                    self.objects.get(&a)
                };
                match found.copied() {
                    Some(r) => Some(r),
                    None => {
                        self.unresolved(what);
                        None
                    }
                }
            }
        };
        if how.temp {
            self.pop()?;
        }
        if let Some(r) = r {
            self.slots.insert(slot, r);
        }
        Ok(r)
    }

    /// Records `v` as the `kind` struct loaded at `at`.
    pub fn remember<T: Any>(&mut self, kind: &'static str, at: u32, v: T) {
        self.memo.insert((kind, at), Box::new(v));
    }

    /// The `kind` struct recorded at `at`, counted as unresolved when
    /// there is none.
    pub fn recall<T: Any + Clone>(&mut self, kind: &'static str, at: u32) -> Option<T> {
        let v = self
            .memo
            .get(&(kind, at))
            .and_then(|v| v.downcast_ref::<T>())
            .cloned();
        if v.is_none() {
            self.unresolved(kind);
        }
        v
    }

    /// `DB_InsertPointer`: a 4-byte alias slot in [`Block::Virtual`].
    pub fn alias_slot(&mut self) -> u32 {
        let v = Block::Virtual as usize;
        self.pos[v] = (self.pos[v] + 3) & !3;
        let a = addr(Block::Virtual, self.pos[v]);
        self.pos[v] += 4;
        self.peak[v] = self.peak[v].max(self.pos[v]);
        a
    }
}

/// How a type's `Load_*Ptr` treats its handle.
#[derive(Clone, Copy, Debug)]
pub struct Handle {
    /// Pushes [`Block::Temp`] around the body.
    pub temp: bool,
    /// Accepts -2 (inserts an alias slot).
    pub insert: bool,
    /// An offset names an alias slot (`DB_ConvertOffsetToAlias`) rather
    /// than the struct (`DB_ConvertOffsetToPointer`).
    pub alias: bool,
}

impl Handle {
    /// The reusable asset types: temp, insert, alias.
    pub const ASSET: Handle = Handle {
        temp: true,
        insert: true,
        alias: true,
    };
    /// A plain pointer to a struct loaded in the current block.
    pub const PLAIN: Handle = Handle {
        temp: false,
        insert: false,
        alias: false,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::AssetRef;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// A rawfile inline, a localize entry inline behind an alias slot, and
    /// a second localize header naming that slot.
    fn zone() -> Vec<u8> {
        const INLINE: u32 = u32::MAX;
        let alias = (4 << 28 | 32) + 1;
        let mut body = le(&[0, 0, 3, INLINE]);
        body.extend(le(&[0x1F, INLINE, 0x16, INLINE - 1, 0x16, alias]));
        body.extend(le(&[INLINE, 3, INLINE]));
        body.extend(b"a\0xyz\0");
        body.extend(le(&[INLINE, INLINE]));
        body.extend(b"v\0n\0");
        let mut inflated = le(&[body.len() as u32, 0, 28, 0, 0, 0, 40, 0, 0, 0, 0]);
        inflated.extend(body);
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(&inflated).unwrap();
        let mut out = b"IWffu100".to_vec();
        out.extend(5u32.to_le_bytes());
        out.extend(z.finish().unwrap());
        out
    }

    #[test]
    fn walks_blocks_and_resolves_alias_slots() {
        let data = zone();
        let w = walk(Zone::open(&data[..]).unwrap(), WalkOptions::default()).unwrap();
        assert_eq!(w.assets.raw_files[0].name, "a");
        assert_eq!(w.assets.raw_files[0].data, b"xyz");
        assert_eq!(w.assets.localize.len(), 1);
        assert_eq!(w.assets.localize[0].value.as_deref(), Some("v"));
        assert!(matches!(w.list[1], Some(AssetRef::Localize(_))));
        assert_eq!(w.list[2], w.list[1]);
        // The list's 24 bytes, the strings, the alias slot at 32.
        assert_eq!(w.stats.end[Block::Virtual as usize], 40);
        assert_eq!(w.stats.peak[Block::Temp as usize], 12);
        assert_eq!(w.stats.end[Block::Temp as usize], 0);
        assert!(w.stats.unresolved.is_empty());
    }
}
