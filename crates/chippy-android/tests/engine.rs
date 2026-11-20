//! Drives the engine as the Compose UI does, on the desktop: without a
//! sound device the output is a silent clock, so playback still moves.

use chippy_android::*;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Seen {
    browser: Option<BrowserState>,
    playlist: Option<PlaylistState>,
    now: Option<NowState>,
    settings: Option<SettingsState>,
    events: Vec<UiEvent>,
}

struct Collect(Arc<Mutex<Seen>>);

impl Listener for Collect {
    fn browser(&self, s: BrowserState) {
        self.0.lock().unwrap().browser = Some(s);
    }
    fn playlist(&self, s: PlaylistState) {
        self.0.lock().unwrap().playlist = Some(s);
    }
    fn now(&self, s: NowState) {
        self.0.lock().unwrap().now = Some(s);
    }
    fn settings(&self, s: SettingsState) {
        self.0.lock().unwrap().settings = Some(s);
    }
    fn export(&self, _: ExportState) {}
    fn art(&self, _: Option<Vec<u8>>) {}
    fn event(&self, e: UiEvent) {
        self.0.lock().unwrap().events.push(e);
    }
}

/// A 3 s, 44.1 kHz stereo sine WAV.
fn tone(path: &std::path::Path) {
    let rate = 44_100u32;
    let mut pcm = Vec::new();
    for i in 0..rate * 3 {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 12_000.0) as i16;
        pcm.extend(s.to_le_bytes());
        pcm.extend(s.to_le_bytes());
    }
    let mut f = b"RIFF".to_vec();
    f.extend((36 + pcm.len() as u32).to_le_bytes());
    f.extend(b"WAVEfmt ");
    f.extend(16u32.to_le_bytes());
    f.extend(1u16.to_le_bytes());
    f.extend(2u16.to_le_bytes());
    f.extend(rate.to_le_bytes());
    f.extend((rate * 4).to_le_bytes());
    f.extend(4u16.to_le_bytes());
    f.extend(16u16.to_le_bytes());
    f.extend(b"data");
    f.extend((pcm.len() as u32).to_le_bytes());
    f.extend(pcm);
    std::fs::write(path, f).unwrap();
}

