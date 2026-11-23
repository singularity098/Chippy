// Chippy's C API over the WSR player built on Mednafen's WonderSwan core.
// The core is a single global machine: one song at a time. build.rs builds
// it twice, the second time (for finding notes) with its namespaces renamed
// and these functions called wsrn_* instead of wsr_*.
// Mednafen's types.h must come before any standard header (it errors if
// NULL, EOF etc. are already defined, as the NDK's <cstdint> does).
#include "wswan/wswan.h"
#include "wswan/sound.h"
#include "wsr_player_common/wsr_player_intf.h"
#include <cstdint>

#ifndef WSR_FN
#define WSR_FN(name) wsr_##name
#endif

namespace rePlayer::mednafen { extern WSRPlayerApi g_wsr_player_api; }
using rePlayer::mednafen::g_wsr_player_api;

extern "C" {

int WSR_FN(open)(const uint8_t *data, unsigned size, unsigned rate) {
    if (!g_wsr_player_api.pLoadWSR(data, size)) return -1;
    g_wsr_player_api.pSetFrequency(rate);
    return 0;
}

int WSR_FN(first_song)(void) { return g_wsr_player_api.pGetFirstSong(); }

void WSR_FN(reset)(unsigned song) { g_wsr_player_api.pResetWSR(song); }

void WSR_FN(render)(int16_t *out, unsigned frames) {
    g_wsr_player_api.pUpdateWSR(out, frames * 4, frames);
}

void WSR_FN(close)(void) { g_wsr_player_api.pCloseWSR(); }

void WSR_FN(set_hook)(MDFN_IEN_WSWAN::WSwan_WriteHookFn f, void *user) {
    MDFN_IEN_WSWAN::WSwan_WriteHook = f;
    MDFN_IEN_WSWAN::WSwan_WriteHookUser = user;
}

}
