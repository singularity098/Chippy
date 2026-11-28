//! CD images: BIN/CUE (and CUE sheets over a WAV, FLAC, MP3... album rip),
//! MAME's CHD, Dreamcast GDI and CloneCD's CCD/IMG. Their audio tracks are
//! Red Book audio: 16-bit stereo PCM at 44.1 kHz, 2352 bytes (588 frames)
//! a sector, 75 sectors a second. Data tracks are left out of the list.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;

use super::sym::SymDecoder;
use super::{Decoder, SubTrack};
use crate::archive;
use crate::formats::{self, Codec, FileKind};
use crate::meta::TrackMeta;
use crate::vfs::Location;

/// The extensions of the CD images and sheets read here.
pub const EXTENSIONS: &[&str] = &["cue", "chd", "gdi", "ccd"];

/// Bytes of audio in a sector.
const SECTOR: u64 = 2352;
/// Stereo frames in a sector.
const SECTOR_FRAMES: u64 = SECTOR / 4;
const SECTORS_PER_SECOND: u64 = 75;
const RATE: u32 = 44_100;
/// A CHD stores each sector with its 96 bytes of subcode.
const CHD_FRAME: u64 = SECTOR + 96;
/// CHD tracks start on a multiple of this many frames.
const CHD_TRACK_PADDING: u64 = 4;

/// A CD image, as its sheet or metadata describes it.
#[derive(Debug, Clone, Default)]
pub struct Disc {
    pub title: String,
    pub performer: String,
    pub genre: String,
    pub date: String,
    /// "BIN/CUE", "CHD"...
    pub format: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone)]
pub struct Track {
    /// The track's number on the disc (1 for the first).
    pub number: u32,
    pub audio: bool,
    pub title: String,
    pub performer: String,
    pub source: Source,
    /// How many sectors it plays, when known (a track that runs to the end
    /// of a compressed file plays until the file ends).
    pub sectors: Option<u64>,
}

impl Track {
    fn length_ms(&self) -> Option<u64> {
        self.sectors.map(|s| s * 1000 / SECTORS_PER_SECOND)
    }
}

#[derive(Debug, Clone)]
pub enum Source {
    /// Raw sectors in a file: the track's first one is at byte `offset`,
    /// each `stride` bytes on (2352, or 2448 with subcode) with the audio
    /// at its start, little-endian unless `big_endian`.
    Raw {
        file: Location,
        offset: u64,
        stride: u64,
        big_endian: bool,
    },
    /// A file Symphonia decodes (a CUE's WAVE, FLAC, MP3...), from sector
    /// `start` of it.
    Encoded { file: Location, start: u64 },
    /// Frames of the CHD itself, from frame `frame`.
    Chd { frame: u64 },
}

// ------------------------------------------------------------------ files

/// A file's size, without reading it when it is on disk.
fn file_len(loc: &Location) -> Result<u64> {
    match loc {
        Location::Fs(p) => Ok(std::fs::metadata(p)?.len()),
        Location::Archive { archive, entry } => archive::list(archive)?
            .iter()
            .find(|e| &e.path == entry)
            .map(|e| e.size)
            .ok_or_else(|| anyhow!("{entry} not found")),
    }
}

/// Shared bytes that read and seek like a file.
struct Shared(Arc<Vec<u8>>);
impl AsRef<[u8]> for Shared {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// A file to read from anywhere in: on disk it is read as needed (an image
/// can be most of a gigabyte), in an archive it is unpacked into memory.
fn open_file(loc: &Location) -> Result<Box<dyn ReadSeek>> {
    Ok(match loc {
        Location::Fs(p) => Box::new(std::io::BufReader::new(
            std::fs::File::open(p).with_context(|| format!("can't open {}", p.display()))?,
        )),
        Location::Archive { .. } => Box::new(Cursor::new(Shared(loc.read()?))),
    })
}

/// A file's name without its extension.
fn stem_of(loc: &Location) -> String {
    formats::split_name(&loc.name()).0.to_string()
}

/// The file a sheet names, beside it. A sheet naming a file that was
/// renamed with the sheet (`Game.cue` naming `Game (Europe).bin`) gets the
/// file with the sheet's name and the named file's extension, if that is
/// all there is.
fn find_file(sheet: &Location, name: &str, only_one: bool) -> Result<Location> {
    if let Some(l) = sheet.sibling(name) {
        return Ok(l);
    }
    if only_one {
        let stem = stem_of(sheet);
        let named = name.replace('\\', "/");
        let named = named.rsplit('/').next().unwrap_or(&named);
        let (_, ext) = formats::split_name(named);
        if let Some(l) = sheet.sibling(&format!("{stem}{ext}")) {
            return Ok(l);
        }
    }
    bail!("can't find {name}, which {} needs", sheet.name())
}

/// The sheet's text, whatever its encoding (UTF-8, with or without a BOM,
/// or Latin-1 from older rippers).
fn text_of(loc: &Location) -> Result<String> {
    let bytes = loc.read()?;
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
    Ok(match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    })
}

