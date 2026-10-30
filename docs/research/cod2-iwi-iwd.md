# CoD2 1.3 archives and images: `.iwd` and `.iwi`

How CoD2 1.3 builds its search path from `.iwd` archives and how it reads an
`.iwi` image, as vcod's `Pk3Fs::open_iwd` and `crate::iwi` reproduce them.

Module: the Mac 1.3 multiplayer build, the thinned i386 slice
`cod2mp_mac13_i386` (md5 `b30966627b680de20bf35603c562a3f6`, from the 1.3
patch installer; local copy under `private/cod2mac13/`). It carries C++
symbols, so every function below is named by its own symbol. Addresses are
that slice's virtual addresses. Assets: a stock CoD2 1.3 install, `main/`
holding `iw_00.iwd`..`iw_15.iwd` and `localized_english_iw00.iwd`..`iw11.iwd`.

## 1. The search path

An `.iwd` is a plain zip, deflate or stored; Python's `zipfile` and the
`zip` crate read every stock one. VERIFIED (the install).

`FS_AddGameDirectory(path, dir, localized, language)` (0x34ffe) pushes the
directory's own search path entry, then calls
`FS_AddIwdFilesForGameDirectory(path, dir)` (0x3470c). VERIFIED (the call
at the function's end). That function:

- lists `*.iwd` in the directory through `Sys_ListFiles(dir, "iwd", ...)`,
  at most 0x400 of them (a warning names the cap). VERIFIED (the call's
  string argument and the 0x400 compare).
- overwrites the first ten bytes of every name starting `localized_` with
  spaces, `qsort`s the list with `iwdsort` (0x34412), and writes
  `localized_` back. VERIFIED (the `0x20202020` stores, the `qsort` call,
  the stores of `"loca" "lize" "d_"`).
- opens each archive in sorted order and links it in at the head of
  `fs_searchpaths`, so the last sorted is searched first. A localized one is
  linked in ahead of the first localized entry instead, under every
  non-localized one already at the head. INFERRED (the list insert's branch
  on the head entry's localized word at +0xc).

`iwdsort` compares two names case-blind, upper-casing lower-case letters
and reading `\` and `:` as `/`. VERIFIED (the `I_islower` / `-0x20` pair and
the 0x5c, 0x3a, 0x2f constants). When both names start with ten spaces it
first reads each one's language, the letters after the ten bytes, and an
`english` one sorts ahead of any other language. INFERRED (the two
`I_stricmp(..., "english")` branches returning -1 and 1).

The result for one directory, highest priority first: the plain iwds in
descending name order, then the localized ones, English last. A mod
directory added after `main` goes over `main`'s plain iwds and its
localized iwds over `main`'s localized ones. INFERRED (from the two rules
above). It is the shape CoD1's `search_paks` already gives `localized_*`
paks; CoD2 differs from 1.1 in the name compare (upper-case where 1.1
lower-cases, no `mp_` rule) and the English tie-break. On the stock install
the plain order is `iw_15` down to `iw_00`. VERIFIED (vcod's
`cod2_search_path_reads_and_overrides` against the install).

Overrides on the stock install: `iw_15` replaces 8 of `iw_06`'s files
(`ui_mp/menus.txt`, five `ui_mp` menus, three team menus) and
`maps/mp/mp_toujane.gsc` of `iw_07`. VERIFIED (the archives' listings and
contents).

`localized_english_iw*` mounts the English language. A localized name whose
language `SEH_GetLanguageIndexForName` does not know is skipped with a
warning. VERIFIED (the two warning strings). vcod does not check the
language yet.

## 2. The `.iwi` header

`Image_LoadFromFile` (0xfcb88) reads `images/<name>.iwi`, checks the low
three bytes against `IWi` and byte 3 against 5, and hands the header and
the bytes from offset 0x1c to `Image_LoadFromData` (0xfc5fa). VERIFIED (the
`"%s%s%s", "images/", ".iwi"` format, the `& 0xffffff` compare, the version
compare and its error string, the `+ 0x1c`).

| Offset | Size | Field |
|---|---|---|
| 0 | 3 | `IWi` |
| 3 | 1 | version, 5 |
| 4 | 1 | format (section 3) |
| 5 | 1 | flags |
| 6 | 2 | width |
| 8 | 2 | height |
| 10 | 2 | depth |
| 12 | 4 x 4 | file offsets where levels 0..3 end |

Offsets 0..12 VERIFIED (the loaders read `+4`, `+5`, `+6`, `+8`, `+10`).
The four ends VERIFIED against every stock image (vcod's `cod2_iwi_census`):
level 0, the largest, ends at the file's end; an end for a level the image
does not have repeats the smallest level's end. No loader reads them; the
pixels are consumed in sequence from 0x1c. INFERRED (no read of `+12..+28`
in the four loaders).

Flags, from `Image_Setup` (0x109218) and `Image_LoadFromFile`. VERIFIED
(the bit tests):

| Bit | Meaning |
|---|---|
| 0x1 | no picmip |
| 0x2 | no mipmaps (one level); also no picmip |
| 0x4 | cube map: `Image_CreateCubeTexture` |
| 0x8 | volume: `Image_Create3DTexture`; no stock image sets it |

Levels: one under 0x2, else one per halving down to 1 of the largest of
width, height and depth. They are stored smallest first; a level's
dimension is the side shifted right by the level, at least 1. A cube map
holds its six faces together within each level, in `D3DCUBEMAP_FACES`
order (`Image_CubemapFace` (0x108ff8) returns its argument). INFERRED (the
loop shapes of `Image_LoadBitmap` and `Image_LoadDxtc`). The stock cube
maps' sizes match six faces a level. VERIFIED (the census).

## 3. Formats

`Image_LoadFromData`'s jump table at 0x32b7a0 (`__TEXT,__const`) and the
arguments each case passes. VERIFIED (the table words and the immediates at
0xfc621..0xfc747):

| Byte | Loader | `D3DFORMAT` | Bytes per pixel or block |
|---|---|---|---|
| 1 | `Image_LoadBitmap` | `A8R8G8B8` (0x15) | 4 |
| 2 | `Image_LoadBitmap` | `X8R8G8B8` (0x16) | 3 |
| 3 | `Image_LoadBitmap` | `A8L8` (0x33) | 2 |
| 4 | `Image_LoadBitmap` | `L8` (0x32) | 1 |
| 5 | `Image_LoadBitmap` | `A8` (0x1c) | 1 |
| 6..10 | `Image_LoadWavelet` | as 1..5 | as 1..5 |
| 11 | `Image_LoadDxtc` | `DXT1` | 8 |
| 12 | `Image_LoadDxtc` | `DXT3` | 16 |
| 13 | `Image_LoadDxtc` | `DXT5` | 16 |

Byte 0 and anything past 13 load nothing. VERIFIED (the `cmp $0xd` bound
and table entry 0). A DXT level takes `ceil(w/4) * ceil(h/4)` blocks.
VERIFIED (`(w + 3) >> 2` in `Image_LoadDxtc`). Format 2 widens each pixel
to four bytes before the upload. VERIFIED (`Image_LoadBitmap`'s 0x16
branch).

Channel order: format 1 is stored B G R A and format 2 B G R, as the D3D
formats lay out in memory; format 3 is L then A. INFERRED (the D3D formats,
and on the install `mtl_egypt_well2.iwi`'s mean bytes 163, 183, 188 read as
a sandstone only with the last as red; `button_shadow.iwi`'s fourth byte is
its only non-zero channel).

Stock census, unique paths on the search path (vcod's `cod2_iwi_census`).
VERIFIED: 4899 images: DXT5 2547, DXT1 1836, DXT3 468, format 2 16,
format 1 5, format 3 2, format 6 13, format 7 12. 31 are cube maps. No
image uses formats 4, 5, 8, 9 or 10, or the volume flag.

## 4. Wavelet images (formats 6..10)

vcod decodes them (`crate::iwi`'s `wavelet` module). 25 stock images use
them, 13 format 6 and 12 format 7, among them menu art (`logo_cod2`,
`background_random`, `popups_alpha`) and a few world textures
(`crusader_vm_c`, `rocks_col_2`, `mtl_egypt_*`). Every one has
power-of-two sides, none is a cube. VERIFIED (the census).

### 4.1 The loader

`Image_LoadWavelet` (0xfc328) allocates one full-size buffer per face,
`width * height * out` bytes, where `out` is the source bytes per pixel
(4, 3, 2, 1, 1 for formats 6..10) except that format 7's 3 becomes 4. It
decodes level by level, smallest first, and within a level face by face,
each level into the tail of its face's buffer through
`Wavelet_DecompressLevel` (0x10a114), handing it the face's previous level
as the base. It uploads each level as decoded, before the next level
touches the base. On the Mac it byte-reverses every 32-bit word of the
copy it uploads. INFERRED (the loop, the `_memcpy` and the per-word swap
after it).

The decoder state, `WaveletDecode`, sits on the loader's stack and is
shared by every level and face: +0 a 16-bit bit window, +2 the bit
position within the byte, +4 the data pointer (the byte after the
header), +8 width, +0xc height, +0x10 source bytes per pixel, +0x14 output
bytes per pixel, +0x18 the level, +0x1c a started flag. INFERRED (the
loader's stack stores and the decoder's field reads).

### 4.2 The bit stream

A level whose width or height (each the full size shifted right by the
level, at least 1) is under 2 is stored raw: its pixels' source bytes
straight from the data pointer, `0xff` appended per pixel when `out`
exceeds the source size. Only the smallest levels are, so they all come
before the first coded level. INFERRED (the decoder's tail loop).

The first coded level starts the stream: the window takes the next two
bytes, little-endian. From then on the window always holds the next 16
bits of a least-significant-bit-first stream: taking `n` bits shifts the
window right by `n` and fills its top from a 32-bit read at the data
pointer shifted by the bit position. vcod reads the same stream with one
bit counter. INFERRED (the refill sequence repeated at every read).

A prefix code is read through a table of 4096 entries indexed by the
window's low 12 bits, each entry a signed 16-bit value and a 16-bit code
length; the length is taken. A value of -0x8000 escapes: the next 9 bits
less 255 (alpha and blue), or the next 10 bits less 510 (red and green),
are the value. INFERRED (the escape branches' masks 0x1ff and 0x3ff and
their biases).

The level's byte count follows: the stream after level `i` ends at the
byte holding its last bit, and that offset is the header's `level_ends[i]`
for every level the header lists and every one of the 25 stock files,
the last being the file's length. VERIFIED (vcod's `cod2_iwi_census` on
the install). That pins the bit order, the window and every table read.

### 4.3 A coded level

A level of `w x h` (both at least 2) refines a base of `w/2 x h/2`, the
previous level's pixels. INFERRED (the decoder's main loop):

1. One bit. When set, every source byte of the base takes a delta through
   the alpha table, added mod 256, pixel by pixel; with format 7 the base's
   fourth byte is set to `0xff`.
2. Then one 2x2 block per base pixel, row by row. Per channel, a rounding
   bit `r` and three details `d0 d1 d2`; with base value `b`, `s = 2b + d0`,
   `t = 2b - d0`:

   | Pixel | Value (`>>` arithmetic, kept mod 256) |
   |---|---|
   | top left | `((s + d1 + d2) >> 1) + r` |
   | top right | `(s - (d1 + d2)) >> 1` |
   | bottom left | `(t + (d1 - d2)) >> 1` |
   | bottom right | `(t - (d1 - d2)) >> 1` |

   Byte 0 (blue) reads its details through `waveletDecodeBlue`; bytes 1
   and 2 (green, red) each read a bit and three residuals through
   `waveletDecodeRedGreen`, added to blue's details; byte 3 (alpha) reads
   through `waveletDecodeAlpha` with no prediction. Format 7 writes `0xff`
   to every alpha. Order per block: blue, green, red, alpha.

The output is in D3D byte order, B G R A, as format 1 is stored: byte 0
reads `waveletDecodeBlue`. INFERRED (the table names). Decoded that way
the 25 images read as pictures (`background_random`'s desert panel sandy,
`logo_cod2`'s letters clean-edged over alpha 0); the by-eye check against
retail is pending. No other copy of any of the 25 ships as DXT or bitmap
(each name is one `.iwi`, plus materials and one xmodel naming it), so
there is no byte reference; the level ends of 4.2 are the exactness
check. VERIFIED (a listing of every `.iwd` for the 25 names).

Formats 8..10 (source 2, 1, 1 bytes) take the same code path: a two-byte
source codes byte 0 through the blue table and its last byte as alpha, a
one-byte source codes it as alpha alone. vcod ports that but no stock
image exercises it. INFERRED.

### 4.4 The tables

`waveletDecodeAlpha` (0x32b9c0), `waveletDecodeRedGreen` (0x32f9c0) and
`waveletDecodeBlue` (0x3339c0) in the Mac slice's `__TEXT` (file offset
VA - 0x1000). VERIFIED (the symbol table, the Mach-O `LC_SEGMENT`). The
same 16 KiB of each lies in the PC 1.3 install's
`gfx_d3d_mp_x86_s.dll` (md5 `2402bf2072f9255b85e3d23297d1d0ed`) at VA
0x10182988 (blue), 0x10186988 (red/green), 0x1018a988 (alpha), `.rdata`,
and at the same file offsets in `gfx_d3d_x86_s.dll`. Neither
`CoD2MP_s.exe` holds them. VERIFIED (a byte search of the install).

Each is a complete prefix code: every window sharing a code's low bits
holds the same entry, and the Kraft sum is 1. Alpha has 108 codes of 1 to
11 bits over -50..50 plus the escape and +-64, +-127, +-128; red/green 80
of 2 to 12 bits over -39..39; blue 180 of 3 to 12 bits over -89..89.
VERIFIED (the extracted tables).

Why vcod reads them from the install: the lengths are a Huffman code
fitted to Activision's own statistics (the outliers in the alpha set, the
uneven counts per length), so no rule rebuilds them, and the
`(value, length)` list that would is the same data. They are Activision's
tables and live in no game asset, so the repo carries none of them.
`Pk3Fs::open_iwd` hands the install root (the parent of `main/`) to
`iwi::load_wavelet_tables`, which reads `gfx_d3d_mp_x86_s.dll`, else
`gfx_d3d_x86_s.dll`, and finds each table by scanning for the prefix-code
shape and comparing an FNV-1a 64 hash committed in
`crates/common/src/iwi/wavelet.rs`; the Mac binary scans the same way
(`WaveletTables::find`). An install without the DLL logs a warning and its
wavelet images draw a 1x1 grey, as before (`iwi::decode_or_placeholder`).
