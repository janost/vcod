# Call of Duty 2 1.3: voice chat

How 1.3 carries, relays, encodes and shows voice: the two out-of-band
packets, the server's relay rules, the codec at 8192 Hz, push to talk,
the talk icon and talker list, the scoreboard's column and the mute list.
CoD4 1.7 inherited all of it (cod4-voice.md); section 7 lists what differs.
vcod's wire is `vcod_common::net::voice` (CoD2's rate in
`vcod_cod2::net::voice`), the server relay `crates/server/src/voice.rs` and
`crates/server/src/cod2/voice.rs`, the client `crates/client/src/voice.rs`.

## 1. Sources

- The Mac 1.3 MP binary (i386 slice, symbols kept), its Ghidra export,
  and `llvm-objdump` / `llvm-nm` over the Mach-O for data. Mac addresses
  are labelled "Mac".
- `cod2_lnxded` 1.3 (md5 `fefed4c4909fb7c5c13b13a003f5ccf7`), read in
  `objdump -d -M intel`, names off `tools/re/symmap.py` where the Mac build
  gives them. Unlabelled addresses are `cod2_lnxded`'s.
- `CoD2MP_s.exe` 1.3 (image base 0x400000, raw offsets equal to virtual
  ones) for the Windows client's data and its capture format, labelled
  "Windows".
- The stock iwds' `ui_mp/hud.menu`, `ui_mp/scriptmenus/muteplayer.menu`,
  `ui_mp/options_voice.menu`, `ui/menudefinition.h` and `default_mp.cfg`.
- Live runs of `crates/cod2/examples/voice_probe.rs` against retail
  `cod2_lnxded` (section 6).

## 2. The wire

The wire is CoD4's byte for byte (cod4-voice.md 2). VERIFIED (each writer
and reader below against that section).

- `SV_ConnectionlessPacket` (0x809594e) reads the first line with
  `MSG_ReadStringLine`, tokenizes it and compares `argv[0]` with `v`
  case-insensitively (string 0x814b6e0) before `getinfo`, handing the
  message to `SV_VoicePacket` (0x8094b56). VERIFIED.
  `CL_ConnectionlessPacket` (Mac 0x14c4c4) does the same into
  `CL_VoicePacket` (Mac 0x1478f2). VERIFIED.
- Client to server (`CL_WriteVoicePacket`, Mac 0x187fac): `v` with its NUL
  (`MSG_WriteString`), the qport (short, `clc+4`), the packet count
  (byte), then per packet its length (byte) and its bytes, through
  `NET_OutOfBandVoiceData` (0x806cc7a, Mac 0x158682), which writes
  `FF FF FF FF` and copies the message after it, no Huffman. VERIFIED. It
  writes only with `clc+0x40188` clear and the connection state 8 or 6.
  VERIFIED; that the field is the demo flag and the states active and
  loading is INFERRED (Q3's `clcState` order). `cl_showSend` prints
  `voice: %i` with the size. VERIFIED.
- Server to client (`SV_SendClientVoiceData`, 0x809c0a2, Mac 0x1bb770):
  `v`, the count (byte), then per packet the talker's client number
  (byte), the length (byte) and the bytes, only to a client at state 4
  with packets queued; `WARNING: voice msg overflowed for %s` on an
  overflow, else the queue is sent and emptied. VERIFIED.
- `SV_SendClientMessages` (Mac 0x197382) calls it right after a slot's
  `SV_SendClientSnapshot`; a slot whose turn sends a fragment instead
  sends no voice that turn. VERIFIED.
- `CL_VoicePacket` reads the count and takes it only for 1..40 (`uVar3 <
  0x29 && 0 < uVar3`); per packet it reads the talker and the length,
  stops on a length outside 1..256 (`Invalid server voice packet of %i
  bytes`), reads the bytes, then stops on a talker past 63 (`Invalid voice
  packet - talker was %i`), and hands the frame to
  `Voice_IncomingVoiceData` unless `CL_IsPlayerMuted` names the talker or
  `cl_voice` is off. VERIFIED (Mac 0x1478f2).

## 3. The server

- `SV_VoicePacket` (0x8094b56) reads the qport, then walks the slots for
  one whose base address (IP alone, `NET_CompareBaseAdr`) and qport match;
  when only the port differs it logs `SV_ReadPackets: fixing up a
  translated port` and takes it. A zombie (state 1) is passed over and the
  walk goes on; otherwise `lastPacketTime` takes `svs.time`, then a state
  under 4 goes to `SV_PreGameUserVoice`, state 4 to `SV_UserVoice`, and the
  walk ends. VERIFIED (Mac 0x159e36).
- The qport is `MSG_ReadShort`'s, sign-extended (Mac 0x1735cc returns
  `(int)(short)`), and compared with the slot's netchan qport as read off
  `connect`'s `atoi`, unmasked; `SV_PacketEvent` (Mac 0x15b836) masks its
  own read with 0xffff. VERIFIED (both compares). So a client whose qport
  is 0x8000 or more matches no slot and is never heard. INFERRED; VERIFIED
  live (6). The retail client's qport is its first event's
  `Sys_Milliseconds() & 0xffff` (`Com_Init` into `Netchan_Init`, Mac
  0x15810a), so about half of retail's clients cannot talk. INFERRED.
