//! What a file is, judged by its extension.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

/// Which decoder handles a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    /// Game Music Emu: NSF, NSFE, SPC, VGM, GBS, HES, KSS, AY, SAP, GYM.
    Gme,
    /// Nintendo 64 USF / miniUSF (lazyusf2).
    Usf,
    /// Common formats through Symphonia: MP3, FLAC, WAV, OGG, M4A...; the
    /// ones it cannot decode (Opus, WMA, AC-3) fall through to vgmstream's
    /// FFmpeg.
    Common,
    /// Everything else vgmstream knows (hundreds of game formats).
    Vgmstream,
    /// Standard MIDI files, played through a SoundFont.
    Midi,
    /// Audio Overload SDK: PSF, PSF2, SSF, DSF, QSF, SPU.
    Ao,
    /// Commodore 64 SID tunes (libsidplayfp).
    Sid,
    /// Sega Master System / Game Gear SGC.
    Sgc,
    /// Game Boy Advance GSF (mGBA).
    Gsf,
    /// Nintendo DS 2SF (vio2sf).
    Twosf,
    /// Bandai WonderSwan WSR.
    Wsr,
    /// Tracker modules (libxmp).
    Module,
    /// Tab files (Guitar Pro, MusicXML), played through a SoundFont.
    Tab,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    Zip,
    SevenZip,
    Tar,
    TarGz,
    /// RAR (and RSN: SNES music packed in RAR).
    Rar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Archive(ArchiveKind),
    Audio(Codec),
    Image,
    Other,
}

/// GME formats that can hold several tracks (opening the file may reveal a
/// list to descend into).
pub const GME_MULTI: &[&str] = &["nsf", "nsfe", "gbs", "hes", "kss", "ay", "sap"];
pub const GME_SINGLE: &[&str] = &["spc", "vgm", "vgz", "gym"];
pub const USF: &[&str] = &["usf", "miniusf"];
pub const COMMON: &[&str] = &[
    "mp3", "mp2", "mp1", "flac", "wav", "wave", "ogg", "oga", "m4a", "m4b", "mp4", "aac", "alac",
    "aif", "aiff", "aifc", "caf", "mka", "webm", "opus", "wma", "ac3",
];
pub const AO: &[&str] = &[
    "psf", "minipsf", "psf1", "minipsf1", "psf2", "minipsf2", "ssf", "minissf", "dsf", "minidsf",
    "qsf", "miniqsf", "spu",
];
pub const SID: &[&str] = &["sid", "psid", "rsid"];
pub const SGC: &[&str] = &["sgc"];
pub const GSF: &[&str] = &["gsf", "minigsf"];
pub const TWOSF: &[&str] = &["2sf", "mini2sf"];
pub const WSR: &[&str] = &["wsr"];
pub const MODULES: &[&str] = &[
    "mod", "xm", "s3m", "it", "mtm", "669", "med", "mmd0", "mmd1", "mmd2", "mmd3", "okt", "stm",
    "far", "ult", "ptm", "dbm", "amf", "mdl", "digi", "psm", "j2b", "gdm", "imf", "dsm", "stx",
    "liq", "masi", "mgt", "rtm", "emod", "fnk", "ims", "xmf", "umx", "abk", "coco",
];
pub const MIDI: &[&str] = &["mid", "midi", "kar", "rmi", "smf"];
/// Tab files: Guitar Pro 3 to 5 and 7 to 8, MusicXML.
pub const TAB: &[&str] = tablature::song::EXTENSIONS;
pub const IMAGES: &[&str] = &["jpg", "jpeg", "png", "gif", "bmp", "webp"];

/// Extensions vgmstream accepts that are better shown as something else, or
/// are too generic to call music.
const VGMSTREAM_SKIP: &[&str] = &[
    "txt", "zip", "7z", "rar", "rsn", "jpg", "png", "nfo", "m3u", "pdf", "exe", "dll",
];

fn vgmstream_exts() -> &'static HashSet<String> {
    static EXTS: OnceLock<HashSet<String>> = OnceLock::new();
    EXTS.get_or_init(|| {
        crate::decode::vgm::extensions()
            .into_iter()
            .filter(|e| !VGMSTREAM_SKIP.contains(&e.as_str()))
            .collect()
    })
}

/// The lowercase extension of a file name ("" when none). `.tar.gz` gives
/// "tar.gz".
pub fn extension(name: &str) -> String {
    let lower = name.to_lowercase();
    if lower.ends_with(".tar.gz") {
        return "tar.gz".into();
    }
    match lower.rfind('.') {
        Some(i) if i > 0 && i + 1 < lower.len() => lower[i + 1..].to_string(),
        _ => String::new(),
    }
}

