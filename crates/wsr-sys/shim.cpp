// Chippy's C API over the WSR player built on Mednafen's WonderSwan core.
// The core is a single global machine: one song at a time.
#include <cstdint>
#include "wsr_player_common/wsr_player_intf.h"

namespace rePlayer::mednafen { extern WSRPlayerApi g_wsr_player_api; }
using rePlayer::mednafen::g_wsr_player_api;

extern "C" {

int wsr_open(const uint8_t *data, unsigned size, unsigned rate) {
    if (!g_wsr_player_api.pLoadWSR(data, size)) return -1;
    g_wsr_player_api.pSetFrequency(rate);
    return 0;
}

int wsr_first_song(void) { return g_wsr_player_api.pGetFirstSong(); }

void wsr_reset(unsigned song) { g_wsr_player_api.pResetWSR(song); }

void wsr_render(int16_t *out, unsigned frames) {
    g_wsr_player_api.pUpdateWSR(out, frames * 4, frames);
}

void wsr_close(void) { g_wsr_player_api.pCloseWSR(); }

}
