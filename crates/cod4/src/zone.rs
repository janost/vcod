//! CoD4 1.7 fastfiles (`zone/<language>/*.ff`): the container, the `XFile`
//! header, the script strings and the asset list. Asset bodies follow the
//! list in the inflated stream; [`Zone::stream`] is left positioned at the
//! first one. Layout and sources: `docs/research/cod4-fastfile.md`.
//!
//! The zone is inflated as it is read, never held whole: a map zone runs
//! to a quarter gigabyte inflated.

use anyhow::{Context, Result, bail, ensure};
use flate2::bufread::ZlibDecoder;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

/// The unsigned fastfile magic. The signed `IWff0100` is refused, as
/// retail refuses it ("authenticated file not supported").
pub const MAGIC: &[u8; 8] = b"IWffu100";
pub const SIGNED_MAGIC: &[u8; 8] = b"IWff0100";
/// `FASTFILE_VERSION` of 1.7.
pub const VERSION: u32 = 5;
/// `sizeof(XFile)`: `size`, `externalSize`, `blockSize[9]`.
pub const XFILE_SIZE: u64 = 0x2C;
/// `sizeof(XAssetList)`: the script string list, `assetCount`, `assets`.
pub const ASSET_LIST_SIZE: u64 = 0x10;
pub const BLOCK_COUNT: usize = 9;

/// More than any stock zone holds by two orders; a count past it is a
/// corrupt file, not an allocation to make.
const MAX_LIST: u32 = 1 << 20;

/// The nine stream blocks, in `blockSize` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Block {
    Temp = 0,
    Runtime = 1,
    LargeRuntime = 2,
    PhysicalRuntime = 3,
    Virtual = 4,
    Large = 5,
    Physical = 6,
    Vertex = 7,
    Index = 8,
}

impl Block {
    pub const ALL: [Block; BLOCK_COUNT] = [
        Block::Temp,
        Block::Runtime,
        Block::LargeRuntime,
        Block::PhysicalRuntime,
        Block::Virtual,
        Block::Large,
        Block::Physical,
        Block::Vertex,
        Block::Index,
    ];

    pub fn from_index(i: u32) -> Option<Self> {
        Self::ALL.get(i as usize).copied()
    }
}

/// The `XFile` header at the start of the inflated stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XFileHeader {
    /// Inflated bytes after this header.
    pub size: u32,
    /// Bytes loaded from outside the zone (image pixels in iwds); only a
    /// progress figure to the loader.
    pub external_size: u32,
    /// Bytes each [`Block`] takes once loaded.
    pub block_size: [u32; BLOCK_COUNT],
}

/// A pointer slot as stored on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// 0.
    Null,
    /// -1: the data follows in the stream, aligned in its block.
    Inline,
    /// -2: as [`Pointer::Inline`], after a 4-byte alias slot reserved in
    /// [`Block::Virtual`] that later offset tokens may name.
    InlineShared,
    /// Anything else: `token - 1` is `block << 28 | offset`, data already
    /// streamed (or, for an alias kind, its alias slot). `block` may be out
    /// of [`Block`]'s range in a slot retail never reads.
    Offset { block: u8, offset: u32 },
}

impl Pointer {
    pub fn from_token(token: u32) -> Self {
        match token {
            0 => Self::Null,
            0xFFFF_FFFF => Self::Inline,
            0xFFFF_FFFE => Self::InlineShared,
            v => {
                let v = v - 1;
                Self::Offset {
                    block: (v >> 28) as u8,
                    offset: v & 0x0FFF_FFFF,
                }
            }
        }
    }

    /// Whether the data behind the slot follows in the stream.
    pub fn is_inline(self) -> bool {
        matches!(self, Self::Inline | Self::InlineShared)
    }
}

/// `XAssetType` of 1.7, in the order of the binary's name table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AssetType {
    XModelPieces = 0x00,
    PhysPreset = 0x01,
    XAnimParts = 0x02,
    XModel = 0x03,
    Material = 0x04,
    TechniqueSet = 0x05,
    Image = 0x06,
    Sound = 0x07,
    SndCurve = 0x08,
    LoadedSound = 0x09,
    ClipMap = 0x0A,
    ClipMapPvs = 0x0B,
    ComWorld = 0x0C,
    GameWorldSp = 0x0D,
    GameWorldMp = 0x0E,
    MapEnts = 0x0F,
    GfxWorld = 0x10,
    LightDef = 0x11,
    UiMap = 0x12,
    Font = 0x13,
    MenuList = 0x14,
    Menu = 0x15,
    Localize = 0x16,
    Weapon = 0x17,
    SndDriverGlobals = 0x18,
    Fx = 0x19,
    ImpactFx = 0x1A,
    AiType = 0x1B,
    MpType = 0x1C,
    Character = 0x1D,
    XModelAlias = 0x1E,
    RawFile = 0x1F,
    StringTable = 0x20,
}