- `SV_UserVoice` (0x809c2ee, Mac 0x1bba24) returns with `sv_voice` off,
  reads the count byte, then per packet a length byte; a length outside
  1..256 logs `Received invalid voice packet of size %i from %s` and ends
  the packet; each frame goes to `G_BroadcastVoice` (0x80f9976, Mac
  0x192d0c). VERIFIED.
- `SV_PreGameUserVoice` (0x809c3c6, Mac 0x1bbada), also gated on
  `sv_voice`, reads each length with `MSG_ReadShort` and queues the frame
  for every other slot past state 1 that has not muted the talker, wants
  voice and holds fewer than 40, with no game rule. VERIFIED. The client
  writes byte lengths, so a loading client's frame misreads there.
  INFERRED off the two readers; VERIFIED live (6).
- `SV_QueueVoicePacket` (0x809c21c, Mac 0x1bb974) keeps at most 40 frames a
  client (`< 0x28`), each 0x105 bytes (talker, 256 bytes, length).
  VERIFIED.
- `G_BroadcastVoice` stamps the talker's `lastVoiceTime` (gclient+0x2808)
  with `level.time` (0x864c56c). VERIFIED (0x80f998a). It then walks the
  64 entity slots and, for each in use (`gentity+0xfc`) with a client:
  INFERRED (the loop 0x80f9997..0x80f9ad7).
  1. A listener at the intermission (`sessionState` 3, gclient+0x26a8)
     or `voice_global` (0x87a2460) on passes; otherwise `OnSameTeam`
     (0x811c0d0: both clients, the first's team at gclient+0x274c not 0,
     the two teams equal) passes, and failing that only a talker of team
     0. INFERRED (0x80f99df..0x80f9a1e).
  2. The two session states are equal, or either is 1 (dead) with
     `voice_deadChat` (0x87a2464) on. INFERRED (0x80f9a23..0x80f9a72).
  3. The listener is not the talker unless `voice_localEcho` (0x87a245c).
     INFERRED (0x80f9a74..0x80f9a87).
  4. `SV_ClientHasClientMuted` (0x809c1fa) is false and
     `SV_ClientWantsVoiceData` (0x809c1dc) true. INFERRED
     (0x80f9a89..0x80f9aae).
  Then `SV_QueueVoicePacket(talker, listener, frame)`. INFERRED. There is
  no Eavesdrop and no distance: CoD4's step 1 adds the perk's cone, the
  rest is the same. INFERRED (no perk read between 0x80f9976 and
  0x80f9ad7).
- `sendVoice` (client+0xb1018): `SV_UserinfoChanged` (0x80907cc) sets it to
  the userinfo's `cl_voice` above 0 and clears it when the rate it just
  settled (a LAN client's 99999 below `dedicated 2`, else the userinfo's
  clamped to 1000..90000, 5000 when empty) is under 5000. VERIFIED (Mac
  0x17cfc4).
- `muteplayer <n>` and `unmuteplayer <n>` (`SV_MutePlayer_f` 0x8090a5a,
  `SV_UnmutePlayer_f` 0x8090aaa), engine client commands of the `ucmds`
  table (cod2-protocol.md 3.4.2), set and clear `muteList[n]`
  (client+0xb0fd8) for `atoi` values 0..64 (`< 0x41`), so 64 writes one past
  the clients; anything else logs `Invalid mute client %i` or `Invalid
  unmute client %i`. VERIFIED (Mac 0x17d8f0).
