#include "ao_notes.h"

#if defined(_MSC_VER)
#define AO_TLS __declspec(thread)
#else
#define AO_TLS _Thread_local
#endif

static AO_TLS ao_notes_fn hook;
static AO_TLS void *hook_user;
static AO_TLS uint64_t samples;

void ao_notes_set_hook(ao_notes_fn f, void *user)
{
	hook = f;
	hook_user = user;
	samples = 0;
}

void ao_notes_tick(void)
{
	if (hook)
		samples++;
}

void ao_notes_write(int kind, uint32_t reg, uint32_t val, const uint8_t *ram, uint32_t ram_size)
{
	if (hook)
		hook(hook_user, kind, reg, val, ram, ram_size, samples);
}
