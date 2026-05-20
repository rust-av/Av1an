//! Internal stuff for plugin FFI handling.
use std::ffi::CString;
use std::fmt::Write;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_void;
use std::ptr::{self};
use std::{mem, panic, process};

use vapoursynth_sys as ffi;

use crate::api::API;
use crate::core::CoreRef;
use crate::map::{MapRef, MapRefMut};
use crate::plugins::{Filter, FilterFunction, FrameContext, Metadata};
use crate::video_info::VideoInfo;

/// Container for the internal filter function data.
pub(crate) struct FilterFunctionData<F: FilterFunction> {
    pub filter_function: F,
    // Store the name since it's supposed to be the same between two invocations (register and
    // create_filter).
    pub name: CString,
}

/// Drops the filter.
unsafe extern "C" fn free(
    instance_data: *mut c_void,
    _core: *mut ffi::VSCore,
    _vsapi: *const ffi::VSAPI,
) {
    let closure = move || {
        // The actual lifetime isn't 'static, it's 'core, but we don't really have a way of
        // retrieving it.
        let filter = Box::from_raw(instance_data as *mut Box<dyn Filter<'static> + 'static>);
        drop(filter);
    };

    if panic::catch_unwind(closure).is_err() {
        process::abort();
    }
}

/// Calls `Filter::get_frame_initial()` and `Filter::get_frame()`.
unsafe extern "C" fn get_frame(
    n: i32,
    activation_reason: i32,
    instance_data: *mut c_void,
    _frame_data: *mut *mut c_void,
    frame_ctx: *mut ffi::VSFrameContext,
    core: *mut ffi::VSCore,
    _vsapi: *const ffi::VSAPI,
) -> *const ffi::VSFrame {
    let closure = move || {
        let api = API::get_cached();
        let core = CoreRef::from_ptr(core);
        let context = FrameContext::from_ptr(frame_ctx);

        // The actual lifetime isn't 'static, it's 'core, but we don't really have a way of
        // retrieving it.
        let filter = Box::from_raw(instance_data as *mut Box<dyn Filter<'static> + 'static>);

        debug_assert!(n >= 0);
        let n = n as usize;

        let rv = match activation_reason {
            x if x == ffi::VSActivationReason_arInitial as _ => {
                match filter.get_frame_initial(api, core, context, n) {
                    Ok(Some(frame)) => {
                        let ptr = frame.deref().deref() as *const _;
                        // The ownership is transferred to the caller.
                        mem::forget(frame);
                        ptr
                    }
                    Ok(None) => ptr::null(),
                    Err(err) => {
                        let mut buf = String::with_capacity(64);

                        write!(buf, "Error in Filter::get_frame_initial(): {}", err).unwrap();

                        let buf = CString::new(buf.replace('\0', "\\0")).unwrap();
                        api.set_filter_error(buf.as_ptr(), frame_ctx);

                        ptr::null()
                    }
                }
            }
            x if x == ffi::VSActivationReason_arAllFramesReady as _ => {
                match filter.get_frame(api, core, context, n) {
                    Ok(frame) => {
                        let ptr = frame.deref().deref() as *const _;
                        // The ownership is transferred to the caller.
                        mem::forget(frame);
                        ptr
                    }
                    Err(err) => {
                        let buf = format!("{}", err);
                        let buf = CString::new(buf.replace('\0', "\\0")).unwrap();
                        api.set_filter_error(buf.as_ptr(), frame_ctx);

                        ptr::null()
                    }
                }
            }
            _ => ptr::null(),
        };

        mem::forget(filter);

        rv
    };

    match panic::catch_unwind(closure) {
        Ok(frame) => frame,
        Err(_) => process::abort(),
    }
}

/// Creates a new instance of the filter.
pub(crate) unsafe extern "C" fn create<F: FilterFunction>(
    in_: *const ffi::VSMap,
    out: *mut ffi::VSMap,
    user_data: *mut c_void,
    core: *mut ffi::VSCore,
    api: *const ffi::VSAPI,
) {
    let closure = move || {
        API::set(api);
        let api = API::get_cached();

        let args = MapRef::from_ptr(in_);
        let mut out = MapRefMut::from_ptr(out);
        let core = CoreRef::from_ptr(core);
        let data = Box::from_raw(user_data as *mut FilterFunctionData<F>);

        let filter = match data.filter_function.create(api, core, &args) {
            Ok(Some(filter)) => Some(Box::new(filter)),
            Ok(None) => None,
            Err(err) => {
                let mut buf = String::with_capacity(64);

                write!(
                    buf,
                    "Error in Filter::create() of {}: {}",
                    data.name.to_str().unwrap(),
                    err
                )
                .unwrap();

                write!(buf, "{}", err).unwrap();

                out.set_error(&buf.replace('\0', "\\0")).unwrap();
                None
            }
        };

        if let Some(filter) = filter {
            // In v4, we need to get the video info before creating the filter
            let vi = filter
                .video_info(api, core)
                .into_iter()
                .map(VideoInfo::ffi_type)
                .collect::<Vec<_>>();

            // For now, assume single output (most common case)
            // TODO: Handle multiple outputs if needed
            let vi_ptr = if !vi.is_empty() {
                vi.as_ptr()
            } else {
                ptr::null()
            };

            api.create_video_filter(
                out.deref_mut().deref_mut(),
                data.name.as_ptr(),
                vi_ptr,
                Some(get_frame),
                Some(free),
                ffi::VSFilterMode_fmParallel as i32,
                ptr::null(), // No dependencies for now
                0,           // numDeps
                Box::into_raw(filter) as *mut _,
                core.ptr(),
            );
        }

        mem::forget(data);
    };

    if panic::catch_unwind(closure).is_err() {
        // The `FilterFunction` might have been left in an inconsistent state, so we have to abort.
        process::abort();
    }
}

