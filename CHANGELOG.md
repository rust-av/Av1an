# Changelog

## Version 0.6.0

- feat: add `--pix-format-converter` option with `vs-resize` mode for reduced disk I/O during pixel format conversion
- feat: add Windows application manifest for long path support
- feat: exit earlier when output file exists and not overwriting
- fix: compatibility with Vapoursynth R74 and R75
- fix: disable mkvmerge chunking on Windows; increase Linux chunk limit to 960
- fix: auto-create scene detection output folder if it does not exist
- fix: ignore encoder pixel format check during scene detection
- perf: scene detection speedup via av-scenechange and v_frame dependency updates (requires Rust 1.95+)
- misc: migrate to Rust edition 2024
- misc: use Cargo resolver v3 for MSRV-stable dependency updates
- misc: clean up unused Vapoursynth loadscript variables from encoding callchain

## Version 0.5.2

- feat: implement cache mode toggle
- fix: fix compilation of Vapoursynth extensions on AArch64
- fix: improved frame count parsing

## Version 0.5.1

- fix: support Vapoursynth R73
- fix: fix extra splits rounding
- fix: support new svt-av1 progress format
- fix: handle ANSI color codes in progress parsing
- fix: fix 8-bit encoding with TQ probing rate of 1
- perf: significant speed increase in scenechange detection
- misc: improve error message if output chunk cannot be created
