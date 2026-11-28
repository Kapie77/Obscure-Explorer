//! PlayStation 2 audio of the games: raw PS-ADPCM streams (`.mib`, with a `.mih` header) and sound
//! banks (`.msb` data with a `.msh` index). PS-ADPCM is the format of the PlayStation SPU: blocks of
//! 16 bytes (shift / filter byte, flag byte, 14 bytes of nibbles = 28 samples).
//!
//! * `.mih` (little-endian): header size (0x40), ?, channels at 0x08, sample rate at 0x0C, interleave
//!   (size in bytes of each channel's chunk) at 0x10, number of chunks at 0x14.
//!   Without a `.mih` a `.mib` is mono; the sample rate is then a guess: 44100 Hz for Obscure 2 (checked
//!   against the Wii version of the same voice line), 22050 Hz for Obscure 1 (the voices of the PS2 version
//!   come out an octave too high at 44100 Hz).
//! * `.msh` (little-endian): file size, ?, sound count at 0x08, then 16-byte entries: size, 0, offset into
//!   the `.msb`, sample rate. The `.msb` is the sounds one after the other, each its own mono PS-ADPCM stream.

use crate::wsi::Decoded;

const FILTERS: [(i32, i32); 5] = [(0, 0), (60, 0), (115, -52), (98, -55), (122, -60)];

/// Decodes one channel of PS-ADPCM (history starts at zero).
pub fn ps_adpcm_decode(data: &[u8]) -> Vec<i16> {
    let mut out = Vec::with_capacity(data.len() / 16 * 28);
    let (mut h1, mut h2) = (0i32, 0i32);
    for block in data.chunks_exact(16) {
        let mut shift = (block[0] & 0x0F) as u32;
        let mut pred = (block[0] >> 4) as usize;
        if shift > 12 {
            shift = 9;
        }
        if pred > 4 {
            pred = 0;
        }
        let (f0, f1) = FILTERS[pred];
        for i in 0..28 {
            let byte = block[2 + i / 2];
            let mut n = if i % 2 == 0 { (byte & 0x0F) as i32 } else { (byte >> 4) as i32 };
            if n >= 8 {
                n -= 16;
            }
            let s = ((n << 12) >> shift) + ((f0 * h1 + f1 * h2 + 32) >> 6);
            let s = s.clamp(-32768, 32767);
            out.push(s as i16);
            h2 = h1;
            h1 = s;
        }
    }
    out
}

