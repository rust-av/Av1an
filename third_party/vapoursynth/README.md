# vapoursynth-rs

[![crates.io](https://img.shields.io/crates/v/vapoursynth.svg)](https://crates.io/crates/vapoursynth)
[![Documentation](https://docs.rs/vapoursynth/badge.svg)](https://docs.rs/vapoursynth)
[![Actions Status](https://github.com/rust-av/vapoursynth-rs/workflows/vapoursynth/badge.svg)](https://github.com/rust-av/vapoursynth-rs/actions)

[ChangeLog](https://github.com/rust-av/vapoursynth-rs/blob/master/vapoursynth/CHANGELOG.md)

[Documentation for the master branch with all features enabled](https://yalter.github.io/vapoursynth-rs)

A safe wrapper for [VapourSynth](https://github.com/vapoursynth/vapoursynth), written in Rust.

The primary goal is safety (that is, safe Rust code should not trigger undefined behavior), and secondary goals include performance and ease of use.

## Functionality

Most of the VapourSynth API is covered. It's possible to evaluate `.vpy` scripts, access their properties and output, retrieve frames; enumerate loaded plugins and invoke their functions as well as create VapourSynth filters.

For an example usage see [examples/vspipe.rs](https://github.com/rust-av/vapoursynth-rs/blob/master/vapoursynth/examples/vspipe.rs), a complete reimplementation of VapourSynth's [vspipe](https://github.com/vapoursynth/vapoursynth/blob/master/src/vspipe/vspipe.cpp) in safe Rust utilizing this crate.

For a VapourSynth plugin example see [sample-plugin](https://github.com/rust-av/vapoursynth-rs/blob/master/sample-plugin) which implements some simple filters.

## vapoursynth-sys

[![crates.io](https://img.shields.io/crates/v/vapoursynth-sys.svg)](https://crates.io/crates/vapoursynth-sys)
[![Documentation](https://docs.rs/vapoursynth-sys/badge.svg)](https://docs.rs/vapoursynth-sys)

[ChangeLog](https://github.com/rust-av/vapoursynth-rs/blob/master/vapoursynth-sys/CHANGELOG.md)

Raw bindings to [VapourSynth](https://github.com/vapoursynth/vapoursynth).

## Supported Versions

All VapourSynth and VSScript API versions starting with 4.0 are supported. By default the crates use the 4.0 feature set, and link to both the VapourSynth and VSScript libraries.

## Building

Make sure you have the corresponding libraries available if you enable the linking features. You can use the `VAPOURSYNTH_LIB_DIR` environment variable to specify a custom directory with the library files.

On Windows the easiest way is to use the VapourSynth installer (make sure the VapourSynth SDK is checked). The crate should pick up the library directory automatically. If it doesn't or if you're cross-compiling, set `VAPOURSYNTH_LIB_DIR` to `<path to the VapourSynth installation>\sdk\lib64` or `<...>\lib32`, depending on the target bitness.

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.
