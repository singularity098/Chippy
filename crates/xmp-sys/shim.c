/* Chippy's C API over libxmp: just what a player needs. */
#include <string.h>
#include "xmp.h"

xmp_context cxmp_open(const void *data, long size, int rate) {
    xmp_context c = xmp_create_context();
    if (!c) return NULL;
    if (xmp_load_module_from_memory(c, data, size) != 0) {
        xmp_free_context(c);
        return NULL;
    }
    if (rate > 0 && xmp_start_player(c, rate, 0) != 0) {
        xmp_release_module(c);
        xmp_free_context(c);
        return NULL;
    }
    if (rate > 0) {
        /* Amiga-like stereo separation and interpolation. */
        xmp_set_player(c, XMP_PLAYER_MIX, 70);
        xmp_set_player(c, XMP_PLAYER_INTERP, XMP_INTERP_SPLINE);
    }
    return c;
}

/* Title, format name and comment (may be empty), and length in ms. */
void cxmp_info(xmp_context c, const char **name, const char **type, const char **comment,
               int *length_ms, int *channels) {
    struct xmp_module_info mi;
    xmp_get_module_info(c, &mi);
    *name = mi.mod->name;
    *type = mi.mod->type;
    *comment = mi.comment ? mi.comment : "";
    *length_ms = mi.num_sequences > 0 ? mi.seq_data[0].duration : 0;
    *channels = mi.mod->chn;
}

/* Renders `frames` stereo frames; returns 0 at the end of the song. */
int cxmp_render(xmp_context c, short *out, int frames) {
    return xmp_play_buffer(c, out, frames * 4, 1) == 0;
}

void cxmp_seek(xmp_context c, int ms) { xmp_seek_time(c, ms); }

void cxmp_close(xmp_context c, int started) {
    if (started) xmp_end_player(c);
    xmp_release_module(c);
    xmp_free_context(c);
}

/* ---- Note finding: the player stepped a frame at a time, and the
 * module's samples. */

struct cxmp_chan {
    int period; /* the mixer's period * 4096 (13696 is C-0) */
    int ins;
    int smp;
    int vol;
    int ev_note; /* the row's event, on every frame of the row */
    int ev_fxt;
    int ev_f2t;
};

/* Plays one frame, then fills where the song is and each channel's state.
 * Returns the number of channels, or -1 at the end of the song. */
int cxmp_scan(xmp_context c, int *time_ms, int *pos, int *row, struct cxmp_chan *ch, int max) {
    struct xmp_frame_info fi;
    struct xmp_module_info mi;
    int i, n;
    if (xmp_play_frame(c) != 0) return -1;
    xmp_get_frame_info(c, &fi);
    if (fi.loop_count > 0) return -1;
    xmp_get_module_info(c, &mi);
    *time_ms = fi.time;
    *pos = fi.pos;
    *row = fi.row;
    n = mi.mod->chn < max ? mi.mod->chn : max;
    for (i = 0; i < n; i++) {
        struct xmp_channel_info *ci = &fi.channel_info[i];
        ch[i].period = (int)ci->period;
        ch[i].ins = ci->instrument;
        ch[i].smp = ci->sample;
        ch[i].vol = ci->volume;
        ch[i].ev_note = ci->event.note;
        ch[i].ev_fxt = ci->event.fxt;
        ch[i].ev_f2t = ci->event.f2t;
    }
    return n;
}

int cxmp_num_samples(xmp_context c) {
    struct xmp_module_info mi;
    xmp_get_module_info(c, &mi);
    return mi.mod->smp;
}

int cxmp_num_instruments(xmp_context c) {
    struct xmp_module_info mi;
    xmp_get_module_info(c, &mi);
    return mi.mod->ins;
}

const char *cxmp_instrument_name(xmp_context c, int i) {
    struct xmp_module_info mi;
    xmp_get_module_info(c, &mi);
    if (i < 0 || i >= mi.mod->ins) return "";
    return mi.mod->xxi[i].name;
}

/* Sample `i`: its name, data (8-bit signed or 16-bit native, maybe
 * interleaved stereo, by `flags`), length and loop in frames. Returns 0 if
 * there is no such sample or it holds no data. */
int cxmp_sample(xmp_context c, int i, const char **name, const void **data, int *len,
                int *lps, int *lpe, int *flags) {
    struct xmp_module_info mi;
    struct xmp_sample *s;
    xmp_get_module_info(c, &mi);
    if (i < 0 || i >= mi.mod->smp) return 0;
    s = &mi.mod->xxs[i];
    *name = s->name;
    *data = s->data;
    *len = s->len;
    *lps = s->lps;
    *lpe = s->lpe;
    *flags = s->flg;
    return s->data != NULL && s->len > 0 && !(s->flg & XMP_SAMPLE_SYNTH);
}
