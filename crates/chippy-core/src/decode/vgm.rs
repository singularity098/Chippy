//! vgmstream: hundreds of game audio formats (ADX, BRSTM, HCA, XWB, ...).
//!
//! Files on disk are opened with vgmstream's own stdio reader, so companion
//! files (headers next to bodies) are found. Files inside archives are read
//! into memory and served through a custom streamfile whose `open` pulls
//! companion files out of the same archive.

use anyhow::{bail, Result};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;
use std::sync::{Arc, Once};

use super::{to_stereo, DecodeOptions, Decoder, SubTrack};
use crate::archive;
use crate::meta::TrackMeta;
use crate::vfs::Location;
use vgmstream_sys as ffi;

fn quiet() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe { ffi::libvgmstream_set_log(ffi::LIBVGMSTREAM_LOG_LEVEL_NONE, None) });
}

/// Every extension vgmstream can open, lowercase.
pub fn extensions() -> Vec<String> {
    let mut n: c_int = 0;
    let p = unsafe { ffi::libvgmstream_get_extensions(&mut n) };
    if p.is_null() {
        return Vec::new();
    }
    (0..n as usize)
        .filter_map(|i| {
            let s = unsafe { *p.add(i) };
            (!s.is_null()).then(|| {
                unsafe { CStr::from_ptr(s) }
                    .to_string_lossy()
                    .to_lowercase()
            })
        })
        .collect()
}

// ------------------------------------------------------- memory streamfile

struct MemFile {
    data: Arc<Vec<u8>>,
    /// The file's virtual path ("/music/pack.zip/dir/file.ext").
    name: CString,
    archive: PathBuf,
}

unsafe extern "C" fn mem_read(ud: *mut c_void, dst: *mut u8, offset: i64, length: c_int) -> c_int {
    let f = &*(ud as *const MemFile);
    if offset < 0 || length <= 0 || offset as usize >= f.data.len() {
        return 0;
    }
    let start = offset as usize;
    let n = (length as usize).min(f.data.len() - start);
    std::ptr::copy_nonoverlapping(f.data.as_ptr().add(start), dst, n);
    n as c_int
}

unsafe extern "C" fn mem_size(ud: *mut c_void) -> i64 {
    (*(ud as *const MemFile)).data.len() as i64
}

unsafe extern "C" fn mem_name(ud: *mut c_void) -> *const c_char {
    (*(ud as *const MemFile)).name.as_ptr()
}

unsafe extern "C" fn mem_open(
    ud: *mut c_void,
    filename: *const c_char,
) -> *mut ffi::libstreamfile_t {
    let f = &*(ud as *const MemFile);
    if filename.is_null() {
        return std::ptr::null_mut();
    }
    let want = CStr::from_ptr(filename).to_string_lossy().into_owned();
    if want == f.name.to_string_lossy() {
        return new_mem_sf(MemFile {
            data: f.data.clone(),
            name: f.name.clone(),
            archive: f.archive.clone(),
        });
    }
    let prefix = format!("{}/", f.archive.to_string_lossy());
    if let Some(inner) = want.strip_prefix(&prefix) {
        let Ok(entries) = archive::list(&f.archive) else {
            return std::ptr::null_mut();
        };
        let Some(entry) = archive::find_ci(&entries, inner) else {
            return std::ptr::null_mut();
        };
        let Ok(data) = archive::read(&f.archive, &entry) else {
            return std::ptr::null_mut();
        };
        let Ok(name) = CString::new(format!("{prefix}{entry}")) else {
            return std::ptr::null_mut();
        };
        return new_mem_sf(MemFile {
            data,
            name,
            archive: f.archive.clone(),
        });
    }
    // Something outside the archive: a real file, if it exists.
    ffi::libstreamfile_open_from_stdio(filename)
}

unsafe extern "C" fn mem_close(sf: *mut ffi::libstreamfile_t) {
    if sf.is_null() {
        return;
    }
    let sf = Box::from_raw(sf);
    drop(Box::from_raw(sf.user_data as *mut MemFile));
}

fn new_mem_sf(f: MemFile) -> *mut ffi::libstreamfile_t {
    Box::into_raw(Box::new(ffi::libstreamfile_t {
        user_data: Box::into_raw(Box::new(f)).cast(),
        read: Some(mem_read),
        get_size: Some(mem_size),
        get_name: Some(mem_name),
        open: Some(mem_open),
        close: Some(mem_close),
    }))
}

/// An open streamfile, closed on drop.
struct Sf(*mut ffi::libstreamfile_t);
impl Drop for Sf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ffi::libstreamfile_close(self.0) }
        }
    }
}