/// Splits a sheet's line into words, a quoted string being one word.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            let w: String = chars.by_ref().take_while(|&c| c != '"').collect();
            out.push(w);
        } else {
            let mut w = String::new();
            while let Some(&c) = chars.peek().filter(|c| !c.is_whitespace()) {
                w.push(c);
                chars.next();
            }
            out.push(w);
        }
    }
    out
}

/// Whether Symphonia, not raw sectors, reads a file named in a sheet.
fn is_encoded(file: &Location) -> bool {
    matches!(
        formats::kind_of(&file.name()),
        FileKind::Audio(Codec::Common)
    )
}

/// The other files a sheet names (the BINs of a CUE, the tracks of a GDI,
/// the IMG and SUB of a CCD), which the file browser need not list beside
/// it. Album rips' FLACs and MP3s stay listed: they are music of their own.
pub fn companions(sheet: &Location) -> Vec<String> {
    let names = match sheet.extension().as_str() {
        "cue" => text_of(sheet)
            .map(|t| {
                t.lines()
                    .map(words)
                    .filter(|w| w.len() >= 2 && w[0].eq_ignore_ascii_case("FILE"))
                    .map(|w| w[1].clone())
                    .collect()
            })
            .unwrap_or_default(),
        "gdi" => text_of(sheet)
            .map(|t| {
                t.lines()
                    .skip(1)
                    .map(words)
                    .filter(|w| w.len() >= 5)
                    .map(|w| w[4].clone())
                    .collect()
            })
            .unwrap_or_default(),
        "ccd" => {
            let stem = stem_of(sheet);
            vec![format!("{stem}.img"), format!("{stem}.sub")]
        }
        _ => Vec::new(),
    };
    names
        .into_iter()
        .map(|n| {
            let n = n.replace('\\', "/");
            n.rsplit('/').next().unwrap_or(&n).to_string()
        })
        .filter(|n| !matches!(formats::kind_of(n), FileKind::Audio(Codec::Common)))
        .collect()
}

// -------------------------------------------------------------------- CUE

/// mm:ss:ff, in sectors.
fn msf(s: &str) -> Option<u64> {
    let mut it = s.split(':').map(|p| p.trim().parse::<u64>().ok());
    let (m, s, f) = (it.next()??, it.next()??, it.next()??);
    Some((m * 60 + s) * SECTORS_PER_SECOND + f)
}

/// The bytes each sector of a track of this type takes in a BIN.
fn cue_sector_size(kind: &str) -> u64 {
    match kind.to_ascii_uppercase().as_str() {
        "MODE1/2048" => 2048,
        "MODE2/2336" | "CDI/2336" => 2336,
        "CDG" => 2448,
        _ => SECTOR,
    }
}

struct CueTrack {
    number: u32,
    kind: String,
    index0: Option<u64>,
    index1: Option<u64>,
    title: String,
    performer: String,
}

impl CueTrack {
    fn audio(&self) -> bool {
        self.kind.eq_ignore_ascii_case("AUDIO") || self.kind.eq_ignore_ascii_case("CDG")
    }
    /// Where the track's sectors start in its file: its pregap, if the
    /// file holds one.
    fn first(&self) -> u64 {
        self.index0.or(self.index1).unwrap_or(0)
    }
    fn start(&self) -> u64 {
        self.index1.or(self.index0).unwrap_or(0)
    }
}

struct CueFile {
    name: String,
    kind: String,
    tracks: Vec<CueTrack>,
}

pub fn read_cue(loc: &Location) -> Result<Disc> {
    let mut disc = Disc {
        format: "BIN/CUE".into(),
        ..Default::default()
    };
    let mut files: Vec<CueFile> = Vec::new();
    for line in text_of(loc)?.lines() {
        let w = words(line);
        let Some(cmd) = w.first() else { continue };
        let arg = |i: usize| w.get(i).cloned().unwrap_or_default();
        let track = files.last_mut().and_then(|f| f.tracks.last_mut());
        match cmd.to_ascii_uppercase().as_str() {
            "FILE" => files.push(CueFile {
                name: arg(1),
                kind: arg(2),
                tracks: Vec::new(),
            }),
            "TRACK" => {
                let f = files
                    .last_mut()
                    .ok_or_else(|| anyhow!("a TRACK before any FILE"))?;
                f.tracks.push(CueTrack {
                    number: arg(1).parse().unwrap_or(f.tracks.len() as u32 + 1),
                    kind: arg(2),
                    index0: None,
                    index1: None,
                    title: String::new(),
                    performer: String::new(),
                });
            }
            "INDEX" => {
                if let (Some(t), Some(at)) = (track, msf(&arg(2))) {
                    match arg(1).parse::<u32>() {
                        Ok(0) => t.index0 = Some(at),
                        Ok(1) => t.index1 = Some(at),
                        _ => {}
                    }
                }
            }
            "TITLE" => match track {
                Some(t) => t.title = arg(1),
                None => disc.title = arg(1),
            },
            "PERFORMER" => match track {
                Some(t) => t.performer = arg(1),
                None => disc.performer = arg(1),
            },
            "REM" if track.is_none() => match arg(1).to_ascii_uppercase().as_str() {
                "GENRE" => disc.genre = w[2..].join(" "),
                "DATE" => disc.date = arg(2),
                _ => {}
            },
            _ => {}
        }
    }
    let only_one = files.len() == 1;
    for f in &files {
        if f.tracks.is_empty() {
            continue;
        }
        let file = find_file(loc, &f.name, only_one)?;
        if is_encoded(&file) {
            disc.format = format!("CUE ({})", file.extension().to_uppercase());
            read_encoded(&mut disc, f, file);
        } else {
            read_raw(&mut disc, f, file)?;
        }
    }
    if disc.tracks.is_empty() {
        bail!("no tracks in {}", loc.name());
    }
    Ok(disc)
}

