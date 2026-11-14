/* Reuse only the unrelated CPU/model exports; M09's fixture stays unchanged. */
#define vmaf_init ownership_init
#define vmaf_close ownership_close
#define vmaf_preallocate_pictures ownership_preallocate_pictures
#include "ownership.c"
#undef vmaf_init
#undef vmaf_close
#undef vmaf_preallocate_pictures

typedef struct { unsigned resource; } CudaState;
typedef struct { void *cu_ctx; } CudaConfiguration;
typedef struct { Geometry pic_params; int method; } CudaPictures;
typedef struct { CudaState state; } CudaContext;
enum {
    STATE_INIT, WRAPPER_ALLOC, WRAPPER_FREE, RELEASE_CALL, RESOURCE_RELEASE,
    CONTEXT_RELEASE, IMPORT, PREALLOCATE, CONTEXT_CLOSE, INIT, CPU_POOL,
    DUPLICATE_RELEASE, BAD_CALL, VMAF_CUDA_FIXTURE_COUNTERS
};
static unsigned cuda_fixture_counts[VMAF_CUDA_FIXTURE_COUNTERS];
/* Resource tombstones outlive both descriptors, so old Rust fails safely. */
static unsigned released;
static CudaState *wrapper;
static int wrapper_freed;

unsigned cuda_fixture_count(unsigned index) { return cuda_fixture_counts[index]; }
int vmaf_cuda_state_init(CudaState **out, CudaConfiguration cfg) {
    (void)cfg;
    cuda_fixture_counts[STATE_INIT]++;
    unsigned scenario = setting("VMAF_CUDA_FIXTURE_SCENARIO");
    if (scenario == 1 || scenario == 3) {
        *out = NULL;
        return scenario == 1 ? -81 : 0;
    }
    wrapper = *out = malloc(sizeof(**out));
    if (!wrapper) return -12;
    cuda_fixture_counts[WRAPPER_ALLOC]++;
    /* Early native errors leave a zeroed, allocated output, not a resource. */
    wrapper->resource = scenario == 2 || scenario == 15 ? 0 : 1;
    return scenario == 2 || scenario == 8 || scenario == 15 ? -81 : 0;
}
static void release_resource(CudaState *state) {
    if (!state->resource) return;
    cuda_fixture_counts[RESOURCE_RELEASE]++;
    if (released++) cuda_fixture_counts[DUPLICATE_RELEASE]++;
    state->resource = 0;
}
int vmaf_cuda_release(CudaState *state) {
    cuda_fixture_counts[RELEASE_CALL]++;
    release_resource(state);
    return 0;
}
void cuda_fixture_free(void *pointer) {
    if (wrapper_freed || pointer != wrapper) {
        cuda_fixture_counts[BAD_CALL]++;
        return;
    }
    wrapper_freed = 1;
    cuda_fixture_counts[WRAPPER_FREE]++;
    wrapper = NULL;
    free(pointer);
}
int vmaf_init(CudaContext **out, Configuration cfg) {
    (void)cfg;
    cuda_fixture_counts[INIT]++;
    unsigned scenario = setting("VMAF_CUDA_FIXTURE_SCENARIO");
    if (scenario == 4 || scenario == 5) {
        *out = NULL;
        return scenario == 4 ? -82 : 0;
    }
    *out = calloc(1, sizeof(**out));
    return *out ? 0 : -12;
}
int vmaf_cuda_import_state(CudaContext *ctx, CudaState *state) {
    cuda_fixture_counts[IMPORT]++;
    if (!ctx || !state) { cuda_fixture_counts[BAD_CALL]++; return -22; }
    if (setting("VMAF_CUDA_FIXTURE_SCENARIO") == 6) return -83;
    ctx->state = *state;
    return 0;
}
int vmaf_cuda_preallocate_pictures(CudaContext *ctx, CudaPictures cfg) {
    (void)cfg;
    cuda_fixture_counts[PREALLOCATE]++;
    if (!ctx || !ctx->state.resource) cuda_fixture_counts[BAD_CALL]++;
    return setting("VMAF_CUDA_FIXTURE_SCENARIO") == 7 ? -84 : 0;
}
int vmaf_close(CudaContext *ctx) {
    cuda_fixture_counts[CONTEXT_CLOSE]++;
    if (ctx->state.resource) {
        cuda_fixture_counts[CONTEXT_RELEASE]++;
        release_resource(&ctx->state);
    }
    free(ctx);
    return 0;
}
int vmaf_preallocate_pictures(CudaContext *ctx, PoolConfiguration cfg) {
    (void)ctx; (void)cfg;
    cuda_fixture_counts[CPU_POOL]++;
    return 0;
}
/* Called after the snapshot even on RED; not part of the measured ownership. */
void cuda_fixture_cleanup(void) {
    if (wrapper && !wrapper_freed) free(wrapper);
}
