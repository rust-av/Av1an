mod internal_integration;

use super::*;

#[test]
fn picture_format_field_preserves_unknown_integer() {
    let mut picture = VmafPicture::zeroed();
    picture.pix_fmt = 777;
    assert_eq!(picture.pix_fmt, 777);
}

#[test]
fn check_accepts_zero_and_rejects_nonzero() {
    assert!(check(0, "vmaf_init").is_ok());
    assert!(matches!(
        check(-22, "vmaf_init"),
        Err(VmafError::CallFailed {
            function: "vmaf_init",
            status: -22,
        })
    ));
}

#[test]
fn a_zeroed_picture_has_neutral_fields() {
    let picture = VmafPicture::zeroed();
    assert_eq!(picture.pix_fmt, VmafPixelFormat::Unknown as c_int);
    assert_eq!(picture.bpc, 0);
    assert!(picture.data.iter().all(|pointer| pointer.is_null()));
    assert!(picture.priv_.is_null());
}

// Rust type contracts only; installed C signatures are checked independently.
fn signatures(api: VmafApi) {
    let _: unsafe extern "C" fn(*mut VmafContext) -> c_int = api.close;
    let _: unsafe extern "C" fn(*mut VmafPicture) -> c_int = api.picture_unref;
    let _: Option<unsafe extern "C" fn(*mut VmafCudaState) -> c_int> = api.cuda.release;
    let _: unsafe extern "C" fn(*mut VmafModel) = api.model_destroy;
}

#[test]
fn rust_cleanup_function_pointer_types_compile() {
    let _ = signatures as fn(VmafApi);
}