- `self isTalking()` (`PlayerCmd_IsTalking`, 0x80fd15a, Mac 0x1c16d4) is 1
  when `level.time - lastVoiceTime` is 0 or more and under
  `g_voiceChatTalkingDuration`. VERIFIED.

The dvars, Mac registrations (`SV_Init` 0x141d62, `G_InitGame` 0x1addca,
`CL_InitOnceForAllClients` 0x1485e2, `Voice_Init` 0x1ea1b0, `CG_Init` 0x145214).
VERIFIED; the 0x1000 bit is on every Mac registration:

| Dvar | Type | Default | Flags |
|---|---|---|---|
| `sv_voice` | bool | 0 | 0x100d (archive, serverinfo, systeminfo) |
| `sv_voiceQuality` | int 0..9 | 1 | 0x1008 (systeminfo) |
| `voice_global` | bool | 0 | 0x1001 |
| `voice_localEcho` | bool | 0 | 0x1001 |
| `voice_deadChat` | bool | 0 | 0x1001 |
| `g_voiceChatTalkingDuration` | int 0..10000 | 500 | 0x1001 |
| `cl_voice` | bool | 1 | 0x1003 (archive, userinfo) |
| `cl_talking` | bool | 0 | 0x1000 |
| `winvoice_mic_mute` | bool | 1 | 0x1001 |
| `winvoice_mic_reclevel` | float 0..65535 | 65535 | 0x1001 |
| `winvoice_save_voice` | bool | 0 | 0x1001 |
| `winvoice_mic_scaler` | float 0.25..2 | 1 | 0x1001 |
| `cg_voiceSpriteTime` | int 0..60000 | 6000 | 0x1001 |
| `cg_voiceIconSize` | float 0..100 | 0 | 0x1001 |

