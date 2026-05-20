# Changelog

## v0.5.6 (2026-04-27)

- Fix compatibility with Arch Linux packaging for Vapoursynth R75

## v0.5.5 (2026-04-27)

- Turn R73 compat flag on by default to avoid breaking change. This will be removed from the defaults in a future semver major release.

## v0.5.4 (2026-04-26) [yanked]

- Fix compat for Vapoursynth R74.

## v0.5.3 (2026-04-26)

- Minor optimizations to filter creation
- Remove useless `maybe_initialize` function

## v0.5.2 (2026-01-08)

- Reduce the number of extra imports needed when using plugin creation macros
- Re-export `anyhow` for convenience

## v0.5.1 (2026-01-02)

- Fix compilation issue on AArch64

## v0.5.0 (2025-11-29)

- Migrate to v4 VapourSynth API
- Migrate to 2024 edition

## v0.4 (12th Jul 2022)

- Migrated from `failure` to `thiserror` and `anyhow`.
- Migrated to the 2021 edition.
- Added `impl From<FormatID> for i32`.
- Changed integer values of the `ColorFamily` enum to match those used by VapourSynth.
- Updated dependencies.

## v0.3 (22nd Oct 2019)

- Added support for VapourSynth API 3.6 (R47):
  - `API::{add,remove}_message_handler` deprecate `API::set_message_handler`.
  - `CoreRef::info()` is implemented via the thread-safe `getCoreInfo2`
    automatically when using the `vapoursynth-api-36` feature.
  - Added `CoreRef::set_{max_cache_size,thread_count}`, which can now be
    implemented safely.
- Added `API::create_core`.
- Added more default VapourSynth .lib folders on Windows: the automatic
  detection should now work with R46 and above in various configurations.
- Changed some Clippy and Rustfmt attributes into their newer versions, fixed
  deprecation warnings (bumps minimum Rust).
- Fixed the lifetimes on the return values of
  `CoreRef::{get_plugin_by_id,get_plugin_by_namespace,plugins}`.

## v0.2 (16th Jun 2018)

- Added plugin support! That includes:
  - `plugins::{Metadata,Filter,FilterFunction}` types and traits for making plugins;
  - `export_vapoursynth_plugin!` macro for exporting a VapourSynth plugin;
  - `make_filter_function!` macro for making filters without much boilerplate.
- Added a sample plugin in the `sample-filter` folder.
- Added the `component::Component` trait and `Frame::plane*()` accessors for safely working with the pixel data without having to manually transmute slices, including an optional half-precision float support using the `half` crate.
- Added `plugin::Plugin` and other relevant APIs for enumerating plugins and invoking their functions.
- Added lifetime parameters to many types to fix soundness issues.
- Split `Frame` into `Frame`, `FrameRef`, `FrameRefMut`.
- Added the `map::Value` trait and generic `Map::{get,get_iter,set,append}()` functions.
- Added format caching in `Frame` to reduce the number of API calls needed.
- Added some convenience `From` impls.

### v0.1.2 (2nd Apr 2018)

- Fixed `Frame::data_row()` returning slices of incorrect rows (using the `plane` value instead of the `row` value).

### v0.1.1 (24th Mar 2018)

- Added support for targetting 32-bit Windows
- Added automatic detection of common Windows VapourSynth library dirs
- Fixed `Frame::data()` and `Frame::data_row()` returning slices of incorrect sizes (too short) for pixel formats with more than 1 byte per pixel

## v0.1.0

- Initial release