fn le32(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub struct Mih {
    pub channels: u32,
    pub sample_rate: u32,
    pub interleave: u32,
    pub chunks: u32,
}

pub fn parse_mih(data: &[u8]) -> Option<Mih> {
    let channels = le32(data, 0x08)?;
    let sample_rate = le32(data, 0x0C)?;
    if !(1..=2).contains(&channels) || !(1000..=192_000).contains(&sample_rate) {
        return None;
    }
    Some(Mih {
        channels,
        sample_rate,
        interleave: le32(data, 0x10).unwrap_or(0),
        chunks: le32(data, 0x14).unwrap_or(0),
    })
}

pub fn mih_summary(data: &[u8]) -> String {
    match parse_mih(data) {
        Some(m) => format!(
            "MIH header (the audio itself is in the .mib with the same name)\n\nchannels:   {}\nsample rate: {} Hz\ninterleave: {} bytes per channel chunk\nchunks:     {}\n",
            m.channels, m.sample_rate, m.interleave, m.chunks
        ),
        None => "This does not look like a .mih header (channels or sample rate are out of range).\n".to_string(),
    }
}

/// A `.mib` stream; `mih` gives the channels, the rate and the interleave when it is known.
pub fn decode_mib(data: &[u8], mih: Option<&Mih>, default_rate: u32) -> Result<Decoded, String> {
    let (channels, sample_rate, interleave) = match mih {
        Some(m) => (m.channels, m.sample_rate, m.interleave as usize),
        None => (1, default_rate, 0),
    };
    if data.len() < 16 {
        return Err("the .mib file is too small".into());
    }
    if channels == 1 {
        let samples = ps_adpcm_decode(data);
        return Ok(Decoded { samples, channels: 1, sample_rate });
    }
    // stereo: chunks of `interleave` bytes alternate between the two channels
    let chunk = if interleave >= 16 && interleave % 16 == 0 { interleave } else { 0x10000 };
    let mut sides: [Vec<u8>; 2] = [Vec::new(), Vec::new()];
    for (k, part) in data.chunks(chunk).enumerate() {
        sides[k % 2].extend_from_slice(part);
    }
    let (left, right) = (ps_adpcm_decode(&sides[0]), ps_adpcm_decode(&sides[1]));
    let n = left.len().min(right.len());
    let mut samples = Vec::with_capacity(n * 2);
    for i in 0..n {
        samples.push(left[i]);
        samples.push(right[i]);
    }
    Ok(Decoded { samples, channels: 2, sample_rate })
}

pub struct BankEntry {
    pub size: usize,
    pub offset: usize,
    pub rate: u32,
}

pub fn parse_msh(data: &[u8]) -> Result<Vec<BankEntry>, String> {
    let count = le32(data, 8).ok_or("the .msh file is too small")? as usize;
    if count == 0 || count > 4096 || 12 + count * 16 > data.len() {
        return Err(format!("implausible sound count {count} in the .msh"));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let o = 12 + i * 16;
        out.push(BankEntry {
            size: le32(data, o).unwrap_or(0) as usize,
            offset: le32(data, o + 8).unwrap_or(0) as usize,
            rate: le32(data, o + 12).unwrap_or(0),
        });
    }
    Ok(out)
}

pub fn msh_summary(data: &[u8]) -> String {
    match parse_msh(data) {
        Ok(entries) => {
            let mut s = format!(
                "MSH sound index: {} sounds (the audio itself is in the .msb with the same name)\n\n  #   offset      size   rate      length\n",
                entries.len()
            );
            for (i, e) in entries.iter().enumerate() {
                let secs = if e.rate > 0 { (e.size / 16 * 28) as f64 / e.rate as f64 } else { 0.0 };
                s.push_str(&format!("{i:3}  {:#9x}  {:7}  {:5} Hz  {secs:6.2} s\n", e.offset, e.size, e.rate));
            }
            s
        }
        Err(e) => format!("Cannot read this .msh: {e}\n"),
    }
}

/// The sounds of a bank that has no `.msh` index (Obscure 1): each sound ends with a block whose flag
/// byte has the "end" bit (1, or 3 for a loop end), usually followed by a lone "7" block that only
/// terminates it. Returns the PS-ADPCM bytes of every sound.
pub fn split_sounds(data: &[u8]) -> Vec<&[u8]> {
    let frames = data.len() / 16;
    let mut out = Vec::new();
    let mut start = 0usize;
    for i in 0..frames {
        let flag = data[i * 16 + 1];
        if flag & 1 != 0 {
            if flag == 7 && i == start {
                start = i + 1; // the terminator block after an end block
                continue;
            }
            out.push(&data[start * 16..(i + 1) * 16]);
            start = i + 1;
        }
    }
    if frames > start + 1 {
        out.push(&data[start * 16..frames * 16]);
    }
    out.retain(|c| c.len() >= 32);
    out
}

/// Sounds of an index-less bank, decoded (the sample rate is a guess, 22050 Hz like the other banks).
pub fn decode_bank_without_index(msb: &[u8], rate: u32) -> Result<Vec<(Vec<i16>, u32)>, String> {
    let clips: Vec<(Vec<i16>, u32)> = split_sounds(msb).into_iter().map(|c| (ps_adpcm_decode(c), rate)).collect();
    if clips.is_empty() {
        return Err("no sounds found in the .msb (it has no .msh index and no end markers)".into());
    }
    Ok(clips)
}

/// Every sound of a bank: (samples, sample rate).
pub fn decode_bank_clips(msb: &[u8], msh: &[u8]) -> Result<Vec<(Vec<i16>, u32)>, String> {
    let entries = parse_msh(msh)?;
    let mut clips = Vec::new();
    for e in &entries {
        let end = e.offset.saturating_add(e.size).min(msb.len());
        let part = msb.get(e.offset.min(end)..end).unwrap_or(&[]);
        if part.len() >= 16 && (1000..=192_000).contains(&e.rate) {
            clips.push((ps_adpcm_decode(part), e.rate));
        }
    }
    if clips.is_empty() {
        return Err("no playable sounds in the bank (is the .msh the index of this .msb?)".into());
    }
    Ok(clips)
}

/// Nearest-neighbour rate conversion (only used when the sounds of a bank have different rates).
fn resample(samples: &[i16], from: u32, to: u32) -> Vec<i16> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let n = (samples.len() as u64 * to as u64 / from as u64) as usize;
    (0..n).map(|i| samples[(i as u64 * from as u64 / to as u64) as usize]).collect()
}