`getinfo` sends `sv_voice` as `voice` (cod2-protocol.md 1.4), the browser
shows it as its voice column (`joinserver.menu`'s `voiceColumn`).

## 4. The client

- Push to talk is `+talk` / `-talk` (`IN_TalkDown`, `IN_TalkUp`, Mac
  0x187f20, 0x187f3a) on the `kbutton_t` at `kb+0x21c`; `IsTalking` (Mac
  0x187f54) reads its active flag at `kb+0x22c`. VERIFIED. The talk key
  sets no cmd button. INFERRED (`CL_CmdButtons` reads no `kb+0x21c`;
  cod2-protocol.md 4.2). `default_mp.cfg` (iw_00.iwd) binds `z "+talk"`;
  `options_multi.menu` lists it as `@MENU_VOICE_CHAT_BUTTON`. VERIFIED.
- `Voice_SendVoiceData` (Mac 0x147ae8) holds when `sv_voice` and
  `cl_voice` are on, `Dvar_GetInt("rate")` is 5000 or more, the connection
  state is 8, and `cl_talking`, the talk key or a waiting frame holds.
  VERIFIED. `options_voice.menu` warns that voice is off under a rate of
  2500, 3000 or 4000 (`dvartest "rate"`). VERIFIED.
- The capture callback `Record_QueueAudioDataForEncoding` (Mac 0x1ea748)
  scales each 16-bit sample by the current scaler, keeps the mean level for
  the options meter, and, past `Voice_SendVoiceData`, encodes only while
  the talk key is held; with it up it calls `CL_VoiceTransmit` to flush.
  It cuts the samples into `g_frame_size` frames, carrying a partial frame
  to the next callback, and hands each `Encode_Sample` result to
  `Client_SendVoiceData` (Mac 0x14a868). VERIFIED. So `cl_talking` keeps
  the batch flowing but encodes nothing by itself. INFERRED.
- The Windows build registers the four `winvoice_*` dvars in `Voice_Init`
  (`CoD2MP_s.exe` 0x4b8dd0) as the table above has them: `mute` bool 1,
  `reclevel` float 65535 in 0..65535, `save_voice` bool 0, `scaler` float
  1 in 0.25..2, flags 0x1001 each. VERIFIED (asm). Then it opens the
  mixer: `reclevel`, `ftol`'d to 16 bits, goes to 0x4b96a0, which finds
  the source line named `Mic` (0x5b3408) under the `DST_WAVEIN` (7)
  destination and sets its `VOLUME` control (0x50030001) through
  `mixerSetControlDetails` (0x4b97f5); `mute` goes to 0x4b9d30, the `Mic`
  source under `DST_SPEAKERS` (4) and its `MUTE` control (0x20010002,
  set at 0x4b9e80). VERIFIED (asm). `reclevel` is read again each frame
  and set when its 16-bit value changes (0x4b91fa..0x4b922c); `mute` only
  at `Voice_Init` (its one other reader, 0x4b8e89). VERIFIED (asm). So
  the level is the recording volume and the mute the microphone's
  monitor on the speakers, not the capture. INFERRED.
- `Voice_GetLocalVoiceData` (Mac 0x1ea65c) moves the scaler to
  `winvoice_mic_scaler` when it changes: 0.5 below 0.5 (0x326190), else at
  most 1.5 (0x3261b8). VERIFIED.
- `Client_SendVoiceData` copies the frame into the batch (`cl+0x195364`,
  0x104 bytes a slot) and, past 199 ms since the last send (`cl+0x26f0`
  less `cl+0x195d90`) or past 9 frames, writes the packet and restarts the
  clock. `CL_VoiceTransmit` (Mac 0x147a9e) applies the same test, and both
  only with a frame waiting. VERIFIED. So CoD2's clock moves only on a
  send, and the first frame after a silence goes at once; CoD4's
  restarts with nothing to send (cod4-voice.md 4). INFERRED.
- The codec is Speex narrowband. `Voice_Init` (Mac 0x1ea1b0) sets
  `g_current_bandwidth_setting` to 0, and `Encode_Init` / `Decode_Init`
  (Mac 0x1eac1c, 0x1ea9e0; Windows 0x4ba0d0, 0x4b9ea0) map 0 to
  `speex_nb_mode` (Windows 0x5a00f4, name `narrowband`, frame 160).
  VERIFIED. `Encode_SetOptions` (Mac 0x1eab70) sets `SPEEX_SET_SAMPLING_RATE`
  (ctl 24) to `g_encoder_samplerate`, the quality (ctl 4), reads the frame
  size (ctl 3) and turns VAD (ctl 30) and DTX (ctl 34) on. The decoder
  gets the enhancer (ctl 0) and the same rate. VERIFIED.
- The rate is 8192 Hz, not narrowband's nominal 8000: `g_encoder_samplerate`
  is 8192 (Mac 0x34d7cc, Windows 0x5d88cc), and so is the capture and
  playback rate `g_sound_recordFrequency` (Mac 0x34d7ec, Windows 0x5d88e4).
  VERIFIED. The Windows capture's `WAVEFORMATEX` (built at 0x4bb181) is
  PCM, `nChannels` from 0x5d88ec (1), `nSamplesPerSec` from 0x5d88e4,
  16 bits. VERIFIED. A 160-sample frame is 19.5 ms of voice, 51.2 frames a
  second. INFERRED (arithmetic).
- `g_encoder_quality` starts at 1 (Mac 0x34d7d0, Windows 0x5d88c8);
  `Encode_Sample` (Mac 0x1ead26) takes `sv_voiceQuality` whenever it
  differs, a systeminfo dvar, so the client encodes at the server's
  quality. VERIFIED. At quality 1 Speex's narrowband mode 8 codes 79 bits,
  10 bytes a frame (VERIFIED, libspeex 1.2, the test
  `cod2_voice_speex_frames_the_tone_at_8192_hz`); at 8192 Hz that is
  4045 bit/s. INFERRED (arithmetic).
- `Voice_IncomingVoiceData` (Mac 0x1ea43c) stamps the talker's
  `s_clientTalkTime` with `Sys_Milliseconds`, decodes the packet in
  `g_frame_size`-byte chunks (`Decode_Sample` 0x1eaaec: `speex_decode`
  into floats, each cast to a short with no clamp) and writes each frame to
  the talker's own sample (`Sound_UpdateSample`); `winvoice_save_voice`
  appends the last decoded frame to `voice.wav`. VERIFIED.
- Playback: `Voice_Init` makes one sample a client with
  `Sound_NewSample`, filling `s_clientSamples` up to `playing`. VERIFIED.
  That is 64. INFERRED (the talker index runs to 63). Each sample takes
  the record frequency, volume 255, pan 128 and one channel
  (`DSOUNDRecord_NewSample`, Mac 0x1f858e; the stores at Windows
  0x4bb0f7..0x4bb11e); the Mac plays it through Miles at that rate
  (`DSound_NewSample`, Mac 0x1f888e) from a 0xc800-byte ring. VERIFIED. No
  dvar sets a talker's volume: every talker plays at full volume, centred.
  INFERRED (no other write to the sample's volume word).
- `Voice_IsClientTalking` (Mac 0x1ea584): a frame in the last 300 ms.
  `CL_IsPlayerTalking` (Mac 0x1479dc) is that alone, for the local client
  too, so we show as talking only when the server echoes us
  (`voice_localEcho`). VERIFIED.
- `CL_Disconnect` (Mac 0x14b8ea) calls `CL_ClearMutedList`, emptying
  `s_playerMute` (64 bytes). VERIFIED.

## 5. The HUD and the menus

- `hud.menu`'s `amitalking` (rect 100 5 24 24, left and top) holds owner
  draw 266, `UI_AMITALKING`; its `talkers` (rect 60 30 16 16) holds 267..270,
  `UI_TALKER1`..`4`, four 16 x 16 items at y 0, 16, 32, 48, text scale
  0.21, shadowed. VERIFIED (the menu and `ui/menudefinition.h`).
- `UI_OwnerDraw` (Mac 0x1536b2): 266 draws `voice_on` in its rect while
  `sv_voice` and `cl_voice` are on and the talk key is held. 267 + n walks
  clients 0..63 for the n-th `CL_IsPlayerTalking` one, ours included,
  builds the player list (`UI_BuildPlayerList`), finds the talker's row,
  draws `voice_on` in the rect and the row's name at x + w + 2 with the
  item's font and scale, its baseline at `y + th + (h - th) / 2` where
  `th` is the scaled text height. VERIFIED. A talker the list lacks reads
  the row before the first. INFERRED (`iVar9 = -1`).
- `CG_Draw2D` (Mac 0x1d114a) paints the HUD menus (`Menu_PaintAll`) only
  for a `pm_type` other than 4 and 5, so a free spectator and the
  intermission show neither. VERIFIED.
- The scoreboard's voice column (`CG_DrawClientScore`, Mac 0x1c51b2,
  column type 6): a 13 x 13 `voice_off` for a client `CL_IsPlayerMuted`
  names, else `voice_on` for one `CL_IsPlayerTalking` names, else nothing,
  at the column's left and the row's top. VERIFIED (cod2-hud.md 13).
- The mute list is the `muteplayer` script menu (the `ingame` menu's
  third button, cod2-protocol.md "Script menus on a CoD2 client"): a list box on `FEEDER_MUTELIST` (0x14,
  columns at 2 and 40), a `@MP_PLAYER_TOGGLE_MUTE` button running
  `uiScript mutePlayer`, the same on a double click, and
  `@MPUI_ENABLE_VOICE_CHAT` / `@MPUI_DISABLE_VOICE_CHAT` setting `cl_voice`.
  VERIFIED (the menu).
- Its rows are `UI_BuildPlayerList` (Mac 0x14f5e2): each client below
  `sv_maxclients` with a name, ours too, the name cleaned (`I_CleanStr`),
  rebuilt by `UI_FeederCount` (Mac 0x156946) at most every 3 s, shared
  with `FEEDER_PLAYER_LIST`. `UI_FeederItemText` (Mac 0x152e8a) gives
  column 0 the translated `MP_MUTED` for a muted client and column 1 the
  name. VERIFIED.
- `mutePlayer` (`UI_RunMenuScript`, Mac 0x1548e8) hands the selected row's
  client to `CL_MutePlayer` (Mac 0x161d2e), which flips `s_playerMute[n]`
  and runs `muteplayer %i` or `unmuteplayer %i`, which the console forwards
  to the server. VERIFIED.
- The head icon for voice (`cg_voiceIconSize`, `headiconvoicechat`) is dead
  code (cod2-hud.md 19.2).

## 6. Live

`crates/cod2/examples/voice_probe.rs` against retail `cod2_lnxded` on
mp_toujane dm, `+set sv_voice 1`, in an `unshare -rn` loopback namespace:
a listener (`ear`, qport 0x1111) joins and spectates, then a talker
(qport 0x2222) joins. Both sit as spectators (team 3, session 2). Run
2026-11-12.

- Before its gamestate the talker sent `v`, qport, count 1, length byte
  3, `11 22 33`, then the same with a short length (`03 00`) and
  `44 55 66`. The server logged `Received invalid voice packet of size
  4355 from talker` (0x1103: the byte length and the first data byte read
  as one short) for the first, and the listener got `v`, 1, talker 1,
  length 3, `44 55 66` after its next snapshot. VERIFIED live.
- In the world the talker sent two frames (10 and 3 bytes) every 200 ms
  for 3 s; each packet reached the listener as one `v` of count 2, talker
  1, the frames in order, 10 to 50 ms after it was sent, 15 of 15. The
  talker heard nothing of his own. VERIFIED live.
- `voice_localEcho 1`: the talker got his own frames too, as talker 1.
  VERIFIED live.
- `sv_voice 0`: nothing reached anyone, and no invalid-size line was
  logged (the gate comes before the read). VERIFIED live.
- The talker on qport 40000: nothing reached anyone. VERIFIED live.

The captures are not fixtures: the in-process gates
(`crates/server/src/cod2/voice.rs`'s tests) pin the same behaviour on
vcod-server.

## 7. vcod

- The wire (`vcod_common::net::voice`) is CoD4's module moved to the
  common crate, both games reading it; `vcod_cod2::net::voice` adds the
  8192 Hz rate and `sv_voiceQuality`'s 1.
- vcod-server (`--game cod2`) registers the dvars above at their
  defaults, reads `v` packets as section 3 (`cod2/voice.rs`), relays
  through the rules CoD4 shares (`crates/server/src/voice.rs`, without the
  perk), takes `muteplayer` / `unmuteplayer`, settles `sendVoice` off the
  settled rate, sends each queue after the slot's snapshot, and answers
  `isTalking`. It reads the qport signed, as retail, so a client on 0x8000
  or more is not heard there either.
- The client keeps its qport under 0x8000, sends `cl_voice` in its
  userinfo (again when it changes),
  binds Z to `+talk`, encodes the microphone (cpal, averaged down to
  8192 Hz mono) through the system's libspeex at the server's
  `sv_voiceQuality`, batches on CoD2's clock, and plays each talker on a
  stream of its own at 8192 Hz, full volume. It draws `amitalking`,
  `talkers`, the scoreboard's column and the mute list, and keeps the mute
  list for the session. `VCOD_VOICE_INPUT=tone` swaps the microphone for a
  440 Hz tone. Without libspeex it joins, shows talkers, and neither sends
  nor plays.
- Against CoD4 (cod4-voice.md): no Eavesdrop; the rate 8192, not 8000;
  `sv_voiceQuality` 1, not 3; the transmit clock moves only on a send; the
  talker list keeps our own number and has no `cg_drawTalk`; our own talk
  icon needs both `sv_voice` and `cl_voice` (CoD4's either); the mute list
  is a script menu over `FEEDER_MUTELIST`; voice goes after a snapshot,
  never after a fragment. Whether 1.7's `SV_VoicePacket` also reads the
  qport signed is not checked.
- Gates: `cod2_voice_from_a_loading_client_reads_short_lengths`,
  `cod2_voice_from_a_high_qport_matches_no_slot` (no game data) and
  `cod2_voice_reaches_the_team_and_voice_global_the_enemy` (`COD2_DIR`,
  three players on tdm) drive wire packets through vcod-server;
  `cod2_voice_reaches_the_session_and_leaves_on_its_qport` the client
  session's socket; `cod2_voice_speex_frames_the_tone_at_8192_hz` the
  codec; `the_talker_list_names_each_talker`,
  `a_two_player_dm_reply_draws_two_rows` (the voice column) and
  `the_mute_list_toggles_the_selected_client` the HUD and the menu.
- The four `winvoice_*` dvars register archived at retail's defaults
  (`Shell::use_cod2_cvars`). The capture scales each sample by
  `winvoice_mic_reclevel / 65535` (vcod's stand-in for the mixer's
  recording volume, which it leaves alone), then by the scaler held to
  0.5..1.5 and stored as 1.3 does, wrapping past 16 bits
  (`voice::mic_gain`, the test
  `the_mic_level_and_scaler_scale_each_sample`). `winvoice_mic_mute`
  stays a registered no-op: it mutes the microphone's playback line,
  and vcod never plays the microphone back.
- Not done: `winvoice_save_voice` (each heard packet's last decoded
  frame appended to a headerless `voice.wav` in the working directory,
  section 4), `options_voice.menu`'s level meter (`UI_RECORDLEVEL`, 265),
  the shadowed text style of the talker names, and `cl_talking`.
