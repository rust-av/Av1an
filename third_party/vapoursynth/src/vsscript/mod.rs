//! VapourSynth script-related things.

use std::collections::HashSet;
use std::env;
use std::process::Command;
use std::ptr::NonNull;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use vapoursynth_sys::{self as ffi, VSSCRIPT_LIB_NAMES, VSSCRIPT_PATH_VARIABLE, VSScriptAPILoader};

/// A wrapper for the VSScript API.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VSScriptAPI {
    handle: NonNull<ffi::VSSCRIPTAPI>,
}

unsafe impl Send for VSScriptAPI {}
unsafe impl Sync for VSScriptAPI {}

static VSSCRIPT_API_LOADER: OnceLock<Option<VSScriptAPILoader>> = OnceLock::new();

fn discover_vsscript_path() -> Option<String> {
    let output = Command::new("vapoursynth")
        .arg("get-vsscript")
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path = String::from_utf8(output.stdout).ok()?;
    let path = path.trim();

    (!path.is_empty()).then(|| path.to_owned())
}

fn candidate_paths(env_path: Option<String>, discovered_path: Option<String>) -> Vec<String> {
    let mut candidates = Vec::with_capacity(2 + VSSCRIPT_LIB_NAMES.len());
    let mut seen = HashSet::new();

    if let Some(path) = env_path.filter(|path| !path.is_empty()) {
        seen.insert(path.clone());
        candidates.push(path);
    }

    if let Some(path) = discovered_path.filter(|path| !path.is_empty()) {
        if seen.insert(path.clone()) {
            candidates.push(path);
        }
    }

    for path in VSSCRIPT_LIB_NAMES {
        let path = (*path).to_owned();
        if seen.insert(path.clone()) {
            candidates.push(path);
        }
    }
    candidates
}

impl VSScriptAPI {
    /// Retrieves the VSScript API.
    ///
    /// Returns `None` on error, for example if the requested API version is not supported.
    #[inline]
    pub(crate) fn get() -> Option<Self> {
        // Check if we already have loaded the library
        let handle = VSSCRIPT_API_LOADER
            .get_or_init(|| {
                // Attempt opening the VSScript library
                let env_path = env::var(VSSCRIPT_PATH_VARIABLE).ok();
                let discovered_path = discover_vsscript_path();
                for path in candidate_paths(env_path, discovered_path) {
                    if let Ok(loader) = unsafe { VSScriptAPILoader::new(path.as_str()) } {
                        let version = ffi::VSSCRIPT_API_MAJOR << 16 | ffi::VSSCRIPT_API_MINOR;
                        let handle =
                            unsafe { loader.getVSScriptAPI(version as i32) } as *mut ffi::VSSCRIPTAPI;

                        if handle.is_null() {
                            continue;
                        }

                        let api_version = unsafe { ((*handle).getAPIVersion.unwrap())() };
                        let major = api_version >> 16;
                        let minor = api_version & 0xFFFF;

                        if major as u32 != ffi::VSSCRIPT_API_MAJOR
                            || (minor as u32) < ffi::VSSCRIPT_API_MINOR
                        {
                            continue;
                        }

                        loader.RAW_VSSCRIPT_API.store(handle, Ordering::Relaxed);
                        return Some(loader);
                    }
                }
                None
            })
            .as_ref()
            .map(|loader| loader.RAW_VSSCRIPT_API.load(Ordering::Relaxed));

        if let Some(ptr) = handle
            && !ptr.is_null()
        {
            Some(Self {
                handle: unsafe { NonNull::new_unchecked(ptr) },
            })
        } else {
            None
        }
    }

    #[inline]
    pub(crate) fn handle(&self) -> &ffi::VSSCRIPTAPI {
        unsafe { self.handle.as_ref() }
    }
}

mod errors;
pub use self::errors::{Error, VSScriptError};

mod environment;
pub use self::environment::{Environment, EvalFlags};
