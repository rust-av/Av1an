/* Stateful ABI markers, NOT CVVDP math. The pinned headers check signatures. */
#define VSHIP_STATIC
#include "../../../av-metrics-fmetrics/tests/headers/fmetrics.h"
#include "../../../av-metrics-vship/tests/headers/VshipAPI.h"
#include <stdio.h>
#include <string.h>
#include <math.h>

typedef struct { int history; double sum; } State;
static void event(const char *name) {
    FILE *f = fopen(getenv("M15_LOG"), "a");
    if (!f) abort();
    fprintf(f, "%s\n", name);
    fclose(f);
}
static void audit(const char *name) {
    const char *s = getenv("M16_FPS_AUDIT");
    if (s && *s) event(name);
}
static void fps_event(const char *name, float fps) {
    uint32_t bits;
    char line[40];
    memcpy(&bits, &fps, sizeof(bits));
    snprintf(line, sizeof(line), "%s=%08x", name, (unsigned)bits);
    audit(line);
}
static int fail(const char *name) {
    const char *s = getenv("M15_FAIL");
    return s && !strcmp(s, name);
}
static double marker(State *s, int sample) {
    if (fail("q05-nan")) return NAN;
    if (fail("q05-inf")) return INFINITY;
    if (fail("q05-neginf")) return -INFINITY;
    if (fail("q05-last") && sample >= 470) return NAN;
    double value = sample + 1000 * s->history + 10 * s->sum;
    s->history = sample;
    s->sum += sample;
    return value;
}
static int pair_sample(int a, int b) {
    const char *scope = getenv("M15_SCOPE");
    return scope && !strncmp(scope, "m27a", 4) ? a + 3 * b : a;
}
const char *fmetrics_version_str(void) { audit("fv"); return "0.1.0"; }
const char *fmetrics_cvvdp_version_str(void) { return "M15-boundary-not-CVVDP"; }
const char *fmetrics_error_str(FmetricsErr e) { (void)e; return "M15 combined reset failure"; }
FmetricsWorkspace *fmetrics_workspace_create(void) { audit("fw+"); return (FmetricsWorkspace *)calloc(1, sizeof(State)); }
void fmetrics_workspace_destroy(FmetricsWorkspace *p) { free(p); }
FmetricsErr fmetrics_ssimu2_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) {
    (void)w; (void)a; (void)b;
    *s = fail("q05-nan") ? NAN : fail("q05-inf") ? INFINITY : fail("q05-neginf") ? -INFINITY : 42;
    return FMETRICS_OK;
}
FmetricsErr fmetrics_iwssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_msssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_ssimu2_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s, uint32_t *m) { (void)m; return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_butteraugli_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s) { (void)o; return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_butteraugli_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s, uint32_t *m) { (void)m; return fmetrics_butteraugli_cmp(w,a,b,o,s); }
FmetricsErr fmetrics_cvvdp_create(int w, int h, float fps, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpCtx **out) {
    (void)w; (void)h; (void)m; (void)t; (void)p; fps_event("ffps", fps);
    event("fc+"); *out = (FmetricsCvvdpCtx *)calloc(1, sizeof(State)); return FMETRICS_OK;
}
void fmetrics_cvvdp_destroy(FmetricsCvvdpCtx *p) { event("fc-"); free(p); }
FmetricsErr fmetrics_cvvdp_process_frame(FmetricsCvvdpCtx *c, const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpResult *r) {
    event("fp"); if (fail("fp") || (fail("fp2") && ((State *)c)->sum > 0)) return FMETRICS_ERR_INTERNAL;
    r->jod = marker((State *)c, pair_sample(*(const uint8_t *)a->data, *(const uint8_t *)b->data)); r->quality = r->jod + 0.5; return FMETRICS_OK;
}
FmetricsErr fmetrics_cvvdp_reset(FmetricsCvvdpCtx *c) {
    event("fr"); if (fail("fr")) return FMETRICS_ERR_INTERNAL;
    memset(c, 0, sizeof(State)); return FMETRICS_OK;
}
FmetricsErr fmetrics_cvvdp_cmp(const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpResult *r) { (void)a; (void)b; (void)m; (void)t; (void)p; (void)r; return FMETRICS_OK; }
FmetricsErr fmetrics_cvvdp_get_display_params(FmetricsCvvdpDisplayModel m, FmetricsCvvdpDisplayParams *p) { (void)m; (void)p; return FMETRICS_OK; }

Vship_Version Vship_GetVersion(void) { audit("vv"); return (Vship_Version){5,1,2,Vship_CPU}; }
Vship_Exception Vship_GetDeviceCount(int *n) { audit("vd"); *n=1; return Vship_NoError; }
Vship_Exception Vship_GetDeviceInfo(Vship_DeviceInfo *info, int gpu) { audit("vi"); (void)gpu; memset(info,0,sizeof(*info)); return Vship_NoError; }
Vship_Exception Vship_GPUFullCheck(int gpu) { audit("vg"); (void)gpu; return Vship_NoError; }
int Vship_GetErrorMessage(Vship_Exception e, char *out, int len) { (void)e; return snprintf(out,(size_t)len,"M15 reset failure"); }
int Vship_GetDetailedLastError(char *out, int len) { (void)out; (void)len; return 0; }
int Vship_GetDetailedLastErrorHandler(Vship_Handle h, char *out, int len) { (void)h; (void)out; (void)len; return 0; }
Vship_Exception Vship_InitHandler(Vship_Handle *h, void *arg) {
    Vship_StructType type = *(Vship_StructType *)arg;
    if (type == Vship_StructInitCVVDP_1)
        fps_event("vfps", ((Vship_InitCVVDP_1 *)arg)->fps);
    else if (type != Vship_StructInitSSIMULACRA2_1 && type != Vship_StructInitButteraugli_1)
        abort();
    event("vc+"); *h=calloc(1,sizeof(State)); return Vship_NoError;
}
Vship_Exception Vship_FreeHandler(Vship_Handle h) { event("vc-"); free(h); return Vship_NoError; }
Vship_Exception Vship_ComputeHandler(Vship_Handle h, void *out, const uint8_t *a[3], const uint8_t *b[3], const int64_t as[3], const int64_t bs[3]) {
    (void)as; (void)bs; event("vp"); if (fail("vp") || (fail("vp2") && ((State *)h)->sum > 0)) return (Vship_Exception)1;
    Vship_ScoreCVVDP *s=out; s->structType=Vship_StructScoreCVVDP; s->score=marker(h,pair_sample(*a[0],*b[0])); return Vship_NoError;
}
Vship_Exception Vship_Reset(Vship_Handle h) {
    event("vr"); if (fail("vr")) return (Vship_Exception)1;
    ((State *)h)->history=0; return Vship_NoError;
}
Vship_Exception Vship_ResetScore(Vship_Handle h) {
    event("vs"); if (fail("vs")) return (Vship_Exception)1;
    ((State *)h)->sum=0; return Vship_NoError;
}
Vship_Exception Vship_PinnedMalloc2(void **p, uint64_t size, int gpu) { audit("vu+"); (void)gpu; *p=malloc((size_t)size); return Vship_NoError; }
Vship_Exception Vship_PinnedFree2(void *p, int gpu) { (void)gpu; free(p); return Vship_NoError; }
