//! The app's dialogs: the system file pickers (open a file, choose a
//! folder), a track's properties, and exporting the playlist.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chippy_core::meta::{self, format_ms, TrackMeta};
use chippy_core::vfs::{self, EntryKind, TrackRef};

use super::{ss, App};
use crate::export::{self, Format, Progress};
use crate::midimode;
use crate::tabs::{Available, TabView};
use crate::{Export, Info, InfoField, Lyrics, Now, TabBar, TabCell, TabLine, Tabs};
use chippy_core::lyrics;
use chippy_core::notes::style::Style;

/// The lyrics dialog's state: the track whose lyrics are shown.
#[derive(Default)]
pub struct LyricsState {
    pub track: Option<TrackRef>,
    pub lyrics: Option<lyrics::Lyrics>,
    pub line: i32,
}

/// The guitar tab dialog's state.
#[derive(Default)]
pub struct TabsState {
    /// What MIDI mode last said about the playing track (shown on its
    /// button, beside the tab's).
    pub midi_status: Option<midimode::Status>,
    pub shown: bool,
    pub view: TabView,
    /// Whether the playing track has lyrics and notes to show.
    pub avail: Available,
    pub line: i32,
    pub playhead: f32,
    /// Where playback was at the last tick.
    pub pos_ms: u64,
    /// What each of the merge menu's items does: merge with that part
    /// (an index into the parts), or (None) take the merge apart.
    pub merges: Vec<Option<usize>>,
}

/// What a file picker running on its own thread came back with.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum Picked {
    MusicDir(PathBuf),
    SoundfontDir(PathBuf),
    ExportDir(PathBuf),
    Open(Vec<PathBuf>),
    /// Where to save the guitar tab.
    SaveTab(PathBuf),
    /// Cancelled, or no picker could be shown.
    Nothing(Option<String>),
}

/// The export dialog's state, between opening it and finishing.
#[derive(Default)]
pub struct ExportState {
    /// Playlist rows to export.
    pub rows: Vec<usize>,
    pub cancel: Option<Arc<AtomicBool>>,
    pub total: usize,
    pub index: usize,
    pub name: String,
}

#[cfg(not(target_os = "android"))]
fn spawn_picker(
    tx: crossbeam_channel::Sender<Picked>,
    f: impl FnOnce() -> Picked + Send + 'static,
) {
    std::thread::Builder::new()
        .name("picker".into())
        .spawn(move || {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
                .unwrap_or_else(|_| Picked::Nothing(Some("The file dialog failed".into())));
            let _ = tx.send(r);
        })
        .ok();
}

impl App {
    // ------------------------------------------------------- file pickers

    /// The system's file picker, to open files (or a folder) as if chosen
    /// in the browser.
    pub(super) fn pick_open(&mut self) {
        if self.picker_open {
            return;
        }
        self.pick_open_desktop();
    }

    #[cfg(not(target_os = "android"))]
    fn pick_open_desktop(&mut self) {
        self.picker_open = true;
        let start = self.picker_start();
        spawn_picker(self.pick_tx.clone(), move || {
            let mut d = rfd::FileDialog::new().set_title("Open music");
            if let Some(s) = start {
                d = d.set_directory(s);
            }
            match d.pick_files() {
                Some(f) if !f.is_empty() => Picked::Open(f),
                _ => Picked::Nothing(None),
            }
        });
    }

    /// The system's folder picker, for a setting ("music", "soundfont") or
    /// the export folder ("export").
    pub(super) fn choose_dir(&mut self, which: &str) {
        if self.picker_open {
            return;
        }
        let current = match which {
            // A new music folder: start from the browser's folder.
            "music" => None,
            "soundfont" => self.settings.soundfont_dir.clone(),
            "export" => self.settings.export_dir.clone(),
            _ => return,
        };
        let start = current
            .filter(|d| d.is_dir())
            .or_else(|| self.picker_start());
        let which = which.to_string();
        let title = match which.as_str() {
            "music" => "Music folder",
            "soundfont" => "SoundFont folder",
            _ => "Export to folder",
        };
        {
            self.picker_open = true;
            spawn_picker(self.pick_tx.clone(), move || {
                let mut d = rfd::FileDialog::new().set_title(title);
                if let Some(s) = start {
                    d = d.set_directory(s);
                }
                match d.pick_folder() {
                    Some(p) => match which.as_str() {
                        "music" => Picked::MusicDir(p),
                        "soundfont" => Picked::SoundfontDir(p),
                        _ => Picked::ExportDir(p),
                    },
                    None => Picked::Nothing(None),
                }
            });
        }
    }

