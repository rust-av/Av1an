/* Test-only ABI sinks. Return 42 only when the real drivers deliver the
 * independently declared metadata (and fmetrics' converted RGB samples).
 * These are not implementations of the metrics. */
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>

typedef struct {
    const uint8_t *data;
    uint32_t width, height, stride;
    int format, colorspace;
    bool hdr;
} Image;

const char *fmetrics_version_str(void) { return "0.1.0"; }
const char *fmetrics_cvvdp_version_str(void) { return "m04-test"; }
const char *fmetrics_error_str(int error) { return "M04 native image mismatch"; }
void *fmetrics_workspace_create(void) { return malloc(1); }
void fmetrics_workspace_destroy(void *workspace) { free(workspace); }
int fmetrics_ssimu2_cmp(void *workspace, const Image *reference, const Image *distorted, double *score) {
    if (reference->width == 34 && distorted->width == 34) {
        const uint8_t native[] = {15, 15, 15};
        if (reference->height != 32 || reference->stride != 102 ||
            distorted->height != 32 || distorted->stride != 102 ||
            reference->colorspace != 1 || distorted->colorspace != 1 ||
            reference->format != 1 || distorted->format != 1 ||
            reference->hdr || distorted->hdr ||
            memcmp(reference->data, native, 3) || memcmp(distorted->data, native, 3)) return 2;
        *score = 42;
        return 0;
    }
    if (reference->width != 32 || reference->height != 32 || reference->stride != 96 ||
        distorted->width != 32 || distorted->height != 32 || distorted->stride != 96) return 2;
    const uint8_t sdr[] = {4, 4, 4};
    if (reference->colorspace == 1 && distorted->colorspace == 1 &&
        reference->format == 1 && distorted->format == 1 &&
        !reference->hdr && !distorted->hdr &&
        !memcmp(reference->data, sdr, 3) && !memcmp(distorted->data, sdr, 3)) {
        *score = 42;
        return 0;
    }
    const uint8_t a[] = {145, 73, 121}, b[] = {152, 75, 124};
    if (reference->colorspace != 2 || distorted->colorspace != 1 ||
        reference->format != 1 || distorted->format != 1 ||
        reference->hdr || distorted->hdr ||
        memcmp(reference->data, a, 3) || memcmp(distorted->data, b, 3)) return 2;
    *score = 42;
    return 0;
}
int fmetrics_iwssim_cmp(void *w, const Image *a, const Image *b, double *s) { return 2; }
int fmetrics_msssim_cmp(void *w, const Image *a, const Image *b, double *s) { return 2; }
int fmetrics_ssimu2_cmp_map(void *w, const Image *a, const Image *b, double *s, uint32_t *m) { return 2; }
int fmetrics_butteraugli_cmp(void *w, const Image *a, const Image *b, const void *o, double *s) { return 2; }
int fmetrics_butteraugli_cmp_map(void *w, const Image *a, const Image *b, const void *o, double *s, uint32_t *m) { return 2; }
int fmetrics_cvvdp_create(int w, int h, float fps, int model, unsigned threads, const void *params, void **ctx) { *ctx = malloc(1); return 0; }
void fmetrics_cvvdp_destroy(void *ctx) { free(ctx); }
int fmetrics_cvvdp_reset(void *ctx) { return 0; }
int fmetrics_cvvdp_process_frame(void *ctx, const Image *a, const Image *b, double *result) {
    int status = fmetrics_ssimu2_cmp(ctx, a, b, result);
    result[1] = result[0];
    return status;
}
int fmetrics_cvvdp_cmp(const Image *a, const Image *b, int model, unsigned threads, const void *params, void *result) { return 2; }
int fmetrics_cvvdp_get_display_params(int model, void *params) { return 2; }

typedef struct {
    int64_t width, height, target_width, target_height;
    int sample, range, subw, subh, chroma_location, family, matrix, transfer, primaries;
    int crop[4];
} Colorspace;
typedef struct { int type; Colorspace reference, distorted; int gpu; } Init;
typedef struct { int type; double score; } Score;
typedef struct { int major, minor, patch, backend; } Version;
Version Vship_GetVersion(void) { return (Version){5, 1, 2, 3}; }
int Vship_GetDeviceCount(int *count) { *count = 1; return 0; }
int Vship_GetDeviceInfo(void *info, int gpu) { return 0; }
int Vship_GPUFullCheck(int gpu) { return 0; }
int Vship_GetErrorMessage(int error, char *buffer, int len) { return 0; }
int Vship_GetDetailedLastError(char *buffer, int len) { return 0; }
int Vship_GetDetailedLastErrorHandler(void *handler, char *buffer, int len) { return 0; }
int Vship_InitHandler(void **handler, void *data) {
    const Init *init = data;
    if (getenv("M07_ABI_LOG")) {
        *handler = malloc(sizeof(Init));
        memcpy(*handler, init, sizeof(Init));
        return 0;
    }
    const Colorspace *a = &init->reference, *b = &init->distorted;
    if (init->type != 1 || a->range != 1 || a->matrix != 6 ||
        a->transfer != 13 || a->primaries != 1 || a->chroma_location != 1 ||
        b->range != 0 || b->matrix != 9 || b->transfer != 16 ||
        b->primaries != 9 || b->chroma_location != 2) return 7;
    *handler = malloc(1);
    return 0;
}
int Vship_FreeHandler(void *handler) { free(handler); return 0; }
int Vship_ComputeHandler(void *handler, void *data, const uint8_t **a, const uint8_t **b, const int64_t *as, const int64_t *bs) {
    const char *log = getenv("M07_ABI_LOG");
    if (log) {
        const Init *init = handler;
        const Colorspace spaces[] = {init->reference, init->distorted};
        FILE *file = fopen(log, "a");
        if (!file) return 7;
        fprintf(file, "%lld %lld %d %lld %lld %lld %lld %lld %lld\n",
            (long long)spaces[0].width, (long long)spaces[0].height, spaces[0].sample,
            (long long)as[0], (long long)as[1], (long long)as[2],
            (long long)bs[0], (long long)bs[1], (long long)bs[2]);
        fclose(file);
        for (int side = 0; side < 2; side++) {
            const Colorspace *space = &spaces[side];
            const uint8_t **planes = side ? b : a;
            const int64_t *strides = side ? bs : as;
            int bytes = space->sample == 2 ? 1 : 2;
            for (int plane = 0; plane < 3; plane++) {
                int64_t width = plane ? ((space->width - 1) >> space->subw) + 1 : space->width;
                int64_t height = plane ? ((space->height - 1) >> space->subh) + 1 : space->height;
                size_t prefix = (height - 1) * strides[plane] + width * bytes;
                /* Copy exactly the production native prefix, NOT height*stride.
                 * memcpy preserves raw padding; this sink proves transport, not GPU accuracy. */
                uint8_t *copy = malloc(prefix);
                memcpy(copy, planes[plane], prefix);
                uint16_t first = 0, last = 0;
                memcpy(&first, copy, bytes);
                memcpy(&last, copy + prefix - bytes, bytes);
                free(copy);
                const uint16_t expected[] = {100, 140, 160};
                if (first != expected[plane] || last != expected[plane]) return 7;
            }
        }
    }
    Score *score = data;
    score->score = 42;
    return 0;
}
int Vship_Reset(void *handler) {
    const char *log = getenv("M07_ABI_LOG");
    if (log) { FILE *file = fopen(log, "a"); if (!file) return 7; fputs("reset\n", file); fclose(file); }
    return 0;
}
int Vship_ResetScore(void *handler) { return 0; }
