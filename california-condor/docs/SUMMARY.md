# Summary

[Introduction](index.md)

# Getting Started

- [Guide](guide.md)
- [Configuration](configuration.md)
- [Screenshots](screenshots.md)

# Commands

- [av1an](commands/condor.md)
  - [init](commands/init.md)
  - [detect-scenes](commands/detect-scenes.md)
  - [zone](commands/zone.md)
  - [scenes](commands/scenes.md)
  - [detect-noise](commands/detect-noise.md)
  - [scale-noise](commands/scale-noise.md)
  - [scale-speed](commands/scale-speed.md)
  - [benchmark](commands/benchmark.md)
  - [encode](commands/encode.md)
  - [target-quality](commands/target-quality.md)
  - [optimize-bitrate](commands/optimize-bitrate.md)
  - [quality-check](commands/quality-check.md)
  - [concatenate](commands/concatenate.md)
  - [clean](commands/clean.md)
- [Editing saved settings](commands/config-editing.md)

# Configuration Reference

- [condor.json schema](configuration/index.md)
- [condor](configuration/condor/index.md)
  - [input](configuration/condor/input.md)
  - [output](configuration/condor/output.md)
  - [encoder](configuration/condor/encoder.md)
  - [scene](configuration/condor/scene.md)
  - [sequence_config](configuration/condor/sequence_config.md)
  - [Step configs](configuration/condor/sequence-config/index.md)
    - [scene-detector](configuration/condor/sequence-config/scene-detector.md)
    - [zoner](configuration/condor/sequence-config/zoner.md)
    - [noise-detector](configuration/condor/sequence-config/noise-detector.md)
    - [noise-scaler](configuration/condor/sequence-config/noise-scaler.md)
    - [benchmarker](configuration/condor/sequence-config/benchmarker.md)
    - [parallel-encoder](configuration/condor/sequence-config/parallel-encoder.md)
    - [scene-concatenator](configuration/condor/sequence-config/scene-concatenator.md)
    - [target-quality](configuration/condor/sequence-config/target-quality.md)
    - [quality-check](configuration/condor/sequence-config/quality-check.md)
    - [bitrate-optimizer](configuration/condor/sequence-config/bitrate-optimizer.md)
    - [speed-scaler](configuration/condor/sequence-config/speed-scaler.md)

# Reference

- [Types](types.md)
  - [Decoder](types/decoder.md)
  - [Encoder](types/encoder.md)
  - [Encoder parameters](types/encoder-params.md)
  - [Filters](types/filters.md)
  - [Scene detection](types/scene-detection.md)
  - [Concatenation](types/concatenation.md)
  - [Quality metric](types/quality-metric.md)
  - [Quality profile](types/quality-profile.md)
  - [Photon noise](types/photon-noise.md)
  - [Zone](types/zone.md)
  - [VapourSynth args](types/vs-args.md)
- [Encoders](encoders/aomenc.md)
  - [aomenc](encoders/aomenc.md)
  - [SVT-AV1](encoders/svt-av1.md)
  - [rav1e](encoders/rav1e.md)
  - [vpx](encoders/vpx.md)

# Installing

- [Installation](installation.md)
- [Docker](docker.md)

# Developing

- [Compiling](compiling.md)
- [Contributing](contributing.md)