/// The tracks of a BIN: raw sectors, a track's regions each as big as its
/// type's sectors.
fn read_raw(disc: &mut Disc, f: &CueFile, file: Location) -> Result<()> {
    let len = file_len(&file)?;
    let big_endian = f.kind.eq_ignore_ascii_case("MOTOROLA");
    // Each track's first sector and the byte it starts at.
    let mut bases = Vec::with_capacity(f.tracks.len());
    let mut base = f.tracks[0].first() * cue_sector_size(&f.tracks[0].kind);
    for (i, t) in f.tracks.iter().enumerate() {
        if i > 0 {
            let prev = &f.tracks[i - 1];
            base += t.first().saturating_sub(prev.first()) * cue_sector_size(&prev.kind);
        }
        bases.push(base);
    }
    for (i, t) in f.tracks.iter().enumerate() {
        let size = cue_sector_size(&t.kind);
        let offset = bases[i] + t.start().saturating_sub(t.first()) * size;
        // A track runs to the next one's pregap, which a CHD of the same
        // disc keeps with the next track (on game discs it is two seconds
        // of silence).
        let end = bases.get(i + 1).copied().unwrap_or(len);
        disc.tracks.push(Track {
            number: t.number,
            audio: t.audio(),
            title: t.title.clone(),
            performer: t.performer.clone(),
            source: Source::Raw {
                file: file.clone(),
                offset,
                stride: size,
                big_endian,
            },
            sectors: Some(end.saturating_sub(offset) / size),
        });
    }
    Ok(())
}

/// The tracks of an album rip: stretches of one compressed file.
fn read_encoded(disc: &mut Disc, f: &CueFile, file: Location) {
    let mut whole = None;
    for (i, t) in f.tracks.iter().enumerate() {
        let sectors = match f.tracks.get(i + 1) {
            Some(n) => Some(n.start().saturating_sub(t.start())),
            None => {
                // The last track runs to the end of the file.
                let total = *whole.get_or_insert_with(|| {
                    SymDecoder::open(&file)
                        .ok()
                        .and_then(|d| d.duration_ms())
                        .map(|ms| ms * SECTORS_PER_SECOND / 1000)
                });
                total.map(|n: u64| n.saturating_sub(t.start()))
            }
        };
        disc.tracks.push(Track {
            number: t.number,
            audio: t.audio(),
            title: t.title.clone(),
            performer: t.performer.clone(),
            source: Source::Encoded {
                file: file.clone(),
                start: t.start(),
            },
            sectors,
        });
    }
}

// -------------------------------------------------------------------- GDI

/// A Dreamcast GD-ROM's sheet: the number of tracks, then a line for each:
/// number, first sector, type (0 audio, 4 data), sector size, file, offset.
pub fn read_gdi(loc: &Location) -> Result<Disc> {
    let mut disc = Disc {
        format: "GDI".into(),
        ..Default::default()
    };
    for line in text_of(loc)?.lines().skip(1) {
        let w = words(line);
        if w.len() < 5 {
            continue;
        }
        let num = |i: usize| w.get(i).and_then(|s| s.parse::<u64>().ok());
        let (Some(number), Some(kind), Some(size)) = (num(0), num(2), num(3)) else {
            continue;
        };
        let size = size.max(1);
        let file = find_file(loc, &w[4], false)?;
        let offset = num(5).unwrap_or(0);
        let len = file_len(&file)?;
        disc.tracks.push(Track {
            number: number as u32,
            audio: kind == 0,
            title: String::new(),
            performer: String::new(),
            source: Source::Raw {
                file,
                offset,
                stride: size,
                big_endian: false,
            },
            sectors: Some(len.saturating_sub(offset) / size),
        });
    }
    if disc.tracks.is_empty() {
        bail!("no tracks in {}", loc.name());
    }
    Ok(disc)
}

// -------------------------------------------------------------------- CCD

