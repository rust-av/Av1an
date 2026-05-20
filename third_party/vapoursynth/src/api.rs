//! Most general VapourSynth API functions.

use std::ffi::{CString, NulError};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicPtr, Ordering};
use vapoursynth_sys as ffi;

use crate::core::CoreRef;

/// A wrapper for the VapourSynth API.
#[derive(Debug, Clone, Copy)]
pub struct API {
    // Note that this is *const, not *mut.
    handle: NonNull<ffi::VSAPI>,
}

unsafe impl Send for API {}
unsafe impl Sync for API {}

/// A cached API pointer. Note that this is `*const ffi::VSAPI`, not `*mut`.
static RAW_API: AtomicPtr<ffi::VSAPI> = AtomicPtr::new(ptr::null_mut());

/// VapourSynth log message types.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum MessageType {
    Debug,
    Warning,
    Critical,

    /// The process will `abort()` after the message handler returns.
    Fatal,
}

// Macros for implementing repetitive functions.
macro_rules! prop_get_something {
    ($name:ident, $func:ident, $rv:ty) => {
        #[inline]
        pub(crate) unsafe fn $name(
            self,
            map: &$crate::ffi::VSMap,
            key: *const c_char,
            index: i32,
            error: &mut i32,
        ) -> $rv {
            (self.handle.as_ref().$func.unwrap())(map, key, index, error)
        }
    };
}

macro_rules! prop_set_something {
    ($name:ident, $func:ident, $type:ty) => {
        #[inline]
        pub(crate) unsafe fn $name(
            self,
            map: &mut $crate::ffi::VSMap,
            key: *const c_char,
            value: $type,
            append: $crate::ffi::VSMapAppendMode,
        ) -> i32 {
            (self.handle.as_ref().$func.unwrap())(map, key, value, append as i32)
        }
    };
}

/// ID of a unique, registered VapourSynth message handler.
///
/// Note: In VapourSynth v4, the message handler registration system has been removed.
/// This type is kept for backward compatibility but is now a dummy type.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct MessageHandlerId(());

impl API {
    /// Retrieves the VapourSynth API.
    ///
    /// Returns `None` on error, for example if the requested API version (selected with features,
    /// see the crate-level docs) is not supported.
    // If we're linking to VSScript anyway, use the VSScript function.
    #[inline]
    pub fn get() -> Option<Self> {
        // Check if we already have the API.
        let handle = RAW_API.load(Ordering::Relaxed);

        let handle = if handle.is_null() {
            // Attempt retrieving it otherwise.
            let vsscript_api = crate::vsscript::VSScriptAPI::get()?;
            let version = (ffi::VAPOURSYNTH_API_MAJOR << 16 | ffi::VAPOURSYNTH_API_MINOR) as i32;
            let handle =
                unsafe { (vsscript_api.handle().getVSAPI.unwrap())(version) } as *mut ffi::VSAPI;

            if !handle.is_null() {
                // If we successfully retrieved the API, cache it.
                RAW_API.store(handle, Ordering::Relaxed);
            }
            handle
        } else {
            handle
        };

        if handle.is_null() {
            None
        } else {
            Some(Self {
                handle: unsafe { NonNull::new_unchecked(handle) },
            })
        }
    }

    /// Returns the cached API.
    ///
    /// # Safety
    /// This function assumes the cache contains a valid API pointer.
    #[inline]
    pub(crate) unsafe fn get_cached() -> Self {
        Self {
            handle: NonNull::new_unchecked(RAW_API.load(Ordering::Relaxed)),
        }
    }

    /// Stores the API in the cache.
    ///
    /// # Safety
    /// The given pointer should be valid.
    #[inline]
    pub(crate) unsafe fn set(handle: *const ffi::VSAPI) {
        RAW_API.store(handle as *mut _, Ordering::Relaxed);
    }

    /// Sends a message through VapourSynth’s logging framework.
    #[inline]
    pub fn log(self, message_type: MessageType, message: &str) -> Result<(), NulError> {
        let message = CString::new(message)?;
        unsafe {
            (self.handle.as_ref().logMessage.unwrap())(
                message_type.ffi_type(),
                message.as_ptr(),
                ptr::null_mut(), // No specific core in v4
            );
        }
        Ok(())
    }

