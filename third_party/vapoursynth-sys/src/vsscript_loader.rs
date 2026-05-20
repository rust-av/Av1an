use std::sync::atomic::AtomicPtr;

#[cfg(all(unix, feature = "vsscript-r73-compat"))]
use libloading::os::unix::{RTLD_GLOBAL, RTLD_NOW};

pub const VSSCRIPT_PATH_VARIABLE: &str = "VSSCRIPT_PATH";

pub const VSSCRIPT_LIB_NAMES: &[&str] = if cfg!(target_os = "windows") {
    &["VSScript.dll"]
} else if cfg!(target_os = "macos") {
    &["libvsscript.dylib", "libvapoursynth-script.dylib"]
} else {
    &["libvsscript.so", "libvapoursynth-script.so"]
};

pub struct VSScriptAPILoader {
    __library: ::libloading::Library,
    pub getVSScriptAPI: Result<
        unsafe extern "C" fn(version: ::std::os::raw::c_int) -> *const VSSCRIPTAPI,
        ::libloading::Error,
    >,

    /// A cached VSScript API pointer.
    ///
    /// Internal convenience cache, shares a lifetime with Library
    pub RAW_VSSCRIPT_API: AtomicPtr<VSSCRIPTAPI>,
}

impl VSScriptAPILoader {
    /// # Safety
    /// Refer to [`libloading::Library`]
    pub unsafe fn new<P>(path: P) -> Result<Self, ::libloading::Error>
    where
        P: libloading::AsFilename,
    {
        unsafe {
            // Older versions don't seem compatible with default flags on UNIX (RTLD_LOCAL)
            #[cfg(all(unix, feature = "vsscript-r73-compat"))]
            let library =
                ::libloading::os::unix::Library::open(Some(path), RTLD_NOW | RTLD_GLOBAL)?;
            #[cfg(not(all(unix, feature = "vsscript-r73-compat")))]
            let library = ::libloading::Library::new(path)?;

            Self::from_library(library)
        }
    }

    /// # Safety
    /// Refer to [`libloading::Library`]
    pub unsafe fn from_library<L>(library: L) -> Result<Self, ::libloading::Error>
    where
        L: Into<::libloading::Library>,
    {
        let __library = library.into();
        let getVSScriptAPI = unsafe { __library.get(b"getVSScriptAPI\0").map(|sym| *sym) };

        Ok(VSScriptAPILoader {
            __library,
            getVSScriptAPI,
            RAW_VSSCRIPT_API: AtomicPtr::new(std::ptr::null_mut()),
        })
    }

    /// # Safety
    pub unsafe fn getVSScriptAPI(&self, version: ::std::os::raw::c_int) -> *const VSSCRIPTAPI {
        unsafe {
            (self
                .getVSScriptAPI
                .as_ref()
                .expect("Expected function, got error."))(version)
        }
    }
}
