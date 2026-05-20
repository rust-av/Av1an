//! VapourSynth plugins.

use std::ffi::{CStr, CString, NulError};
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;
use vapoursynth_sys as ffi;

use crate::api::API;
use crate::map::{Map, OwnedMap};
use crate::plugins::{self, FilterFunction};

/// A VapourSynth plugin.
#[derive(Debug, Clone, Copy)]
pub struct Plugin<'core> {
    handle: NonNull<ffi::VSPlugin>,
    _owner: PhantomData<&'core ()>,
}

unsafe impl<'core> Send for Plugin<'core> {}
unsafe impl<'core> Sync for Plugin<'core> {}

impl<'core> Plugin<'core> {
    /// Wraps `handle` in a `Plugin`.
    ///
    /// # Safety
    /// The caller must ensure `handle` is valid and API is cached.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSPlugin) -> Self {
        Self {
            handle: unsafe { NonNull::new_unchecked(handle) },
            _owner: PhantomData,
        }
    }

    /// Returns the absolute path to the plugin, including the plugin's file name. This is the real
    /// location of the plugin, i.e. there are no symbolic links in the path.
    ///
    /// Path elements are always delimited with forward slashes.
    #[inline]
    pub fn path(&self) -> Option<&'core CStr> {
        let ptr = unsafe { API::get_cached().get_plugin_path(self.handle.as_ptr()) };
        if ptr.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(ptr) })
        }
    }

    /// Invokes a filter.
    ///
    /// `invoke()` makes sure the filter has no compat input nodes, checks that the args passed to
    /// the filter are consistent with the argument list registered by the plugin that contains the
    /// filter, creates the filter, and checks that the filter doesn't return any compat nodes. If
    /// everything goes smoothly, the filter will be ready to generate frames after `invoke()`
    /// returns.
    ///
    /// Returns a map containing the filter's return value(s). Use `Map::error()` to check if the
    /// filter was invoked successfully.
    ///
    /// Most filters will either add an error to the map, or one or more clips with the key `clip`.
    /// The exception to this are functions, for example `LoadPlugin`, which doesn't return any
    /// clips for obvious reasons.
    #[inline]
    pub fn invoke(&self, name: &str, args: &Map<'core>) -> Result<OwnedMap<'core>, NulError> {
        let name = CString::new(name)?;
        Ok(unsafe {
            OwnedMap::from_ptr(API::get_cached().invoke(
                self.handle.as_ptr(),
                name.as_ptr(),
                args.deref(),
            ))
        })
    }

    /// Registers a filter function to be exported by a non-readonly plugin.
    #[inline]
    pub fn register_function<F: FilterFunction>(&self, filter_function: F) -> Result<(), NulError> {
        // TODO: this is almost the same code as plugins::ffi::call_register_function().
        let name_cstring = CString::new(filter_function.name())?;
        let args_cstring = CString::new(filter_function.args())?;
        let return_type_cstring = CString::new("clip:vnode;")?;

        let data = Box::new(plugins::ffi::FilterFunctionData::<F> {
            filter_function,
            name: name_cstring,
        });

        unsafe {
            API::get_cached().register_function(
                data.name.as_ptr(),
                args_cstring.as_ptr(),
                return_type_cstring.as_ptr(),
                Some(plugins::ffi::create::<F>),
                Box::into_raw(data) as _,
                self.handle.as_ptr(),
            );
        }

        Ok(())
    }

    /// Returns a plugin function by name.
    ///
    /// This function retrieves a specific filter function exported by the plugin. In VapourSynth v4,
    /// this is the recommended way to query plugin functions, as the `functions()` method has been
    /// removed.
    ///
    /// Returns `None` if no function with the given name exists.
    #[inline]
    pub fn get_plugin_function_by_name(
        &self,
        name: &str,
    ) -> Result<Option<PluginFunction<'core>>, NulError> {
        let name = CString::new(name)?;
        let ptr = unsafe {
            API::get_cached().get_plugin_function_by_name(name.as_ptr(), self.handle.as_ptr())
        };
        if ptr.is_null() {
            Ok(None)
        } else {
            Ok(Some(unsafe { PluginFunction::from_ptr(ptr) }))
        }
    }
}

/// A VapourSynth plugin function.
///
/// This represents a specific filter function exported by a plugin. In VapourSynth v4, plugin
/// functions must be queried individually by name using `Plugin::get_plugin_function_by_name()`.
#[derive(Debug, Clone, Copy)]
pub struct PluginFunction<'core> {
    handle: NonNull<ffi::VSPluginFunction>,
    _owner: PhantomData<&'core ()>,
}

unsafe impl<'core> Send for PluginFunction<'core> {}
unsafe impl<'core> Sync for PluginFunction<'core> {}

impl<'core> PluginFunction<'core> {
    /// Wraps `handle` in a `PluginFunction`.
    ///
    /// # Safety
    /// The caller must ensure `handle` is valid and API is cached.
    #[inline]
    pub(crate) unsafe fn from_ptr(handle: *mut ffi::VSPluginFunction) -> Self {
        Self {
            handle: unsafe { NonNull::new_unchecked(handle) },
            _owner: PhantomData,
        }
    }

    /// Returns the name of this plugin function.
    #[inline]
    pub fn name(&self) -> &'core CStr {
        let ptr = unsafe { API::get_cached().get_plugin_function_name(self.handle.as_ptr()) };
        unsafe { CStr::from_ptr(ptr) }
    }

    /// Returns the argument specification string for this plugin function.
    ///
    /// The argument string describes the parameters the function accepts using VapourSynth's
    /// argument specification format (e.g., "clip:vnode;width:int:opt;height:int:opt;").
    #[inline]
    pub fn arguments(&self) -> &'core CStr {
        let ptr = unsafe { API::get_cached().get_plugin_function_arguments(self.handle.as_ptr()) };
        unsafe { CStr::from_ptr(ptr) }
    }

    /// Returns the return type specification string for this plugin function.
    ///
    /// The return type string describes what the function returns using VapourSynth's
    /// type specification format (typically "vnode" for filters that return video nodes).
    #[inline]
    pub fn return_type(&self) -> &'core CStr {
        let ptr =
            unsafe { API::get_cached().get_plugin_function_return_type(self.handle.as_ptr()) };
        unsafe { CStr::from_ptr(ptr) }
    }
}
