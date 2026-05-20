//!  A safe wrapper for [VapourSynth](https://github.com/vapoursynth/vapoursynth), written in Rust.
//!
//! The primary goal is safety (that is, safe Rust code should not trigger undefined behavior), and
//! secondary goals include performance and ease of use.
//!
//! ## Functionality
//!
//! Most of the VapourSynth API is covered. It's possible to evaluate `.vpy` scripts, access their
//! properties and output, retrieve frames; enumerate loaded plugins and invoke their functions as
//! well as create VapourSynth filters.
//!
//! For an example usage see
//! [examples/vspipe.rs](https://github.com/rust-av/vapoursynth-rs/blob/master/vapoursynth/examples/vspipe.rs),
//! a complete reimplementation of VapourSynth's
//! [vspipe](https://github.com/vapoursynth/vapoursynth/blob/master/src/vspipe/vspipe.cpp) in safe
//! Rust utilizing this crate.
//!
//! For a VapourSynth plugin example see
//! [sample-plugin](https://github.com/rust-av/vapoursynth-rs/blob/master/sample-plugin) which
//! implements some simple filters.
//!
//! ## Short example
//!
//! ```no_run
//! # extern crate vapoursynth;
//! # use anyhow::Error;
//! #
//! # fn foo() -> Result<(), Error> {
//! use vapoursynth::prelude::*;
//!
//! let env = Environment::from_file("test.vpy", EvalFlags::SetWorkingDir)?;
//! let node = env.get_output(0)?.0; // Without `.0` for VSScript API 3.0
//! let frame = node.get_frame(0)?;
//!
//! println!("Resolution: {}×{}", frame.width(0), frame.height(0));
//! # Ok(())
//! # }
//! # fn main() {
//! # }
//! ```
//!
//! ## Plugins
//!
//! To make a VapourSynth plugin, start by creating a new Rust library with
//! `crate-type = ["cdylib"]`. Then add filters by implementing the `plugins::Filter` trait. Bind
//! them to functions by implementing `plugins::FilterFunction`, which is much more easily done via
//! the `make_filter_function!` macro. Finally, put `export_vapoursynth_plugin!` at the top level
//! of `src/lib.rs` to export the functionality.
//!
//! **Important note:** due to what seems to be a
//! [bug](https://github.com/rust-lang/rust/issues/50176) in rustc, it's impossible to make plugins
//! on the `i686-pc-windows-gnu` target (all other variations of `x86_64` and `i686` do work).
//! Please use `i686-pc-windows-msvc` for an i686 Windows plugin.
//!
//! ## Short plugin example
//!
//! ```no_run
//! #[macro_use]
//! extern crate vapoursynth;
//!
//! use anyhow::{anyhow, Error};
//! use vapoursynth::prelude::*;
//! use vapoursynth::core::CoreRef;
//! use vapoursynth::plugins::{Filter, FilterArgument, FrameContext, Metadata};
//! use vapoursynth::video_info::VideoInfo;
//!
//! // A simple filter that passes the frames through unchanged.
//! struct Passthrough<'core> {
//!     source: Node<'core>,
//! }
//!
//! impl<'core> Filter<'core> for Passthrough<'core> {
//!     fn video_info(&self, _api: API, _core: CoreRef<'core>) -> Vec<VideoInfo<'core>> {
//!         vec![self.source.info()]
//!     }
//!
//!     fn get_frame_initial(
//!         &self,
//!         _api: API,
//!         _core: CoreRef<'core>,
//!         context: FrameContext,
//!         n: usize,
//!     ) -> Result<Option<FrameRef<'core>>, Error> {
//!         self.source.request_frame_filter(context, n);
//!         Ok(None)
//!     }
//!
//!     fn get_frame(
//!         &self,
//!         _api: API,
//!         _core: CoreRef<'core>,
//!         context: FrameContext,
//!         n: usize,
//!     ) -> Result<FrameRef<'core>, Error> {
//!         self.source
//!             .get_frame_filter(context, n)
//!             .ok_or(anyhow!("Couldn't get the source frame"))
//!     }
//! }
//!
//! make_filter_function! {
//!     PassthroughFunction, "Passthrough"
//!
//!     fn create_passthrough<'core>(
//!         _api: API,
//!         _core: CoreRef<'core>,
//!         clip: Node<'core>,
//!     ) -> Result<Option<Box<dyn Filter<'core> + 'core>>, Error> {
//!         Ok(Some(Box::new(Passthrough { source: clip })))
//!     }
//! }
//!
//! export_vapoursynth_plugin! {
//!     Metadata {
//!         identifier: "com.example.passthrough",
//!         namespace: "passthrough",
//!         name: "Example Plugin",
//!         read_only: true,
//!     },
//!     [PassthroughFunction::new()]
//! }
//! # fn main() {
//! # }
//! ```
//!
//! Check [sample-plugin](https://github.com/rust-av/vapoursynth-rs/blob/master/sample-plugin) for
//! an example plugin which exports some simple filters.
//!
//! ## Supported Versions
//!
//! All VapourSynth and VSScript API versions starting with 4.0 are supported.
//!
//! ## Building
//!
//! Make sure you have the corresponding libraries available if you enable the linking features.
//! You can use the `VAPOURSYNTH_LIB_DIR` environment variable to specify a custom directory with
//! the library files.
//!
//! On Windows the easiest way is to use the VapourSynth installer (make sure the VapourSynth SDK
//! is checked). The crate should pick up the library directory automatically. If it doesn't or if
//! you're cross-compiling, set `VAPOURSYNTH_LIB_DIR` to
//! `<path to the VapourSynth installation>\sdk\lib64` or `<...>\lib32`, depending on the target
//! bitness.

#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(test)]
pub extern crate vapoursynth_sys;

// Re-export vapoursynth_sys as ffi for use in macros
#[doc(hidden)]
pub use vapoursynth_sys as ffi;

pub mod api;
pub mod component;
pub mod core;
pub mod format;
pub mod frame;
pub mod function;
pub mod map;
pub mod node;
pub mod plugin;
pub mod plugins;
pub mod video_info;
pub mod vsscript;

pub mod prelude {
    //! The VapourSynth prelude.
    //!
    //! Contains the types you most likely want to import anyway.
    pub use super::api::{API, MessageType};
    pub use super::component::Component;
    pub use super::format::{ColorFamily, PresetFormat, SampleType};
    pub use super::frame::{Frame, FrameRef, FrameRefMut};
    pub use super::map::{Map, OwnedMap, ValueType};
    pub use super::node::{GetFrameError, Node};
    pub use super::plugin::Plugin;
    pub use super::video_info::Property;
    pub use super::vsscript::{self, Environment, EvalFlags};
}

pub use anyhow;

mod tests;