/// A CloneCD image: an INI-style CCD whose `[TRACK n]` sections give each
/// track's mode (0 for audio) and indexes, over an IMG of raw sectors with
/// the CCD's name.
pub fn read_ccd(loc: &Location) -> Result<Disc> {
    struct T {
        number: u32,
        mode: u32,
        index0: Option<u64>,
        index1: Option<u64>,
    }
    let mut tracks: Vec<T> = Vec::new();
    let mut leadout: Option<u64> = None;
    let mut section = String::new();
    let mut point: Option<u32> = None;
    for line in text_of(loc)?.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = s.trim().to_ascii_uppercase();
            point = None;
            if let Some(n) = section.strip_prefix("TRACK ") {
                tracks.push(T {
                    number: n.trim().parse().unwrap_or(tracks.len() as u32 + 1),
                    mode: 0,
                    index0: None,
                    index1: None,
                });
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_uppercase(), v.trim());
        let int = || {
            v.strip_prefix("0x")
                .or_else(|| v.strip_prefix("0X"))
                .map_or_else(
                    || v.parse::<i64>().ok(),
                    |h| i64::from_str_radix(h, 16).ok(),
                )
        };
        if section.starts_with("TRACK ") {
            let t = tracks.last_mut().unwrap();
            match k.as_str() {
                "MODE" => t.mode = int().unwrap_or(0) as u32,
                "INDEX 0" => t.index0 = int().map(|v| v.max(0) as u64),
                "INDEX 1" => t.index1 = int().map(|v| v.max(0) as u64),
                _ => {}
            }
        } else if section.starts_with("ENTRY ") {
            // The lead-out (point A2) says where the last track ends.
            match k.as_str() {
                "POINT" => point = int().map(|v| v as u32),
                "PLBA" if point == Some(0xa2) => leadout = int().map(|v| v.max(0) as u64),
                _ => {}
            }
        }
    }
    if tracks.is_empty() {
        bail!("no tracks in {}", loc.name());
    }
    let stem = stem_of(loc);
    let file = find_file(loc, &format!("{stem}.img"), false)?;
    let end = (file_len(&file)? / SECTOR).min(leadout.unwrap_or(u64::MAX));
    let mut disc = Disc {
        format: "CloneCD".into(),
        ..Default::default()
    };
    for (i, t) in tracks.iter().enumerate() {
        let start = t.index1.or(t.index0).unwrap_or(0);
        let stop = tracks
            .get(i + 1)
            .and_then(|n| n.index0.or(n.index1))
            .unwrap_or(end);
        disc.tracks.push(Track {
            number: t.number,
            audio: t.mode == 0,
            title: String::new(),
            performer: String::new(),
            source: Source::Raw {
                file: file.clone(),
                offset: start * SECTOR,
                stride: SECTOR,
                big_endian: false,
            },
            sectors: Some(stop.saturating_sub(start)),
        });
    }
    Ok(disc)
}

// -------------------------------------------------------------------- CHD

type Chd = chd::Chd<Box<dyn ReadSeek>>;

fn open_chd(loc: &Location) -> Result<Chd> {
    let chd = chd::Chd::open(open_file(loc)?, None).map_err(|e| anyhow!("{}: {e}", loc.name()))?;
    if chd.header().has_parent() {
        bail!(
            "{} is a diff of another CHD, which isn't supported",
            loc.name()
        );
    }
    if chd.header().unit_bytes() as u64 != CHD_FRAME {
        bail!("{} isn't a CD image", loc.name());
    }
    Ok(chd)
}

/// A CHD's tracks, from its CD (CHTR, CHT2) or GD-ROM (CHGD) metadata:
/// lines of `TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:12345 PREGAP:150
/// PGTYPE:VAUDIO ...`. Each track's frames start on a multiple of four,
/// and a pregap of type `V...` is stored at its start.
pub fn read_chd(loc: &Location) -> Result<Disc> {
    let mut chd = open_chd(loc)?;
    let metas: Vec<chd::metadata::Metadata> = chd
        .metadata_refs()
        .try_into()
        .map_err(|e| anyhow!("{}: {e}", loc.name()))?;
    let tag = |t: &[u8; 4]| u32::from_be_bytes(*t);
    let (cd, gd) = ([tag(b"CHTR"), tag(b"CHT2")], [tag(b"CHGD")]);
    let mut lines: Vec<(bool, String)> = metas
        .iter()
        .filter(|m| cd.contains(&m.metatag) || gd.contains(&m.metatag))
        .map(|m| {
            let s = String::from_utf8_lossy(&m.value);
            (
                gd.contains(&m.metatag),
                s.trim_end_matches('\0').to_string(),
            )
        })
        .collect();
    if lines.is_empty() {
        bail!("{} has no CD track list", loc.name());
    }
    let field = |s: &str, key: &str| -> Option<String> {
        s.split_whitespace()
            .find_map(|kv| kv.split_once(':').filter(|(k, _)| *k == key))
            .map(|(_, v)| v.to_string())
    };
    let num = |s: &str, key: &str| field(s, key).and_then(|v| v.parse::<u64>().ok());
    lines.sort_by_key(|(_, s)| num(s, "TRACK").unwrap_or(0));
    let mut disc = Disc {
        format: "CHD".into(),
        ..Default::default()
    };
    let mut frame = 0;
    for (gdrom, s) in &lines {
        let frames = num(s, "FRAMES").unwrap_or(0);
        let pregap = num(s, "PREGAP").unwrap_or(0);
        let stored_pregap = field(s, "PGTYPE").is_some_and(|t| t.starts_with('V'));
        let skip = if stored_pregap { pregap.min(frames) } else { 0 };
        let pad = match num(s, "PAD") {
            Some(p) if *gdrom => p,
            _ => frames.div_ceil(CHD_TRACK_PADDING) * CHD_TRACK_PADDING - frames,
        };
        disc.tracks.push(Track {
            number: num(s, "TRACK").unwrap_or(disc.tracks.len() as u64 + 1) as u32,
            audio: field(s, "TYPE").is_some_and(|t| t == "AUDIO"),
            title: String::new(),
            performer: String::new(),
            source: Source::Chd {
                frame: frame + skip,
            },
            sectors: Some(frames - skip),
        });
        frame += frames + pad;
    }
    Ok(disc)
}