/// The whole bank as one track: the sounds one after the other with a short silence in between.
pub fn bank_to_pcm(clips: &[(Vec<i16>, u32)]) -> Decoded {
    let rate = clips[0].1;
    let gap = (rate as usize) * 3 / 10;
    let mut samples = Vec::new();
    for (i, (s, r)) in clips.iter().enumerate() {
        if i > 0 {
            samples.extend(std::iter::repeat(0i16).take(gap));
        }
        samples.extend(resample(s, *r, rate));
    }
    Decoded { samples, channels: 1, sample_rate: rate }
}

pub fn decode_bank_msb(msb: &[u8], msh: &[u8]) -> Result<Decoded, String> {
    Ok(bank_to_pcm(&decode_bank_clips(msb, msh)?))
}

/// Whether a `.pss` has an MPEG video stream (0xE0..0xEF). The cutscenes of the PS2 version of Obscure 1
/// do not: the game draws them itself and the file only carries the sound plus animation data.
pub fn pss_has_video(data: &[u8]) -> bool {
    let mut pos = 0usize;
    while pos + 6 <= data.len() {
        if data[pos] != 0 || data[pos + 1] != 0 || data[pos + 2] != 1 {
            match data[pos..].windows(3).position(|w| w == [0, 0, 1]) {
                Some(p) => pos += p,
                None => return false,
            }
            continue;
        }
        let sid = data[pos + 3];
        if (0xE0..=0xEF).contains(&sid) {
            return true;
        }
        if sid == 0xBA {
            pos += match data.get(pos + 4) {
                Some(b) if b >> 6 == 1 => 14 + (data.get(pos + 13).copied().unwrap_or(0) & 7) as usize,
                _ => 12,
            };
        } else if sid >= 0xBB {
            let len = u16::from_be_bytes([data[pos + 4], data[pos + 5]]) as usize;
            pos += 6 + len;
        } else {
            pos += 4;
        }
    }
    false
}

/// The audio of a PS2 `.pss` movie. A PSS is an MPEG program stream; the sound is PS-ADPCM in the
/// "private stream 1" packets (0xBD). Each packet starts with a 4-byte substream header (FF, id, 0, 0);
/// the first one is followed by an `SShd` block (codec 0x10 = PS-ADPCM, sample rate, channels, interleave,
/// all little-endian) and an `SSbd` header, then the audio. The video players that skip these packets
/// show the movie without sound.
pub fn pss_audio(data: &[u8]) -> Option<Decoded> {
    let mut audio: Vec<u8> = Vec::new();
    let mut header: Option<(u32, u32, u32)> = None; // sample rate, channels, interleave
    let mut pos = 0usize;
    while pos + 4 <= data.len() {
        if data[pos] != 0 || data[pos + 1] != 0 || data[pos + 2] != 1 {
            pos += 1;
            continue;
        }
        let sid = data[pos + 3];
        if sid == 0xBA {
            // pack header: MPEG-2 (14 bytes + stuffing) or MPEG-1 (12 bytes)
            pos += match data.get(pos + 4) {
                Some(b) if b >> 6 == 1 => 14 + (data.get(pos + 13).copied().unwrap_or(0) & 7) as usize,
                _ => 12,
            };
            continue;
        }
        if matches!(sid, 0xBB | 0xBD | 0xBE | 0xBF) || (0xC0..=0xEF).contains(&sid) {
            let len = match data.get(pos + 4..pos + 6) {
                Some(b) => u16::from_be_bytes([b[0], b[1]]) as usize,
                None => break,
            };
            let end = (pos + 6 + len).min(data.len());
            let body = &data[pos + 6..end];
            if sid == 0xBD && body.len() > 3 {
                let header_len = body[2] as usize;
                if let Some(payload) = body.get(3 + header_len..) {
                    if payload.len() > 4 && payload[0] == 0xFF {
                        let mut d = &payload[4..];
                        if d.starts_with(b"SShd") && d.len() >= 0x20 {
                            if le32(d, 8) != Some(0x10) {
                                return None; // not PS-ADPCM
                            }
                            header = Some((le32(d, 12)?, le32(d, 16)?, le32(d, 20)?));
                            d = &d[0x20..];
                        }
                        if d.starts_with(b"SSbd") && d.len() >= 8 {
                            d = &d[8..];
                        }
                        audio.extend_from_slice(d);
                    }
                }
            }
            pos += 6 + len;
            continue;
        }
        pos += 1;
    }
    let (sample_rate, channels, interleave) = header?;
    if audio.len() < 16 || !(1000..=192_000).contains(&sample_rate) || !(1..=2).contains(&channels) {
        return None;
    }
    let mih = Mih { channels, sample_rate, interleave, chunks: 0 };
    decode_mib(&audio, Some(&mih), 44100).ok()
}

