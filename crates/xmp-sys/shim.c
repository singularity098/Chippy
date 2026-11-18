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
