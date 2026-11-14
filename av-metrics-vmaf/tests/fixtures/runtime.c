/* No enlarged picture writes: rejection is observed through counted calls. */
#define vmaf_version ownership_version
#define vmaf_init ownership_init
#define vmaf_preallocate_pictures ownership_preallocate
#define vmaf_fetch_preallocated_picture ownership_fetch
#include "ownership.c"
#undef vmaf_version
#undef vmaf_init
#undef vmaf_preallocate_pictures
#undef vmaf_fetch_preallocated_picture

static unsigned calls[3];
unsigned runtime_fixture_count(unsigned index) { return calls[index]; }
const char *vmaf_version(void) {
    const char *version = getenv("VMAF_RUNTIME_FIXTURE_VERSION");
    if (!strcmp(version, "null")) return NULL;
    if (!strcmp(version, "non-UTF8")) return "\xff";
    return version;
}
int vmaf_init(Context **out, Configuration cfg) {
    calls[0]++;
    return ownership_init(out, cfg);
}
int vmaf_preallocate_pictures(Context *ctx, PoolConfiguration cfg) {
    calls[1]++;
    return ownership_preallocate(ctx, cfg);
}
int vmaf_fetch_preallocated_picture(Context *ctx, Picture *out) {
    calls[2]++;
    return ownership_fetch(ctx, out);
}
/* Presence only: the loader must NEVER call these unverified entry points. */
#if VMAF_RUNTIME_FIXTURE_MARKER == 1
void vmaf_picture_convert_context_init(void) { abort(); }
#elif VMAF_RUNTIME_FIXTURE_MARKER == 2
void vmaf_picture_convert(void) { abort(); }
#elif VMAF_RUNTIME_FIXTURE_MARKER == 3
void vmaf_picture_convert_context_close(void) { abort(); }
#elif VMAF_RUNTIME_FIXTURE_MARKER == 4
void vmaf_picture_wrap(void) { abort(); }
#endif
