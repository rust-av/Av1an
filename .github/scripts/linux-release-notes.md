## Linux host requirements

The Linux archive bundles `av1an`, `libvmaf.so`, `libvship.so`,
`libfmetrics.so` and the VMAF models. The host still provides:

- `libffms2` (`libffms2-5` on Debian/Ubuntu, `ffms2` on Fedora/Arch):
  `av1an` links against it, so the loader fails before the program
  starts without it — even for `--version`
- glibc 2.39 or newer: the executable and every bundled library are built
  against that baseline, so older hosts (Ubuntu 22.04 or earlier, Debian 12,
  Rocky 9, Alpine's musl) cannot load them
- Encoder binaries: aomenc, SvtAv1EncApp, rav1e, vpxenc, x264, x265, ffmpeg, mkvmerge
- VapourSynth with its plugins (vsjetpack, vsfgs, ZooMVTools) plus FFmpegSource
  (`vs-ffms2`) for VapourSynth inputs and the plugin metric path; not needed for
  FFMS2 inputs scored by `libvmaf`/`libvship`/`libfmetrics`
- vulkan-icd-loader, for libvship
