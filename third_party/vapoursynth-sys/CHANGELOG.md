# Changelog

## v0.6.0 (2026-04-26)

- Fix compat for Vapoursynth R74.
- This requires a breaking change where `getVSScriptAPI` must now be accessed through `VSScriptAPILoader`

## v0.5.0 (2025-11-29)

- Replace with bindings to v4 VapourSynth API

## v0.4.1 (14th Sep 2024)

- Added default library paths for macOS for automatic detection.

## v0.4 (12th Jul 2022)

- Migrated to the 2021 edition.
- Updated dependencies.

## v0.3 (22nd Oct 2019)

- Added support for VapourSynth API 3.6 (R47)
- Added more default VapourSynth .lib folders on Windows: the automatic
  detection should now work with R46 and above in various configurations
- Changed some Clippy attributes into their newer versions (bumps minimum Rust)

### v0.2.1 (16th Jun 2018)

- Added missing plugin-related types
- Silenced some clippy warnings

## v0.2.0 (24th Mar 2018)

- Added support for targetting 32-bit Windows
- Added automatic detection of common Windows VapourSynth library dirs

### v0.1.4 (11th Mar 2018)

- Added an AppVeyor badge
- Added a link to vapoursynth-rs in the README

### v0.1.3 (18th Feb 2018)

- Added a docs.rs badge to the README
- Added CHANGELOG.md

### v0.1.2 (18th Feb 2018)

- Changed the documentation to be hosted at docs.rs

### v0.1.1 (2nd Feb 2018)

- Added new bindings from VSScript API 3.2

## v0.1.0

- Initial release
