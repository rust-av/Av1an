/* Pinned safe picture sink; model markers test selection, not VMAF accuracy. */
#define vmaf_version ownership_version
#define vmaf_init ownership_init
#define vmaf_close ownership_close
#define vmaf_model_load ownership_model_load
#define vmaf_model_load_from_path ownership_model_load_from_path
#define vmaf_model_destroy ownership_model_destroy
#include "ownership.c"
#undef vmaf_version
#undef vmaf_init
#undef vmaf_close
#undef vmaf_model_load
#undef vmaf_model_load_from_path
#undef vmaf_model_destroy

/* version, init, builtin, path, model allocation, model destroy, close, marker */
static unsigned calls[8];
static char selected_path[4096];
unsigned model_fixture_count(unsigned index) { return calls[index]; }
const char *model_fixture_path(void) { return selected_path; }
const char *vmaf_version(void) { calls[0]++; return "3.2.0"; }
int vmaf_init(Context **out, Configuration cfg) {
    calls[1]++;
    return ownership_init(out, cfg);
}
int vmaf_close(Context *ctx) { calls[6]++; return ownership_close(ctx); }
static int allocate_model(void **out) {
    *out = malloc(1);
    if (!*out) return -12;
    calls[4]++;
    return 0;
}
int vmaf_model_load(void **out, void *cfg, const char *name) {
    (void)cfg; (void)name;
    calls[2]++;
    if (setting("VMAF_MODEL_FIXTURE_NO_BUILTIN")) return -22;
    return allocate_model(out);
}
int vmaf_model_load_from_path(void **out, void *cfg, const char *path) {
    (void)cfg;
    calls[3]++;
    if (snprintf(selected_path, sizeof(selected_path), "%s", path) >= (int)sizeof(selected_path))
        abort();
    FILE *file = fopen(path, "r");
    if (!file) return -2;
    int marker = fgetc(file);
    fclose(file);
    calls[7] = (unsigned)marker;
    int status = allocate_model(out);
    if (status) return status;
    /* Like the pinned JSON loader, failure leaves an owned initialized partial. */
    return marker == '!' ? -87 : 0;
}
void vmaf_model_destroy(void *handle) { free(handle); calls[5]++; }
