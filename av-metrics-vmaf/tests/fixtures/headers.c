/* Layout oracle: REAL public C headers, not a copy of the Rust declarations.
 * CUDA's public header falls back to the unmodified pinned upstream copy when
 * a CPU-only package omits it. See headers/README.md for immutable provenance. */
#include <stddef.h>
#include <stdio.h>
#include <libvmaf/picture.h>
#include <libvmaf/libvmaf.h>
#include <libvmaf/model.h>
#include <libvmaf/version.h>
#include <libvmaf/libvmaf_cuda.h>
#include "headers/cuda_release.h"

_Static_assert(VMAF_API_VERSION_MAJOR == 3 && VMAF_API_VERSION_MINOR == 2 &&
               VMAF_API_VERSION_PATCH == 0, "unverified header version");

/* Unevaluated C11 type checks need no native library or CUDA driver at link. */
#define SIGNATURE(name, ...) \
    _Static_assert(_Generic(&(name), __VA_ARGS__: 1, default: 0), #name " signature")
SIGNATURE(vmaf_version, const char *(*)(void));
SIGNATURE(vmaf_init, int (*)(VmafContext **, VmafConfiguration));
SIGNATURE(vmaf_close, int (*)(VmafContext *));
SIGNATURE(vmaf_read_pictures, int (*)(VmafContext *, VmafPicture *, VmafPicture *, unsigned));
SIGNATURE(vmaf_score_at_index, int (*)(VmafContext *, VmafModel *, double *, unsigned));
SIGNATURE(vmaf_feature_score_pooled, int (*)(VmafContext *, const char *, enum VmafPoolingMethod, double *, unsigned, unsigned));
SIGNATURE(vmaf_model_load, int (*)(VmafModel **, VmafModelConfig *, const char *));
SIGNATURE(vmaf_model_load_from_path, int (*)(VmafModel **, VmafModelConfig *, const char *));
SIGNATURE(vmaf_model_destroy, void (*)(VmafModel *));
SIGNATURE(vmaf_use_features_from_model, int (*)(VmafContext *, VmafModel *));
SIGNATURE(vmaf_use_feature, int (*)(VmafContext *, const char *, VmafFeatureDictionary *));
SIGNATURE(vmaf_picture_unref, int (*)(VmafPicture *));
SIGNATURE(vmaf_fetch_preallocated_picture, int (*)(VmafContext *, VmafPicture *));
SIGNATURE(vmaf_preallocate_pictures, int (*)(VmafContext *, VmafPictureConfiguration));
SIGNATURE(vmaf_cuda_state_init, int (*)(VmafCudaState **, VmafCudaConfiguration));
SIGNATURE(vmaf_cuda_release, int (*)(VmafCudaState *));
SIGNATURE(vmaf_cuda_import_state, int (*)(VmafContext *, VmafCudaState *));
SIGNATURE(vmaf_cuda_preallocate_pictures, int (*)(VmafContext *, VmafCudaPictureConfiguration));

/* GCC/Clang typeof names the actual anonymous members, without redefining them. */
typedef __typeof__(((VmafPictureConfiguration *)0)->pic_params) VmafPictureConfigPictureParams;
typedef __typeof__(((VmafCudaPictureConfiguration *)0)->pic_params) VmafCudaPictureConfigPictureParams;
#define TYPE(T) printf(#T ".size %zu\n" #T ".align %zu\n", sizeof(T), _Alignof(T))
#define FIELD(T, f) printf(#T "." #f ".offset %zu\n" #T "." #f ".size %zu\n", \
                          offsetof(T, f), sizeof(((T *)0)->f))
#define GEOMETRY(T) TYPE(T); FIELD(T, w); FIELD(T, h); FIELD(T, bpc); FIELD(T, pix_fmt)

int main(void) {
    TYPE(VmafPicture);
    FIELD(VmafPicture, pix_fmt); FIELD(VmafPicture, bpc);
    FIELD(VmafPicture, w); FIELD(VmafPicture, h); FIELD(VmafPicture, stride);
    FIELD(VmafPicture, data); FIELD(VmafPicture, ref); FIELD(VmafPicture, priv);
    TYPE(VmafConfiguration);
    FIELD(VmafConfiguration, log_level); FIELD(VmafConfiguration, n_threads);
    FIELD(VmafConfiguration, n_subsample); FIELD(VmafConfiguration, cpumask);
    FIELD(VmafConfiguration, gpumask);
    TYPE(VmafModelConfig); FIELD(VmafModelConfig, name); FIELD(VmafModelConfig, flags);
    TYPE(VmafCudaConfiguration); FIELD(VmafCudaConfiguration, cu_ctx);
    GEOMETRY(VmafPictureConfigPictureParams);
    TYPE(VmafPictureConfiguration);
    FIELD(VmafPictureConfiguration, pic_params); FIELD(VmafPictureConfiguration, pic_cnt);
    GEOMETRY(VmafCudaPictureConfigPictureParams);
    TYPE(VmafCudaPictureConfiguration);
    FIELD(VmafCudaPictureConfiguration, pic_params);
    FIELD(VmafCudaPictureConfiguration, pic_prealloc_method);
    return 0;
}
