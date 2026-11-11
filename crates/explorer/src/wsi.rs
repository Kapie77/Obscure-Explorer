//! `.wsi` audio of the Wii version: Nintendo DSP-ADPCM (the format of GameCube / Wii sound
//! streams) in a block container.
//!
//! Layout (big-endian), worked out from the game files:
//! * 0x20 bytes of file header (the first word is the offset of the first block).
//! * Blocks, one after the other: a 16-byte header (block size including the header, 1, channel
//!   number - 1 / 2 for the left / right channel, 3 for mono - and 0) and the payload, normally 256
//!   bytes. The last block of each channel is shorter.
//! * Joining the payloads of one channel gives its stream: a 0x60-byte DSP header (sample count,
//!   nibble count, sample rate, format, addresses, 16 predictor coefficients, gain, initial scale,
//!   two history samples) followed by the ADPCM frames (8 bytes = 1 header byte + 14 samples).
//!
//! Reading the file as one long stream (ignoring the block headers) puts 16 bytes of garbage in
//! the audio every 256 bytes, which sounds like a metallic buzz.

const DSP_HEADER_SIZE: usize = 0x60;

pub struct Decoded {
    /// Interleaved when there are two channels.
    pub samples: Vec<i16>,
    pub channels: u16,
    pub sample_rate: u32,
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    let b = d.get(o..o + 4)?;
    Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn be16(d: &[u8], o: usize) -> Option<u16> {
    let b = d.get(o..o + 2)?;
    Some(u16::from_be_bytes([b[0], b[1]]))
}

/// Decodes the stream of one channel (DSP header + frames).
fn decode_channel(stream: &[u8]) -> Result<(Vec<i16>, u32), String> {
    if stream.len() <= DSP_HEADER_SIZE {
        return Err("a channel of the .wsi has no audio data".into());
    }
    let samples = be32(stream, 0).unwrap_or(0) as usize;
    let sample_rate = be32(stream, 8).unwrap_or(0);
    let format = be16(stream, 14).unwrap_or(1);
    if format != 0 {
        return Err(format!("unsupported .wsi sample format {format} (only DSP-ADPCM is known)"));
    }
    if !(1000..=192_000).contains(&sample_rate) || samples == 0 || samples > 200_000_000 {
        return Err(format!("implausible .wsi header ({samples} samples at {sample_rate} Hz)"));
    }
    let mut coefs = [0i32; 16];
    for (i, c) in coefs.iter_mut().enumerate() {
        *c = be16(stream, 0x1C + i * 2).unwrap_or(0) as i16 as i32;
    }
    let mut hist1 = be16(stream, 0x40).unwrap_or(0) as i16 as i32;
    let mut hist2 = be16(stream, 0x42).unwrap_or(0) as i16 as i32;

    let mut out: Vec<i16> = Vec::with_capacity(samples);
    'frames: for frame in stream[DSP_HEADER_SIZE..].chunks(8) {
        if frame.len() < 2 {
            break;
        }
        let ps = frame[0];
        let scale = 1i32 << (ps & 0x0F);
        let idx = ((ps >> 4) & 7) as usize;
        let (c1, c2) = (coefs[idx * 2], coefs[idx * 2 + 1]);
        for i in 0..14 {
            if out.len() >= samples {
                break 'frames;
            }
            // the very last frame of a stream can be cut short
            let Some(&byte) = frame.get(1 + i / 2) else { break 'frames };
            let mut n = if i % 2 == 0 { (byte >> 4) as i32 } else { (byte & 0x0F) as i32 };
            if n >= 8 {
                n -= 16;
            }
            let s = (((n * scale) << 11) + 1024 + c1 * hist1 + c2 * hist2) >> 11;
            let s = s.clamp(-32768, 32767);
            out.push(s as i16);
            hist2 = hist1;
            hist1 = s;
        }
    }
    if out.is_empty() {
        return Err("no audio data in the .wsi file".into());
    }
    Ok((out, sample_rate))
}

pub fn decode_wsi(data: &[u8]) -> Result<Decoded, String> {
    let first = be32(data, 0).unwrap_or(0) as usize;
    if first < 8 || first + 16 > data.len() {
        return Err("not a .wsi file (bad header)".into());
    }
    // join the payloads of every channel
    let mut streams: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut off = first;
    while off + 16 <= data.len() {
        let size = be32(data, off).unwrap_or(0) as usize;
        let channel = be32(data, off + 8).unwrap_or(0);
        if size < 16 {
            break;
        }
        let payload = &data[off + 16..(off + size).min(data.len())];
        match streams.iter_mut().find(|(c, _)| *c == channel) {
            Some((_, v)) => v.extend_from_slice(payload),
            None => streams.push((channel, payload.to_vec())),
        }
        off += size;
    }
    match streams.len() {
        0 => Err("no audio blocks found in the .wsi file".into()),
        1 => {
            let (samples, sample_rate) = decode_channel(&streams[0].1)?;
            Ok(Decoded { samples, channels: 1, sample_rate })
        }
        2 => {
            if streams[0].1 == streams[1].1 {
                // the same audio on both sides: play it once
                let (samples, sample_rate) = decode_channel(&streams[0].1)?;
                return Ok(Decoded { samples, channels: 1, sample_rate });
            }
            let (left, sample_rate) = decode_channel(&streams[0].1)?;
            let (right, _) = decode_channel(&streams[1].1)?;
            let n = left.len().min(right.len());
            let mut samples = Vec::with_capacity(n * 2);
            for i in 0..n {
                samples.push(left[i]);
                samples.push(right[i]);
            }
            Ok(Decoded { samples, channels: 2, sample_rate })
        }
        n => Err(format!("{n} channels found; only mono and stereo .wsi files are supported")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DSP header with all coefficients 0 (every sample is just nibble * scale).
    fn dsp_header(samples: u32) -> Vec<u8> {
        let mut h = vec![0u8; DSP_HEADER_SIZE];
        h[0..4].copy_from_slice(&samples.to_be_bytes());
        h[8..12].copy_from_slice(&44100u32.to_be_bytes());
        h
    }

    fn block(channel: u32, payload: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend((16 + payload.len() as u32).to_be_bytes());
        b.extend(1u32.to_be_bytes());
        b.extend(channel.to_be_bytes());
        b.extend(0u32.to_be_bytes());
        b.extend_from_slice(payload);
        b
    }

    fn file(blocks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut d = vec![0u8; 0x20];
        d[0..4].copy_from_slice(&0x20u32.to_be_bytes());
        for b in blocks {
            d.extend(b);
        }
        d
    }

    fn with_header(samples: u32, frames: &[u8]) -> Vec<u8> {
        let mut p = dsp_header(samples);
        p.extend_from_slice(frames);
        p
    }

    #[test]
    fn decodes_plain_nibbles() {
        // header byte 0 (scale 1, predictor 0), nibbles 1 2 3 F(-1) then zeros
        let d = file(vec![block(3, &with_header(14, &[0x00, 0x12, 0x3F, 0, 0, 0, 0, 0]))]);
        let out = decode_wsi(&d).unwrap();
        assert_eq!((out.channels, out.sample_rate), (1, 44100));
        assert_eq!(out.samples.len(), 14);
        assert_eq!(&out.samples[..4], &[1, 2, 3, -1]);
    }

    #[test]
    fn frames_continue_across_block_boundaries() {
        // the second frame sits in the next block: its block header must not be read as audio
        let f1 = [0x00, 0x11, 0, 0, 0, 0, 0, 0];
        let f2 = [0x00, 0x22, 0, 0, 0, 0, 0, 0];
        let d = file(vec![block(3, &with_header(28, &f1)), block(3, &f2)]);
        let out = decode_wsi(&d).unwrap();
        assert_eq!(out.samples.len(), 28);
        assert_eq!(&out.samples[..2], &[1, 1]);
        assert_eq!(&out.samples[14..16], &[2, 2]);
    }

    #[test]
    fn two_different_channels_are_interleaved() {
        let left = [0x00, 0x11, 0, 0, 0, 0, 0, 0];
        let right = [0x00, 0x55, 0, 0, 0, 0, 0, 0];
        let d = file(vec![block(1, &with_header(14, &left)), block(2, &with_header(14, &right))]);
        let out = decode_wsi(&d).unwrap();
        assert_eq!(out.channels, 2);
        assert_eq!(&out.samples[..4], &[1, 5, 1, 5]);
    }

    #[test]
    fn identical_channels_play_as_mono() {
        let f = with_header(14, &[0x00, 0x33, 0, 0, 0, 0, 0, 0]);
        let d = file(vec![block(1, &f), block(2, &f)]);
        let out = decode_wsi(&d).unwrap();
        assert_eq!(out.channels, 1);
        assert_eq!(out.samples[0], 3);
    }

    #[test]
    fn prediction_uses_the_previous_samples() {
        // coefficient pair 0 = (2048, 0): each sample adds the previous one
        let mut p = with_header(14, &[0x02, 0x11, 0, 0, 0, 0, 0, 0]);
        p[0x1C..0x1E].copy_from_slice(&2048i16.to_be_bytes());
        let out = decode_wsi(&file(vec![block(3, &p)])).unwrap();
        assert_eq!(&out.samples[..2], &[4, 8]); // scale 4: 1 * 4, then 1 * 4 + 4 = 8
    }

    #[test]
    fn a_short_last_frame_and_the_sample_count() {
        // 6 samples declared, the frame only has its header byte and 3 data bytes
        let d = file(vec![block(3, &with_header(6, &[0x00, 0x12, 0x34, 0x56]))]);
        assert_eq!(decode_wsi(&d).unwrap().samples, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn rejects_other_data() {
        assert!(decode_wsi(b"short").is_err());
        let mut p = with_header(14, &[0; 8]);
        p[14..16].copy_from_slice(&1u16.to_be_bytes()); // not DSP-ADPCM
        assert!(decode_wsi(&file(vec![block(3, &p)])).is_err());
        let f = with_header(14, &[0; 8]);
        let three = file(vec![block(1, &f), block(2, &f), block(3, &f)]);
        assert!(decode_wsi(&three).unwrap_err().contains("channels"));
    }
}