fn wait_for(seen: &Arc<Mutex<Seen>>, what: &str, ok: impl Fn(&Seen) -> bool) {
    let start = Instant::now();
    while !ok(&seen.lock().unwrap()) {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn browses_plays_and_seeks() {
    let data = tempfile::tempdir().unwrap();
    let storage = tempfile::tempdir().unwrap();
    let music = storage.path().join("Music").join("Chip");
    std::fs::create_dir_all(&music).unwrap();
    tone(&music.join("a tone.wav"));
    tone(&music.join("b tone.wav"));

    let seen = Arc::new(Mutex::new(Seen::default()));
    let engine = Engine::new(
        data.path().to_string_lossy().into(),
        storage.path().to_string_lossy().into(),
        Box::new(Collect(seen.clone())),
    );

    // It starts in the shared storage, named as Android names it.
    wait_for(&seen, "the first listing", |s| s.browser.is_some());
    {
        let s = seen.lock().unwrap();
        let b = s.browser.as_ref().unwrap();
        assert_eq!(b.title, "Internal storage");
        assert_eq!(b.items.len(), 1);
        assert_eq!(b.items[0].kind, FileKind::Folder);
        assert!(s.settings.is_some() && s.now.is_some());
    }

    engine.activate(0);
    engine.go_path(music.to_string_lossy().into());
    {
        let s = seen.lock().unwrap();
        let b = s.browser.as_ref().unwrap();
        assert_eq!(
            b.crumbs
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            ["Internal storage", "Music", "Chip"]
        );
        assert_eq!(b.items.len(), 2);
        assert_eq!(b.items[0].stem, "a tone");
        assert_eq!(b.items[0].ext, "wav");
        assert!(b.can_up && b.can_back);
    }

    // Up again marks the folder just left.
    engine.go_up();
    assert_eq!(seen.lock().unwrap().browser.as_ref().unwrap().selected, 0);
    engine.go_back();

    // A filter narrows the listing.
    engine.set_filter("b to".into());
    assert_eq!(
        seen.lock().unwrap().browser.as_ref().unwrap().items.len(),
        1
    );
    engine.set_filter(String::new());

    // A tap plays it, with the folder as the playlist.
    engine.activate(1);
    wait_for(&seen, "playback to start", |s| {
        s.now
            .as_ref()
            .is_some_and(|n| n.state == PlayState::Playing && n.seekable)
    });
    wait_for(&seen, "the playlist's tags", |s| {
        s.playlist
            .as_ref()
            .is_some_and(|p| p.rows.len() == 2 && p.rows.iter().all(|r| !r.pending))
    });
    {
        let s = seen.lock().unwrap();
        let p = s.playlist.as_ref().unwrap();
        assert_eq!(p.playing, 1);
        assert!(p.rows[1].playing);
        assert!(p.summary.starts_with("2 tracks"), "{}", p.summary);
        let n = s.now.as_ref().unwrap();
        assert_eq!(n.duration_ms, Some(3000));
        assert_eq!(n.total, "0:03");
        // Repeat All by default: the first track comes next.
        assert_eq!(n.up_next, "a tone");
        assert!(s.browser.as_ref().unwrap().items[1].playing);
    }

    // The position moves, and the spectrum with it.
    let first = engine.frame().position_ms;
    std::thread::sleep(Duration::from_millis(300));
    let f = engine.frame();
    assert!(f.position_ms > first, "{} then {}", first, f.position_ms);
    assert_eq!(f.bars.len(), 32);

    engine.seek_fraction(0.5);
    let f = engine.frame();
    assert!((1400..2200).contains(&f.position_ms), "{}", f.position_ms);

    engine.play_pause();
    assert_eq!(
        seen.lock().unwrap().now.as_ref().unwrap().state,
        PlayState::Paused
    );

    let info = engine.track_info(1).unwrap();
    assert!(info
        .fields
        .iter()
        .any(|f| f.label == "Sample rate" && f.value == "44100 Hz"));

    // Settings come back as the UI shows them.
    engine.step("fade".into(), 1);
    assert_eq!(seen.lock().unwrap().settings.as_ref().unwrap().fade, "9 s");
    engine.add_browser_dir();
    assert_eq!(
        seen.lock().unwrap().settings.as_ref().unwrap().music_dirs[0].path,
        "Internal storage/Music/Chip"
    );

    // The first music folder is where the browser starts.
    engine.go_home();
    assert_eq!(seen.lock().unwrap().browser.as_ref().unwrap().title, "Chip");
    // A search below the shared storage finds both tones.
    engine.go_path(storage.path().to_string_lossy().into());
    engine.set_search("tone".into());
    engine.run_search();
    wait_for(&seen, "the search", |s| {
        s.browser
            .as_ref()
            .is_some_and(|b| b.in_results && !b.searching && b.items.len() == 2)
    });
    assert_eq!(
        seen.lock().unwrap().browser.as_ref().unwrap().items[0].detail,
        "Music/Chip"
    );

    // Revealing a playlist row opens the browser on it.
    engine.reveal_row(0);
    {
        let s = seen.lock().unwrap();
        assert!(s.events.contains(&UiEvent::OpenBrowser));
        assert_eq!(s.browser.as_ref().unwrap().selected, 0);
    }

    engine.clear_playlist();
    {
        let s = seen.lock().unwrap();
        assert!(s.playlist.as_ref().unwrap().rows.is_empty());
        assert!(!s.now.as_ref().unwrap().has_track);
    }

    engine.shutdown();
    assert!(data.path().join("settings.json").is_file());
}
