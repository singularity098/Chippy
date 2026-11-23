// The other end of config.h's hooks: forwards register writes to whatever
// callback the current thread installed (none while just playing).
#include "config.h"

extern "C" {
typedef void (*chippy_gme_hook_fn)(void* user, int kind, long time, int addr, int data);
}

namespace {
thread_local chippy_gme_hook_fn hook = nullptr;
thread_local void* hook_user = nullptr;
}

extern "C" void chippy_gme_set_hook(chippy_gme_hook_fn f, void* user)
{
	hook = f;
	hook_user = user;
}

extern "C" void chippy_gme_apu_write(long time, int addr, int data)
{
	if (hook)
		hook(hook_user, 0, time, addr, data);
}

extern "C" void chippy_gme_block(long clocks, long rate)
{
	if (hook)
		hook(hook_user, 4, clocks, (int)rate, 0);
}

extern "C" void chippy_gme_frame(void)
{
	if (hook)
		hook(hook_user, 1, 0, 0, 0);
}

extern "C" void chippy_gme_spc_dsp_write(long time, int addr, int data)
{
	if (hook)
		hook(hook_user, 2, time, addr, data);
}

extern "C" void chippy_gme_other_write(long time, int addr, int data)
{
	if (hook)
		hook(hook_user, 3, time, addr, data);
}
