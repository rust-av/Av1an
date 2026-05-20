//! Video clip formats.

use std::fmt::Debug;
use std::ops::Deref;

use vapoursynth_sys as ffi;

use crate::format::Format;

/// Represents video resolution.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Resolution {
    /// Width of the clip, greater than 0.
    pub width: usize,

    /// Height of the clip, greater than 0.
    pub height: usize,
}

/// Represents video framerate.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub struct Framerate {
    /// FPS numerator, greater than 0.
    pub numerator: u64,

    /// FPS denominator, greater than 0.
    pub denominator: u64,
}

/// Represents a property that can be either constant or variable, like the resolution or the
/// framerate.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Property<T: Debug + Clone + Copy + Eq + PartialEq> {
    /// This property is variable.
    Variable,

    /// This property is constant.
    Constant(T),
}

/// Contains information about a video clip.
#[derive(Debug, Copy, Clone)]
pub struct VideoInfo<'core> {
    /// Format of the clip.
    pub format: Format<'core>,

    /// Framerate of the clip.
    pub framerate: Property<Framerate>,

    /// Resolution of the clip.
    pub resolution: Property<Resolution>,

    /// Length of the clip, greater than 0.
    pub num_frames: usize,
}

impl<'core> VideoInfo<'core> {
    /// Creates a `VideoInfo` from a raw pointer.
    ///
    /// # Safety
    /// The caller must ensure `ptr` and the lifetime is valid.
    pub(crate) unsafe fn from_ptr(ptr: *const ffi::VSVideoInfo) -> Self {
        let info = unsafe { &*ptr };

        debug_assert!(info.fpsNum >= 0);
        debug_assert!(info.fpsDen >= 0);
        debug_assert!(info.width >= 0);
        debug_assert!(info.height >= 0);
        debug_assert!(info.numFrames >= 0);

        let format = unsafe { Format::from_ptr(&info.format) };

        let framerate = if info.fpsNum == 0 {
            debug_assert!(info.fpsDen == 0);
            Property::Variable
        } else {
            debug_assert!(info.fpsDen != 0);
            Property::Constant(Framerate {
                numerator: info.fpsNum as _,
                denominator: info.fpsDen as _,
            })
        };

        let resolution = if info.width == 0 {
            debug_assert!(info.height == 0);
            Property::Variable
        } else {
            debug_assert!(info.height != 0);
            Property::Constant(Resolution {
                width: info.width as _,
                height: info.height as _,
            })
        };

        let num_frames = {
            debug_assert!(info.numFrames != 0);
            info.numFrames as _
        };

        Self {
            format,
            framerate,
            resolution,
            num_frames,
        }
    }

    /// Converts the Rust struct into a C struct.
    pub(crate) fn ffi_type(self) -> ffi::VSVideoInfo {
        let format = self.format.deref();

        let (fps_num, fps_den) = match self.framerate {
            Property::Variable => (0, 0),
            Property::Constant(Framerate {
                numerator,
                denominator,
            }) => (numerator as i64, denominator as i64),
        };

        let (width, height) = match self.resolution {
            Property::Variable => (0, 0),
            Property::Constant(Resolution { width, height }) => (width as i32, height as i32),
        };

        let num_frames = self.num_frames as i32;

        ffi::VSVideoInfo {
            format: *format,
            fpsNum: fps_num,
            fpsDen: fps_den,
            width,
            height,
            numFrames: num_frames,
        }
    }
}

impl<T> From<T> for Property<T>
where
    T: Debug + Clone + Copy + Eq + PartialEq,
{
    #[inline]
    fn from(x: T) -> Self {
        Property::Constant(x)
    }
}