// ------------------------------------------------------------------ discs

pub fn read_disc(loc: &Location) -> Result<Disc> {
    match loc.extension().as_str() {
        "cue" => read_cue(loc),
        "gdi" => read_gdi(loc),
        "ccd" => read_ccd(loc),
        "chd" => read_chd(loc),
        e => bail!("not a CD image: .{e}"),
    }
}

/// The track `sub` names (its number less one), or the first audio track.
fn pick(disc: &Disc, sub: Option<u32>) -> Result<&Track> {
    match sub {
        Some(s) => disc
            .tracks
            .iter()
            .find(|t| t.number == s + 1)
            .ok_or_else(|| anyhow!("no track {} on this disc", s + 1)),
        None => disc
            .tracks
            .iter()
            .find(|t| t.audio)
            .ok_or_else(|| anyhow!("no audio tracks on this disc")),
    }
    .and_then(|t| {
        if t.audio {
            Ok(t)
        } else {
            bail!("track {} is data, not audio", t.number)
        }
    })
}

/// The disc's audio tracks, numbered as on the disc.
pub fn track_list(loc: &Location) -> Result<Vec<SubTrack>> {
    let disc = read_disc(loc)?;
    let tracks: Vec<SubTrack> = disc
        .tracks
        .iter()
        .filter(|t| t.audio)
        .map(|t| SubTrack {
            index: t.number.saturating_sub(1),
            title: t.title.clone(),
            length_ms: t.length_ms(),
        })
        .collect();
    if tracks.is_empty() {
        bail!("{} has no audio tracks", loc.name());
    }
    Ok(tracks)
}

pub fn meta(loc: &Location, sub: Option<u32>) -> Result<TrackMeta> {
    let disc = read_disc(loc)?;
    let t = pick(&disc, sub)?;
    let encoded = matches!(t.source, Source::Encoded { .. });
    let album = if disc.title.is_empty() {
        stem_of(loc)
    } else {
        disc.title.clone()
    };
    Ok(TrackMeta {
        title: if t.title.is_empty() {
            format!("Track {:02}", t.number)
        } else {
            t.title.clone()
        },
        artist: if t.performer.is_empty() {
            disc.performer.clone()
        } else {
            t.performer.clone()
        },
        album,
        track_no: Some(t.number),
        length_ms: t.length_ms(),
        genre: disc.genre.clone(),
        year: disc.date.clone(),
        format: disc.format.clone(),
        sample_rate: (!encoded).then_some(RATE),
        channels: Some(2),
        bitrate_kbps: (!encoded).then_some(1411),
        ..Default::default()
    })
}

// ---------------------------------------------------------------- decoder

enum Reader {
    Raw {
        file: Box<dyn ReadSeek>,
        offset: u64,
        stride: u64,
        big_endian: bool,
    },
    Chd {
        chd: Box<Chd>,
        first: u64,
        frames_per_hunk: u64,
        hunk: Vec<u8>,
        compressed: Vec<u8>,
        loaded: Option<u32>,
    },
    Encoded {
        dec: Box<SymDecoder>,
        start_ms: u64,
    },
}

/// Plays one audio track of a disc.
pub struct DiscDecoder {
    reader: Reader,
    rate: u32,
    /// Frames played into the track.
    pos: u64,
    /// The track's length in frames, when known.
    len: Option<u64>,
    sector: Vec<u8>,
}

pub fn open(loc: &Location, sub: Option<u32>) -> Result<DiscDecoder> {
    let disc = read_disc(loc)?;
    let t = pick(&disc, sub)?;
    let (reader, rate) = match &t.source {
        Source::Raw {
            file,
            offset,
            stride,
            big_endian,
        } => (
            Reader::Raw {
                file: open_file(file)?,
                offset: *offset,
                stride: *stride,
                big_endian: *big_endian,
            },
            RATE,
        ),
        Source::Chd { frame } => {
            let chd = open_chd(loc)?;
            let hunk = chd.get_hunksized_buffer();
            let frames_per_hunk = (hunk.len() as u64 / CHD_FRAME).max(1);
            (
                Reader::Chd {
                    chd: Box::new(chd),
                    first: *frame,
                    frames_per_hunk,
                    hunk,
                    compressed: Vec::new(),
                    loaded: None,
                },
                RATE,
            )
        }
        Source::Encoded { file, start } => {
            let mut dec = SymDecoder::open(file)?;
            let start_ms = start * 1000 / SECTORS_PER_SECOND;
            if start_ms > 0 {
                dec.seek(start_ms)?;
            }
            let rate = dec.sample_rate();
            (
                Reader::Encoded {
                    dec: Box::new(dec),
                    start_ms,
                },
                rate,
            )
        }
    };
    Ok(DiscDecoder {
        reader,
        rate,
        pos: 0,
        len: t.sectors.map(|s| s * rate as u64 / SECTORS_PER_SECOND),
        sector: vec![0; SECTOR as usize],
    })
}

