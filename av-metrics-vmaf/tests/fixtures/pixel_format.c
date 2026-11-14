/* Reuse audited tombstone owners. No failed fetch output becomes Rust-owned. */
#define vmaf_fetch_preallocated_picture pixel_format_fetch
#define vmaf_picture_unref pixel_format_unref
#include "setup.c"
#undef vmaf_fetch_preallocated_picture
#undef vmaf_picture_unref
static unsigned invalid_returns;
unsigned pixel_format_invalid_returns(void) { return invalid_returns; }
int vmaf_fetch_preallocated_picture(Context *ctx, Picture *out) {
    int status = pixel_format_fetch(ctx, out);
    if (ctx->fetch == setting("VMAF_PIXEL_FORMAT_FIXTURE_FETCH")) out->pix_fmt = atoi(getenv("VMAF_PIXEL_FORMAT_FIXTURE_PIXFMT"));
    return status;
}
int vmaf_picture_unref(Picture *p) {
    if (p->pix_fmt != 1) {
        Record *r = p->ref;
        invalid_returns++;
        /* Reject malformed metadata before any copy or native submission. */
        for (unsigned i=0; i<3; i++) for (unsigned j=0; j<64; j++) bad_if(r->bytes[i][j] != 0);
    }
    return pixel_format_unref(p);
}
