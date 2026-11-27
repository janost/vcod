# Call of Duty 4 1.7: voice chat

How 1.7 carries, relays and shows voice: the two out-of-band packets, the
server's relay rules, the client's codec, push to talk, the talker list,
the scoreboard's icon, Eavesdrop and the mute list. vcod's server relay is
`crates/server/src/cod4/voice.rs` over the rules CoD2 shares
(`crates/server/src/voice.rs`), the wire `vcod_common::net::voice` (CoD4's
rate in `vcod_cod4::net::voice`), the client `crates/client/src/voice.rs`.
CoD2 1.3's voice, which 1.7 inherits, is cod2-voice.md.

## 1. Sources

- `cod4_lnxded` (md5 `bf8448740b3f4cb74c2254f01af14cdd`, the binary of
  `cod4-handshake.md`), read in `objdump -d -M intel`. Stripped: each
  function below was found from a string xref or a call site. Function
  names are the Q3 lineage's. A claim labelled INFERRED that names no
  1.7 address is not yet confirmed on 1.7.
- `iw3mp.exe` 1.7 (image base 0x400000), the same way.
- Dvar defaults off `tools/re/cod4_dvars.py` over `cod4_lnxded`.
- One live run against vcod-server (section 5). There is no retail voice
  capture: the retail Linux server has no talking client, and no retail
  client was run.

## 2. The wire

Both directions ride out of band (`FF FF FF FF`), never the netchan.

- `SV_ConnectionlessPacket` compares the packet's first token with `v`
  (string 0x823df6f, `Q_stricmp` at 0x81776e9) and hands the rest to
  `SV_VoicePacket` (0x8177701). VERIFIED.
- Client to server (`CL_WriteVoicePacket`, `iw3mp.exe` 0x476660): `v`
  with its NUL (string 0x6d0b00), the qport (short), the packet count
  (byte), then per packet its length (byte) and its bytes, in a 0x800-byte
  buffer, sent by `NET_OutOfBandVoiceData` (0x508e80). VERIFIED.
- Server to client (`SV_SendClientVoiceData`, 0x817bab2): `v`, the count
  (byte), then per packet the talker's client number (byte), the length
  (byte) and the bytes. `NET_OutOfBandVoiceData` (0x813c980) writes
  `FF FF FF FF` and copies the message after it, no compression.
  VERIFIED.
- `SV_VoicePacket` finds the slot whose address and qport match (0x81777e8,
  0x81777f7), logs `SV_ReadPackets: fixing up a translated port`
  (0x8232424) and takes the new port when only the port differs, ignores a
  zombie (state 1), sets `lastPacketTime` to `svs.time` (0x8177a47), then
  calls `SV_PreGameUserVoice` (0x817bc58) for a state up to 3 and
  `SV_UserVoice` (0x817b95e) for an active client. INFERRED (the branches
  at 0x8177a39..0x8177b26). vcod refreshes `lastPacketTime` only for a packet
  from the slot's own port, and takes no new port (`Cod4Server::voice_packet`).
- `SV_UserVoice` returns at once with `sv_voice` (dvar 0x13ed7c78) off,
  reads the count, then per packet a length byte, refuses one outside
  1..256 with `Received invalid voice packet of size %i from %s`
  (0x8232d64) and stops, else reads the bytes and calls `G_BroadcastVoice`
  (0x80a7170). INFERRED (the loop at 0x817b9db).
- `SV_PreGameUserVoice` reads each length with the short reader (0x8130900,
  the one the qport is read with) where `SV_UserVoice` uses the byte
  reader (0x81308b8). VERIFIED (the two calls at 0x817bcc8, 0x817b9de).
  Since the client writes byte lengths, a loading client's frames misread
  there. INFERRED. It queues them for every other slot at state 2 or past
  that has not muted the talker and wants voice. INFERRED (0x817bd47..).
- `SV_QueueVoicePacket` (0x817ba20) keeps at most 40 frames a client
  (`cmp edx,0x27`). VERIFIED. `SV_SendClientVoiceData` sends only to a
  client at state 4 with frames queued, warns `WARNING: voice msg
  overflowed for %s` (0x8232d98) on an overflow, and empties the queue.
  INFERRED (0x817badc, 0x817bbbf, 0x817bc29). `SV_SendClientMessages`
  calls it after each slot's message (0x817b3d1). VERIFIED (the call
  site; cod4-server-frame.md 4).