impl AssetType {
    pub const COUNT: usize = 0x21;

    pub const ALL: [AssetType; Self::COUNT] = [
        Self::XModelPieces,
        Self::PhysPreset,
        Self::XAnimParts,
        Self::XModel,
        Self::Material,
        Self::TechniqueSet,
        Self::Image,
        Self::Sound,
        Self::SndCurve,
        Self::LoadedSound,
        Self::ClipMap,
        Self::ClipMapPvs,
        Self::ComWorld,
        Self::GameWorldSp,
        Self::GameWorldMp,
        Self::MapEnts,
        Self::GfxWorld,
        Self::LightDef,
        Self::UiMap,
        Self::Font,
        Self::MenuList,
        Self::Menu,
        Self::Localize,
        Self::Weapon,
        Self::SndDriverGlobals,
        Self::Fx,
        Self::ImpactFx,
        Self::AiType,
        Self::MpType,
        Self::Character,
        Self::XModelAlias,
        Self::RawFile,
        Self::StringTable,
    ];

    /// The engine's own names (`g_assetNames` in `cod4_lnxded-bin`).
    const NAMES: [&'static str; Self::COUNT] = [
        "xmodelpieces",
        "physpreset",
        "xanim",
        "xmodel",
        "material",
        "techset",
        "image",
        "sound",
        "sndcurve",
        "loaded_sound",
        "col_map_sp",
        "col_map_mp",
        "com_map",
        "game_map_sp",
        "game_map_mp",
        "map_ents",
        "gfx_map",
        "lightdef",
        "ui_map",
        "font",
        "menufile",
        "menu",
        "localize",
        "weapon",
        "snddriverglobals",
        "fx",
        "impactfx",
        "aitype",
        "mptype",
        "character",
        "xmodelalias",
        "rawfile",
        "stringtable",
    ];

    pub fn from_u32(v: u32) -> Option<Self> {
        Self::ALL.get(v as usize).copied()
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }

    /// Whether the 1.7 server's `Load_XAssetHeader` loads a header of this
    /// type at all. The others fall through its switch: their slot is
    /// never read and no body follows in the stream.
    pub fn has_loader(self) -> bool {
        !matches!(
            self,
            Self::XModelPieces
                | Self::UiMap
                | Self::SndDriverGlobals
                | Self::AiType
                | Self::MpType
                | Self::Character
                | Self::XModelAlias
        )
    }
}

/// One `XAsset` of the list: its type and its header pointer slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AssetEntry {
    pub kind: AssetType,
    pub header: Pointer,
}

/// The inflated stream, counting bytes from the start of the `XFile`.
pub struct ZoneStream<R: BufRead> {
    inner: BufReader<ZlibDecoder<R>>,
    pos: u64,
}

impl<R: BufRead> ZoneStream<R> {
    /// Offset in the inflated stream; the `XFile` header starts at 0.
    pub fn pos(&self) -> u64 {
        self.pos
    }

    pub fn read_bytes(&mut self, buf: &mut [u8]) -> Result<()> {
        self.inner
            .read_exact(buf)
            .with_context(|| format!("zone truncated at inflated offset {:#x}", self.pos))?;
        self.pos += buf.len() as u64;
        Ok(())
    }

