//! Binary framing for the ASR helper-process protocol.
//!
//! Every message on stdin/stdout is `u8 type + u32 LE length + payload`,
//! capped at 64 MiB. stderr is free for logs. All integers are
//! little-endian; audio samples are `f32` LE at 16 kHz mono.

use std::io::{Read, Write};

use anyhow::{anyhow, Context};

/// Maximum payload bytes per frame. Anything larger is a bug, not audio.
pub const MAX_FRAME: u32 = 64 * 1024 * 1024;

// --- Client -> server request types ---
pub const REQ_LOAD: u8 = 0x01;
pub const REQ_UNLOAD: u8 = 0x02;
pub const REQ_BEGIN: u8 = 0x10;
pub const REQ_PUSH: u8 = 0x11;
pub const REQ_FINISH: u8 = 0x12;
pub const REQ_CANCEL: u8 = 0x13;
pub const REQ_PARTIAL: u8 = 0x14;
pub const REQ_TRANSCRIBE: u8 = 0x20;
pub const REQ_STATUS: u8 = 0x30;
pub const REQ_PING: u8 = 0x31;

// --- Server -> client reply/event types ---
pub const REP_LOADED: u8 = 0x81;
pub const REP_LOAD_FAILED: u8 = 0x82;
pub const REP_FINAL: u8 = 0x83;
pub const REP_PARTIAL: u8 = 0x84;
pub const REP_STATUS: u8 = 0x85;
pub const REP_PONG: u8 = 0x86;
pub const REP_ERROR: u8 = 0x87;

/// Write one framed message.
pub fn write_frame(w: &mut impl Write, msg_type: u8, payload: &[u8]) -> anyhow::Result<()> {
    if payload.len() > MAX_FRAME as usize {
        return Err(anyhow!(
            "frame payload {} bytes exceeds {} MiB cap",
            payload.len(),
            MAX_FRAME / 1024 / 1024
        ));
    }
    let mut hdr = [0u8; 5];
    hdr[0] = msg_type;
    hdr[1..5].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&hdr).context("writing frame header")?;
    w.write_all(payload).context("writing frame payload")?;
    Ok(())
}

/// Read one framed message. `Ok(None)` is a clean EOF before any header
/// byte (child exited normally); a truncated header or payload is an error.
pub fn read_frame(r: &mut impl Read) -> anyhow::Result<Option<(u8, Vec<u8>)>> {
    // Read the first byte alone: zero bytes is a clean EOF (child exited
    // normally). Anything after that must be a whole frame. This avoids
    // relying on how much a failed `read_exact` fills (nothing, on some
    // readers), which would make a truncated header look like clean EOF.
    let mut first = [0u8; 1];
    let n = r.read(&mut first).context("reading frame header")?;
    if n == 0 {
        return Ok(None);
    }
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)
        .context("truncated frame header")?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    if len > MAX_FRAME as usize {
        return Err(anyhow!(
            "frame of {} bytes exceeds {} MiB cap",
            len,
            MAX_FRAME / 1024 / 1024
        ));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)
        .context("truncated frame payload")?;
    Ok(Some((first[0], payload)))
}

/// Encode a `u64` id payload.
pub fn encode_id(id: u64) -> [u8; 8] {
    id.to_le_bytes()
}

/// Decode a `u64` id payload.
pub fn decode_id(payload: &[u8]) -> anyhow::Result<u64> {
    if payload.len() != 8 {
        return Err(anyhow!("id payload is {} bytes, want 8", payload.len()));
    }
    Ok(u64::from_le_bytes(payload.try_into().expect("checked len")))
}

/// Encode `id + f32 samples` (PUSH).
pub fn encode_push(id: u64, samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 4 * samples.len());
    out.extend_from_slice(&id.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Decode `id + f32 samples` (PUSH).
pub fn decode_push(payload: &[u8]) -> anyhow::Result<(u64, Vec<f32>)> {
    if payload.len() < 8 || (payload.len() - 8) % 4 != 0 {
        return Err(anyhow!("bad PUSH payload of {} bytes", payload.len()));
    }
    let id = u64::from_le_bytes(payload[..8].try_into().expect("checked len"));
    let samples = payload[8..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("chunk of 4")))
        .collect();
    Ok((id, samples))
}

/// Encode raw `f32` samples (TRANSCRIBE).
pub fn encode_samples(samples: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 * samples.len());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Decode raw `f32` samples (TRANSCRIBE).
pub fn decode_samples(payload: &[u8]) -> anyhow::Result<Vec<f32>> {
    if payload.len() % 4 != 0 {
        return Err(anyhow!(
            "sample payload of {} bytes is not a multiple of 4",
            payload.len()
        ));
    }
    Ok(payload
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().expect("chunk of 4")))
        .collect())
}

/// Encode `id + utf8 text` (FINAL / PARTIAL / ERROR).
pub fn encode_id_text(id: u64, text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + text.len());
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

