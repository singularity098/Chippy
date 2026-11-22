//! MIDI mode on a generated Master System VGM (an SN76489 playing a scale):
//! the notes are found, the MIDI fades in over the chip, follows seeks,
//! changes style, and fades back out to the chip as it was.

use chippy_core::decode::midi::{load_soundfont, set_soundfont};
use chippy_core::notes::style::Style;
use chippy_core::{open_decoder, DecodeOptions, Decoder, Location, TrackRef};
use chippy_engine::midimode::{self, Status};

const SF: &str = "/usr/share/sounds/sf2/TimGM6mb.sf2";

/// Square wave notes up a C major scale, half a second each, for `seconds`.
fn scale_vgm(seconds: u32) -> Vec<u8> {
    const CLOCK: u32 = 3_579_545;
    let mut d = Vec::new();
    let keys = [60u8, 62, 64, 65, 67, 69, 71, 72];
    let half = 22_050u32;
    for i in 0..seconds * 2 {
        let key = keys[i as usize % keys.len()];
        let hz = 440.0 * 2f64.powf((key as f64 - 69.0) / 12.0);
        let n = (CLOCK as f64 / (32.0 * hz)).round() as u16;
        d.extend_from_slice(&[0x50, 0x80 | (n & 0x0F) as u8, 0x50, (n >> 4) as u8 & 0x3F]);
        d.extend_from_slice(&[0x50, 0x90]); // loud
        d.push(0x61);
        d.extend_from_slice(&((half - 2_205) as u16).to_le_bytes());
        d.extend_from_slice(&[0x50, 0x9F]); // silent: the note ends
        d.push(0x61);
        d.extend_from_slice(&2_205u16.to_le_bytes());
    }
    d.push(0x66);
    let total = seconds * 44_100;
    let mut h = vec![0u8; 0x40];
    let le = |h: &mut Vec<u8>, at: usize, v: u32| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
    h[..4].copy_from_slice(b"Vgm ");
    le(&mut h, 0x04, (0x40 + d.len() - 4) as u32);
    le(&mut h, 0x08, 0x150);
    le(&mut h, 0x0C, CLOCK);
    le(&mut h, 0x18, total);
    h[0x28] = 9;
    h[0x2A] = 16;
    le(&mut h, 0x34, 0x40 - 0x34);
    h.extend(d);
    h
}

/// Reads `frames` frames from each, returning (chip energy, energy of the
/// difference).
fn compare(a: &mut dyn Decoder, b: &mut dyn Decoder, frames: usize) -> (f64, f64) {
    let (mut ba, mut bb) = (vec![0f32; 2048], vec![0f32; 2048]);
    let (mut e, mut diff, mut left) = (0f64, 0f64, frames);
    while left > 0 {
        let want = left.min(1024) * 2;
        let na = a.read(&mut ba[..want]).unwrap();
        let nb = b.read(&mut bb[..want]).unwrap();
        assert_eq!(na, nb);
        if na == 0 {
            break;
        }
        for i in 0..na * 2 {
            e += (bb[i] as f64).powi(2);
            diff += (ba[i] as f64 - bb[i] as f64).powi(2);
        }
        left -= na;
    }
    (e, diff)
}

#[test]
fn fades_between_chip_and_midi() {
    if !std::path::Path::new(SF).exists() {
        eprintln!("no test SoundFont; skipped");
        return;
    }
    set_soundfont(Some(load_soundfont(std::path::Path::new(SF)).unwrap()));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scale.vgm");
    std::fs::write(&path, scale_vgm(20)).unwrap();
    let track = TrackRef::file(Location::Fs(path));
    assert!(midimode::possible(&track));
    let opts = DecodeOptions::default();
    let open = || open_decoder(&track, &opts).unwrap();
    let mut mixed = midimode::wrap(open(), &track, opts.fade_ms);
    let mut plain = open();
    let rate = plain.sample_rate() as usize;

    // Off: the chip as it is.
    let (e, diff) = compare(&mut *mixed, &mut *plain, rate / 2);
    assert!(e > 1.0, "silent chip");
    assert_eq!(diff, 0.0);
    assert_eq!(midimode::status(), Status::Off);

    // On: the notes are worked out, then the MIDI comes in.
    midimode::set(Some(Style::Piano));
    let mut waited = 0;
    loop {
        compare(&mut *mixed, &mut *plain, 1024);
        match midimode::status() {
            Status::Playing => break,
            Status::Preparing => {}
            s => panic!("{s:?}: {}", midimode::error()),
        }
        waited += 1;
        assert!(waited < 2_000, "the notes never came");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    compare(&mut *mixed, &mut *plain, rate); // the crossfade
    let (e, diff) = compare(&mut *mixed, &mut *plain, rate);
    assert!(diff > e * 0.3, "still the chip: {diff} vs {e}");

    // Seeking keeps both going, and a change of style crossfades.
    mixed.seek(9_000).unwrap();
    plain.seek(9_000).unwrap();
    midimode::set(Some(Style::Strings));
    let (e, diff) = compare(&mut *mixed, &mut *plain, rate * 2);
    assert!(diff > e * 0.3, "the MIDI stopped after seeking");
    assert_eq!(midimode::status(), Status::Playing);

    // Off again: back to exactly the chip once faded.
    midimode::set(None);
    compare(&mut *mixed, &mut *plain, rate);
    let (_, diff) = compare(&mut *mixed, &mut *plain, rate);
    assert_eq!(diff, 0.0);
    assert_eq!(midimode::status(), Status::Off);
}