    /// Frees `node`.
    ///
    /// # Safety
    /// The caller must ensure `node` is valid.
    #[inline]
    pub(crate) unsafe fn free_node(self, node: *mut ffi::VSNode) {
        (self.handle.as_ref().freeNode.unwrap())(node);
    }

    /// Clones `node`.
    ///
    /// # Safety
    /// The caller must ensure `node` is valid.
    #[inline]
    pub(crate) unsafe fn clone_node(self, node: *mut ffi::VSNode) -> *mut ffi::VSNode {
        (self.handle.as_ref().addNodeRef.unwrap())(node)
    }

    /// Returns a pointer to the video info associated with `node`. The pointer is valid as long as
    /// the node lives.
    ///
    /// # Safety
    /// The caller must ensure `node` is valid.
    #[inline]
    pub(crate) unsafe fn get_video_info(self, node: *mut ffi::VSNode) -> *const ffi::VSVideoInfo {
        (self.handle.as_ref().getVideoInfo.unwrap())(node)
    }

    /// Generates a frame directly.
    ///
    /// # Safety
    /// The caller must ensure `node` is valid.
    ///
    /// # Panics
    /// Panics if `err_msg` is larger than `i32::MAX`.
    #[inline]
    pub(crate) unsafe fn get_frame(
        self,
        n: i32,
        node: *mut ffi::VSNode,
        err_msg: &mut [c_char],
    ) -> *const ffi::VSFrame {
        let len = err_msg.len();
        assert!(len <= i32::MAX as usize);
        let len = len as i32;

        (self.handle.as_ref().getFrame.unwrap())(n, node, err_msg.as_mut_ptr(), len)
    }

    /// Generates a frame directly.
    ///
    /// # Safety
    /// The caller must ensure `node` and `callback` are valid.
    #[inline]
    pub(crate) unsafe fn get_frame_async(
        self,
        n: i32,
        node: *mut ffi::VSNode,
        callback: ffi::VSFrameDoneCallback,
        user_data: *mut c_void,
    ) {
        (self.handle.as_ref().getFrameAsync.unwrap())(n, node, callback, user_data);
    }

    /// Frees `frame`.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid.
    #[inline]
    pub(crate) unsafe fn free_frame(self, frame: &ffi::VSFrame) {
        (self.handle.as_ref().freeFrame.unwrap())(frame);
    }

    /// Clones `frame`.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid.
    #[inline]
    pub(crate) unsafe fn clone_frame(self, frame: &ffi::VSFrame) -> *const ffi::VSFrame {
        (self.handle.as_ref().addFrameRef.unwrap())(frame)
    }

    /// Retrieves the format of a frame.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid.
    #[inline]
    pub(crate) unsafe fn get_frame_format(self, frame: &ffi::VSFrame) -> *const ffi::VSVideoFormat {
        (self.handle.as_ref().getVideoFrameFormat.unwrap())(frame)
    }

    /// Returns the width of a plane of a given frame, in pixels.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and `plane` is valid for the given `frame`.
    #[inline]
    pub(crate) unsafe fn get_frame_width(self, frame: &ffi::VSFrame, plane: i32) -> i32 {
        (self.handle.as_ref().getFrameWidth.unwrap())(frame, plane)
    }

    /// Returns the height of a plane of a given frame, in pixels.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and `plane` is valid for the given `frame`.
    #[inline]
    pub(crate) unsafe fn get_frame_height(self, frame: &ffi::VSFrame, plane: i32) -> i32 {
        (self.handle.as_ref().getFrameHeight.unwrap())(frame, plane)
    }

    /// Returns the distance in bytes between two consecutive lines of a plane of a frame.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and `plane` is valid for the given `frame`.
    #[inline]
    pub(crate) unsafe fn get_frame_stride(self, frame: &ffi::VSFrame, plane: i32) -> isize {
        (self.handle.as_ref().getStride.unwrap())(frame, plane)
    }

    /// Returns a read-only pointer to a plane of a frame.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and `plane` is valid for the given `frame`.
    #[inline]
    pub(crate) unsafe fn get_frame_read_ptr(self, frame: &ffi::VSFrame, plane: i32) -> *const u8 {
        (self.handle.as_ref().getReadPtr.unwrap())(frame, plane)
    }

