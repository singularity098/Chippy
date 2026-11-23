/* Chippy: a hook for the sound chips' register writes, for finding a
   song's notes. Thread-local: only the thread that installed it sees
   writes. */
#ifndef AO_NOTES_H
#define AO_NOTES_H

#include <stdint.h>

/* kind 1: the PlayStation SPU (reg & 0xfff), 2: the PS2's SPU2
   (reg & 0xffff); with the SPU's RAM, and the output samples generated
   before the write. */
typedef void (*ao_notes_fn)(void *user, int kind, uint32_t reg, uint32_t val,
                            const uint8_t *ram, uint32_t ram_size, uint64_t sample);

void ao_notes_set_hook(ao_notes_fn f, void *user);
/* An output sample was generated. */
void ao_notes_tick(void);
void ao_notes_write(int kind, uint32_t reg, uint32_t val, const uint8_t *ram, uint32_t ram_size);

#endif