/// Decode `id + utf8 text` (FINAL / PARTIAL / ERROR).
pub fn decode_id_text(payload: &[u8]) -> anyhow::Result<(u64, String)> {
    if payload.len() < 8 {
        return Err(anyhow!(
            "id+text payload is {} bytes, want >= 8",
            payload.len()
        ));
    }
    let id = u64::from_le_bytes(payload[..8].try_into().expect("checked len"));
    let text = std::str::from_utf8(&payload[8..])
        .context("id+text payload is not utf8")?
        .to_string();
    Ok((id, text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn round_trip(t: u8, payload: &[u8]) -> (u8, Vec<u8>) {
        let mut buf = Vec::new();
        write_frame(&mut buf, t, payload).unwrap();
        let mut cur = Cursor::new(buf);
        read_frame(&mut cur).unwrap().expect("one frame")
    }

    #[test]
    fn round_trip_every_message_shape() {
        // Empty payloads.
        assert_eq!(round_trip(REQ_PING, &[]), (REQ_PING, vec![]));
        assert_eq!(round_trip(REQ_UNLOAD, &[]), (REQ_UNLOAD, vec![]));
        // Model id string.
        let id = b"parakeet-tdt-0.6b-v3-int8";
        assert_eq!(round_trip(REQ_LOAD, id), (REQ_LOAD, id.to_vec()));
        // Id payloads.
        for t in [REQ_BEGIN, REQ_FINISH, REQ_CANCEL, REQ_PARTIAL] {
            assert_eq!(round_trip(t, &encode_id(7)), (t, encode_id(7).to_vec()));
        }
        // Push with samples, including odd values.
        let samples = vec![0.0, -1.5, 3.25, f32::MIN_POSITIVE];
        assert_eq!(
            round_trip(REQ_PUSH, &encode_push(42, &samples)),
            (REQ_PUSH, encode_push(42, &samples))
        );
        // One-shot samples.
        assert_eq!(
            round_trip(REQ_TRANSCRIBE, &encode_samples(&samples)),
            (REQ_TRANSCRIBE, encode_samples(&samples))
        );
        // Id + text, including empty and non-ASCII text.
        for text in ["", "hello world", "Shivan O’Connor — 42"] {
            let p = encode_id_text(9, text);
            assert_eq!(round_trip(REP_FINAL, &p), (REP_FINAL, p.clone()));
            assert_eq!(round_trip(REP_PARTIAL, &p).0, REP_PARTIAL);
            assert_eq!(round_trip(REP_ERROR, &p).0, REP_ERROR);
        }
        // JSON-ish status and load latency.
        let ms = 1156u64.to_le_bytes();
        assert_eq!(round_trip(REP_LOADED, &ms), (REP_LOADED, ms.to_vec()));
        let json = br#"{"state":"loaded","model":"x","rss_mb":12}"#;
        assert_eq!(round_trip(REP_STATUS, json), (REP_STATUS, json.to_vec()));
    }

    #[test]
    fn clean_eof_gives_none() {
        let mut cur = Cursor::new(vec![]);
        assert!(read_frame(&mut cur).unwrap().is_none());
    }

    #[test]
    fn truncated_header_is_rejected() {
        let mut cur = Cursor::new(vec![REQ_PING, 0x03]);
        assert!(read_frame(&mut cur).is_err());
    }

    #[test]
    fn truncated_payload_is_rejected() {
        let mut buf = vec![REP_FINAL];
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(b"short");
        let mut cur = Cursor::new(buf);
        assert!(read_frame(&mut cur).is_err());
    }

    #[test]
    fn oversized_frame_is_rejected_before_allocating() {
        let mut buf = vec![REQ_PUSH];
        buf.extend_from_slice(&(MAX_FRAME + 1).to_le_bytes());
        let mut cur = Cursor::new(buf);
        let err = read_frame(&mut cur).unwrap_err().to_string();
        assert!(err.contains("exceeds"), "{err}");
        // Writer side enforces the cap too.
        let mut out = Vec::new();
        let big = vec![0u8; MAX_FRAME as usize + 1];
        assert!(write_frame(&mut out, REQ_PUSH, &big).is_err());
        assert!(out.is_empty(), "nothing written on oversize");
    }

    #[test]
    fn bad_payloads_are_rejected() {
        assert!(decode_id(b"short").is_err());
        assert!(decode_push(b"1234567").is_err());
        assert!(decode_push(b"123456789").is_err());
        assert!(decode_samples(b"abc").is_err());
        assert!(decode_id_text(b"1234567").is_err());
        let mut bad = encode_id(1).to_vec();
        bad.extend_from_slice(&[0xff, 0xfe]);
        assert!(decode_id_text(&bad).is_err());
        // Good shapes decode back.
        assert_eq!(decode_id(&encode_id(123)).unwrap(), 123);
        let (id, s) = decode_push(&encode_push(5, &[1.0, -2.0])).unwrap();
        assert_eq!((id, s), (5, vec![1.0, -2.0]));
        assert_eq!(decode_samples(&encode_samples(&[0.5])).unwrap(), vec![0.5]);
        assert_eq!(
            decode_id_text(&encode_id_text(3, "hi")).unwrap(),
            (3, "hi".to_string())
        );
    }
}