fn open_sf(loc: &Location) -> Result<Sf> {
    let sf = match loc {
        Location::Fs(p) => {
            let c = CString::new(p.to_string_lossy().as_bytes())?;
            unsafe { ffi::libstreamfile_open_from_stdio(c.as_ptr()) }
        }
        Location::Archive { archive, .. } => {
            let data = loc.read()?;
            let name = CString::new(loc.vpath().to_string_lossy().as_bytes())?;
            new_mem_sf(MemFile {
                data,
                name,
                archive: archive.clone(),
            })
        }
    };
    if sf.is_null() {
        bail!("cannot open {}", loc.vpath().display());
    }
    Ok(Sf(sf))
}

// ------------------------------------------------------------- the stream

struct Lib(*mut ffi::libvgmstream_t);
unsafe impl Send for Lib {}
impl Drop for Lib {
    fn drop(&mut self) {
        unsafe { ffi::libvgmstream_free(self.0) }
    }
}

impl Lib {
    fn new(opts: &DecodeOptions) -> Result<Lib> {
        quiet();
        let lib = unsafe { ffi::libvgmstream_init() };
        if lib.is_null() {
            bail!("vgmstream could not start");
        }
        let mut cfg = ffi::libvgmstream_config_t {
            loop_count: opts.loops,
            fade_time: opts.fade_ms as f64 / 1000.0,
            force_sfmt: ffi::LIBVGMSTREAM_SFMT_FLOAT,
            ..Default::default()
        };
        unsafe { ffi::libvgmstream_setup(lib, &mut cfg) };
        Ok(Lib(lib))
    }

    /// Opens subsong `sub` (1-based; 0 = the default one).
    fn open(&self, sf: &Sf, sub: i32) -> Result<()> {
        if unsafe { ffi::libvgmstream_open_stream(self.0, sf.0, sub) } < 0 {
            bail!("not a format vgmstream knows");
        }
        Ok(())
    }

    fn format(&self) -> &ffi::libvgmstream_format_t {
        unsafe { &*(*self.0).format }
    }

    fn title(&self, filename: &str) -> String {
        let fname = CString::new(filename).unwrap_or_default();
        let mut cfg = ffi::libvgmstream_title_t {
            force_title: false,
            subsong_range: false,
            remove_extension: true,
            remove_archive: true,
            filename: fname.as_ptr(),
        };
        let mut buf = vec![0 as c_char; 512];
        unsafe {
            ffi::libvgmstream_get_title(self.0, &mut cfg, buf.as_mut_ptr(), buf.len() as c_int)
        };
        unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    }
}

fn carr(a: &[c_char]) -> String {
    unsafe { CStr::from_ptr(a.as_ptr()) }
        .to_string_lossy()
        .trim()
        .to_string()
}

pub struct VgmDecoder {
    lib: Lib,
    rate: u32,
    channels: usize,
    duration: Option<u64>,
    pending: Vec<f32>,
    done: bool,
}

