/* Pinned-header C ABI sink, not a GPU or a racy C++ lastError reproduction.
 * All cross-thread state is atomic, including controls and entry gates. */
#define VSHIP_STATIC
#include "../headers/VshipAPI.h"
#include <stdatomic.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* count, info, probe, init, compute, reset, reset_score, free, short, detail */
static _Atomic unsigned calls[10], active, overlaps, entered, released;
static _Atomic int errors[8], gate = -1;
static _Atomic unsigned next_id, freed, live;
typedef struct { int type; unsigned id; } Handle;

unsigned transaction_count(unsigned i) {
    if (i < 10) return calls[i];
    switch (i) {
        case 10: return overlaps;
        case 11: return entered;
        case 12: return live;
        case 13: return freed;
        default: abort();
    }
}
void transaction_set(unsigned i, int value) {
    if (i < 8) errors[i] = value;
    else if (i == 8) { entered = 0; released = 0; gate = value; }
    else if (i == 9) released = 1;
    else abort();
}
static void enter(unsigned kind) {
    if (atomic_fetch_add(&active, 1)) overlaps++;
    calls[kind]++;
    if (atomic_load(&gate) == (int)kind) {
        entered = 1;
        while (!atomic_load(&released)) sched_yield();
    }
}
static Vship_Exception leave(unsigned kind) {
    int status = kind < 8 ? atomic_load(&errors[kind]) : 0;
    active--;
    return (Vship_Exception)status;
}
Vship_Version Vship_GetVersion(void) { return (Vship_Version){5, 1, 2, Vship_CPU}; }
Vship_Exception Vship_GetDeviceCount(int *n) {
    enter(0); *n = 3; return leave(0);
}
Vship_Exception Vship_GetDeviceInfo(Vship_DeviceInfo *info, int gpu) {
    enter(1); memset(info, 0, sizeof(*info)); info->integrated = gpu != 1; return leave(1);
}
Vship_Exception Vship_GPUFullCheck(int gpu) {
    enter(2);
    Vship_Exception status = leave(2);
    return gpu < 0 || gpu >= 3 ? Vship_BadDeviceArgument : status;
}
int Vship_GetErrorMessage(Vship_Exception e, char *out, int len) {
    if ((unsigned)e > 17) abort(); /* M14: unknown integers must never reach C++ enum translation. */
    enter(8);
    int n = snprintf(out, (size_t)len, "short status %d", (int)e);
    leave(8); return n + 1;
}
int Vship_GetDetailedLastError(char *out, int len) {
    calls[9]++; return snprintf(out, (size_t)len, "detail only") + 1;
}
int Vship_GetDetailedLastErrorHandler(Vship_Handle h, char *out, int len) {
    (void)h; return Vship_GetDetailedLastError(out, len);
}
Vship_Exception Vship_InitHandler(Vship_Handle *out, void *args) {
    enter(3);
    if (atomic_load(&errors[3])) {
        *out = &errors[3]; /* Non-null, deliberately NOT an owned handle. */
    } else {
        Handle *h = malloc(sizeof(*h)); if (!h) abort();
        memcpy(&h->type, args, sizeof(int)); h->id = atomic_fetch_add(&next_id, 1);
        if (h->id >= 32) abort();
        live++; *out = h;
    }
    return leave(3);
}
Vship_Exception Vship_FreeHandler(Vship_Handle ptr) {
    enter(7); Handle *h = ptr;
    if (atomic_fetch_or(&freed, 1u << h->id) & (1u << h->id)) abort();
    live--; free(h); return leave(7);
}
Vship_Exception Vship_ComputeHandler(Vship_Handle ptr, void *out,
        const uint8_t *a[3], const uint8_t *b[3], const int64_t as[3], const int64_t bs[3]) {
    (void)a; (void)b; (void)as; (void)bs;
    enter(4); Handle *h = ptr;
    if (h->type == 1) ((Vship_ScoreSSIMULACRA2*)out)->score = 42;
    else if (h->type == 2) {
        Vship_ScoreButteraugli *s = out; s->normQ = 42; s->norm3 = 43; s->norminf = 44;
    } else ((Vship_ScoreCVVDP*)out)->score = 42;
    return leave(4);
}
Vship_Exception Vship_Reset(Vship_Handle h) { (void)h; enter(5); return leave(5); }
Vship_Exception Vship_ResetScore(Vship_Handle h) { (void)h; enter(6); return leave(6); }