    /// Where pickers open: the browser's folder on disk, else the music
    /// folder.
    fn picker_start(&self) -> Option<PathBuf> {
        let mut node = self.node.clone();
        while let Some(n) = node {
            if let vfs::Node::Dir(d) = &n {
                return Some(d.clone());
            }
            node = n.parent();
        }
        self.settings.music_dir.clone()
    }

    pub(super) fn picked(&mut self, p: Picked) {
        self.picker_open = false;
        match p {
            Picked::MusicDir(d) => self.add_music_dir(d),
            Picked::SoundfontDir(d) => self.set_soundfont_dir(d),
            Picked::ExportDir(d) => {
                self.settings.export_dir = Some(d);
                self.mark_dirty();
                self.sync_export();
            }
            Picked::Open(files) => self.open_paths(files),
            Picked::SaveTab(path) => self.save_tab_to(path),
            Picked::Nothing(Some(e)) => self.toast(&e, true),
            Picked::Nothing(None) => {}
        }
    }

    /// Opens files picked or dropped: the first plays as if opened from
    /// outside; any others join the playlist.
    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let mut paths = paths.into_iter();
        let Some(first) = paths.next() else { return };
        self.open_external(&first);
        let rest: Vec<PathBuf> = paths.collect();
        if rest.is_empty() {
            return;
        }
        let opts = self.decode_options();
        let mut tracks: Vec<TrackRef> = Vec::new();
        for p in rest {
            let Ok(node) = vfs::resolve(&p) else { continue };
            match node {
                vfs::Node::Tracks(loc) => tracks.push(TrackRef::file(loc)),
                n => {
                    if let Ok(list) = vfs::list(&n, false, &opts) {
                        tracks.extend(
                            list.into_iter()
                                .filter(|e| {
                                    e.kind == EntryKind::Audio || e.kind == EntryKind::Track
                                })
                                .filter_map(|e| e.track),
                        );
                    }
                }
            }
        }
        // Tracks the first one already queued are left where they are.
        tracks.retain(|t| !self.queue.items.iter().any(|i| &i.track == t));
        if tracks.is_empty() {
            return;
        }
        let n = tracks.len();
        let start = self.queue.len();
        self.queue.append(tracks);
        self.rebuild_rows();
        self.load_meta_from(start);
        self.toast(
            &format!("Also added {n} {}", if n == 1 { "track" } else { "tracks" }),
            false,
        );
    }

    // ---------------------------------------------------------- properties

    pub(super) fn show_info(&mut self, i: i32) {
        let Some(item) = self.queue.items.get(i.max(0) as usize) else {
            return;
        };
        let t = item.track.clone();
        let m = item
            .meta
            .clone()
            .unwrap_or_else(|| meta::read(&t, &self.decode_options()));
        let fields = info_fields(&t, &m);
        let all: String = fields
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        let model: Vec<InfoField> = fields
            .into_iter()
            .map(|(label, value)| InfoField {
                label: ss(label),
                value: ss(value),
            })
            .collect();
        let ui = self.ui();
        let info = ui.global::<Info>();
        info.set_title(ss(if m.title.is_empty() {
            t.loc.name()
        } else {
            m.title.clone()
        }));
        info.set_fields(ModelRc::new(VecModel::from(model)));
        info.set_all_text(ss(all));
        info.set_shown(true);
    }

    pub(super) fn close_info(&mut self) {
        let ui = self.ui();
        ui.global::<Info>().set_shown(false);
        ui.invoke_focus_main();
    }

    // -------------------------------------------------------------- export

    /// Opens the export dialog for one playlist row, or all (-1).
    pub(super) fn open_export(&mut self, row: i32) {
        if self.export.cancel.is_some() {
            self.ui().global::<Export>().set_shown(true);
            return;
        }
        let rows: Vec<usize> = if row < 0 {
            (0..self.queue.len()).collect()
        } else if (row as usize) < self.queue.len() {
            vec![row as usize]
        } else {
            Vec::new()
        };
        if rows.is_empty() {
            self.toast("The playlist is empty", true);
            return;
        }
        let what = if rows.len() == 1 {
            let item = &self.queue.items[rows[0]];
            format!(
                "“{}”",
                item.meta
                    .as_ref()
                    .map(|m| m.title.clone())
                    .unwrap_or_else(|| item.track.loc.name())
            )
        } else {
            format!("{} tracks", rows.len())
        };
        self.export.rows = rows;
        let ui = self.ui();
        let e = ui.global::<Export>();
        e.set_what(ss(what));
        e.set_status(SharedString::new());
        e.set_progress(0.0);
        self.sync_export();
        e.set_shown(true);
    }

    /// How many of the rows to export can be MIDI, of how many.
    fn midi_count(&self) -> (usize, usize) {
        let rows = &self.export.rows;
        let ok = rows
            .iter()
            .filter_map(|&i| self.queue.items.get(i))
            .filter(|item| export::midi_possible(&item.track))
            .count();
        (ok, rows.len())
    }

    /// The format to export to: the one chosen, unless that is the notes
    /// and none of the tracks' notes can be found.
    fn export_format(&self) -> Format {
        match Format::from_index(self.settings.export_format) {
            f if f.is_notes() && self.midi_count().0 == 0 => Format::Flac,
            f => f,
        }
    }

    pub(super) fn sync_export(&self) {
        let ui = self.ui();
        let e = ui.global::<Export>();
        let s = &self.settings;
        let (ok, total) = self.midi_count();
        e.set_midi_ok(ok > 0);
        e.set_midi_note(ss(if ok > 0 && ok < total {
            format!("Only {ok} of the {total} tracks can be: the others are left out.")
        } else {
            String::new()
        }));
        let format = self.export_format();
        e.set_format(format.index() as i32);
        e.set_format_detail(ss(format.detail()));
        e.set_bitrate(match format {
            Format::Opus => s.export_opus_kbps,
            _ => s.export_mp3_kbps,
        } as i32);
        e.set_folder(ss(s
            .export_dir
            .as_deref()
            .map(super::tilde)
            .unwrap_or_default()));
    }

    pub(super) fn set_export_format(&mut self, i: i32) {
        if Format::from_index(i.max(0) as u32).is_notes() && self.midi_count().0 == 0 {
            return;
        }
        self.settings.export_format = i.clamp(0, 6) as u32;
        self.mark_dirty();
        self.sync_export();
    }

    pub(super) fn step_export_bitrate(&mut self, d: i32) {
        const MP3: &[u32] = &[96, 128, 160, 192, 224, 256, 320];
        const OPUS: &[u32] = &[64, 96, 128, 160, 192, 256];
        let (steps, v) = match Format::from_index(self.settings.export_format) {
            Format::Opus => (OPUS, &mut self.settings.export_opus_kbps),
            _ => (MP3, &mut self.settings.export_mp3_kbps),
        };
        let pos = steps
            .iter()
            .position(|&s| s >= *v)
            .unwrap_or(steps.len() - 1) as i32;
        *v = steps[(pos + d).clamp(0, steps.len() as i32 - 1) as usize];
        self.mark_dirty();
        self.sync_export();
    }

    pub(super) fn start_export(&mut self) {
        if self.export.cancel.is_some() {
            return;
        }
        let Some(folder) = self.settings.export_dir.clone() else {
            self.toast("Choose a folder first", true);
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&folder) {
            self.toast(&format!("Cannot use {}: {e}", folder.display()), true);
            return;
        }
        let opts = self.decode_options();
        let format = self.export_format();
        let tracks: Vec<(TrackRef, TrackMeta)> = self
            .export
            .rows
            .iter()
            .filter_map(|&i| self.queue.items.get(i))
            .filter(|item| !format.is_notes() || export::midi_possible(&item.track))
            .map(|item| {
                let m = item
                    .meta
                    .clone()
                    .unwrap_or_else(|| meta::read(&item.track, &opts));
                (item.track.clone(), m)
            })
            .collect();
        if tracks.is_empty() {
            return;
        }
        // The WonderSwan core plays one song at a time.
        let is_wsr = |t: &TrackRef| t.loc.extension() == "wsr";
        if tracks.iter().any(|(t, _)| is_wsr(t)) && self.current_track().is_some_and(is_wsr) {
            self.stop();
            self.toast(
                "Stopped playback: WonderSwan songs export one at a time",
                false,
            );
        }
        let kbps = match format {
            Format::Opus => self.settings.export_opus_kbps,
            _ => self.settings.export_mp3_kbps,
        };
        let shown = self.tabs.view.shown_for_export(&tracks, format);
        let cancel = Arc::new(AtomicBool::new(false));
        self.export.cancel = Some(cancel.clone());
        self.export.total = tracks.len();
        self.export.index = 0;
        export::spawn(
            export::Job {
                tracks,
                folder,
                format,
                kbps,
                opts,
                shown,
            },
            cancel,
            self.export_tx.clone(),
        );
        let ui = self.ui();
        let e = ui.global::<Export>();
        e.set_running(true);
        e.set_progress(0.0);
        e.set_status(ss("Starting…"));
    }

    pub(super) fn cancel_export(&mut self) {
        if let Some(c) = &self.export.cancel {
            c.store(true, Ordering::Relaxed);
            self.ui().global::<Export>().set_status(ss("Cancelling…"));
        }
    }

    pub(super) fn close_export(&mut self) {
        if self.export.cancel.is_some() {
            return;
        }
        let ui = self.ui();
        ui.global::<Export>().set_shown(false);
        ui.invoke_focus_main();
    }

    pub(super) fn export_progress(&mut self, p: Progress) {
        let ui = self.ui();
        let e = ui.global::<Export>();
        match p {
            Progress::Track { index, total, name } => {
                self.export.index = index;
                self.export.total = total;
                self.export.name = name;
                e.set_status(ss(format!(
                    "{} of {}: {}",
                    index + 1,
                    total,
                    self.export.name
                )));
                e.set_progress(index as f32 / total.max(1) as f32);
            }
            Progress::Fraction(f) => {
                let total = self.export.total.max(1) as f32;
                e.set_progress((self.export.index as f32 + f) / total);
            }
            Progress::Finished {
                written,
                failed,
                cancelled,
            } => {
                self.export.cancel = None;
                e.set_running(false);
                e.set_progress(if cancelled { 0.0 } else { 1.0 });
                let files = |n: usize| if n == 1 { "file" } else { "files" };
                let msg = if cancelled {
                    format!("Cancelled after {written} {}", files(written))
                } else if failed.is_empty() {
                    format!("Exported {written} {}", files(written))
                } else {
                    format!(
                        "Exported {written} {}; {} failed ({})",
                        files(written),
                        failed.len(),
                        failed[0]
                    )
                };
                e.set_status(ss(&msg));
                let shown = e.get_shown();
                if !shown {
                    self.toast(&msg, !failed.is_empty());
                }
            }
        }
    }
}