    /// Returns a read-write pointer to a plane of a frame.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and `plane` is valid for the given `frame`.
    #[inline]
    pub(crate) unsafe fn get_frame_write_ptr(
        self,
        frame: &mut ffi::VSFrame,
        plane: i32,
    ) -> *mut u8 {
        (self.handle.as_ref().getWritePtr.unwrap())(frame, plane)
    }

    /// Returns a read-only pointer to a frame's properties.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and the correct lifetime is assigned to the
    /// returned map (it can't outlive `frame`).
    #[inline]
    pub(crate) unsafe fn get_frame_props_ro(self, frame: &ffi::VSFrame) -> *const ffi::VSMap {
        (self.handle.as_ref().getFramePropertiesRO.unwrap())(frame)
    }

    /// Returns a read-write pointer to a frame's properties.
    ///
    /// # Safety
    /// The caller must ensure `frame` is valid and the correct lifetime is assigned to the
    /// returned map (it can't outlive `frame`).
    #[inline]
    pub(crate) unsafe fn get_frame_props_rw(self, frame: &mut ffi::VSFrame) -> *mut ffi::VSMap {
        (self.handle.as_ref().getFramePropertiesRW.unwrap())(frame)
    }

    /// Creates a new `VSMap`.
    #[inline]
    pub(crate) fn create_map(self) -> *mut ffi::VSMap {
        unsafe { (self.handle.as_ref().createMap.unwrap())() }
    }

    /// Clears `map`.
    ///
    /// # Safety
    /// The caller must ensure `map` is valid.
    #[inline]
    pub(crate) unsafe fn clear_map(self, map: &mut ffi::VSMap) {
        (self.handle.as_ref().clearMap.unwrap())(map);
    }

    /// Frees `map`.
    ///
    /// # Safety
    /// The caller must ensure `map` is valid.
    #[inline]
    pub(crate) unsafe fn free_map(self, map: &mut ffi::VSMap) {
        (self.handle.as_ref().freeMap.unwrap())(map);
    }

    /// Returns a pointer to the error message contained in the map, or NULL if there is no error
    /// message.
    ///
    /// # Safety
    /// The caller must ensure `map` is valid.
    #[inline]
    pub(crate) unsafe fn get_error(self, map: &ffi::VSMap) -> *const c_char {
        (self.handle.as_ref().mapGetError.unwrap())(map)
    }

    /// Adds an error message to a map. The map is cleared first. The error message is copied.
    ///
    /// # Safety
    /// The caller must ensure `map` and `errorMessage` are valid.
    #[inline]
    pub(crate) unsafe fn set_error(self, map: &mut ffi::VSMap, error_message: *const c_char) {
        (self.handle.as_ref().mapSetError.unwrap())(map, error_message)
    }

    /// Returns the number of keys contained in a map.
    ///
    /// # Safety
    /// The caller must ensure `map` is valid.
    #[inline]
    pub(crate) unsafe fn prop_num_keys(self, map: &ffi::VSMap) -> i32 {
        (self.handle.as_ref().mapNumKeys.unwrap())(map)
    }

    /// Returns a key from a property map.
    ///
    /// # Safety
    /// The caller must ensure `map` is valid and `index` is valid for `map`.
    #[inline]
    pub(crate) unsafe fn prop_get_key(self, map: &ffi::VSMap, index: i32) -> *const c_char {
        (self.handle.as_ref().mapGetKey.unwrap())(map, index)
    }