- `CL_ConnectionlessPacket` hands `v` to `CL_VoicePacket` (0x4767b0; the
  compare at 0x46b60b). VERIFIED. That ignores a count over 40 (`cmp
  ebx,0x28`, 0x4767ed), stops on a length outside 1..256 (`Invalid server
  voice packet of %i bytes`, 0x6d2f18) or a talker past 63 (`Invalid voice
  packet - talker was %i`, 0x6d2f44), and drops a frame of a talker in the
  mute table (0xe11518) or with `cl_voice` (dvar 0xc5f8c0) off before
  `Voice_IncomingVoiceData` (0x57af60). INFERRED (0x4768b1..0x4768df).

## 3. The server's relay

`G_BroadcastVoice` (0x80a7170) sets the talker's `lastVoiceTime` (client
offset 0x30d4) to `level.time` (0x836f8ac). VERIFIED (0x80a7187). It then
walks the 64 entity slots in order, and for each in use with a client:
INFERRED (the loop at 0x80a71d1):

1. A listener at the intermission (`sessionState` 3, client offset
   0x2f64) passes to step 2. Otherwise one of: `voice_global` (0x84bc2c8)
   on; `OnSameTeam` (0x80c8c88: both clients, the first's team not 0, the
   two teams equal); the talker's team (offset 0x3010) 0; or the listener
   holds Eavesdrop (`ps.perks` bit 9, byte 0x5fd bit 1, 0x80a72aa) and the
   talker is inside `perk_parabolicAngle` (`cos(angle * pi / 180)` at most
   the dot of the unit vector to the talker with the listener's
   `clientinfo.playerAngles` forward, 0x80a7354) and within
   `perk_parabolicRadius` (0x80a7370). INFERRED.
2. The two session states equal, or either is dead (1) with
   `voice_deadChat` (0x84bc2cc) on. INFERRED (0x80a719f..0x80a71ab,
   0x80a726b).
3. Not the talker himself unless `voice_localEcho` (0x84bc2c4). INFERRED
   (0x80a721c, 0x80a7393).
4. The listener has not muted the talker (`SV_ClientHasClientMuted`,
   0x817b948, `muteList` at client offset 0xa35f8) and wants voice
   (`SV_ClientWantsVoiceData`, 0x817b934, `sendVoice` at 0xa3638).
   INFERRED (0x80a7231, 0x80a7241).

Then `SV_QueueVoicePacket(talker, listener, frame)`. INFERRED (0x80a7261).

The dvars, from `G_RegisterDvars` and `BG_RegisterDvars`. VERIFIED (the
registrations at 0x80b4c86..0x80b4d93, 0x8055e2c, 0x8055e5e and the
dump):

| Dvar | Type | Default | Flags |
|---|---|---|---|
| `sv_voice` | bool | 0 | 0xd |
| `sv_voiceQuality` | int 0..9 | 3 | 0x8 |
| `voice_global` | bool | 0 | 0x1 |
| `voice_localEcho` | bool | 0 | 0x1 |
| `voice_deadChat` | bool | 0 | 0x1 |
| `g_voiceChatTalkingDuration` | int 0..10000 | 500 | 0x1 |
| `perk_parabolicRadius` | float | 400 | 0x80 |
| `perk_parabolicAngle` | float 0..180 | 180 | 0x80 |

At the default 180 the angle test always passes, so Eavesdrop is a
400-unit sphere. INFERRED (cos 180 is -1).

- `sendVoice`: `SV_UserinfoChanged` sets it to `cl_voice` (0x82312ef)
  above 0 and clears it for a `rate` of 4999 or less (0x81703f8,
  0x81703ff). INFERRED. A LAN client's rate reads 99999 there
  (0x8170399). VERIFIED.
- `muteplayer <n>` (0x81702ae) and `unmuteplayer <n>` (0x817023c), client
  commands, set and clear `muteList[n]` for `n` 0..64 (`cmp eax,0x40;
  ja`), so 64 writes one past the 64 clients; anything else logs `Invalid
  mute client %i` (0x82312d1) or `Invalid unmute client %i` (0x82312b7).
  INFERRED (the bounds test).
- `self isTalking()` (`PlayerCmd_IsTalking`, 0x80a96f8) is 1 when
  `level.time - lastVoiceTime` is 0 or more and under
  `g_voiceChatTalkingDuration` (0x84bc2d0). INFERRED (0x80a9739..0x80a974b).

## 4. The client

- `cl_voice` is a bool, default 1, flags 3 (archived, userinfo; 0x46d74c).
  `cl_talking` is a bool, default 0, flags 0 (0x46e36a). VERIFIED.
