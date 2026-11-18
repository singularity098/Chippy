// A C API over libsidplayfp for Chippy: load a tune from memory, pick a
// song, render stereo 16-bit samples with reSIDfp.
#include <cstring>
#include <cstdint>
#include <new>

#include "sidplayfp/sidplayfp.h"
#include "sidplayfp/SidTune.h"
#include "sidplayfp/SidTuneInfo.h"
#include "sidplayfp/SidConfig.h"
#include "sidplayfp/SidInfo.h"
#include "builders/residfp-builder/residfp.h"

struct csid {
    sidplayfp player;
    ReSIDfpBuilder builder;
    SidTune tune;
    csid(const uint8_t *data, uint32_t len)
        : builder("chippy"), tune(data, len) {}
};

static void copy_err(char *err, size_t n, const char *msg) {
    if (err && n) {
        strncpy(err, msg ? msg : "unknown error", n - 1);
        err[n - 1] = 0;
    }
}

extern "C" {

csid *csid_open(const uint8_t *data, uint32_t len, char *err, size_t errlen) {
    csid *s = new (std::nothrow) csid(data, len);
    if (!s) {
        copy_err(err, errlen, "out of memory");
        return nullptr;
    }
    if (!s->tune.getStatus()) {
        copy_err(err, errlen, s->tune.statusString());
        delete s;
        return nullptr;
    }
    return s;
}

void csid_close(csid *s) { delete s; }

unsigned csid_songs(csid *s) { return s->tune.getInfo()->songs(); }
unsigned csid_start_song(csid *s) { return s->tune.getInfo()->startSong(); }

// 0 title, 1 author, 2 released.
const char *csid_info(csid *s, unsigned i) {
    const SidTuneInfo *info = s->tune.getInfo();
    return i < info->numberOfInfoStrings() ? info->infoString(i) : "";
}

// 0 unknown, 1 6581, 2 8580, 3 any.
int csid_sid_model(csid *s) { return (int)s->tune.getInfo()->sidModel(0); }
// 0 unknown, 1 PAL, 2 NTSC, 3 any.
int csid_clock(csid *s) { return (int)s->tune.getInfo()->clockSpeed(); }
unsigned csid_sid_chips(csid *s) { return s->tune.getInfo()->sidChips(); }

// The MD5 used by the HVSC song length database (33 bytes with the NUL).
void csid_md5(csid *s, char *out) { s->tune.createMD5New(out); }

// Starts song `song` (1-based) at `rate` Hz; returns 0 on success.
int csid_start(csid *s, unsigned song, uint32_t rate, char *err, size_t errlen) {
    s->tune.selectSong(song);
    if (!s->builder.getStatus()) {
        copy_err(err, errlen, s->builder.error());
        return -1;
    }
    s->builder.create(s->player.info().maxsids());
    if (!s->builder.getStatus()) {
        copy_err(err, errlen, s->builder.error());
        return -1;
    }
    s->builder.filter(true);
    SidConfig cfg;
    cfg.frequency = rate;
    cfg.playback = SidConfig::STEREO;
    cfg.samplingMethod = SidConfig::RESAMPLE_INTERPOLATE;
    cfg.fastSampling = false;
    cfg.sidEmulation = &s->builder;
    cfg.defaultSidModel = SidConfig::MOS6581;
    cfg.defaultC64Model = SidConfig::PAL;
    if (!s->player.config(cfg)) {
        copy_err(err, errlen, s->player.error());
        return -1;
    }
    if (!s->player.load(&s->tune)) {
        copy_err(err, errlen, s->player.error());
        return -1;
    }
    return 0;
}

// Fills `buf` with `frames` stereo frames; returns the frames made.
uint32_t csid_play(csid *s, int16_t *buf, uint32_t frames) {
    return s->player.play(buf, frames * 2) / 2;
}

}
