// Picked up by libgme's blargg_config.h (built with HAVE_CONFIG_H): turns on
// the library's own hook points for sound-chip register writes, so Chippy
// can see the notes a tune plays. With no hook installed on the thread each
// one is a single thread-local load and branch.
#ifndef CHIPPY_GME_CONFIG_H
#define CHIPPY_GME_CONFIG_H

#ifdef __cplusplus
extern "C" {
#endif
void chippy_gme_apu_write(long time, int addr, int data);
void chippy_gme_block(long clocks, long rate);
void chippy_gme_frame(void);
void chippy_gme_spc_dsp_write(long time, int addr, int data);
void chippy_gme_other_write(long time, int addr, int data);
#ifdef __cplusplus
}
#endif

// NES, Game Boy, PC Engine, KSS, SAP and AY (patched in by build.rs; the
// Amstrad CPC's AY with bit 8 set): a write to the APU's registers
// (addr relative to the APU's first register).
#define GME_APU_HOOK(emu, addr, data) chippy_gme_apu_write(-1, (int)(addr), (int)(data))
// The same, with the time in the emulator's clocks from the start of the
// block of emulation it is in (patched in by build.rs where it is known).
#define GME_APU_HOOK_T(time, addr, data) chippy_gme_apu_write((long)(time), (int)(addr), (int)(data))
// A block of emulation ended: `clocks` long, at `rate` clocks a second.
#define GME_BLOCK_HOOK(clocks, rate) chippy_gme_block((long)(clocks), (long)(rate))
// The tune's play routine was called (once per driver tick).
#define GME_FRAME_HOOK(emu) chippy_gme_frame()
// SNES: a write to an S-DSP register, timed in SPC clocks (1.024 MHz) from
// the start of the current block of emulation.
#define SPC_DSP_WRITE_HOOK(time, addr, data) chippy_gme_spc_dsp_write((long)(time), (int)(addr), (int)(data))

// Added to libgme's sources by build.rs: writes to the sound chips libgme
// has no hook point for (KSS's SCC and OUT ports, NSF cartridge chips),
// timed as GME_APU_HOOK_T's.
#define GME_OTHER_HOOK(time, addr, data) chippy_gme_other_write((long)(time), (int)(addr), (int)(data))

#endif