pub fn kind_of(name: &str) -> FileKind {
    let ext = extension(name);
    let e = ext.as_str();
    match e {
        "zip" => return FileKind::Archive(ArchiveKind::Zip),
        "7z" => return FileKind::Archive(ArchiveKind::SevenZip),
        "tar" => return FileKind::Archive(ArchiveKind::Tar),
        "tgz" | "tar.gz" => return FileKind::Archive(ArchiveKind::TarGz),
        "rar" | "rsn" => return FileKind::Archive(ArchiveKind::Rar),
        _ => {}
    }
    if GME_MULTI.contains(&e) || GME_SINGLE.contains(&e) {
        return FileKind::Audio(Codec::Gme);
    }
    if USF.contains(&e) {
        return FileKind::Audio(Codec::Usf);
    }
    if COMMON.contains(&e) {
        return FileKind::Audio(Codec::Common);
    }
    if AO.contains(&e) {
        return FileKind::Audio(Codec::Ao);
    }
    if SID.contains(&e) {
        return FileKind::Audio(Codec::Sid);
    }
    if SGC.contains(&e) {
        return FileKind::Audio(Codec::Sgc);
    }
    if GSF.contains(&e) {
        return FileKind::Audio(Codec::Gsf);
    }
    if TWOSF.contains(&e) {
        return FileKind::Audio(Codec::Twosf);
    }
    if WSR.contains(&e) {
        return FileKind::Audio(Codec::Wsr);
    }
    if MODULES.contains(&e) {
        return FileKind::Audio(Codec::Module);
    }
    if MIDI.contains(&e) {
        return FileKind::Audio(Codec::Midi);
    }
    if TAB.contains(&e) {
        return FileKind::Audio(Codec::Tab);
    }
    if IMAGES.contains(&e) {
        return FileKind::Image;
    }
    if !e.is_empty() && vgmstream_exts().contains(e) {
        return FileKind::Audio(Codec::Vgmstream);
    }
    FileKind::Other
}

pub fn kind_of_path(p: &Path) -> FileKind {
    p.file_name()
        .map(|n| kind_of(&n.to_string_lossy()))
        .unwrap_or(FileKind::Other)
}

/// Whether a file plays through the SoundFont (MIDI files and tab files).
pub fn plays_through_soundfont(name: &str) -> bool {
    matches!(
        kind_of(name),
        FileKind::Audio(Codec::Midi) | FileKind::Audio(Codec::Tab)
    )
}

/// Whether opening this file might show a list of tracks rather than play.
pub fn may_hold_tracks(name: &str) -> bool {
    let ext = extension(name);
    match kind_of(name) {
        FileKind::Audio(Codec::Gme) => GME_MULTI.contains(&ext.as_str()),
        FileKind::Audio(Codec::Vgmstream | Codec::Sid | Codec::Sgc | Codec::Wsr) => true,
        _ => false,
    }
}

/// Splits a file name into the part shown bright and the extension shown
/// dim: "Song.flac" -> ("Song", ".flac"). Names without an extension, and
/// names starting with a dot, have no dim part.
pub fn split_name(name: &str) -> (&str, &str) {
    let lower = name.to_lowercase();
    let cut = if lower.ends_with(".tar.gz") {
        Some(name.len() - 7)
    } else {
        name.rfind('.').filter(|&i| i > 0 && i + 1 < name.len())
    };
    match cut {
        Some(i) => (&name[..i], &name[i..]),
        None => (name, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of("a.NSFE"), FileKind::Audio(Codec::Gme));
        assert_eq!(kind_of("a.miniusf"), FileKind::Audio(Codec::Usf));
        assert_eq!(kind_of("a.flac"), FileKind::Audio(Codec::Common));
        assert_eq!(kind_of("a.opus"), FileKind::Audio(Codec::Common));
        assert_eq!(kind_of("x.tar.gz"), FileKind::Archive(ArchiveKind::TarGz));
        assert_eq!(kind_of("cover.JPG"), FileKind::Image);
        assert_eq!(kind_of("song.adx"), FileKind::Audio(Codec::Vgmstream));
        assert_eq!(kind_of("riff.gp5"), FileKind::Audio(Codec::Tab));
        assert_eq!(kind_of("riff.GP"), FileKind::Audio(Codec::Tab));
        assert_eq!(kind_of("score.mxl"), FileKind::Audio(Codec::Tab));
        assert!(plays_through_soundfont("a.mid") && plays_through_soundfont("a.gp4"));
        assert!(!plays_through_soundfont("a.nsf"));
        assert_eq!(kind_of("readme.txt"), FileKind::Other);
        assert_eq!(kind_of("noext"), FileKind::Other);
    }

    #[test]
    fn split() {
        assert_eq!(split_name("Song.flac"), ("Song", ".flac"));
        assert_eq!(split_name("a.b.c"), ("a.b", ".c"));
        assert_eq!(split_name(".hidden"), (".hidden", ""));
        assert_eq!(split_name("pack.tar.gz"), ("pack", ".tar.gz"));
        assert_eq!(split_name("plain"), ("plain", ""));
    }
}
