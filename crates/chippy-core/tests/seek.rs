//! Seeking in an emulated track: a generated Sega Genesis VGM (a YM2612
//! FM tone and an SN76489 square wave) that loops forever.

use chippy_core::{open_decoder, DecodeOptions, Decoder, Location, TrackRef};

/// A VGM that holds one chord for `seconds`, then loops.
fn looping_vgm(seconds: u32) -> Vec<u8> {
    let mut d = Vec::new();
    let mut fm = |reg: u8, val: u8| d.extend_from_slice(&[0x52, reg, val]);
    fm(0x22, 0); // LFO off
    fm(0x27, 0);
    fm(0x2B, 0); // no DAC
    fm(0xB0, 0x07); // channel 1: algorithm 7, every operator heard
    fm(0xB4, 0xC0); // both speakers
    for op in [0u8, 4, 8, 12] {
        fm(0x30 + op, 0x01);
        fm(0x40 + op, if op == 12 { 0 } else { 0x7F }); // only the last one loud
        fm(0x50 + op, 0x1F);
        fm(0x60 + op, 0);
        fm(0x70 + op, 0);
        fm(0x80 + op, 0x0F);
    }
    fm(0xA4, 0x22); // about 440 Hz
    fm(0xA0, 0x69);
    fm(0x28, 0xF0); // key on
    d.extend_from_slice(&[0x50, 0x8E, 0x50, 0x0F, 0x50, 0x90]); // PSG tone
    let total = seconds * 44_100;
    let loop_at = 0x40 + d.len();
    let mut left = total;
    while left > 0 {
        let n = left.min(0xFFFF);
        d.push(0x61); // wait n samples
        d.extend_from_slice(&(n as u16).to_le_bytes());
        left -= n;
    }
    d.push(0x66); // end (back to the loop point)

    let mut h = vec![0u8; 0x40];
    let le = |h: &mut Vec<u8>, at: usize, v: u32| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
    h[..4].copy_from_slice(b"Vgm ");
    le(&mut h, 0x04, (0x40 + d.len() - 4) as u32);
    le(&mut h, 0x08, 0x150);
    le(&mut h, 0x0C, 3_579_545); // SN76489
    le(&mut h, 0x18, total);
    le(&mut h, 0x1C, (loop_at - 0x1C) as u32);
    le(&mut h, 0x20, total);
    h[0x28] = 9; // SN76489 feedback and shift width
    h[0x2A] = 16;
    le(&mut h, 0x2C, 7_670_453); // YM2612
    le(&mut h, 0x34, 0x40 - 0x34); // the commands follow the header
    h.extend(d);
    h
}

/// Plays up to `ms`; returns the loudness (RMS) and how long it played.
fn play(dec: &mut dyn Decoder, ms: u64) -> (f32, u64) {
    let rate = dec.sample_rate() as u64;
    let mut buf = vec![0f32; 2048];
    let (mut sum, mut frames) = (0f64, 0u64);
    while frames < rate * ms / 1000 {
        let n = dec.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        sum += buf[..n * 2]
            .iter()
            .map(|&s| s as f64 * s as f64)
            .sum::<f64>();
        frames += n as u64;
    }
    (
        (sum / (frames.max(1) * 2) as f64).sqrt() as f32,
        frames * 1000 / rate,
    )
}

/// Each test its own file: they run at the same time.
fn open_genesis(name: &str, seconds: u32) -> Box<dyn Decoder> {
    let dir = std::env::temp_dir().join(format!("chippy-seek-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.vgm"));
    std::fs::write(&path, looping_vgm(seconds)).unwrap();
    open_decoder(
        &TrackRef::file(Location::Fs(path)),
        &DecodeOptions::default(),
    )
    .unwrap()
}

#[test]
fn genesis_plays_after_seeking_either_way() {
    let mut dec = open_genesis("either-way", 4);
    let (start, _) = play(&mut *dec, 300);
    assert!(start > 0.05, "silent from the start: {start}");
    for ms in [5_000, 1_000, 6_500, 0] {
        dec.seek(ms).unwrap();
        let (level, played) = play(&mut *dec, 300);
        assert!(played >= 300, "ended {played} ms after seeking to {ms}");
        assert!(
            (level - start).abs() < 0.02,
            "level {level} after seeking to {ms}, {start} before"
        );
    }
}

#[test]
fn genesis_still_ends_after_seeking_back() {
    let mut dec = open_genesis("ends", 2);
    let duration = dec.duration_ms().unwrap();
    dec.seek(duration - 1_000).unwrap();
    dec.seek(1_000).unwrap();
    let (_, played) = play(&mut *dec, duration * 3);
    assert!(
        played.abs_diff(duration - 1_000) < 100,
        "played {played} ms after seeking back to 1 s in a {duration} ms track"
    );
}

#[test]
fn genesis_seeks_in_small_steps() {
    let mut dec = open_genesis("steps", 4);
    let mut steps = 0;
    while !dec.seek_step(5_000).unwrap() {
        steps += 1;
    }
    // 250 ms of the song per step.
    assert!(steps >= 19, "{steps} steps");
    let (level, _) = play(&mut *dec, 300);
    assert!(level > 0.05, "silent after a stepped seek: {level}");
}

/// A mono 16-bit WAV whose every sample holds the second it is in (0.1 in
/// the first, 0.2 in the second...), at 8 kHz.
fn second_marked_wav(seconds: u32) -> Vec<u8> {
    let rate = 8_000u32;
    let mut pcm = Vec::new();
    for s in 0..seconds {
        let v = ((s + 1) as f32 * 0.1 * 32767.0) as i16;
        for _ in 0..rate {
            pcm.extend_from_slice(&v.to_le_bytes());
        }
    }
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    w.extend(pcm);
    w
}

/// Seeks a decoder of the marked WAV around and checks where it landed.
fn check_marked(dec: &mut dyn Decoder, what: &str) {
    let mut buf = vec![0f32; 512];
    for s in [5u32, 2, 7, 0, 3] {
        // The middle of second `s`, clear of resampling at the edges.
        dec.seek(s as u64 * 1000 + 500).unwrap();
        let got = dec.read(&mut buf).unwrap();
        assert!(got > 1, "{what}: nothing after seeking to {s}.5 s");
        let v = buf[2]; // the second frame, left channel
        let want = (s + 1) as f32 * 0.1;
        assert!(
            (v - want).abs() < 0.01,
            "{what}: {v} after seeking to {s}.5 s, wanted {want}"
        );
    }
}

#[test]
fn streamed_formats_land_where_asked() {
    let dir = std::env::temp_dir().join(format!("chippy-seek-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("marked.wav");
    std::fs::write(&path, second_marked_wav(8)).unwrap();
    let loc = Location::Fs(path);
    let opts = DecodeOptions::default();
    let mut sym = chippy_core::decode::sym::SymDecoder::open(&loc).unwrap();
    check_marked(&mut sym, "Symphonia");
    #[cfg(feature = "vgmstream")]
    {
        let mut vgm = chippy_core::decode::vgm::VgmDecoder::open(&loc, None, &opts).unwrap();
        check_marked(&mut vgm, "vgmstream");
    }
    let _ = opts;
}
