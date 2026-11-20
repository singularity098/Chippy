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
use crate::{Export, Info, InfoField};

/// What a file picker running on its own thread came back with.
#[cfg_attr(target_os = "android", allow(dead_code))]
pub enum Picked {
    MusicDir(PathBuf),
    SoundfontDir(PathBuf),
    ExportDir(PathBuf),
    Open(Vec<PathBuf>),
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

    pub(super) fn sync_export(&self) {
        let ui = self.ui();
        let e = ui.global::<Export>();
        let s = &self.settings;
        let format = Format::from_index(s.export_format);
        e.set_format(match format {
            Format::Wav => 0,
            Format::Flac => 1,
            Format::Mp3 => 2,
            Format::Opus => 3,
        });
        e.set_format_detail(ss(match format {
            Format::Wav => "Uncompressed, 16-bit.",
            Format::Flac => "Lossless and compressed, 16-bit.",
            Format::Mp3 => "Lossy; plays everywhere.",
            Format::Opus => "Lossy; best quality for its size.",
        }));
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
        self.settings.export_format = i.clamp(0, 3) as u32;
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
        let tracks: Vec<(TrackRef, TrackMeta)> = self
            .export
            .rows
            .iter()
            .filter_map(|&i| self.queue.items.get(i))
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
        let format = Format::from_index(self.settings.export_format);
        let kbps = match format {
            Format::Opus => self.settings.export_opus_kbps,
            _ => self.settings.export_mp3_kbps,
        };
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