- Push to talk is `+talk` / `-talk` (strings 0x6cf914, 0x6cf91c).
  VERIFIED. 1.7's stock binds put it on Z (vcod's `COD4_BINDS`, read off
  `default_mp_controls.cfg`).
- The codec is Speex. `Encode_Init` (0x4eca60) and `Decode_Init`
  (0x4ec880) pick the mode off the bandwidth setting (0xd5ec43c, 0 in
  `.bss`): the mode at 0x6a083c, whose frame is 160 samples, is
  narrowband. VERIFIED (its first word, 160). `Encode_SetOptions`
  (0x4ec9c0) sets the rate (`g_encoder_samplerate` 8000 at 0x724ed4), the
  quality (`g_encoder_quality` 1 at 0x724ed0), VAD (`ctl` 30) and DTX
  (`ctl` 34); the decoder gets the enhancer (`ctl` 0) and the rate.
  VERIFIED (the `ctl` numbers and the two words). `Encode_Sample` takes
  `sv_voiceQuality`, a systeminfo dvar, when it differs. INFERRED. The 1.7
  systeminfo carries 3 (cod4-handshake.md 4).
- At quality 3 narrowband Speex codes 8000 bit/s: 20 bytes a 20 ms frame.
  VERIFIED (libspeex 1.2's `SPEEX_GET_BITRATE` and the frame lengths, the
  test `cod4_voice_speex_frames_the_tone`).
- Each encoded frame is one voice packet (`Client_SendVoiceData`,
  0x46c800), and `CL_VoiceTransmit` (0x46c780) sends the batch once 200 ms
  passed since the last transmit or 10 frames wait, restarting the clock
  either way. INFERRED (0x46c7af, 0x46c7bb).
- `Voice_SendVoiceData` (0x46c850): `sv_voice` and `cl_voice` on, the
  `rate` dvar at 5000 or more, the connection active (state 9), and
  `cl_talking`, the talk key, or frames waiting. INFERRED. Frames are
  encoded only while the talk key is held
  (`Record_QueueAudioDataForEncoding`). INFERRED.
- `Voice_IncomingVoiceData` stamps the talker's time; a talker is talking
  while his last frame is under 300 ms old (0x476984, `cmp eax,0x12c`).
  INFERRED. The local client is talking while the talk key is held with
  `sv_voice` or `cl_voice` on (`CL_IsPlayerTalking`). INFERRED.
- The talker list is the `talkers` menu (rect 0 120 24 24), four items of
  24 by 24 at y 120, 144, 168 and 192, owner draws 193..196 (`menu_dump`
  of `common_mp`). VERIFIED. `CG_DrawTalkerNum` (0x443de4) takes the
  n-th talking client but us (`UI_GetTalkerClientNum`), skips
  one without valid client info (`client %i has invalid info but
  talking`, 0x6cced4), and reads `cg_drawTalk`, an enum `NONE`, `ALL`,
  `FRIENDLY`, `ENEMY` (0x726664) defaulting to `ALL` (0x440ed4).
  VERIFIED (the strings and the push). An enemy is a talker off our team
  while we are on one (0x443e43..0x443e65). INFERRED. With Eavesdrop
  (`nextSnap->ps.perks` bit 9, 0x443eaf) on an enemy, off the
  intermission (`pm_type` 5), the icon is `perk_parabolicIcon` (default
  `specialty_parabolic`) and the name takes the talker's relative team
  colour (0x431f40); otherwise `voice_on` (0x6ccf08). INFERRED. The name
  draws 2 past the icon's right edge, centred on its height. INFERRED.
- The scoreboard's talking column draws `voice_off` for a muted player and
  `voice_on` for a talking one (`LCT_TALKING_ICON`). INFERRED.
- The Mute Players page (`muteplayer` menu, list feeder 20, `uiScript
  mutePlayer`) toggles the client's mute table and sends `muteplayer %i`
  or `unmuteplayer %i` (0x6d24e8, 0x6d24f8). VERIFIED (the strings and the
  menu).
- `headicontalkballoon` is not voice: it draws over a teammate whose
  `eFlags` carries 0x200000, which pmove sets from the cmd's talk button
  (a menu or the console open). INFERRED (`CG_PlayerSprites`,
  `PM_UpdateViewAngles`).
