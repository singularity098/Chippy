/* Chippy's C API over vio2sf: a Nintendo DS set up from a 2SF's ROM image
 * and save state, rendering 44.1 kHz stereo. */
#include <stdlib.h>
#include <string.h>
#include "state.h"

NDS_state *twosf_open(const u8 *rom, u32 rom_size, const u8 *state, u32 state_size,
                      int initial_frames, int sync_type, int arm7_clockdown,
                      int arm9_clockdown, int interpolation) {
    NDS_state *s = calloc(1, sizeof *s);
    if (!s) return NULL;
    if (state_init(s)) {
        free(s);
        return NULL;
    }
    s->dwInterpolation = interpolation;
    s->dwChannelMute = 0;
    s->initial_frames = initial_frames;
    s->sync_type = sync_type;
    s->arm7_clockdown_level = arm7_clockdown;
    s->arm9_clockdown_level = arm9_clockdown;
    if (rom && rom_size)
        state_setrom(s, (u8 *)rom, rom_size, 0);
    state_loadstate(s, state, state_size);
    return s;
}

void twosf_render(NDS_state *s, s16 *out, unsigned frames) { state_render(s, out, frames); }

void twosf_close(NDS_state *s) {
    if (!s) return;
    state_deinit(s);
    free(s);
}
