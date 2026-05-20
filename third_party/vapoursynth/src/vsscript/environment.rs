use std::ffi::{CStr, CString};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::ptr;
use std::ptr::NonNull;
use vapoursynth_sys as ffi;

use crate::api::API;
use crate::core::CoreRef;
use crate::map::Map;
use crate::node::Node;
use crate::vsscript::errors::Result;
use crate::vsscript::*;

use crate::vsscript::VSScriptError;

/// VSScript file evaluation flags.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EvalFlags {
    Nothing,
    /// The working directory will be changed to the script's directory for the evaluation.
    SetWorkingDir,
}

/// Contains two possible variants of arguments to `Environment::evaluate_script()`.
#[derive(Clone, Copy)]
enum EvaluateScriptArgs<'a> {
    /// Evaluate a script contained in the string.
    Script(&'a str),
    /// Evaluate a script contained in the file.
    File(&'a Path, EvalFlags),
}

/// A wrapper for the VSScript environment.
#[derive(Debug)]
pub struct Environment {
    handle: NonNull<ffi::VSScript>,
}

unsafe impl Send for Environment {}
unsafe impl Sync for Environment {}

impl Drop for Environment {
    #[inline]
    fn drop(&mut self) {
        let api = VSScriptAPI::get().expect("VSScript API not available");
        unsafe {
            (api.handle().freeScript.unwrap())(self.handle.as_ptr());
        }
    }
}

impl Environment {
    /// Retrieves the VSScript error message.
    ///
    /// # Safety
    /// This function must only be called if an error is present.
    #[inline]
    unsafe fn error(&self) -> CString {
        let api = VSScriptAPI::get().expect("VSScript API not available");
        let message = (api.handle().getError.unwrap())(self.handle.as_ptr());
        CStr::from_ptr(message).to_owned()
    }

    /// Creates an empty script environment.
    ///
    /// Useful if it is necessary to set some variable in the script environment before evaluating
    /// any scripts.
    pub fn new() -> Result<Self> {
        let api = VSScriptAPI::get().expect("VSScript API not available");
        let handle = unsafe { (api.handle().createScript.unwrap())(ptr::null_mut()) };

        if handle.is_null() {
            Err(Error::ScriptCreationFailed)
        } else {
            Ok(Self {
                handle: unsafe { NonNull::new_unchecked(handle) },
            })
        }
    }

    /// Evaluates a script using the VSScript API.
    ///
    /// `self` is taken by a mutable reference mainly to ensure the atomicity of a call to
    /// `evaluateBuffer/evaluateFile` (a function that could produce an error) and the following call
    /// to `getError()`. If atomicity is not enforced, another thread could perform some
    /// operation between these two and clear or change the error message.
    fn evaluate_script(&mut self, args: EvaluateScriptArgs) -> Result<()> {
        let api = VSScriptAPI::get().expect("VSScript API not available");

        let rv = match args {
            EvaluateScriptArgs::Script(script) => {
                let script = CString::new(script)?;
                let filename = CString::new("<string>").unwrap();
                unsafe {
                    (api.handle().evaluateBuffer.unwrap())(
                        self.handle.as_ptr(),
                        script.as_ptr(),
                        filename.as_ptr(),
                    )
                }
            }
            EvaluateScriptArgs::File(path, flags) => {
                // Set working directory flag if requested
                if flags == EvalFlags::SetWorkingDir {
                    unsafe {
                        (api.handle().evalSetWorkingDir.unwrap())(self.handle.as_ptr(), 1);
                    }
                }

                // vsscript throws an error if the path is not valid UTF-8 anyway.
                let path_str = path.to_str().ok_or(Error::PathInvalidUnicode)?;
                let path_cstr = CString::new(path_str)?;

                let rv = unsafe {
                    (api.handle().evaluateFile.unwrap())(self.handle.as_ptr(), path_cstr.as_ptr())
                };

                // Reset working directory flag if it was set
                if flags == EvalFlags::SetWorkingDir {
                    unsafe {
                        (api.handle().evalSetWorkingDir.unwrap())(self.handle.as_ptr(), 0);
                    }
                }

                rv
            }
        };

        if rv != 0 {
            Err(VSScriptError::new(unsafe { self.error() }).into())
        } else {
            Ok(())
        }
    }

    /// Creates a script environment and evaluates a script contained in a string.
    #[inline]
    pub fn from_script(script: &str) -> Result<Self> {
        let mut environment = Self::new()?;
        environment.evaluate_script(EvaluateScriptArgs::Script(script))?;
        Ok(environment)
    }

    /// Creates a script environment and evaluates a script contained in a file.
    #[inline]
    pub fn from_file<P: AsRef<Path>>(path: P, flags: EvalFlags) -> Result<Self> {
        let mut environment = Self::new()?;
        environment.evaluate_script(EvaluateScriptArgs::File(path.as_ref(), flags))?;
        Ok(environment)
    }

    /// Evaluates a script contained in a string.
    #[inline]
    pub fn eval_script(&mut self, script: &str) -> Result<()> {
        self.evaluate_script(EvaluateScriptArgs::Script(script))
    }

    /// Evaluates a script contained in a file.
    #[inline]
    pub fn eval_file<P: AsRef<Path>>(&mut self, path: P, flags: EvalFlags) -> Result<()> {
        self.evaluate_script(EvaluateScriptArgs::File(path.as_ref(), flags))
    }

    /// Clears the script environment.
    ///
    /// Note: In VapourSynth v4, this is a no-op. To clear the environment,
    /// drop the Environment and create a new one.
    #[inline]
    pub fn clear(&self) {
        // The clearEnvironment function was removed in VapourSynth v4.
        // Users should drop and recreate the Environment instead.
    }

    /// Retrieves a node from the script environment. A node in the script must have been marked
    /// for output with the requested index. The second node, if any, contains the alpha clip.
    #[inline]
    pub fn get_output(&self, index: i32) -> Result<(Node<'_>, Option<Node<'_>>)> {
        // Node needs the API.
        API::get().ok_or(Error::NoAPI)?;

        let vsscript_api = VSScriptAPI::get().expect("VSScript API not available");
        let node_handle =
            unsafe { (vsscript_api.handle().getOutputNode.unwrap())(self.handle.as_ptr(), index) };

        if node_handle.is_null() {
            return Err(Error::NoOutput);
        }

        let node = unsafe { Node::from_ptr(node_handle) };

        // Get the alpha node separately
        let alpha_handle = unsafe {
            (vsscript_api.handle().getOutputAlphaNode.unwrap())(self.handle.as_ptr(), index)
        };
        let alpha_node = if alpha_handle.is_null() {
            None
        } else {
            Some(unsafe { Node::from_ptr(alpha_handle) })
        };

        Ok((node, alpha_node))
    }

    /// Cancels a node set for output. The node will no longer be available to `get_output()`.
    ///
    /// Note: In VapourSynth v4, this function has been removed. This is now a no-op that always
    /// returns Ok. To clear outputs, drop the Environment and create a new one.
    #[inline]
    pub fn clear_output(&self, _index: i32) -> Result<()> {
        // The clearOutput function was removed in VapourSynth v4.
        Ok(())
    }

    /// Retrieves the VapourSynth core that was created in the script environment. If a VapourSynth
    /// core has not been created yet, it will be created now, with the default options.
    pub fn get_core(&self) -> Result<CoreRef<'_>> {
        // CoreRef needs the API.
        API::get().ok_or(Error::NoAPI)?;

        let vsscript_api = VSScriptAPI::get().expect("VSScript API not available");
        let ptr = unsafe { (vsscript_api.handle().getCore.unwrap())(self.handle.as_ptr()) };
        if ptr.is_null() {
            Err(Error::NoCore)
        } else {
            Ok(unsafe { CoreRef::from_ptr(ptr) })
        }
    }

    /// Retrieves a variable from the script environment.
    pub fn get_variable(&self, name: &str, map: &mut Map) -> Result<()> {
        let vsscript_api = VSScriptAPI::get().expect("VSScript API not available");
        let name = CString::new(name)?;
        let rv = unsafe {
            (vsscript_api.handle().getVariable.unwrap())(
                self.handle.as_ptr(),
                name.as_ptr(),
                map.deref_mut(),
            )
        };
        if rv != 0 {
            Err(Error::NoSuchVariable)
        } else {
            Ok(())
        }
    }

    /// Sets variables in the script environment.
    pub fn set_variables(&self, variables: &Map) -> Result<()> {
        let vsscript_api = VSScriptAPI::get().expect("VSScript API not available");
        let rv = unsafe {
            (vsscript_api.handle().setVariables.unwrap())(self.handle.as_ptr(), variables.deref())
        };
        if rv != 0 {
            Err(Error::NoSuchVariable)
        } else {
            Ok(())
        }
    }

    /// Deletes a variable from the script environment.
    ///
    /// Note: In VapourSynth v4, the clearVariable function has been removed.
    /// This is now a no-op that always returns Ok.
    #[inline]
    pub fn clear_variable(&self, _name: &str) -> Result<()> {
        // The clearVariable function was removed in VapourSynth v4.
        Ok(())
    }
}
