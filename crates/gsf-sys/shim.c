/* Chippy's C API over mGBA for GSF: a GBA running a ROM image, its sound
 * resampled to the asked rate. */
#include <stdlib.h>
#include <string.h>
#include <mgba/core/core.h>
#include <mgba/core/blip_buf.h>
#include <mgba/core/config.h>
#include <mgba-util/vfs.h>
#include <mgba-util/memory.h>
#include <mgba/internal/gba/gba.h>

void* anonymousMemoryMap(size_t size) { return calloc(1, size); }
void mappedMemoryFree(void* memory, size_t size) { (void)size; free(memory); }

#define CHUNK 2048

struct gsf_stream {
    struct mAVStream av;
    int16_t samples[CHUNK * 2];
    unsigned count, pos;
};

struct gsf {
    struct gsf_stream stream;
    struct mCore *core;
    uint8_t *rom;
    size_t rom_size;
};

static void post_audio(struct mAVStream *s, blip_t *left, blip_t *right) {
    struct gsf_stream *st = (struct gsf_stream *)s;
    int n = blip_samples_avail(left);
    if (n > CHUNK) n = CHUNK;
    blip_read_samples(left, st->samples, n, 1);
    blip_read_samples(right, st->samples + 1, n, 1);
    st->count = n;
    st->pos = 0;
}

struct gsf *gsf_open(const uint8_t *rom, size_t size, unsigned rate) {
    struct gsf *g = calloc(1, sizeof *g);
    if (!g) return NULL;
    /* The VFile reads the ROM where it lies: keep our own copy. */
    g->rom = malloc(size);
    if (!g->rom) { free(g); return NULL; }
    memcpy(g->rom, rom, size);
    g->rom_size = size;
    struct VFile *vf = VFileFromConstMemory(g->rom, size);
    struct mCore *core = vf ? mCoreFindVF(vf) : NULL;
    if (!core) {
        if (vf) vf->close(vf);
        free(g->rom);
        free(g);
        return NULL;
    }
    g->stream.av.postAudioBuffer = post_audio;
    core->init(core);
    core->setAVStream(core, &g->stream.av);
    mCoreInitConfig(core, NULL);
    core->setAudioBufferSize(core, CHUNK);
    blip_set_rates(core->getAudioChannel(core, 0), core->frequency(core), rate);
    blip_set_rates(core->getAudioChannel(core, 1), core->frequency(core), rate);
    struct mCoreOptions opts;
    memset(&opts, 0, sizeof opts);
    opts.useBios = false;
    opts.skipBios = true;
    opts.volume = 0x100;
    opts.sampleRate = rate;
    mCoreConfigLoadDefaults(&core->config, &opts);
    core->loadROM(core, vf);
    core->reset(core);
    g->core = core;
    return g;
}

/* Fills `out` with `frames` stereo frames. */
void gsf_render(struct gsf *g, int16_t *out, unsigned frames) {
    unsigned guard = 0;
    while (frames) {
        if (g->stream.pos >= g->stream.count) {
            g->stream.count = g->stream.pos = 0;
            g->core->runFrame(g->core);
            if (g->stream.count == 0) {
                /* No sound this frame (yet): give up after a while. */
                if (++guard > 600) { memset(out, 0, frames * 4); return; }
                continue;
            }
            guard = 0;
        }
        unsigned n = g->stream.count - g->stream.pos;
        if (n > frames) n = frames;
        memcpy(out, g->stream.samples + g->stream.pos * 2, n * 4);
        out += n * 2;
        g->stream.pos += n;
        frames -= n;
    }
}

/* For finding notes: the sound register hook (see struct GBA's notesHook),
 * running a frame at a time, the global cycle count, and reading memory. */
typedef void (*gsf_notes_fn)(void *user, uint64_t cycles, uint32_t address, uint32_t value, int bytes);

void gsf_set_notes_hook(struct gsf *g, gsf_notes_fn f, void *user) {
    struct GBA *gba = g->core->board;
    gba->notesHook = f;
    gba->notesUser = user;
}

void gsf_run_frame(struct gsf *g) {
    g->core->runFrame(g->core);
    g->stream.count = g->stream.pos = 0;
}

uint64_t gsf_cycles(struct gsf *g) {
    struct GBA *gba = g->core->board;
    return (uint32_t) mTimingCurrentTime(&gba->timing);
}

uint32_t gsf_read8(struct gsf *g, uint32_t address) { return g->core->busRead8(g->core, address); }

uint32_t gsf_read32(struct gsf *g, uint32_t address) { return g->core->busRead32(g->core, address); }

void gsf_close(struct gsf *g) {
    if (!g) return;
    mCoreConfigDeinit(&g->core->config);
    g->core->deinit(g->core);
    free(g->rom);
    free(g);
}
