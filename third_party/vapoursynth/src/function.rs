//! VapourSynth callable functions.

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_void;
use std::ptr::NonNull;
use std::{mem, panic, process};
use vapoursynth_sys as ffi;

use crate::api::API;
use crate::core::CoreRef;
use crate::map::{Map, MapRef, MapRefMut};

/// Holds a reference to a function that may be called.
#[derive(Debug)]
pub struct Function<'core> {
    handle: NonNull<ffi::VSFunction>,
    _owner: PhantomData<&'core ()>,
}

unsafe impl<'core> Send for Function<'core> {}
unsafe impl<'core> Sync for Function<'core> {}

impl<'core> Drop for Function<'core> {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            API::get_cached().free_func(self.handle.as_ptr());
        }
    }
}

impl<'core> Clone for Function<'core> {
    #[inline]
    fn clone(&self) -> Self {
        let handle = unsafe { API::get_cached().clone_func(self.handle.as_ptr()) };
        Self {
            handle: unsafe { NonNull::new_unchecked(handle) },
            _owner: PhantomData,
        }
    }
}

impl<'core> Function<'core> {
    /// Wraps `handle` in a `Function`.
    ///
    /// # Safety
    /// The caller must ensure `handle` and the lifetime are valid and API is cached.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSFunction) -> Self {
        Self {
            handle: NonNull::new_unchecked(handle),
            _owner: PhantomData,
        }
    }

    /// Returns the underlying pointer.
    #[inline]
    pub(crate) fn ptr(&self) -> *mut ffi::VSFunction {
        self.handle.as_ptr()
    }

    /// Creates a new function.
    ///
    /// To indicate an error from the callback, set an error on the output map.
    #[inline]
    pub fn new<F>(_api: API, core: CoreRef<'core>, callback: F) -> Self
    where
        F: Fn(API, CoreRef<'core>, &Map<'core>, &mut Map<'core>) + Send + Sync + 'core,
    {
        unsafe extern "C" fn c_callback<'core, F>(
            in_: *const ffi::VSMap,
            out: *mut ffi::VSMap,
            user_data: *mut c_void,
            core: *mut ffi::VSCore,
            _vsapi: *const ffi::VSAPI,
        ) where
            F: Fn(API, CoreRef<'core>, &Map<'core>, &mut Map<'core>) + Send + Sync + 'core,
        {
            let closure = move || {
                let api = API::get_cached();
                let core = CoreRef::from_ptr(core);
                let in_ = MapRef::from_ptr(in_);
                let mut out = MapRefMut::from_ptr(out);
                let callback = Box::from_raw(user_data as *mut F);

                callback(api, core, &in_, &mut out);

                mem::forget(callback);
            };

            if panic::catch_unwind(closure).is_err() {
                process::abort();
            }
        }

        unsafe extern "C" fn c_free<F>(user_data: *mut c_void) {
            drop(Box::from_raw(user_data as *mut F))
        }

        let data = Box::new(callback);

        let handle = unsafe {
            API::get_cached().create_func(
                Some(c_callback::<'core, F>),
                Box::into_raw(data) as _,
                Some(c_free::<F>),
                core.ptr(),
            )
        };

        Self {
            handle: unsafe { NonNull::new_unchecked(handle) },
            _owner: PhantomData,
        }
    }

    /// Calls the function. If the call fails `out` will have an error set.
    #[inline]
    pub fn call(&self, in_: &Map<'core>, out: &mut Map<'core>) {
        unsafe { API::get_cached().call_func(self.handle.as_ptr(), in_.deref(), out.deref_mut()) };
    }
}