impl DiscDecoder {
    /// Reads sector `n` of the track into `self.sector`, as little-endian
    /// samples. False past the end of the file.
    fn load_sector(&mut self, n: u64) -> Result<bool> {
        match &mut self.reader {
            Reader::Raw {
                file,
                offset,
                stride,
                big_endian,
            } => {
                file.seek(SeekFrom::Start(*offset + n * *stride))?;
                let mut got = 0;
                while got < self.sector.len() {
                    match file.read(&mut self.sector[got..])? {
                        0 => break,
                        k => got += k,
                    }
                }
                if got == 0 {
                    return Ok(false);
                }
                self.sector[got..].fill(0);
                if *big_endian {
                    swap(&mut self.sector);
                }
            }
            Reader::Chd {
                chd,
                first,
                frames_per_hunk,
                hunk,
                compressed,
                loaded,
            } => {
                let frame = *first + n;
                let h = (frame / *frames_per_hunk) as u32;
                if h >= chd.header().hunk_count() {
                    return Ok(false);
                }
                if *loaded != Some(h) {
                    *loaded = None;
                    chd.hunk(h)
                        .and_then(|mut k| k.read_hunk_in(compressed, hunk))
                        .map_err(|e| anyhow!("CHD hunk {h}: {e}"))?;
                    *loaded = Some(h);
                }
                let at = ((frame % *frames_per_hunk) * CHD_FRAME) as usize;
                self.sector.copy_from_slice(&hunk[at..at + SECTOR as usize]);
                // CHDs keep CD audio big-endian.
                swap(&mut self.sector);
            }
            Reader::Encoded { .. } => unreachable!(),
        }
        Ok(true)
    }
}

fn swap(b: &mut [u8]) {
    for p in b.chunks_exact_mut(2) {
        p.swap(0, 1);
    }
}

