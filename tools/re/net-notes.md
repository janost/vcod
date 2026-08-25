# cod_lnxded 1.1d disassembly notes

Disassembly notes for the 1.1d Linux dedicated server, `cod_lnxded`
(md5 `49717db56f6da717545838ce88d4865e`, ELF32 i386), read with `objdump -d -j .text`.
Addresses are virtual. They back the playerstate delta reader in
`crates/common/src/net/msg.rs`.

## MSG_ReadDeltaPlayerstate (0x807e2f0)

Layout (matches `crates/common/src/net/msg.rs::read_delta_playerstate`):

1. `lc = MSG_ReadByte` (whole byte).
2. Main field loop, fields `0..lc` of the 103-entry playerStateFields table
   (0x80d229c). Per field: 1 change bit; if changed and `bits == 0`, a float
   with a single integral/full selector (no zero flag, unlike the entity delta);
   if changed and `bits != 0`, `read_packed_bits(|bits|)` with no sign
   extension (0x807e5d7..0x807e70a assembles the value unsigned).
3. Trailing array blocks, all discarded by the spectator but consumed to stay
   aligned for the packet entities that follow (0x807e7b3..0x807eeb6):
   - **Block 1** (gated): 6-bit mask, then per set bit a short, short, short,
     6-bit int, short, byte.
   - **Block 2** (group gate + 4 sub): each sub gated, a 16-bit changed mask then
     a short per set bit.
   - **Block 3** (no group gate, 4 sub): same shape as a block-2 sub. Block 2's
     gate `jae` lands on block 3's first sub, so block 3 always reads its four
     sub-gates.
   - **Block 4** (gated): 16 weapons, each a 3-bit value then a gate; if set, six
     delta fields off HUD table entries 0..6 (origin[0..2], icon/12, entNum/10,
     teamNum/4) via the generic field reader (0x807c904).
   - **Block 5** (gated): two HUD-element arrays via 0x807cf5c. Each array is a
     5-bit outer count, then per entry a 5-bit field count `j` and `j+1` delta
     fields off HUD table entries `6+k`.

The generic per-field reader 0x807c904 (used by blocks 4/5 and the entity delta)
carries the zero flags; the playerState main loop does not.

## HUD-element field table (0x80de384, 34 entries)

No symbol; widths read out of `.data` with `tools/re/dump_field_table.py`. Entries 0..6
back block 4, 6..34 back the block-5 arrays. See `HUD_FIELD_BITS` in `crates/common/src/net/msg.rs`.

## MSG_ReadDeltaUsercmdKey (0x807b7f8)

Transcribed in full, with per-field VAs, in `docs/protocol-1.1.md` under "Client
to server message body (clc)".