    pub fn read_u32(&mut self) -> Result<u32> {
        let mut b = [0; 4];
        self.read_bytes(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// A NUL-terminated string, the NUL consumed but not returned.
    pub fn read_cstr(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        let n = self.inner.read_until(0, &mut out)?;
        self.pos += n as u64;
        ensure!(out.pop() == Some(0), "zone ends inside a string");
        Ok(out)
    }

    /// Inflates what is left, returning the total inflated length; fails
    /// on compressed bytes past the end of the zlib stream.
    fn finish(mut self) -> Result<u64> {
        self.pos += std::io::copy(&mut self.inner, &mut std::io::sink())?;
        let mut rest = self.inner.into_inner().into_inner();
        ensure!(rest.fill_buf()?.is_empty(), "bytes after the zlib stream");
        Ok(self.pos)
    }
}

/// A fastfile read up to its asset bodies.
pub struct Zone<R: BufRead> {
    pub header: XFileHeader,
    /// The script string table; `None` where the slot was null.
    pub script_strings: Vec<Option<String>>,
    pub assets: Vec<AssetEntry>,
    stream: ZoneStream<R>,
    /// [`Block::Virtual`] bytes the list took (string pointers, strings,
    /// asset array): where the first body's virtual data starts.
    virtual_pos: u32,
}

impl Zone<BufReader<File>> {
    pub fn open_path(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("{}", path.display()))?;
        Self::open(BufReader::with_capacity(1 << 16, file))
            .with_context(|| format!("{}", path.display()))
    }
}

impl<R: BufRead> Zone<R> {
    /// Reads the container header, the `XFile` and the asset list.
    pub fn open(mut reader: R) -> Result<Self> {
        let mut head = [0; 12];
        reader.read_exact(&mut head).context("fastfile header")?;
        let magic = &head[..8];
        if magic == SIGNED_MAGIC {
            bail!(
                "signed fastfile ({}) not supported",
                String::from_utf8_lossy(magic)
            );
        }
        ensure!(magic == MAGIC, "not a fastfile: magic {:02x?}", magic);
        let version = u32::from_le_bytes(head[8..12].try_into().unwrap());
        ensure!(
            version == VERSION,
            "fastfile version {version}, expected {VERSION}"
        );

        let mut stream = ZoneStream {
            inner: BufReader::with_capacity(1 << 16, ZlibDecoder::new(reader)),
            pos: 0,
        };
        let size = stream.read_u32()?;
        let external_size = stream.read_u32()?;
        let mut block_size = [0; BLOCK_COUNT];
        for b in &mut block_size {
            *b = stream.read_u32()?;
        }
        let header = XFileHeader {
            size,
            external_size,
            block_size,
        };

        // XAssetList: ScriptStringList {count, strings}, assetCount, assets.
        let string_count = stream.read_u32()?;
        let strings_token = stream.read_u32()?;
        let asset_count = stream.read_u32()?;
        let assets_token = stream.read_u32()?;
        ensure!(
            string_count <= MAX_LIST,
            "script string count {string_count}"
        );
        ensure!(asset_count <= MAX_LIST, "asset count {asset_count}");

        // Everything below lands in the virtual block. Retail reads any
        // non-null array slot as inline.
        let mut vpos: u32 = 0;
        let mut script_strings = Vec::new();
        if strings_token != 0 {
            vpos = align(vpos, 3);
            let tokens: Vec<u32> = (0..string_count)
                .map(|_| stream.read_u32())
                .collect::<Result<_>>()?;
            vpos += 4 * string_count;
            // Virtual offset of each inline string, for offset tokens.
            let mut at: HashMap<u32, usize> = HashMap::new();
            for (i, &token) in tokens.iter().enumerate() {
                let s = match Pointer::from_token(token) {
                    Pointer::Null => None,
                    Pointer::Inline | Pointer::InlineShared => {
                        let bytes = stream.read_cstr()?;
                        at.insert(vpos, i);
                        vpos += bytes.len() as u32 + 1;
                        Some(latin1(&bytes))
                    }
                    Pointer::Offset { block, offset } => {
                        let j = (block == Block::Virtual as u8)
                            .then(|| at.get(&offset))
                            .flatten()
                            .with_context(|| {
                                format!(
                                    "script string {i}: token {token:#x} names no earlier string"
                                )
                            })?;
                        script_strings.get(*j).cloned().flatten()
                    }
                };
                script_strings.push(s);
            }
        }

        let mut assets = Vec::new();
        if assets_token != 0 {
            vpos = align(vpos, 3);
            for i in 0..asset_count {
                let kind = stream.read_u32()?;
                let header = Pointer::from_token(stream.read_u32()?);
                let kind = AssetType::from_u32(kind)
                    .with_context(|| format!("asset {i}: unknown type {kind:#x}"))?;
                assets.push(AssetEntry { kind, header });
            }
            vpos += 8 * asset_count;
        }

        Ok(Self {
            header,
            script_strings,
            assets,
            stream,
            virtual_pos: vpos,
        })
    }

