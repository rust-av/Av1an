/* ABI double, not a metric. Public signatures are checked by the real header. */
#include "../headers/fmetrics.h"
#include <stddef.h>
#include <stdio.h>
#include <string.h>

static unsigned workspaces, destroyed, butteraugli_calls, version_probes;
static int status(void) { const char *s = getenv("FMETRICS_BOUNDARY_FIXTURE_STATUS"); return s ? atoi(s) : 0; }
unsigned boundary_count(unsigned i) { return i == 3 ? version_probes : i == 2 ? butteraugli_calls : i ? destroyed : workspaces; }
static void result_fixture_log(const char *event) {
    const char *path = getenv("FMETRICS_RESULT_FIXTURE_ABI_LOG");
    if (path) { FILE *f = fopen(path, "a"); if (!f) abort(); fputs(event, f); fclose(f); }
}
const char *fmetrics_version_str(void) {
    version_probes++;
    const char *s = getenv("FMETRICS_BOUNDARY_FIXTURE_VERSION");
    if (!s) return "0.1.0";
    if (!strcmp(s, "null")) return NULL;
    if (!strcmp(s, "non-UTF8")) return "\xff";
    return s;
}
const char *fmetrics_cvvdp_version_str(void) { return "boundary-fixture"; }
const char *fmetrics_error_str(FmetricsErr err) {
    if ((unsigned)err > 6) abort(); /* Rust must not interpret unknown via C. */
    return "known fmetrics status";
}
FmetricsWorkspace *fmetrics_workspace_create(void) { workspaces++; result_fixture_log("create\n"); return (FmetricsWorkspace *)malloc(1); }
void fmetrics_workspace_destroy(FmetricsWorkspace *w) { destroyed++; result_fixture_log("destroy\n"); free(w); }
FmetricsErr fmetrics_ssimu2_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) {
    if (!w || !a->data || !b->data || a->format != FMETRICS_PIX_FMT_RGB_UINT8) abort();
    *s = 42;
    return (FmetricsErr)status();
}
FmetricsErr fmetrics_iwssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_msssim_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s) { return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_ssimu2_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, double *s, uint32_t *m) { (void)m; return fmetrics_ssimu2_cmp(w,a,b,s); }
FmetricsErr fmetrics_butteraugli_cmp(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s) {
    FmetricsErr result = fmetrics_ssimu2_cmp(w,a,b,s);
    if (!o) abort();
    butteraugli_calls++;
    char event[128];
    snprintf(event, sizeof(event), "score %d %.0f\n", o->pnorm, (double)o->intensity_target);
    result_fixture_log(event);
    const char *path = getenv("FMETRICS_RESULT_FIXTURE_RESULT_FILE");
    if (path) {
        FILE *f = fopen(path, "r"); if (!f || fscanf(f, "%lf", s) != 1) abort();
        int raw_status;
        if (fscanf(f, "%d", &raw_status) == 1) result = (FmetricsErr)raw_status;
        fclose(f);
    }
    return result;
}
FmetricsErr fmetrics_butteraugli_cmp_map(FmetricsWorkspace *w, const FmetricsImg *a, const FmetricsImg *b, const FmetricsButteraugliOptions *o, double *s, uint32_t *m) { (void)m; return fmetrics_butteraugli_cmp(w,a,b,o,s); }
FmetricsErr fmetrics_cvvdp_create(int w, int h, float fps, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpCtx **out) {
    (void)w; (void)h; (void)fps; (void)m; (void)t; (void)p;
    *out = (FmetricsCvvdpCtx *)malloc(1); return FMETRICS_OK;
}
void fmetrics_cvvdp_destroy(FmetricsCvvdpCtx *p) { free(p); }
FmetricsErr fmetrics_cvvdp_process_frame(FmetricsCvvdpCtx *c, const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpResult *r) { (void)c; (void)a; (void)b; (void)r; return (FmetricsErr)status(); }
FmetricsErr fmetrics_cvvdp_reset(FmetricsCvvdpCtx *c) { (void)c; return (FmetricsErr)status(); }
FmetricsErr fmetrics_cvvdp_cmp(const FmetricsImg *a, const FmetricsImg *b, FmetricsCvvdpDisplayModel m, unsigned t, const FmetricsCvvdpDisplayParams *p, FmetricsCvvdpResult *r) { (void)a; (void)b; (void)m; (void)t; (void)p; (void)r; return (FmetricsErr)status(); }
FmetricsErr fmetrics_cvvdp_get_display_params(FmetricsCvvdpDisplayModel m, FmetricsCvvdpDisplayParams *p) { (void)m; (void)p; return (FmetricsErr)status(); }

#ifdef FMETRICS_BOUNDARY_FIXTURE_PROBE
#define TYPE(T) printf("%zu %zu ", sizeof(T), _Alignof(T))
#define FIELD(T,F) printf("%zu %zu ", offsetof(T,F), sizeof(((T*)0)->F))
int main(void) {
    TYPE(FmetricsErr); TYPE(FmetricsPixFmt); TYPE(FmetricsColorspace); TYPE(FmetricsCvvdpDisplayModel);
    TYPE(FmetricsImg);
    FIELD(FmetricsImg,data); FIELD(FmetricsImg,width); FIELD(FmetricsImg,height); FIELD(FmetricsImg,stride);
    FIELD(FmetricsImg,format); FIELD(FmetricsImg,colorspace); FIELD(FmetricsImg,hdr);
    TYPE(FmetricsButteraugliOptions); FIELD(FmetricsButteraugliOptions,intensity_target); FIELD(FmetricsButteraugliOptions,pnorm);
    TYPE(FmetricsCvvdpDisplayParams);
    FIELD(FmetricsCvvdpDisplayParams,resolution_width); FIELD(FmetricsCvvdpDisplayParams,resolution_height);
    FIELD(FmetricsCvvdpDisplayParams,viewing_distance_meters); FIELD(FmetricsCvvdpDisplayParams,diagonal_size_inches);
    FIELD(FmetricsCvvdpDisplayParams,max_luminance); FIELD(FmetricsCvvdpDisplayParams,contrast);
    FIELD(FmetricsCvvdpDisplayParams,ambient_light); FIELD(FmetricsCvvdpDisplayParams,reflectivity); FIELD(FmetricsCvvdpDisplayParams,is_hdr);
    TYPE(FmetricsCvvdpResult); FIELD(FmetricsCvvdpResult,jod); FIELD(FmetricsCvvdpResult,quality);
    printf("%d %d %d %d %d %d %d %d %d %d %d %d %d %d\n", FMETRICS_OK, FMETRICS_ERR_INVALID_ARGUMENT,
        FMETRICS_ERR_UNSUPPORTED_FORMAT, FMETRICS_ERR_DIMENSION_MISMATCH, FMETRICS_ERR_OUT_OF_MEMORY,
        FMETRICS_ERR_IWSSIM_IMG_TOO_SMALL, FMETRICS_ERR_INTERNAL, FMETRICS_PIX_FMT_RGB_UINT8,
        FMETRICS_PIX_FMT_RGB_FLOAT, FMETRICS_PIX_FMT_RGB_UINT16, FMETRICS_COLORSPACE_SRGB,
        FMETRICS_COLORSPACE_LINEAR_SRGB, FMETRICS_CVVDP_DISPLAY_STANDARD_FHD, FMETRICS_CVVDP_DISPLAY_CUSTOM);
    return 0;
}
#endif