/// Registers the plugin.
///
/// This function is for internal use only.
///
/// # Safety
/// The caller must ensure the pointers are valid.
#[inline]
pub unsafe fn call_config_func(
    vspapi: *const ffi::VSPLUGINAPI,
    plugin: *mut ffi::VSPlugin,
    metadata: Metadata,
) {
    let identifier_cstring = CString::new(metadata.identifier)
        .expect("Couldn't convert the plugin identifier to a CString");
    let namespace_cstring = CString::new(metadata.namespace)
        .expect("Couldn't convert the plugin namespace to a CString");
    let name_cstring =
        CString::new(metadata.name).expect("Couldn't convert the plugin name to a CString");

    let api_version = (ffi::VAPOURSYNTH_API_MAJOR << 16 | ffi::VAPOURSYNTH_API_MINOR) as i32;
    let flags = if metadata.read_only {
        0 // Read-only means NOT modifiable
    } else {
        ffi::VSPluginConfigFlags_pcModifiable as i32
    };

    ((*vspapi).configPlugin.unwrap())(
        identifier_cstring.as_ptr(),
        namespace_cstring.as_ptr(),
        name_cstring.as_ptr(),
        1, // Plugin version
        api_version,
        flags,
        plugin,
    );
}

/// Registers the filter `F`.
///
/// This function is for internal use only.
///
/// # Safety
/// The caller must ensure the pointers are valid.
#[inline]
pub unsafe fn call_register_func<F: FilterFunction>(
    vspapi: *const ffi::VSPLUGINAPI,
    plugin: *mut ffi::VSPlugin,
    filter_function: F,
) {
    let name_cstring = CString::new(filter_function.name())
        .expect("Couldn't convert the filter name to a CString");
    let args_cstring = CString::new(filter_function.args())
        .expect("Couldn't convert the filter args to a CString");
    let return_type_cstring =
        CString::new("clip:vnode;").expect("Couldn't convert return type to a CString");

    let data = Box::new(FilterFunctionData {
        filter_function,
        name: name_cstring,
    });

    ((*vspapi).registerFunction.unwrap())(
        data.name.as_ptr(),
        args_cstring.as_ptr(),
        return_type_cstring.as_ptr(),
        Some(create::<F>),
        Box::into_raw(data) as _,
        plugin,
    );
}

/// Exports a VapourSynth plugin from this library.
///
/// This macro should be used only once at the top level of the library. The library should have a
/// `cdylib` crate type.
///
/// The first parameter is a `Metadata` expression containing your plugin's metadata.
///
/// Following it is a list of values implementing `FilterFunction`, those are the filter functions
/// the plugin will export.
///
/// # Example
/// ```ignore
/// export_vapoursynth_plugin! {
///     Metadata {
///         identifier: "com.example.invert",
///         namespace: "invert",
///         name: "Invert Example Plugin",
///         read_only: true,
///     },
///     [SampleFilterFunction::new(), OtherFunction::new()]
/// }
/// ```
#[macro_export]
macro_rules! export_vapoursynth_plugin {
    ($metadata:expr, [$($filter:expr),*$(,)*]) => (
        #[allow(non_snake_case)]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn VapourSynthPluginInit2(
            plugin: *mut $crate::ffi::VSPlugin,
            vspapi: *const $crate::ffi::VSPLUGINAPI,
        ) {
            use ::std::{panic, process};
            use $crate::plugins::ffi::{call_config_func, call_register_func};

            let closure = move || {
                call_config_func(vspapi, plugin, $metadata);

                $(
                    call_register_func(vspapi, plugin, $filter);
                )*
            };

            if panic::catch_unwind(closure).is_err() {
                process::abort();
            }
        }
    )
}
