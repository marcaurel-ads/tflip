/* Force-included (-include) into tflip's probes when probes.sh builds them
 * for x86_64 to run against another client engine.
 *
 * - stdout is unbuffered, so a probe that crashes still leaves what it
 *   printed up to the crash.
 * - GetProcAddress of an Ads* function the DLL does not export returns a
 *   stub that answers 99999, a code no engine gives (ace.h has no "function
 *   not available"), instead of NULL, so the probe goes on past the call
 *   instead of jumping to address 0. On x64 the caller cleans the stack,
 *   so a stub without parameters is safe whatever the signature. Its
 *   output parameters stay as the probe set them. probes.sh lists the
 *   missing functions on its own. */
#ifndef PROBE64_H
#define PROBE64_H
#include <windows.h>
#include <stdio.h>
#include <string.h>

static unsigned long probe64_missing(void) { return 99999; }

static FARPROC probe64_get_proc(HMODULE module, LPCSTR name) {
    FARPROC proc = GetProcAddress(module, name);
    if (!proc && HIWORD((ULONG_PTR)name) && strncmp(name, "Ads", 3) == 0)
        return (FARPROC)probe64_missing;
    return proc;
}

__attribute__((constructor)) static void probe64_unbuffered(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
}

#define GetProcAddress probe64_get_proc
#endif