    /// Removes the key from a property map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_delete_key(self, map: &mut ffi::VSMap, key: *const c_char) -> i32 {
        (self.handle.as_ref().mapDeleteKey.unwrap())(map, key)
    }

    /// Returns the number of elements associated with a key in a property map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_num_elements(self, map: &ffi::VSMap, key: *const c_char) -> i32 {
        (self.handle.as_ref().mapNumElements.unwrap())(map, key)
    }

    /// Returns the type of the elements associated with the given key in a property map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_get_type(self, map: &ffi::VSMap, key: *const c_char) -> i32 {
        (self.handle.as_ref().mapGetType.unwrap())(map, key)
    }

    /// Returns the size in bytes of a property of type ptData.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_get_data_size(
        self,
        map: &ffi::VSMap,
        key: *const c_char,
        index: i32,
        error: &mut i32,
    ) -> i32 {
        (self.handle.as_ref().mapGetDataSize.unwrap())(map, key, index, error)
    }

    prop_get_something!(prop_get_int, mapGetInt, i64);
    prop_get_something!(prop_get_float, mapGetFloat, f64);
    prop_get_something!(prop_get_data, mapGetData, *const c_char);
    prop_get_something!(prop_get_node, mapGetNode, *mut ffi::VSNode);
    prop_get_something!(prop_get_frame, mapGetFrame, *const ffi::VSFrame);
    prop_get_something!(prop_get_func, mapGetFunction, *mut ffi::VSFunction);

    prop_set_something!(prop_set_int, mapSetInt, i64);
    prop_set_something!(prop_set_float, mapSetFloat, f64);
    prop_set_something!(prop_set_node, mapSetNode, *mut ffi::VSNode);
    prop_set_something!(prop_set_frame, mapSetFrame, *const ffi::VSFrame);
    prop_set_something!(prop_set_func, mapSetFunction, *mut ffi::VSFunction);

    /// Retrieves an array of integers from a map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_get_int_array(
        self,
        map: &ffi::VSMap,
        key: *const c_char,
        error: &mut i32,
    ) -> *const i64 {
        (self.handle.as_ref().mapGetIntArray.unwrap())(map, key, error)
    }

    /// Retrieves an array of floating point numbers from a map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    #[inline]
    pub(crate) unsafe fn prop_get_float_array(
        self,
        map: &ffi::VSMap,
        key: *const c_char,
        error: &mut i32,
    ) -> *const f64 {
        (self.handle.as_ref().mapGetFloatArray.unwrap())(map, key, error)
    }

    /// Adds a data property to the map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    ///
    /// # Panics
    /// Panics if `value.len()` can't fit in an `i32`.
    #[inline]
    pub(crate) unsafe fn prop_set_data(
        self,
        map: &mut ffi::VSMap,
        key: *const c_char,
        value: &[u8],
        append: ffi::VSMapAppendMode,
    ) -> i32 {
        let length = value.len();
        assert!(length <= i32::MAX as usize);
        let length = length as i32;

        (self.handle.as_ref().mapSetData.unwrap())(
            map,
            key,
            value.as_ptr() as _,
            length,
            ffi::VSDataTypeHint_dtUnknown, // type hint
            append as i32,
        )
    }

    /// Adds an array of integers to the map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    ///
    /// # Panics
    /// Panics if `value.len()` can't fit in an `i32`.
    #[inline]
    pub(crate) unsafe fn prop_set_int_array(
        self,
        map: &mut ffi::VSMap,
        key: *const c_char,
        value: &[i64],
    ) -> i32 {
        let length = value.len();
        assert!(length <= i32::MAX as usize);
        let length = length as i32;

        (self.handle.as_ref().mapSetIntArray.unwrap())(map, key, value.as_ptr(), length)
    }

    /// Adds an array of floating point numbers to the map.
    ///
    /// # Safety
    /// The caller must ensure `map` and `key` are valid.
    ///
    /// # Panics
    /// Panics if `value.len()` can't fit in an `i32`.
    #[inline]
    pub(crate) unsafe fn prop_set_float_array(
        self,
        map: &mut ffi::VSMap,
        key: *const c_char,
        value: &[f64],
    ) -> i32 {
        let length = value.len();
        assert!(length <= i32::MAX as usize);
        let length = length as i32;

        (self.handle.as_ref().mapSetFloatArray.unwrap())(map, key, value.as_ptr(), length)
    }

    /// Frees `function`.
    ///
    /// # Safety
    /// The caller must ensure `function` is valid.
    #[inline]
    pub(crate) unsafe fn free_func(self, function: *mut ffi::VSFunction) {
        (self.handle.as_ref().freeFunction.unwrap())(function);
    }

    /// Clones `function`.
    ///
    /// # Safety
    /// The caller must ensure `function` is valid.
    #[inline]
    pub(crate) unsafe fn clone_func(self, function: *mut ffi::VSFunction) -> *mut ffi::VSFunction {
        (self.handle.as_ref().addFunctionRef.unwrap())(function)
    }

    /// Returns information about the VapourSynth core.
    ///
    /// # Safety
    /// The caller must ensure `core` is valid.
    #[inline]
    pub(crate) unsafe fn get_core_info(self, core: *mut ffi::VSCore) -> ffi::VSCoreInfo {
        use std::mem::MaybeUninit;

        let mut core_info = MaybeUninit::uninit();
        (self.handle.as_ref().getCoreInfo.unwrap())(core, core_info.as_mut_ptr());
        core_info.assume_init()
    }

    /// Returns a VSFormat structure from a video format identifier.
    ///
    /// # Safety
    /// The caller must ensure `core` is valid.
    #[inline]
    pub(crate) unsafe fn get_format_preset(
        self,
        id: i32,
        core: *mut ffi::VSCore,
    ) -> *const ffi::VSVideoFormat {
        use std::mem::MaybeUninit;

        // V4 API uses output parameters, so we need to allocate and box the format
        let mut format = Box::new(MaybeUninit::<ffi::VSVideoFormat>::uninit());
        let result = (self.handle.as_ref().getVideoFormatByID.unwrap())(
            format.as_mut_ptr(),
            id as u32,
            core,
        );

        if result != 0 {
            Box::into_raw(format) as *const ffi::VSVideoFormat
        } else {
            ptr::null()
        }
    }

    /// Registers a custom video format.
    ///
    /// # Safety
    /// The caller must ensure `core` is valid.
    #[inline]
    pub(crate) unsafe fn register_format(
        self,
        color_family: ffi::VSColorFamily,
        sample_type: ffi::VSSampleType,
        bits_per_sample: i32,
        sub_sampling_w: i32,
        sub_sampling_h: i32,
        core: *mut ffi::VSCore,
    ) -> *const ffi::VSVideoFormat {
        use std::mem::MaybeUninit;

        // V4 API uses queryVideoFormat which fills in the format struct
        let mut format = Box::new(MaybeUninit::<ffi::VSVideoFormat>::uninit());
        let result = (self.handle.as_ref().queryVideoFormat.unwrap())(
            format.as_mut_ptr(),
            color_family as i32,
            sample_type as i32,
            bits_per_sample,
            sub_sampling_w,
            sub_sampling_h,
            core,
        );

        if result != 0 {
            Box::into_raw(format) as *const ffi::VSVideoFormat
        } else {
            ptr::null()
        }
    }

    /// Creates a new video filter node.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub(crate) unsafe fn create_video_filter(
        self,
        out: *mut ffi::VSMap,
        name: *const c_char,
        vi: *const ffi::VSVideoInfo,
        get_frame: ffi::VSFilterGetFrame,
        free: ffi::VSFilterFree,
        filter_mode: i32,
        dependencies: *const ffi::VSFilterDependency,
        num_deps: i32,
        instance_data: *mut c_void,
        core: *mut ffi::VSCore,
    ) {
        (self.handle.as_ref().createVideoFilter.unwrap())(
            out,
            name,
            vi,
            get_frame,
            free,
            filter_mode,
            dependencies,
            num_deps,
            instance_data,
            core,
        );
    }

    /// Adds an error message to a frame context, replacing the existing message, if any.
    ///
    /// This is the way to report errors in a filter's "get frame" function. Such errors are not
    /// necessarily fatal, i.e. the caller can try to request the same frame again.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn set_filter_error(
        self,
        message: *const c_char,
        frame_ctx: *mut ffi::VSFrameContext,
    ) {
        (self.handle.as_ref().setFilterError.unwrap())(message, frame_ctx);
    }

    /// Requests a frame from a node and returns immediately.
    ///
    /// This is only used in filters' "get frame" functions.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid and this is called from a filter "get frame"
    /// function.
    #[inline]
    pub(crate) unsafe fn request_frame_filter(
        self,
        n: i32,
        node: *mut ffi::VSNode,
        frame_ctx: *mut ffi::VSFrameContext,
    ) {
        (self.handle.as_ref().requestFrameFilter.unwrap())(n, node, frame_ctx);
    }

    /// Retrieves a frame that was previously requested with `request_frame_filter()`.
    ///
    /// This is only used in filters' "get frame" functions.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid and this is called from a filter "get frame"
    /// function.
    #[inline]
    pub(crate) unsafe fn get_frame_filter(
        self,
        n: i32,
        node: *mut ffi::VSNode,
        frame_ctx: *mut ffi::VSFrameContext,
    ) -> *const ffi::VSFrame {
        (self.handle.as_ref().getFrameFilter.unwrap())(n, node, frame_ctx)
    }

    /// Duplicates the frame (not just the reference). As the frame buffer is shared in a
    /// copy-on-write fashion, the frame content is not really duplicated until a write operation
    /// occurs. This is transparent for the user.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn copy_frame(
        self,
        f: &ffi::VSFrame,
        core: *mut ffi::VSCore,
    ) -> *mut ffi::VSFrame {
        (self.handle.as_ref().copyFrame.unwrap())(f, core)
    }

    /// Creates a new frame, optionally copying the properties attached to another frame. The new
    /// frame contains uninitialised memory.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid and that the uninitialized plane data of the
    /// returned frame is handled carefully.
    #[inline]
    pub(crate) unsafe fn new_video_frame(
        self,
        format: &ffi::VSVideoFormat,
        width: i32,
        height: i32,
        prop_src: *const ffi::VSFrame,
        core: *mut ffi::VSCore,
    ) -> *mut ffi::VSFrame {
        (self.handle.as_ref().newVideoFrame.unwrap())(format, width, height, prop_src, core)
    }

    /// Queries a video format ID from format properties.
    ///
    /// # Safety
    /// The caller must ensure the core pointer is valid.
    #[inline]
    pub(crate) unsafe fn query_video_format_id(
        self,
        color_family: i32,
        sample_type: i32,
        bits_per_sample: i32,
        sub_sampling_w: i32,
        sub_sampling_h: i32,
        core: *mut ffi::VSCore,
    ) -> u32 {
        (self.handle.as_ref().queryVideoFormatID.unwrap())(
            color_family,
            sample_type,
            bits_per_sample,
            sub_sampling_w,
            sub_sampling_h,
            core,
        )
    }

    /// Gets the printable name of a video format.
    ///
    /// # Safety
    /// The caller must ensure pointers are valid and buffer is large enough.
    #[inline]
    pub(crate) unsafe fn get_video_format_name(
        self,
        format: *const ffi::VSVideoFormat,
        buffer: *mut c_char,
    ) -> i32 {
        (self.handle.as_ref().getVideoFormatName.unwrap())(format, buffer)
    }

    /// Returns a pointer to the plugin with the given identifier, or a null pointer if not found.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_by_id(
        self,
        identifier: *const c_char,
        core: *mut ffi::VSCore,
    ) -> *mut ffi::VSPlugin {
        (self.handle.as_ref().getPluginByID.unwrap())(identifier, core)
    }

    /// Returns a pointer to the plugin with the given namespace, or a null pointer if not found.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_by_ns(
        self,
        namespace: *const c_char,
        core: *mut ffi::VSCore,
    ) -> *mut ffi::VSPlugin {
        (self.handle.as_ref().getPluginByNamespace.unwrap())(namespace, core)
    }

    /// Returns the absolute path to the plugin, including the plugin's file name. This is the real
    /// location of the plugin, i.e. there are no symbolic links in the path.
    ///
    /// Path elements are always delimited with forward slashes.
    ///
    /// VapourSynth retains ownership of the returned pointer.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    // This was introduced in R25 without bumping the API version (R3) but we must be sure it's
    // there, so require R3.1.
    #[inline]
    pub(crate) unsafe fn get_plugin_path(self, plugin: *mut ffi::VSPlugin) -> *const c_char {
        (self.handle.as_ref().getPluginPath.unwrap())(plugin)
    }

    /// Invokes a filter.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn invoke(
        self,
        plugin: *mut ffi::VSPlugin,
        name: *const c_char,
        args: *const ffi::VSMap,
    ) -> *mut ffi::VSMap {
        (self.handle.as_ref().invoke.unwrap())(plugin, name, args)
    }

    /// Creates a user-defined function.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn create_func(
        self,
        func: ffi::VSPublicFunction,
        user_data: *mut c_void,
        free: ffi::VSFreeFunctionData,
        core: *mut ffi::VSCore,
    ) -> *mut ffi::VSFunction {
        (self.handle.as_ref().createFunction.unwrap())(func, user_data, free, core)
    }

    /// Calls a function. If the call fails out will have an error set.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn call_func(
        self,
        func: *mut ffi::VSFunction,
        in_: *const ffi::VSMap,
        out: *mut ffi::VSMap,
    ) {
        (self.handle.as_ref().callFunction.unwrap())(func, in_, out);
    }

    /// Registers a filter exported by the plugin. A plugin can export any number of filters.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn register_function(
        self,
        name: *const c_char,
        args: *const c_char,
        return_type: *const c_char,
        args_func: ffi::VSPublicFunction,
        function_data: *mut c_void,
        plugin: *mut ffi::VSPlugin,
    ) {
        (self.handle.as_ref().registerFunction.unwrap())(
            name,
            args,
            return_type,
            args_func,
            function_data,
            plugin,
        );
    }

    /// Sets the maximum size of the framebuffer cache. Returns the new maximum size.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid. On VapourSynth API below 3.6, the caller
    /// must ensure there are no concurrent accesses to the core info.
    #[inline]
    pub(crate) unsafe fn set_max_cache_size(self, bytes: i64, core: *mut ffi::VSCore) -> i64 {
        (self.handle.as_ref().setMaxCacheSize.unwrap())(bytes, core)
    }

    /// Sets the number of worker threads for the given core.
    ///
    /// If the requested number of threads is zero or lower, the number of hardware threads will be
    /// detected and used.
    ///
    /// Returns the new thread count.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid. On VapourSynth API below 3.6, the caller
    /// must ensure there are no concurrent accesses to the core info.
    #[inline]
    pub(crate) unsafe fn set_thread_count(self, threads: c_int, core: *mut ffi::VSCore) -> c_int {
        (self.handle.as_ref().setThreadCount.unwrap())(threads, core)
    }

    /// Creates and returns a new core.
    ///
    /// Note that there's currently no safe way of freeing the returned core, and the lifetime is
    /// unbounded, because it can live for an arbitrary long time. You may use the (unsafe)
    /// `vapoursynth_sys::VSAPI::freeCore()` after ensuring that all frame requests have completed
    /// and all objects belonging to the core have been released.
    #[inline]
    pub fn create_core<'core>(self, threads: i32) -> CoreRef<'core> {
        unsafe {
            let handle = (self.handle.as_ref().createCore.unwrap())(0);
            (self.handle.as_ref().setThreadCount.unwrap())(threads, handle);
            CoreRef::from_ptr(handle)
        }
    }

    /// Returns a pointer to a plugin function with the given name, or a null pointer if not found.
    ///
    /// # Safety
    /// The caller must ensure all pointers are valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_function_by_name(
        self,
        name: *const c_char,
        plugin: *mut ffi::VSPlugin,
    ) -> *mut ffi::VSPluginFunction {
        (self.handle.as_ref().getPluginFunctionByName.unwrap())(name, plugin)
    }

    /// Returns the name of a plugin function.
    ///
    /// # Safety
    /// The caller must ensure the function pointer is valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_function_name(
        self,
        func: *mut ffi::VSPluginFunction,
    ) -> *const c_char {
        (self.handle.as_ref().getPluginFunctionName.unwrap())(func)
    }

    /// Returns the argument string of a plugin function.
    ///
    /// # Safety
    /// The caller must ensure the function pointer is valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_function_arguments(
        self,
        func: *mut ffi::VSPluginFunction,
    ) -> *const c_char {
        (self.handle.as_ref().getPluginFunctionArguments.unwrap())(func)
    }

    /// Returns the return type string of a plugin function.
    ///
    /// # Safety
    /// The caller must ensure the function pointer is valid.
    #[inline]
    pub(crate) unsafe fn get_plugin_function_return_type(
        self,
        func: *mut ffi::VSPluginFunction,
    ) -> *const c_char {
        (self.handle.as_ref().getPluginFunctionReturnType.unwrap())(func)
    }
}

impl MessageType {
    #[inline]
    fn ffi_type(self) -> c_int {
        let rv = match self {
            MessageType::Debug => ffi::VSMessageType_mtDebug,
            MessageType::Warning => ffi::VSMessageType_mtWarning,
            MessageType::Critical => ffi::VSMessageType_mtCritical,
            MessageType::Fatal => ffi::VSMessageType_mtFatal,
        };
        rv as c_int
    }

    #[inline]
    #[expect(dead_code)]
    fn from_ffi_type(x: c_int) -> Option<Self> {
        match x {
            x if x == ffi::VSMessageType_mtDebug as c_int => Some(MessageType::Debug),
            x if x == ffi::VSMessageType_mtWarning as c_int => Some(MessageType::Warning),
            x if x == ffi::VSMessageType_mtCritical as c_int => Some(MessageType::Critical),
            x if x == ffi::VSMessageType_mtFatal as c_int => Some(MessageType::Fatal),
            _ => None,
        }
    }
}
