//! CoD4 1.7's voice chat on the wire (docs/research/cod4-voice.md): both
//! directions ride out of band, `FF FF FF FF` then a `v` string, never
//! the netchan.
//!
//! - client to server: `v\0`, the qport (short), a packet count (byte),
//!   then per packet its length (byte) and its bytes (`CL_WriteVoicePacket`,
//!   `iw3mp.exe` 0x476660);
//! - server to client: `v\0`, a count (byte), then per packet the
//!   talker's client number (byte), its length (byte) and its bytes
//!   (`SV_SendClientVoiceData`, `cod4_lnxded` 0x817bab2).
//!
//! A packet is one Speex frame: narrowband, 8000 Hz, 160 samples (20 ms),
//! VAD and DTX on, at `sv_voiceQuality` (`Encode_SetOptions`,
//! `iw3mp.exe` 0x4ec9c0).

/// `SV_QueueVoicePacket`'s cap a client a frame (`cmp 0x27`, 0x817ba3e)
/// and `CL_VoicePacket`'s refusal past it (`cmp ebx,0x28`, 0x4767ed).
pub const MAX_QUEUED: usize = 40;
/// The largest packet either side takes (`dataSize - 1 < 0x100`).
pub const MAX_PACKET: usize = 256;
/// `CL_VoiceTransmit` (0x46c780): the batch goes 200 ms after the last or
/// once it holds 10 packets.
pub const TRANSMIT_MS: i32 = 200;
pub const TRANSMIT_PACKETS: usize = 10;
/// `Voice_IsClientTalking`: a packet in the last 300 ms (0x476984).
pub const TALKING_MS: i32 = 300;
/// The codec's rate and frame (`g_encoder_samplerate` 8000 at 0x724ed4,
/// `speex_nb_mode`'s 160-sample frame).
pub const SAMPLE_RATE: u32 = 8000;
pub const FRAME_SAMPLES: usize = 160;
/// `g_encoder_quality`'s initial 1 (0x724ed0) before `sv_voiceQuality`
/// takes over at the first encode.
pub const DEFAULT_QUALITY: i32 = 1;

/// A server packet's frames, (talker, bytes).
pub type Talked = Vec<(u8, Vec<u8>)>;

/// The client's datagram: `frames` past [`MAX_PACKET`] or empty are left
/// out (the client asserts them away).
pub fn client_packet(qport: u16, frames: &[Vec<u8>]) -> Vec<u8> {
    let frames: Vec<&Vec<u8>> = frames
        .iter()
        .filter(|f| (1..MAX_PACKET).contains(&f.len()))
        .take(255)
        .collect();
    let mut p = vec![0xff, 0xff, 0xff, 0xff, b'v', 0];
    p.extend_from_slice(&qport.to_le_bytes());
    p.push(frames.len() as u8);
    for f in frames {
        p.push(f.len() as u8);
        p.extend_from_slice(f);
    }
    p
}

/// A client datagram's body past the `FF FF FF FF` and the `v\0`: the
/// qport and the frames read before the first bad length, as
/// `SV_UserVoice` (0x817b95e) stops on one (`Received invalid voice
/// packet of size %i from %s`, 0x8232d64).
pub fn parse_client(body: &[u8]) -> Option<(u16, Vec<Vec<u8>>)> {
    let qport = u16::from_le_bytes(body.get(..2)?.try_into().ok()?);
    let count = *body.get(2)? as usize;
    let mut at = 3;
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(&n) = body.get(at) else { break };
        let n = n as usize;
        let Some(f) = body.get(at + 1..at + 1 + n).filter(|_| n > 0) else {
            break;
        };
        out.push(f.to_vec());
        at += 1 + n;
    }
    Some((qport, out))
}

/// [`parse_client`] as `SV_PreGameUserVoice` (`cod4_lnxded` 0x817bc58)
/// reads a loading client's packet: each length a short (0x8130900, the
/// qport's reader) of 1..=256. The 1.7 client writes byte lengths, so its
/// frames misread there (INFERRED off the two readers).
pub fn parse_client_pregame(body: &[u8]) -> Option<(u16, Vec<Vec<u8>>)> {
    let qport = u16::from_le_bytes(body.get(..2)?.try_into().ok()?);
    let count = *body.get(2)? as usize;
    let mut at = 3;
    let mut out = Vec::new();
    for _ in 0..count {
        let Some(n) = body.get(at..at + 2) else { break };
        let n = u16::from_le_bytes([n[0], n[1]]) as usize;
        let Some(f) = body
            .get(at + 2..at + 2 + n)
            .filter(|_| (1..=MAX_PACKET).contains(&n))
        else {
            break;
        };
        out.push(f.to_vec());
        at += 2 + n;
    }
    Some((qport, out))
}

/// The server's datagram for one client's queue.
pub fn server_packet(queued: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut p = vec![0xff, 0xff, 0xff, 0xff, b'v', 0, queued.len() as u8];
    for (talker, f) in queued {
        p.push(*talker);
        p.push(f.len() as u8);
        p.extend_from_slice(f);
    }
    p
}

/// A server datagram's body past the `v\0`, as `CL_VoicePacket`
/// (0x4767b0) reads it: a count past [`MAX_QUEUED`] is ignored, a bad
/// length or a talker past 63 ends the read (`Invalid server voice packet
/// of %i bytes`, `Invalid voice packet - talker was %i`).
pub fn parse_server(body: &[u8]) -> Talked {
    let mut out = Vec::new();
    let Some(&count) = body.first() else {
        return out;
    };
    if count as usize > MAX_QUEUED {
        return out;
    }
    let mut at = 1;
    for _ in 0..count {
        let (Some(&talker), Some(&n)) = (body.get(at), body.get(at + 1)) else {
            break;
        };
        let n = n as usize;
        let Some(f) = body.get(at + 2..at + 2 + n).filter(|_| n > 0) else {
            break;
        };
        if talker >= 64 {
            break;
        }
        out.push((talker, f.to_vec()));
        at += 2 + n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip_both_ways() {
        let frames = vec![vec![1, 2, 3], vec![9; 20]];
        let c = client_packet(0x1234, &frames);
        assert_eq!(&c[..9], &[0xff, 0xff, 0xff, 0xff, b'v', 0, 0x34, 0x12, 2]);
        assert_eq!(parse_client(&c[6..]), Some((0x1234, frames.clone())));
        let s = server_packet(&[(3, frames[0].clone()), (7, frames[1].clone())]);
        assert_eq!(&s[..10], &[0xff, 0xff, 0xff, 0xff, b'v', 0, 2, 3, 3, 1]);
        assert_eq!(
            parse_server(&s[6..]),
            vec![(3, frames[0].clone()), (7, frames[1].clone())]
        );
    }

    #[test]
    fn a_bad_length_or_talker_ends_the_read() {
        // A zero length stops the client's read after the first frame.
        assert_eq!(
            parse_client(&[0, 0, 3, 1, 5, 0, 2, 7, 7]),
            Some((0, vec![vec![5]]))
        );
        assert_eq!(parse_server(&[2, 1, 1, 8, 64, 1, 9]), vec![(1, vec![8])]);
        assert!(parse_server(&[41, 1, 1, 8]).is_empty());
    }
}