- The `winvoice_*` dvars (`iw3mp.exe` 0x57a92e..0x57aa37, flags 1 each):
  `winvoice_mic_mute` bool 1 ("Mute the microphone" 0x6f5e58),
  `winvoice_mic_reclevel` float 65535 in 0..65535 (0x70bec0),
  `winvoice_save_voice` bool 0, `winvoice_mic_scaler` float 1 in
  0.25..2 (0x70b2b4, 0x700c0c). VERIFIED (asm, strings). As on CoD2
  (cod2-voice.md 4): the level, `ftol`'d to 16 bits (0x57aa5c, again on
  change at 0x57ae21), sets the `VOLUME` control (0x50030001) of the `Mic`
  (0x6f5f10) source under `DST_WAVEIN` (7) at 0x57b1d0, and the mute,
  once at init (0x57aa92), the `MUTE` control (0x20010002) of `Mic` under
  `DST_SPEAKERS` (4) at 0x57b860, both through `mixerSetControlDetails`
  (0x57b330, 0x57b9b8). VERIFIED (asm). The scaler is held to 0.5
  (0x6be648) .. 1.5 (0x70b4e0) into 0x71fb08 (0x57aedb..0x57af2e), and
  the capture stores each sample as `_ftol2(sample * scaler)`'s low 16
  bits (0x4ed14a..0x4ed165). VERIFIED (asm).

### 4.1 The winvoice dvars' readers

Each dvar's pointer is the register call's return (0x56c350), stored by
0x57a94f (`winvoice_mic_mute`, 0xcc1b4c4), 0x57a998 (`reclevel`,
0xcc1b3bc), 0x57a9e4 (`save_voice`, 0xcc1b6e8) and 0x57aa37 (`scaler`,
0xcc1b4c8). VERIFIED (the instructions).

- **`winvoice_mic_mute`.** The one read of 0xcc1b4c4 is at 0x57aa92, in
  the init, which passes the byte to 0x57b860, the `DST_SPEAKERS` `Mic`
  `MUTE` control (section 4). VERIFIED (the instructions: a search of the
  whole `.text` for the address finds only the store and that load). Nothing
  on the capture side reads it, and a later change of the dvar is not
  noticed. It is the mixer's speaker monitor of the microphone (sidetone),
  not the capture. INFERRED (the mixer's names). vcod plays no microphone
  monitor, so the dvar has nothing to drive: it stays registered and unread.
- **`winvoice_save_voice`.** The one read of 0xcc1b6e8 is at 0x57b024, the
  end of the function `Voice_IncomingVoiceData` calls for a packet whose
  talker's byte in the table at 0xe11518 is 0 and while the dvar at
  0xc5f8c0 (+0xc) is nonzero (0x4768b8..0x4768df). VERIFIED (the
  instructions). The table is the mute list and the dvar `sv_voice` or
  `cl_voice`. INFERRED. The function decodes the packet in chunks of at most
  0xd5ec444 bytes through `Decode_Sample` (0x4ec940, which returns twice
  the encoder's frame size 0xd5ec444, 320 for 160 samples, the Speex float
  output cast to 16 bits), hands each chunk to `Sound_UpdateSample`
  (0x4ecc40), then with the dvar nonzero calls `fopen("voice.wav", "ab")`
  (strings 0x6f5f18, 0x6f5f14; 0x671e8f), `fwrite(buffer, ebp, 1, file)`
  (0x6722b8) and `fclose` (0x671d4f). VERIFIED (the instructions,
  0x57b024..0x57b05b). `ebp` is the last chunk's `Decode_Sample` return, so
  a packet writes its last decoded frame only: 160 samples, 320 bytes,
  raw little-endian 16-bit mono at 8000 Hz. No RIFF header is built
  anywhere in the function, the name is a misnomer. VERIFIED (the
  instructions). The file opens and closes for each packet, in the process's
  working directory, and a failed `fopen` skips the write. VERIFIED (the
  instructions). The samples are the decoded ones, after the talker's
  stream is updated; the local microphone is never written (the function
  has no capture caller). INFERRED. A packet of several chunks
  writes only the last chunk's frame. VERIFIED (the instructions:
  `ebp` is overwritten each pass).