impl Decoder for DiscDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }

    fn duration_ms(&self) -> Option<u64> {
        self.len.map(|f| f * 1000 / self.rate as u64)
    }

    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        let mut want = out.len() / 2;
        if let Some(len) = self.len {
            want = want.min(len.saturating_sub(self.pos) as usize);
        }
        if let Reader::Encoded { dec, .. } = &mut self.reader {
            let n = dec.read(&mut out[..want * 2])?;
            self.pos += n as u64;
            return Ok(n);
        }
        let mut done = 0;
        while done < want {
            let (s, within) = (self.pos / SECTOR_FRAMES, self.pos % SECTOR_FRAMES);
            if !self.load_sector(s)? {
                break;
            }
            let take = ((SECTOR_FRAMES - within) as usize).min(want - done);
            let from = within as usize * 4;
            for (i, f) in self.sector[from..from + take * 4]
                .chunks_exact(2)
                .enumerate()
            {
                out[done * 2 + i] = i16::from_le_bytes([f[0], f[1]]) as f32 / 32768.0;
            }
            done += take;
            self.pos += take as u64;
        }
        Ok(done)
    }

    fn seek(&mut self, ms: u64) -> Result<()> {
        let mut frame = ms * self.rate as u64 / 1000;
        if let Some(len) = self.len {
            frame = frame.min(len);
        }
        if let Reader::Encoded { dec, start_ms } = &mut self.reader {
            dec.seek(*start_ms + ms)?;
        }
        self.pos = frame;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sector of audio whose left channel counts up from `n` and right
    /// channel down, as little-endian samples.
    fn sector(n: i16) -> Vec<u8> {
        (0..SECTOR_FRAMES as i16)
            .flat_map(|i| {
                let (l, r) = (n.wrapping_add(i), n.wrapping_sub(i));
                [l.to_le_bytes(), r.to_le_bytes()].concat()
            })
            .collect()
    }

    fn left(out: &[f32]) -> Vec<i16> {
        out.chunks_exact(2)
            .map(|f| (f[0] * 32768.0).round() as i16)
            .collect()
    }

    fn read_all(d: &mut DiscDecoder) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = vec![0.0; 1000];
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                break all;
            }
            all.extend_from_slice(&buf[..n * 2]);
        }
    }

    #[test]
    fn splits_words() {
        assert_eq!(
            words(r#"FILE "My Game (Track 1).bin" BINARY"#),
            ["FILE", "My Game (Track 1).bin", "BINARY"]
        );
        assert_eq!(words("  INDEX 01 00:02:00 "), ["INDEX", "01", "00:02:00"]);
        assert_eq!(msf("01:02:03"), Some((62 * 75) + 3));
    }

    #[test]
    fn single_bin_with_a_data_track() {
        let dir = tempfile::tempdir().unwrap();
        // Track 1: 4 data sectors of 2048 bytes; track 2: 2 sectors of
        // pregap, then 3 of audio; track 3: 2 of audio.
        let mut bin = vec![0xffu8; 4 * 2048];
        for n in 0..5 {
            bin.extend(sector(n * 1000));
        }
        bin.extend(sector(7000));
        bin.extend(sector(8000));
        std::fs::write(dir.path().join("game.bin"), &bin).unwrap();
        std::fs::write(
            dir.path().join("game.cue"),
            "TITLE \"A Game\"\r\nFILE \"GAME.BIN\" BINARY\r\n  TRACK 01 MODE1/2048\r\n    INDEX 01 00:00:00\r\n  \
             TRACK 02 AUDIO\r\n    TITLE \"Stage 1\"\r\n    INDEX 00 00:00:04\r\n    INDEX 01 00:00:06\r\n  \
             TRACK 03 AUDIO\r\n    INDEX 01 00:00:09\r\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("game.cue"));
        let list = track_list(&loc).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].index, list[0].title.as_str()), (1, "Stage 1"));
        assert_eq!(list[0].length_ms, Some(3 * 1000 / 75));
        assert_eq!(list[1].index, 2);
        let m = meta(&loc, Some(1)).unwrap();
        assert_eq!((m.album.as_str(), m.track_no), ("A Game", Some(2)));
        assert_eq!(meta(&loc, Some(2)).unwrap().title, "Track 03");
        assert!(open(&loc, Some(0)).is_err());

        let mut d = open(&loc, Some(1)).unwrap();
        let all = read_all(&mut d);
        assert_eq!(all.len() as u64, 3 * SECTOR_FRAMES * 2);
        let l = left(&all);
        assert_eq!((l[0], l[1]), (2000, 2001));
        assert_eq!(l[SECTOR_FRAMES as usize], 3000);
        assert_eq!((all[3] * 32768.0).round() as i16, 1999);
        // Seeking lands on the frame.
        d.seek(1000 / 75 * 2).unwrap();
        let mut buf = vec![0.0; 4];
        d.read(&mut buf).unwrap();
        // 26 ms is frame 1146: frame 558 of the track's second sector.
        assert_eq!(left(&buf)[0], 3558);
        // With no track named, the first audio track plays.
        let mut d = open(&loc, None).unwrap();
        assert_eq!(left(&read_all(&mut d))[0], 2000);
        // The browser leaves the BIN out of the listing.
        assert_eq!(companions(&loc), ["GAME.BIN"]);
    }

    #[test]
    fn one_bin_a_track_and_renamed_sheets() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("x (Track 1).bin"), sector(1)).unwrap();
        // Big-endian (MOTOROLA) audio.
        let mut be = sector(500);
        swap(&mut be);
        std::fs::write(dir.path().join("x (Track 2).bin"), &be).unwrap();
        std::fs::write(
            dir.path().join("x.cue"),
            "FILE \"x (Track 1).bin\" BINARY\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n\
             FILE \"x (Track 2).bin\" MOTOROLA\n TRACK 02 AUDIO\n  INDEX 00 00:00:00\n  INDEX 01 00:00:00\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("x.cue"));
        assert_eq!(track_list(&loc).unwrap().len(), 2);
        let mut d = open(&loc, Some(1)).unwrap();
        assert_eq!(left(&read_all(&mut d))[..2], [500, 501]);

        // A sheet naming a file it was renamed away from.
        std::fs::write(dir.path().join("Renamed.bin"), sector(9)).unwrap();
        std::fs::write(
            dir.path().join("Renamed.cue"),
            "FILE \"Original Name.bin\" BINARY\n TRACK 01 AUDIO\n  INDEX 01 00:00:00\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("Renamed.cue"));
        let mut d = open(&loc, Some(0)).unwrap();
        assert_eq!(left(&read_all(&mut d))[0], 9);
    }

    #[test]
    fn album_rip_over_a_wav() {
        let dir = tempfile::tempdir().unwrap();
        // 2 seconds of 44.1 kHz stereo: the first second at one level, the
        // second at another.
        let mut pcm = Vec::new();
        for i in 0..88_200u32 {
            let v: i16 = if i < 44_100 { 1000 } else { -2000 };
            pcm.extend(v.to_le_bytes());
            pcm.extend(v.to_le_bytes());
        }
        let mut wav = b"RIFF".to_vec();
        wav.extend((36 + pcm.len() as u32).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(44_100u32.to_le_bytes());
        wav.extend((44_100u32 * 4).to_le_bytes());
        wav.extend(4u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend((pcm.len() as u32).to_le_bytes());
        wav.extend(pcm);
        std::fs::write(dir.path().join("album.wav"), wav).unwrap();
        std::fs::write(
            dir.path().join("album.cue"),
            "PERFORMER \"Band\"\nREM DATE 1994\nFILE \"album.wav\" WAVE\n TRACK 01 AUDIO\n  TITLE \"One\"\n  INDEX 01 00:00:00\n\
             TRACK 02 AUDIO\n  TITLE \"Two\"\n  INDEX 01 00:01:00\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("album.cue"));
        let list = track_list(&loc).unwrap();
        assert_eq!(list[0].length_ms, Some(1000));
        assert_eq!(list[1].length_ms, Some(1000));
        let m = meta(&loc, Some(1)).unwrap();
        assert_eq!(
            (m.title.as_str(), m.artist.as_str(), m.year.as_str()),
            ("Two", "Band", "1994")
        );
        assert_eq!(m.format, "CUE (WAV)");
        let mut d = open(&loc, Some(1)).unwrap();
        let all = read_all(&mut d);
        assert_eq!(all.len(), 44_100 * 2);
        assert!(left(&all).iter().all(|&v| v == -2000));
        let mut d = open(&loc, Some(0)).unwrap();
        let all = read_all(&mut d);
        assert_eq!(all.len(), 44_100 * 2);
        assert!(left(&all).iter().all(|&v| v == 1000));
        // The WAV is music of its own: still listed.
        assert!(companions(&loc).is_empty());
    }

    #[test]
    fn gdi() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("track01.bin"), vec![0u8; 2352 * 3]).unwrap();
        let mut raw = sector(10);
        raw.extend(sector(20));
        std::fs::write(dir.path().join("track02.raw"), &raw).unwrap();
        std::fs::write(dir.path().join("track03.bin"), vec![0u8; 2048 * 5]).unwrap();
        std::fs::write(
            dir.path().join("disc.gdi"),
            "3\n1 0 4 2352 track01.bin 0\n2 600 0 2352 \"track02.raw\" 0\n3 45000 4 2048 track03.bin 0\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("disc.gdi"));
        let list = track_list(&loc).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].index, list[0].length_ms), (1, Some(2 * 1000 / 75)));
        let mut d = open(&loc, Some(1)).unwrap();
        let l = left(&read_all(&mut d));
        assert_eq!((l[0], l[588]), (10, 20));
        assert_eq!(
            companions(&loc),
            ["track01.bin", "track02.raw", "track03.bin"]
        );
    }

    #[test]
    fn clone_cd() {
        let dir = tempfile::tempdir().unwrap();
        let mut img = vec![0u8; 2352 * 2];
        img.extend(sector(100));
        img.extend(sector(200));
        img.extend(sector(300));
        std::fs::write(dir.path().join("Disc.img"), &img).unwrap();
        std::fs::write(
            dir.path().join("Disc.ccd"),
            "[CloneCD]\nVersion=3\n[Entry 0]\nPoint=0xa2\nPLBA=5\n\
             [TRACK 1]\nMODE=1\nINDEX 1=0\n[TRACK 2]\nMODE=0\nINDEX 0=2\nINDEX 1=2\n\
             [TRACK 3]\nMODE=0\nINDEX 1=4\n",
        )
        .unwrap();
        let loc = Location::Fs(dir.path().join("Disc.ccd"));
        let list = track_list(&loc).unwrap();
        assert_eq!(list.iter().map(|t| t.index).collect::<Vec<_>>(), [1, 2]);
        let mut d = open(&loc, Some(1)).unwrap();
        let l = left(&read_all(&mut d));
        assert_eq!((l.len(), l[0], l[588]), (2 * 588, 100, 200));
        let mut d = open(&loc, Some(2)).unwrap();
        assert_eq!(left(&read_all(&mut d))[0], 300);
        assert_eq!(companions(&loc), ["Disc.img", "Disc.sub"]);
    }

    #[test]
    fn chd() {
        use chd::metadata::KnownMetadata;
        use chd::write::{ChdWriteOptions, ChdWriter};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("game.chd");
        // Track 1: 3 frames of data (padded to 4); track 2: 2 frames of
        // stored pregap then 5 of audio (padded to 8). Audio is stored
        // big-endian.
        let frame = |data: Vec<u8>| {
            let mut f = data;
            f.resize(CHD_FRAME as usize, 0);
            f
        };
        let mut frames: Vec<Vec<u8>> = (0..3).map(|_| frame(vec![0x55; 2352])).collect();
        frames.push(frame(Vec::new()));
        for n in 0..7 {
            let mut s = sector(n * 100);
            swap(&mut s);
            frames.push(frame(s));
        }
        frames.push(frame(Vec::new()));
        let mut w =
            ChdWriter::new(std::fs::File::create(&path).unwrap(), ChdWriteOptions::cd()).unwrap();
        for (i, t) in [
            "TRACK:1 TYPE:MODE1_RAW SUBTYPE:NONE FRAMES:3 PREGAP:0 PGTYPE:MODE1 PGSUB:RW POSTGAP:0",
            "TRACK:2 TYPE:AUDIO SUBTYPE:NONE FRAMES:7 PREGAP:2 PGTYPE:VAUDIO PGSUB:RW POSTGAP:0",
        ]
        .iter()
        .enumerate()
        {
            let text = std::ffi::CString::new(*t).unwrap();
            w.set_metadata(KnownMetadata::CdRomTrack2, i as u32, text.as_c_str(), true)
                .unwrap();
        }
        let hunk = frames.concat();
        let per = 8 * CHD_FRAME as usize;
        for h in hunk.chunks(per) {
            let mut h = h.to_vec();
            h.resize(per, 0);
            w.write_hunk(&h).unwrap();
        }
        w.finish().unwrap();

        let loc = Location::Fs(path);
        let list = track_list(&loc).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].index, list[0].length_ms), (1, Some(5 * 1000 / 75)));
        assert_eq!(meta(&loc, Some(1)).unwrap().format, "CHD");
        let mut d = open(&loc, Some(1)).unwrap();
        let l = left(&read_all(&mut d));
        assert_eq!(l.len(), 5 * 588);
        assert_eq!((l[0], l[1], l[588], l[4 * 588]), (200, 201, 300, 600));
    }
}