/// Interleaved 16-bit samples as stereo at 44100 Hz (what the video player expects).
pub fn to_stereo_44100(samples: &[i16], channels: u16, rate: u32) -> Vec<i16> {
    let frames = samples.len() / channels.max(1) as usize;
    let out_frames = (frames as u64 * 44100 / rate.max(1) as u64) as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let src = ((i as u64 * rate as u64 / 44100) as usize).min(frames.saturating_sub(1));
        if channels >= 2 {
            out.push(samples[src * channels as usize]);
            out.push(samples[src * channels as usize + 1]);
        } else {
            let v = samples[src];
            out.push(v);
            out.push(v);
        }
    }
    out
}

/// A 16-bit PCM `.wav` file.
pub fn wav_bytes(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + samples.len() * 2);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&channels.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
    w.extend_from_slice(&(channels * 2).to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block with filter 0, shift 12 and the first two nibbles `a`, `b`.
    fn block(a: u8, b: u8) -> [u8; 16] {
        let mut f = [0u8; 16];
        f[0] = 0x0C;
        f[2] = a | (b << 4);
        f
    }

    #[test]
    fn decodes_ps_adpcm_blocks() {
        let mut d = block(1, 2).to_vec();
        d.extend(block(0xF, 3)); // -1 and 3
        let out = ps_adpcm_decode(&d);
        assert_eq!(out.len(), 56);
        assert_eq!(&out[..3], &[1, 2, 0]);
        assert_eq!(&out[28..30], &[-1, 3]);
    }

    #[test]
    fn the_filter_adds_the_previous_sample() {
        let mut b = block(1, 1);
        b[0] = 0x1C; // filter 1 (60/64 of the previous sample), shift 12
        let out = ps_adpcm_decode(&b);
        assert_eq!(&out[..2], &[1, 2]); // 1, then 1 + (60 * 1 + 32) >> 6
    }

    #[test]
    fn mih_fields() {
        let mut h = vec![0u8; 0x40];
        h[0..4].copy_from_slice(&0x40u32.to_le_bytes());
        h[8..12].copy_from_slice(&2u32.to_le_bytes());
        h[12..16].copy_from_slice(&44100u32.to_le_bytes());
        h[16..20].copy_from_slice(&0x10000u32.to_le_bytes());
        h[20..24].copy_from_slice(&17u32.to_le_bytes());
        let m = parse_mih(&h).unwrap();
        assert_eq!((m.channels, m.sample_rate, m.interleave, m.chunks), (2, 44100, 0x10000, 17));
        assert!(parse_mih(&[0u8; 8]).is_none());
        assert!(mih_summary(&h).contains("44100"));
    }

    #[test]
    fn stereo_mib_alternates_chunks_between_channels() {
        // interleave 32 bytes = 2 blocks per chunk: left chunk then right chunk
        let mut data = Vec::new();
        data.extend(block(1, 0));
        data.extend(block(1, 0));
        data.extend(block(3, 0));
        data.extend(block(3, 0));
        let mih = Mih { channels: 2, sample_rate: 22050, interleave: 32, chunks: 1 };
        let d = decode_mib(&data, Some(&mih), 44100).unwrap();
        assert_eq!((d.channels, d.sample_rate), (2, 22050));
        assert_eq!(&d.samples[..4], &[1, 3, 0, 0]);
        assert_eq!(d.samples.len(), 2 * 56);
    }

    #[test]
    fn mib_without_mih_is_mono_44100() {
        let d = decode_mib(&block(1, 2), None, 44100).unwrap();
        assert_eq!((d.channels, d.sample_rate, d.samples.len()), (1, 44100, 28));
        let d = decode_mib(&block(1, 2), None, 22050).unwrap();
        assert_eq!(d.sample_rate, 22050);
    }

    fn msh(entries: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut h = vec![0u8; 12];
        h[8..12].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        for (size, offset, rate) in entries {
            h.extend(size.to_le_bytes());
            h.extend(0u32.to_le_bytes());
            h.extend(offset.to_le_bytes());
            h.extend(rate.to_le_bytes());
        }
        h
    }

    #[test]
    fn sound_bank_clips_and_track() {
        let mut msb = Vec::new();
        msb.extend(block(1, 0)); // sound 0: 32 bytes (two blocks)
        msb.extend(block(2, 0));
        msb.extend(block(3, 0)); // sound 1: 16 bytes
        let index = msh(&[(32, 0, 22050), (16, 32, 22050)]);
        let clips = decode_bank_clips(&msb, &index).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[0].0.len(), 56);
        assert_eq!(clips[1].0[0], 3);
        let track = bank_to_pcm(&clips);
        assert_eq!(track.sample_rate, 22050);
        assert_eq!(track.samples.len(), 56 + 6615 + 28); // gap: 0.3 s
        assert!(msh_summary(&index).contains("2 sounds"));
        assert!(decode_bank_clips(&msb, b"nope").is_err());
    }

    #[test]
    fn resampling_and_stereo_conversion() {
        assert_eq!(resample(&[1, 2, 3, 4], 2, 4).len(), 8);
        let st = to_stereo_44100(&[5, 6], 1, 44100);
        assert_eq!(st, [5, 5, 6, 6]);
        let st = to_stereo_44100(&[1, 2, 3, 4], 2, 44100);
        assert_eq!(st, [1, 2, 3, 4]);
    }

    /// A tiny PSS: a pack header, a video packet, and the stereo audio split over two private-stream packets.
    #[test]
    fn audio_inside_a_pss_movie() {
        let mut pss = vec![0, 0, 1, 0xBA, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        // a video packet whose payload contains something that looks like a start code
        pss.extend([0, 0, 1, 0xE0, 0, 8, 0, 0, 1, 0xBD, 9, 9, 9, 9]);
        let packet = |payload: Vec<u8>| {
            let mut body = vec![0x81, 0x00, 0x00];
            body.extend(payload);
            let mut p = vec![0, 0, 1, 0xBD];
            p.extend((body.len() as u16).to_be_bytes());
            p.extend(body);
            p
        };
        let mut first = vec![0xFF, 0xA1, 0, 0];
        first.extend(b"SShd");
        for v in [0x18u32, 0x10, 22050, 2, 0x20, 0xFFFF_FFFF, 0xFFFF_FFFF] {
            first.extend(v.to_le_bytes());
        }
        first.extend(b"SSbd");
        first.extend(64u32.to_le_bytes());
        first.extend(block(1, 0)); // left chunk (2 blocks)
        first.extend(block(1, 0));
        let mut second = vec![0xFF, 0xA0, 0, 0];
        second.extend(block(3, 0)); // right chunk
        second.extend(block(3, 0));
        pss.extend(packet(first));
        pss.extend(packet(second));
        let d = pss_audio(&pss).unwrap();
        assert_eq!((d.channels, d.sample_rate), (2, 22050));
        assert_eq!(&d.samples[..4], &[1, 3, 0, 0]);
        assert_eq!(d.samples.len(), 2 * 56);
        // a movie without audio gives nothing
        assert!(pss_audio(&[0, 0, 1, 0xBA, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn sounds_of_a_bank_without_index() {
        let mut flagged = |flag: u8| {
            let mut b = block(1, 0);
            b[1] = flag;
            b
        };
        let mut msb = Vec::new();
        msb.extend(block(1, 0));
        msb.extend(flagged(1)); // end of sound 0
        msb.extend(flagged(7)); // terminator
        msb.extend(block(2, 0));
        msb.extend(block(2, 0));
        msb.extend(flagged(1)); // end of sound 1
        msb.extend(flagged(7));
        let sounds = split_sounds(&msb);
        assert_eq!(sounds.len(), 2);
        assert_eq!(sounds[0].len(), 32);
        assert_eq!(sounds[1].len(), 48);
        let clips = decode_bank_without_index(&msb, 22050).unwrap();
        assert_eq!(clips.len(), 2);
        assert_eq!(clips[1].1, 22050);
        assert!(decode_bank_without_index(&[0u8; 16], 22050).is_err());
    }

    #[test]
    fn pss_with_and_without_video() {
        let pack = [0u8, 0, 1, 0xBA, 0x44, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut only_private = pack.to_vec();
        only_private.extend([0, 0, 1, 0xBD, 0, 4, 0x81, 0, 0, 0xFF]);
        assert!(!pss_has_video(&only_private));
        let mut with_video = only_private.clone();
        with_video.extend([0, 0, 1, 0xE0, 0, 2, 0, 0]);
        assert!(pss_has_video(&with_video));
    }

    #[test]
    fn wav_header() {
        let w = wav_bytes(&[1, -1], 1, 8000);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes([w[24], w[25], w[26], w[27]]), 8000);
        assert_eq!(w.len(), 44 + 4);
        assert_eq!(&w[44..48], &[1, 0, 0xFF, 0xFF]);
    }
}