- **The record level meter.** `voicechat_level_indicator`, the last item of
  `options_voice` (`menu_dump ui_mp options_voice`), type 8, rect 446 106
  100 10, owner draw 265, colour white, no background and no dvar. VERIFIED
  (zone walk). `UI_OwnerDraw` (0x544820) takes `ownerDraw - 205` through
  the byte table at 0x544aa8 into the jump table at 0x544a78: 265 is index
  60, byte 8, arm 0x544a1e, which calls 0x5443a0. VERIFIED (the
  instructions). That function is the meter's draw. INFERRED (the name).
  - The level: 0x57ad60 returns 0 until the voice init set 0xd5ec510
    (0x57aade). Else it takes 0xcc1b4cc, divides it by 32767.0 (0x70ae48)
    and by 6.0 (0x70aea8), stores it in the next of six floats at
    0xcc1b6d0..0xcc1b6e4 (the counter 0xd5ec514, modulo 6) and returns their
    sum: the mean of the last six draws' values, a full-scale signal
    reading 1.0. VERIFIED (the instructions). It runs once for each
    paint of the item, not for each capture batch. INFERRED.
  - 0xcc1b4cc is set by the capture callback (0x4ed120, registered by
    0x4ed330): zeroed at the entry, then, for a 16-bit buffer, the sum of
    `|sample * scaler|` (the scaler at 0x71fb08, section 4) over the batch
    divided by its sample count. VERIFIED (the instructions,
    0x4ed12a..0x4ed19b). It does not look at the talk key. INFERRED (no
    key read in the function; whether the capture runs while the talk key is
    up rests on the per-frame call at 0x46c9bd, 0x57aeb0, which the client
    frame makes whenever 0xd5ec510 is set).
  - The draw (0x5443a0): with a level above 0 and `level * w - 4.0`
    (0x70b340) above 0, a white bar at `x + 2.0`, `y + 2.0`, width
    `level * w - 4.0`, height `h - 4.0`; then four 1.0 lines of the frame:
    the top (`x`, `y`, `w`), the bottom (`y + h - 1.0`), the left (`y + 1.0`,
    height `h - 2.0`) and the right (`x + w - 1.0`). The colour is the
    static white at 0x6b4518 (1, 1, 1, 1), the material 0xcaf06f0 (the
    white pixel, INFERRED). VERIFIED (the instructions, 0x5443d3..0x5445cf;
    the 1.0 and 2.0 doubles at 0x70ad08, 0x70ae50).
  - The text above it is the item before it, `@MENU_VOICECHAT_LEVEL_INDICATOR`
    (rect -84 120 180 20). VERIFIED (zone walk).

## 5. vcod

- vcod-server relays as section 3 (`cod4/voice.rs`), its dvars registered
  at the defaults above. The listener's facing for Eavesdrop is his
  playerstate's view angles; retail's `clientinfo.playerAngles` is the
  body's, the same yaw on a live player. INFERRED.
- The client sends `cl_voice\1` in its userinfo, encodes the microphone
  (cpal, averaged down to 8000 Hz mono) through the system's libspeex,
  opened at run time (`libspeex.so.1` and kin), and plays each talker on
  a stream of its own. One decoder serves every talker, as on retail.
  Without libspeex the client still joins, shows nothing it cannot hear,
  and sends nothing. `VCOD_VOICE_INPUT=tone` swaps the
  microphone for a 440 Hz tone.
- The live gate `cod4_live_voice_reaches_the_team_and_voice_global_the_enemy`
  runs three clients on vcod-server's `war`: a tone through the
  session's batch reaches the teammate (Speex frames of 20 bytes that
  decode back to the tone) and not the enemy, `voice_global 1` reaches
  the enemy, the teammate's talker list names the talker, and the server
  stamps `lastVoiceTime`. VERIFIED live (vcod against vcod).
- The `winvoice_*` dvars register archived at 1.7's defaults
  (`Shell::register_cod4_options`); the voice page's record level slider
  sets `winvoice_mic_reclevel`, and the capture applies the level and the
  scaler as CoD2's does (section 4, `voice::mic_gain`).
- The compass pings a talking friendly (`cod4_radar`, cod4-hud.md 15.9).
- `winvoice_mic_mute` stays registered and unread: it drives the mixer's
  speaker monitor of the microphone (4.1) and vcod plays none.
- `winvoice_save_voice` appends each heard, unmuted packet's decoded frame
  to `voice.wav` (`voice::append_voice`, the test
  `save_voice_appends_raw_samples_to_voice_wav`), headerless as 4.1 reads
  retail's. It lands in the base directory of the client's search path
  rather than the working directory, through `vcod_common::game_dir::confined`.
  CoD4 only; CoD2's client passes no directory. INFERRED (it follows 4.1).
- The voice page's record level meter (`voice_meter`): the six-draw mean
  of the batch's mean absolute sample, drawn as 4.1 reads it. The
  microphone opens on the first paint of the item and closes after 500 ms
  without one (retail records from init). The level comes through
  `winvoice_mic_reclevel` and `winvoice_mic_scaler` (`MicGain`), where
  retail's level is the mixer's. The bar and frame rects are tested
  (`the_bar_starts_past_the_frame_and_stops_at_the_level`); how the meter
  looks and moves against a real microphone is a pixel check still
  pending.