impl App {
    // ---------------------------------------------------- lyrics and tabs

    /// The playing (or chosen) track, its title and its length.
    fn sheet_track(&self) -> Option<(TrackRef, String, Option<u64>)> {
        let i = self.queue.current?;
        let item = self.queue.items.get(i)?;
        let title = item
            .meta
            .as_ref()
            .map(|m| m.title.clone())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| item.track.loc.name());
        let length = item
            .meta
            .as_ref()
            .and_then(|m| m.length_ms)
            .or(self.duration_ms);
        Some((item.track.clone(), title, length))
    }

    pub(super) fn show_lyrics(&mut self) {
        let ui = self.ui();
        let l = ui.global::<Lyrics>();
        if l.get_shown() {
            return self.close_lyrics();
        }
        if self.sheet_track().is_none() {
            self.toast("Play something first", true);
            return;
        }
        if self.tabs.shown {
            self.close_tabs();
        }
        self.load_lyrics();
        l.set_shown(true);
    }

    fn load_lyrics(&mut self) {
        let Some((track, title, _)) = self.sheet_track() else {
            return;
        };
        let found = lyrics::find(&track);
        let ui = self.ui();
        let l = ui.global::<Lyrics>();
        l.set_title(ss(format!("Lyrics: {title}")));
        match &found {
            Some(ly) => {
                let lines: Vec<SharedString> = ly.lines.iter().map(|x| ss(&x.text)).collect();
                l.set_lines(ModelRc::new(VecModel::from(lines)));
                let times: Vec<SharedString> = ly
                    .lines
                    .iter()
                    .map(|x| {
                        x.ms.map_or_else(SharedString::new, |ms| ss(lyrics::stamp(ms)))
                    })
                    .collect();
                l.set_times(ModelRc::new(VecModel::from(times)));
                l.set_synced(ly.synced);
                l.set_source(ss(format!(
                    "From {}{}",
                    ly.source,
                    if ly.synced { " · timed" } else { "" }
                )));
                l.set_all_text(ss(ly.text()));
                l.set_message(SharedString::new());
            }
            None => {
                l.set_lines(ModelRc::default());
                l.set_times(ModelRc::default());
                l.set_synced(false);
                l.set_source(SharedString::new());
                l.set_all_text(SharedString::new());
                let name = track.loc.name();
                let (stem, _) = chippy_core::formats::split_name(&name);
                l.set_message(ss(format!(
                    "No lyrics found for this track.\n\nPut them in a file beside it, \
                     “{stem}.lrc” (timed, so they follow the song) or “{stem}.txt”; \
                     lyrics in the file's own tags are found too."
                )));
            }
        }
        l.set_current(-1);
        self.lyrics = LyricsState {
            track: Some(track),
            lyrics: found,
            line: -1,
        };
    }

    pub(super) fn close_lyrics(&mut self) {
        self.lyrics = LyricsState::default();
        let ui = self.ui();
        ui.global::<Lyrics>().set_shown(false);
        ui.invoke_focus_main();
    }

    pub(super) fn seek_lyrics_line(&mut self, i: i32) {
        let ms = self
            .lyrics
            .lyrics
            .as_ref()
            .and_then(|l| l.lines.get(i.max(0) as usize))
            .and_then(|l| l.ms);
        if let Some(ms) = ms {
            self.seek(ms);
        }
    }

    pub(super) fn show_tabs(&mut self) {
        if self.tabs.shown {
            return self.close_tabs();
        }
        if self.sheet_track().is_none() {
            self.toast("Play something first", true);
            return;
        }
        if self.lyrics.track.is_some() {
            self.close_lyrics();
        }
        self.tabs.shown = true;
        self.load_tabs();
        self.ui().global::<Tabs>().set_shown(true);
    }

    fn load_tabs(&mut self) {
        let Some((track, title, length)) = self.sheet_track() else {
            return;
        };
        self.tabs.view.load(&track, length);
        self.tabs.line = -1;
        self.ui()
            .global::<Tabs>()
            .set_title(ss(format!("Guitar tab: {title}")));
        self.sync_tabs();
    }

    /// Puts the tab away; what was worked out is kept, to come back to.
    pub(super) fn close_tabs(&mut self) {
        self.tabs.shown = false;
        let ui = self.ui();
        ui.global::<Tabs>().set_shown(false);
        ui.invoke_focus_main();
    }

    pub(super) fn step_tab_part(&mut self, d: i32) {
        self.keeping_tab_view(|v| v.step_part(d));
    }

    pub(super) fn set_tab_part(&mut self, i: i32) {
        self.keeping_tab_view(|v| v.set_part(i.max(0) as usize));
    }

    /// Item `i` of the merge menu: the shown part merged with another, or
    /// taken apart.
    pub(super) fn merge_tab_part(&mut self, i: i32) {
        let Some(&what) = self.tabs.merges.get(i.max(0) as usize) else {
            return;
        };
        self.keeping_tab_view(|v| match what {
            Some(other) => v.merge_with(other),
            None => v.unmerge(),
        });
    }

    pub(super) fn set_tab_tuning(&mut self, i: i32) {
        self.keeping_tab_view(|v| v.set_tuning(i.max(0) as usize));
    }

    /// Changes the tab with `f` (another part, instrument...), keeping the
    /// same bars in view rather than jumping to the playhead.
    fn keeping_tab_view(&mut self, f: impl FnOnce(&mut TabView)) {
        let top = self.ui().global::<Tabs>().get_top_line();
        let seen = self.tabs.view.in_view(top);
        f(&mut self.tabs.view);
        self.sync_tabs();
        let Some(seen) = seen else { return };
        let to = self.tabs.view.keep_in_view(seen);
        let (line, x) = self
            .tabs
            .view
            .position(self.tabs.pos_ms)
            .map_or((-1, 0.0), |(i, x)| (i as i32, x));
        self.tabs.line = line;
        self.tabs.playhead = x;
        let ui = self.ui();
        let t = ui.global::<Tabs>();
        t.set_current(line);
        t.set_playhead(x);
        t.set_keep_line(to);
        t.set_keep((t.get_keep() + 1) & 0x3FFF);
    }

    pub(super) fn shift_tab_octave(&mut self, d: i32) {
        self.tabs.view.shift_octave(d);
        self.sync_tabs();
    }

    pub(super) fn nudge_tab(&mut self, d: i32) {
        self.tabs.view.nudge(d);
        self.sync_tabs();
    }

    pub(super) fn reset_tab_adjust(&mut self) {
        self.tabs.view.reset_adjust();
        self.sync_tabs();
    }

    pub(super) fn set_tab_width(&mut self, digits: i32) {
        if self.tabs.view.set_width(digits as f32) {
            self.tabs.line = -1;
            self.sync_tabs();
        }
    }

    pub(super) fn select_tab_bar(&mut self, i: i32, extend: bool) {
        self.tabs.view.select_bar(i.max(0) as usize, extend);
        self.sync_tabs();
    }

    pub(super) fn seek_tab_bar(&mut self, i: i32) {
        if let Some(ms) = self.tabs.view.bar_start(i.max(0) as usize) {
            self.seek(ms);
        }
    }

    pub(super) fn set_tab_sig(&mut self, i: i32) {
        self.tabs.view.set_sig(i.max(0) as usize);
        self.tabs.line = -1;
        self.sync_tabs();
    }

    pub(super) fn step_tab_speed(&mut self, d: i32) {
        self.tabs.view.step_speed(d);
        self.tabs.line = -1;
        self.sync_tabs();
    }

    pub(super) fn shift_tab_bars(&mut self, d: i32) {
        self.tabs.view.shift_bars(d);
        self.tabs.line = -1;
        self.sync_tabs();
    }

    pub(super) fn reset_tab_meter(&mut self) {
        self.tabs.view.reset_meter();
        self.tabs.line = -1;
        self.sync_tabs();
    }

    /// Asks where to save the tab (MusicXML or Guitar Pro).
    pub(super) fn save_tab(&mut self) {
        if self.picker_open || self.tabs.view.meter().is_none() {
            return;
        }
        self.save_tab_desktop();
    }

    #[cfg(not(target_os = "android"))]
    fn save_tab_desktop(&mut self) {
        self.picker_open = true;
        let title = self.tab_title();
        let start = self.settings.export_dir.clone().filter(|d| d.is_dir());
        spawn_picker(self.pick_tx.clone(), move || {
            let mut d = rfd::FileDialog::new()
                .set_title("Save the tab")
                .add_filter("MusicXML (sheet music and tab)", &["musicxml"])
                .add_filter("Guitar Pro 5", &["gp5"])
                .set_file_name(format!("{}.musicxml", export::safe_name(&title)));
            if let Some(s) = start {
                d = d.set_directory(s);
            }
            match d.save_file() {
                Some(p) => Picked::SaveTab(p),
                None => Picked::Nothing(None),
            }
        });
    }

    /// The title of the track in the tab.
    fn tab_title(&self) -> String {
        let t = self.ui().global::<Tabs>().get_title().to_string();
        t.trim_start_matches("Guitar tab: ").to_string()
    }

    fn save_tab_to(&mut self, mut path: PathBuf) {
        let format = match path.extension().and_then(|e| e.to_str()) {
            Some(e) if e.eq_ignore_ascii_case("gp5") => Format::GuitarPro,
            Some(e) if e.eq_ignore_ascii_case("musicxml") || e.eq_ignore_ascii_case("xml") => {
                Format::MusicXml
            }
            _ => {
                path.set_extension("musicxml");
                Format::MusicXml
            }
        };
        let Some(score) = self.tabs.view.score(&self.tab_title()) else {
            return;
        };
        match export::write_notes(&score, format, &path) {
            Ok(()) => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string());
                self.toast(&format!("Saved {}", name.unwrap_or_default()), false);
            }
            Err(e) => self.toast(&format!("Couldn't save the tab: {e:#}"), true),
        }
    }

    pub(super) fn seek_tab_line(&mut self, i: i32) {
        if let Some(l) = self.tabs.view.lines().get(i.max(0) as usize) {
            self.seek(l.start_ms as u64);
        }
    }

    /// Sends the whole tab view to the dialog.
    fn sync_tabs(&mut self) {
        let ui = self.ui();
        let t = ui.global::<Tabs>();
        let v = &self.tabs.view;
        t.set_busy(v.busy());
        t.set_message(ss(v.message()));
        let parts: Vec<SharedString> = v.part_names().iter().map(ss).collect();
        t.set_part_names(ModelRc::new(VecModel::from(parts)));
        t.set_part(v.part_index() as i32);
        let tunings: Vec<SharedString> = TabView::tuning_names().iter().map(ss).collect();
        t.set_tunings(ModelRc::new(VecModel::from(tunings)));
        t.set_tuning(v.tuning_index() as i32);
        let strings: Vec<SharedString> = v.strings().iter().map(ss).collect();
        t.set_strings(ModelRc::new(VecModel::from(strings)));
        let selected = v.selection();
        let nudged = v.nudged_bars();
        let lines: Vec<TabLine> = v
            .lines()
            .iter()
            .map(|l| TabLine {
                start: ss(&l.label),
                cells: ModelRc::new(VecModel::from(
                    l.cells
                        .iter()
                        .map(|&(x, string, fret, soft)| TabCell {
                            x,
                            string: string as i32,
                            fret: ss(if soft {
                                format!("({fret})")
                            } else {
                                fret.to_string()
                            }),
                            soft,
                        })
                        .collect::<Vec<_>>(),
                )),
                bars: ModelRc::new(VecModel::from(
                    l.segments
                        .iter()
                        .map(|&(x0, x1, i)| TabBar {
                            x0,
                            x1,
                            index: i as i32,
                            line: l.bars.iter().any(|b| b.1 == i),
                            label: ss((i + 1).to_string()),
                            selected: selected.is_some_and(|(a, b)| (a..=b).contains(&i)),
                            nudged: nudged.contains(&i),
                        })
                        .collect::<Vec<_>>(),
                )),
            })
            .collect();
        t.set_lines(ModelRc::new(VecModel::from(lines)));
        let mut merges: Vec<Option<usize>> = Vec::new();
        let mut items: Vec<SharedString> = Vec::new();
        for (k, name) in v.merge_choices() {
            merges.push(Some(k));
            items.push(ss(format!("With {name}")));
        }
        if v.is_merged() {
            merges.push(None);
            items.push(ss("Take this merge apart"));
        }
        t.set_merge_items(ModelRc::new(VecModel::from(items)));
        t.set_detail(ss(v.detail()));
        let adjust = v.adjust();
        t.set_octave(adjust.octave);
        t.set_position(v.steps());
        t.set_adjusted(v.adjusted());
        t.set_selection(ss(match selected {
            Some((a, b)) if a == b => format!("Bar {}", a + 1),
            Some((a, b)) => format!("Bars {}–{}", a + 1, b + 1),
            None => String::new(),
        }));
        t.set_has_bars(v.meter().is_some());
        t.set_meter_text(ss(v.meter_text()));
        let sigs: Vec<SharedString> = TabView::sig_names().iter().map(ss).collect();
        t.set_sigs(ModelRc::new(VecModel::from(sigs)));
        t.set_sig(v.sig_index() as i32);
        t.set_speed(v.speed());
        t.set_meter_adjusted(v.meter_adjusted());
        t.set_can_up(v.can_nudge(true));
        t.set_can_down(v.can_nudge(false));
        let title = t.get_title().to_string();
        t.set_all_text(ss(v.text(title.trim_start_matches("Guitar tab: "))));
        t.set_current(-1);
        self.tabs.line = -1;
        self.tabs.merges = merges;
    }

    // ---------------------------------------------------------- MIDI mode

    /// Puts the saved MIDI mode in place, and the styles on the button.
    pub(super) fn init_midi_mode(&mut self) {
        let style = Style::from_id(&self.settings.midi_style).unwrap_or(Style::Original);
        midimode::set(self.settings.midi_mode.then_some(style));
        let ui = self.ui();
        let n = ui.global::<Now>();
        let names: Vec<SharedString> = Style::ALL.iter().map(|s| ss(s.name())).collect();
        n.set_midi_styles(ModelRc::new(VecModel::from(names)));
        n.set_midi_style(style.index() as i32);
        n.set_midi_mode(self.settings.midi_mode);
    }

    pub(super) fn toggle_midi_mode(&mut self) {
        self.settings.midi_mode = !self.settings.midi_mode;
        self.apply_midi_mode();
    }

    pub(super) fn set_midi_style(&mut self, i: i32) {
        let style = Style::from_index(i.max(0) as usize).unwrap_or(Style::Original);
        self.settings.midi_style = style.id().into();
        self.settings.midi_mode = true;
        self.apply_midi_mode();
    }

    fn apply_midi_mode(&mut self) {
        let style = Style::from_id(&self.settings.midi_style).unwrap_or(Style::Original);
        midimode::set(self.settings.midi_mode.then_some(style));
        let ui = self.ui();
        let n = ui.global::<Now>();
        n.set_midi_mode(self.settings.midi_mode);
        n.set_midi_style(style.index() as i32);
        self.save_settings();
        if self.settings.midi_mode && self.soundfont_index().is_none() {
            self.toast(
                "MIDI mode needs a SoundFont: choose a folder of SF2 files in Settings, under MIDI",
                true,
            );
        }
    }

    /// Shows what MIDI mode is doing with the playing track.
    fn midi_mode_tick(&mut self) {
        let status = midimode::status();
        if self.tabs.midi_status == Some(status) {
            return;
        }
        self.tabs.midi_status = Some(status);
        self.ui().global::<Now>().set_midi_state(match status {
            midimode::Status::Off => 0,
            midimode::Status::Unsupported => 1,
            midimode::Status::Preparing => 2,
            midimode::Status::Playing => 3,
            midimode::Status::Failed => 4,
        });
        if status == midimode::Status::Failed {
            let e = midimode::error();
            self.toast(&format!("Can't play this track as MIDI: {e}"), true);
        }
    }

    /// Follows playback in the open lyrics and tab, and follows the
    /// playlist to the next track.
    pub(super) fn sheets_tick(&mut self, pos_ms: u64) {
        self.midi_mode_tick();
        let playing = self.current_track().cloned();
        // Whether there are lyrics and notes, for the buttons.
        let a = &mut self.tabs.avail;
        if a.check(playing.as_ref()) | a.poll() || self.tabs.view.failed() {
            let (lyrics, notes) = (a.lyrics(), a.notes(&self.tabs.view));
            let ui = self.ui();
            let n = ui.global::<Now>();
            if n.get_has_lyrics() != lyrics {
                n.set_has_lyrics(lyrics);
            }
            if n.get_has_notes() != notes {
                n.set_has_notes(notes);
            }
        }
        if self.lyrics.track.is_some() {
            if playing.is_some() && playing != self.lyrics.track {
                self.load_lyrics();
            }
            let line = self
                .lyrics
                .lyrics
                .as_ref()
                .and_then(|l| l.line_at(pos_ms))
                .map_or(-1, |i| i as i32);
            if line != self.lyrics.line {
                self.lyrics.line = line;
                self.ui().global::<Lyrics>().set_current(line);
            }
        }
        self.tabs.pos_ms = pos_ms;
        if self.tabs.shown {
            if playing.is_some() && playing != self.tabs.view.track {
                self.load_tabs();
            }
            if self.tabs.view.poll() {
                self.sync_tabs();
            }
            let (line, x) = self
                .tabs
                .view
                .position(pos_ms)
                .map_or((-1, 0.0), |(i, x)| (i as i32, x));
            let ui = self.ui();
            let t = ui.global::<Tabs>();
            if line != self.tabs.line {
                self.tabs.line = line;
                t.set_current(line);
            }
            if (x - self.tabs.playhead).abs() > 0.0005 {
                self.tabs.playhead = x;
                t.set_playhead(x);
            }
        }
    }
}