impl VgmDecoder {
    pub fn open(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<Self> {
        let sf = open_sf(loc)?;
        let lib = Lib::new(opts)?;
        lib.open(&sf, sub.map(|s| s as i32 + 1).unwrap_or(0))?;
        drop(sf);
        let f = lib.format();
        let rate = f.sample_rate.max(1) as u32;
        let duration = (f.play_samples > 0).then(|| f.play_samples as u64 * 1000 / rate as u64);
        Ok(VgmDecoder {
            channels: f.channels.max(1) as usize,
            rate,
            duration,
            lib,
            pending: Vec::new(),
            done: false,
        })
    }
}

impl Decoder for VgmDecoder {
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn duration_ms(&self) -> Option<u64> {
        self.duration
    }
    fn read(&mut self, out: &mut [f32]) -> Result<usize> {
        while self.pending.len() < out.len() && !self.done {
            if unsafe { ffi::libvgmstream_render(self.lib.0) } < 0 {
                bail!("vgmstream decoding failed");
            }
            let d = unsafe { &*(*self.lib.0).decoder };
            if d.buf_samples > 0 && !d.buf.is_null() {
                let src = unsafe {
                    std::slice::from_raw_parts(
                        d.buf as *const f32,
                        d.buf_samples as usize * self.channels,
                    )
                };
                to_stereo(src, self.channels, &mut self.pending);
            }
            self.done = d.done;
            if d.buf_samples == 0 && !d.done {
                // Guard against a stream that stalls without ending.
                self.done = true;
            }
        }
        let n = self.pending.len().min(out.len()) & !1;
        out[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n / 2)
    }
    fn seek(&mut self, ms: u64) -> Result<()> {
        unsafe { ffi::libvgmstream_seek(self.lib.0, (ms * self.rate as u64 / 1000) as i64) };
        self.pending.clear();
        self.done = false;
        Ok(())
    }
}

/// Files with more subsongs than this are not opened one by one for names.
const NAME_LIMIT: i32 = 400;

pub fn track_list(loc: &Location, opts: &DecodeOptions) -> Result<Vec<SubTrack>> {
    let sf = open_sf(loc)?;
    let lib = Lib::new(opts)?;
    lib.open(&sf, 0)?;
    let count = lib.format().subsong_count;
    let name = loc.name();
    let length = |f: &ffi::libvgmstream_format_t| {
        (f.play_samples > 0 && f.sample_rate > 0)
            .then(|| f.play_samples as u64 * 1000 / f.sample_rate as u64)
    };
    if count <= 1 {
        return Ok(vec![SubTrack {
            index: 0,
            title: lib.title(&name),
            length_ms: length(lib.format()),
        }]);
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 1..=count {
        if count <= NAME_LIMIT && lib.open(&sf, i).is_ok() {
            let f = lib.format();
            let mut title = carr(&f.stream_name);
            if title.is_empty() {
                title = String::new();
            }
            out.push(SubTrack {
                index: (i - 1) as u32,
                title,
                length_ms: length(f),
            });
        } else {
            out.push(SubTrack {
                index: (i - 1) as u32,
                title: String::new(),
                length_ms: None,
            });
        }
    }
    Ok(out)
}

pub fn meta(loc: &Location, sub: Option<u32>, opts: &DecodeOptions) -> Result<TrackMeta> {
    let sf = open_sf(loc)?;
    let lib = Lib::new(opts)?;
    lib.open(&sf, sub.map(|s| s as i32 + 1).unwrap_or(0))?;
    let f = lib.format();
    let rate = f.sample_rate.max(1) as u64;
    let stream = carr(&f.stream_name);
    let mut m = TrackMeta {
        length_ms: (f.play_samples > 0).then(|| f.play_samples as u64 * 1000 / rate),
        format: carr(&f.meta_name),
        sample_rate: Some(f.sample_rate as u32),
        channels: Some(f.input_channels.max(f.channels) as u32),
        bitrate_kbps: (f.stream_bitrate > 0).then_some((f.stream_bitrate / 1000) as u32),
        comment: carr(&f.codec_name),
        ..Default::default()
    };
    if f.subsong_count > 1 {
        m.title = if stream.is_empty() {
            String::new()
        } else {
            stream
        };
        m.track_no = Some(f.subsong_index.max(1) as u32);
    }
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knows_many_extensions() {
        let e = extensions();
        assert!(e.len() > 300, "only {} extensions", e.len());
        assert!(e.iter().any(|x| x == "adx"));
        assert!(e.iter().any(|x| x == "brstm"));
    }

    /// A 16-bit PCM WAV file, decoded through the in-memory streamfile.
    #[test]
    fn decodes_wav_from_a_zip() {
        use std::io::Write;
        let rate = 22_050u32;
        let samples: Vec<i16> = (0..rate)
            .map(|i| ((i as f32 * 0.1).sin() * 8000.0) as i16)
            .collect();
        let mut wav = Vec::new();
        let data_len = samples.len() as u32 * 2;
        wav.extend(b"RIFF");
        wav.extend((36 + data_len).to_le_bytes());
        wav.extend(b"WAVEfmt ");
        wav.extend(16u32.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(1u16.to_le_bytes());
        wav.extend(rate.to_le_bytes());
        wav.extend((rate * 2).to_le_bytes());
        wav.extend(2u16.to_le_bytes());
        wav.extend(16u16.to_le_bytes());
        wav.extend(b"data");
        wav.extend(data_len.to_le_bytes());
        for s in &samples {
            wav.extend(s.to_le_bytes());
        }
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("w.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
            w.start_file("a/tone.wav", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(&wav).unwrap();
            w.finish().unwrap();
        }
        let loc = Location::Archive {
            archive: z,
            entry: "a/tone.wav".into(),
        };
        let mut d = VgmDecoder::open(&loc, None, &DecodeOptions::default()).unwrap();
        assert_eq!(d.sample_rate(), rate);
        assert_eq!(d.duration_ms(), Some(1000));
        let mut buf = vec![0f32; 2048];
        let mut frames = 0;
        loop {
            let n = d.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            frames += n;
        }
        assert_eq!(frames, rate as usize);
    }
}