    /// Inflated offset of the first asset body (the end of the list).
    pub fn body_offset(&self) -> u64 {
        self.stream.pos()
    }

    /// [`Block::Virtual`] bytes the list took.
    pub fn virtual_pos(&self) -> u32 {
        self.virtual_pos
    }

    /// The inflated stream, positioned at the first asset body until read.
    pub fn stream(&mut self) -> &mut ZoneStream<R> {
        &mut self.stream
    }

    /// Inflates the rest and checks the `XFile`'s `size` against it and
    /// that nothing trails the zlib stream. Returns the inflated length.
    pub fn finish(self) -> Result<u64> {
        let expect = XFILE_SIZE + u64::from(self.header.size);
        let len = self.stream.finish()?;
        ensure!(
            len == expect,
            "inflated {len} bytes, XFile size says {expect}"
        );
        Ok(len)
    }
}

/// `DB_AllocStreamPos`: `(p + a) & ~a`.
fn align(p: u32, a: u32) -> u32 {
    (p + a) & !a
}

/// The engine's strings are bytes; each maps to the char of its value.
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::ZlibEncoder;
    use std::io::Write;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// A fastfile holding an XFile and `list`, then `trailing` past the
    /// zlib stream.
    fn zone(list: &[u8], trailing: &[u8]) -> Vec<u8> {
        let mut inflated = le(&[list.len() as u32, 7, 1, 0, 0, 0, 64, 0, 0, 0, 0]);
        inflated.extend_from_slice(list);
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(&inflated).unwrap();
        let mut out = b"IWffu100".to_vec();
        out.extend_from_slice(&5u32.to_le_bytes());
        out.extend(z.finish().unwrap());
        out.extend_from_slice(trailing);
        out
    }

    fn list() -> Vec<u8> {
        // 3 strings (inline, null, offset to the first), 2 assets.
        let mut l = le(&[3, u32::MAX, 2, u32::MAX, u32::MAX, 0, (4 << 28 | 12) + 1]);
        l.extend_from_slice(b"tag_origin\0");
        l.extend(le(&[5, u32::MAX, 0x18, 0x0051_C354]));
        l.extend(le(&[u32::MAX])); // the first body's name slot
        l
    }

    #[test]
    fn reads_the_list_and_stops_at_the_first_body() {
        let data = zone(&list(), b"");
        let mut z = Zone::open(&data[..]).unwrap();
        assert_eq!(z.header.size as usize, list().len());
        assert_eq!(z.header.block_size[Block::Virtual as usize], 64);
        assert_eq!(
            z.script_strings,
            [Some("tag_origin".into()), None, Some("tag_origin".into())]
        );
        assert_eq!(z.assets.len(), 2);
        assert_eq!(
            z.assets[0],
            AssetEntry {
                kind: AssetType::TechniqueSet,
                header: Pointer::Inline
            }
        );
        assert_eq!(z.assets[1].kind, AssetType::SndDriverGlobals);
        assert!(!z.assets[1].kind.has_loader());
        // 12 pointer bytes + 11 string bytes, aligned to 24, + 16.
        assert_eq!(z.virtual_pos(), 40);
        assert_eq!(z.body_offset(), XFILE_SIZE + ASSET_LIST_SIZE + 12 + 11 + 16);
        assert_eq!(z.stream().read_u32().unwrap(), u32::MAX);
        assert_eq!(z.finish().unwrap(), XFILE_SIZE + list().len() as u64);
    }

    #[test]
    fn refuses_trailing_bytes_and_signed_zones() {
        let data = zone(&list(), b"x");
        assert!(Zone::open(&data[..]).unwrap().finish().is_err());
        let mut signed = zone(&list(), b"");
        signed[..8].copy_from_slice(SIGNED_MAGIC);
        assert!(Zone::open(&signed[..]).is_err());
    }

    #[test]
    fn decodes_pointer_tokens() {
        assert_eq!(Pointer::from_token(0), Pointer::Null);
        assert_eq!(Pointer::from_token(u32::MAX), Pointer::Inline);
        assert_eq!(Pointer::from_token(u32::MAX - 1), Pointer::InlineShared);
        assert_eq!(
            Pointer::from_token((7 << 28 | 0x40) + 1),
            Pointer::Offset {
                block: 7,
                offset: 0x40
            }
        );
    }
}