/// A track's properties, as (label, value) pairs; empty values left out.
fn info_fields(t: &TrackRef, m: &TrackMeta) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = Vec::new();
    let mut add = |k: &str, v: String| {
        if !v.trim().is_empty() {
            f.push((k.to_string(), v));
        }
    };
    add("Title", m.title.clone());
    add("Artist", m.artist.clone());
    add("Album / game", m.album.clone());
    add(
        "Track",
        m.track_no.map(|n| n.to_string()).unwrap_or_default(),
    );
    add("Length", m.length_ms.map(format_ms).unwrap_or_default());
    add("Year", m.year.clone());
    add("Genre", m.genre.clone());
    add("System", m.system.clone());
    add("Copyright", m.copyright.clone());
    add("Ripped by", m.dumper.clone());
    add("Comment", m.comment.clone());
    add("Format", m.format.clone());
    add(
        "Sample rate",
        m.sample_rate.map(|r| format!("{r} Hz")).unwrap_or_default(),
    );
    add(
        "Channels",
        m.channels.map(|c| c.to_string()).unwrap_or_default(),
    );
    add(
        "Bitrate",
        m.bitrate_kbps
            .map(|b| format!("{b} kbps"))
            .unwrap_or_default(),
    );
    add(
        "Track in file",
        t.sub.map(|s| format!("{}", s + 1)).unwrap_or_default(),
    );
    add("File name", t.loc.name());
    add("Location", t.loc.vpath().to_string_lossy().into_owned());
    if let Some(size) = file_size(t) {
        add("File size", size);
    }
    // Every tag of PSF-family files (PSF, PSF2, USF, GSF, 2SF, SSF...),
    // including the ones not shown above.
    if let Ok(data) = t.loc.read() {
        if let Ok(p) = chippy_core::psf::parse(&data) {
            for (k, v) in &p.tags {
                add(&format!("Tag: {k}"), v.clone());
            }
        }
    }
    f
}

fn file_size(t: &TrackRef) -> Option<String> {
    let bytes = match &t.loc {
        vfs::Location::Fs(p) => std::fs::metadata(p).ok()?.len(),
        vfs::Location::Archive { .. } => t.loc.read().ok()?.len() as u64,
    };
    Some(human_size(bytes))
}

fn human_size(b: u64) -> String {
    const UNITS: &[&str] = &["bytes", "KB", "MB", "GB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} bytes")
    } else {
        format!("{v:.1} {} ({b} bytes)", UNITS[u])
    }
}
