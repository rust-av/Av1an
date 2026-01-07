# Condor

![Demonstration of the California Condor TUI](./california-condor/media/demo.avif)

Condor is a video encoding library and command-line tool designed to be as fast as possible, easy to use, and extremely extensible. Both are written in Rust and can be used on multiple platforms.

Condor is a fork of [Av1an](https://rust-av.github.io/Av1an/) but shares very little code - perhaps less than 5%. Despite having a CLI module and a core module, Av1an is not designed for use outside of the CLI. Condor started as a rewrite with the following goals:

1. Provide better feedback for programmatic use
2. Be easier to add features to and maintain
3. Allow users to modify or augment functionality as they see fit
4. Be merged back to Av1an

For more information on the differences between Condor and Av1an, see the [Av1an](#av1an) Section below.

## Features

* Cross-platform support
* Encoding with every CPU core
* Resumable processing
* TUI with support for piping
* Sequence modules for abstract processing

## Getting Started

Windows users can find prebuilt binaries of the [California Condor](./california-condor/README.md) CLI and TUI in the [Releases](https://github.com/BoatsMcGee/Av1an/releases) section. For other platforms, see the [Compiling](#compiling) section. For using the library, see the [Andean Condor](./andean-condor/readme.md) framework and library.

You may also need the following dependencies:

* [VapourSynth](https://github.com/vapoursynth/vapoursynth/releases) - Recommended decoder by default, but not strictly required
* The following VapourSynth plugins:
    * [BestSource](https://github.com/vapoursynth/bestsource) - Recommended plugin by default. Slow to initialize, but the most accurate
    * [L-SMASH](https://github.com/HomeOfAviSynthPlusEvolution/L-SMASH-Works) - Fast but can introduce artifacts
    * [DGDecNV](https://www.rationalqm.us/dgdecnv/dgdecnv.html) - Very fast and accurate chunking, but requires a compatible NVIDIA GPU with CUVID
    * [FFMS2](https://github.com/FFMS/ffms2) - Fast but can occasionally introduce artifacts
* At least one of the following encoders:
    * [aomenc](https://aomedia.googlesource.com/aom/)
    * [SvtAv1EncApp](https://gitlab.com/AOMediaCodec/SVT-AV1) - Recommended encoder by default
    * [rav1e](https://github.com/xiph/rav1e)
    * [vpxenc](https://chromium.googlesource.com/webm/libvpx/)
    * [x264](https://www.videolan.org/developers/x264.html)
    * [x265](https://www.videolan.org/developers/x265.html)
    * [vvenc](https://github.com/fraunhoferhhi/vvenc)
    * [FFmpeg](https://ffmpeg.org/download.html)
* [mkvmerge](https://mkvtoolnix.download/download.html) - Recommended concatenator by default, but not strictly required
* [FFmpeg](https://ffmpeg.org/download.html) - Alternative concatenator to mkvmerge

### California Condor CLI

California Condor CLI uses a configuration file to initialize, allow modification, and start encoding. Initialization can be skipped by using the `start` command and specifying both `--input` and `--output`. Below is an example that creates a configuration file for later modification and encoding. For more examples and information, see the [California Condor CLI](./condor/readme.md) documentation.

#### Initialize

As an example, let's initialize a new configuration file for encoding an `INPUT.mkv` file to `OUTPUT.mkv` using 4 workers, logging to `condor.log`, and using a temporary directory of `./deletemelater`. By default this decodes the input video with VapourSynth BestSource and formats it to YUV 4:2:0 10-bit, encodes it with SVT-AV1, and outputs to `OUTPUT.mkv` with mkvmerge. This creates a configuration file at `./deletemelater/condor.json` that we can modify ourselves or with the `config` command. For more information on default behaviors, see the [California Condor](./california-condor/readme.md) documentation.

```sh
condor --logs condor.log init INPUT.mkv OUTPUT.mkv --temp ./deletemelater --workers 4
```

#### Start

Continuing from our initialized configuration file we can specify parameters to use with the `start` command and override the ones in the configuration file. With the parameters below, Condor will use FFmpeg to output to `SMALLER.mp4` instead, use VapourSynth to trim the first 180 frames and resize to 720p YUV 4:2:0 10-bit, encode with rav1e using a quantizer of 80 and a speed of 8, use a photon noise ISO of 2400.

```sh
condor --logs condor.log start --output SMALLER.mp4 --concat ffmpeg --filters "resize:scaler=bilinear;width=1280;height=720;format=yuv420p10le" --filters "trim:start=180;" --encoder rav1e --params "--quantizer 80 --speed 8" --photon-noise 2400
```

### [Andean Condor](./andean-condor/README.md)

Andean Condor is a framework using the Condor library which consists of the following core components: `Input`, `Output`, `Encoder`, `Scene`, and `Processor`. Below is a minimal example. For more examples and information, see the [Condor](./andean-condor/README.md#API) API documentation.

<details>
<summary>Example Usage</summary>

```rust
use andean_condor::{
    core::{
        input::Input,
        output::Output,
        sequence::{
            parallel_encoder::ParallelEncoder,
            scene_concatenator::SceneConcatenator,
            scene_detect::SceneDetector,
            Sequence,
            SequenceCompletion,
            SequenceStatus,
            Status,
        },
        AndeanCondor,
        Condor,
        DefaultAndeanCondor,
    },
    models::{
        encoder::{
            cli_parameter::CLIParameter,
            photon_noise::PhotonNoise,
            Encoder,
            EncoderBase,
        },
        input::{Input as InputModel, VapourSynthImportMethod, VapourSynthScriptSource},
        output::Output as OutputModel,
        scene::Scene,
        sequence::{
            scene_concatenate::ConcatMethod,
            scene_detect::SceneDetectConfig,
            DefaultSequenceConfig,
            DefaultSequenceData,
        },
    },
    vapoursynth::{
        plugins::{
            dgdecodenv::DGSource,
            rescale::{ArtCNNModel, BorderHandling, Doubler, RescaleBuilder, VSJETKernel},
            resize::bilinear::Bilinear,
        },
        script_builder::{script::VapourSynthScript, VapourSynthPluginScript},
    },
};

let input_model = InputModel::VapourSynth {
    path: PathBuf::from("input.mkv"),
    import_method: VapourSynthImportMethod::BestSource {
        index: None,
    },
    cache_path: None,
};
let output_data = OutputModel {
    path: PathBuf::from("output.mkv"),
    tags: HashMap::new(),
    video_tags: HashMap::new(),
};
let scenes_directory = PathBuf::from("./scenes");

let input = Input::from_vapoursynth(&input_model, None).expect("input.mkv is valid");
let output = Output::new(&output_data).expect("output.mkv is valid");
let encoder = Encoder::SVTAV1 {
    executable: None,
    pass: EncoderBase::SVTAV1.default_passes(),
    parameters: EncoderBase::SVTAV1.default_parameters(),
};

let scene_detector = SceneDetector::new(SceneDetectionMethod::default());
let parallel_encoder = ParallelEncoder::new(2, &scenes_directory);
let scene_concatenator = SceneConcatenator::new(&scenes_directory, ConcatMethod::MKVMerge);

let condor = Condor {
    input,
    output,
    encoder,
    scenes: Vec::new(),
    save_callback: Box::new(|data| Ok(())),
    sequence_config: DefaultSequenceConfig::default(),
};
let sequences = vec![
    Box::new(scene_detector) as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
    Box::new(parallel_encoder) as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
    Box::new(scene_concatenator) as Box<dyn Sequence<DefaultSequenceData, DefaultSequenceConfig>>,
];

let mut andean_condor = DefaultAndeanCondor {
    condor,
    sequences,
};

let (event_tx, event_rx) = std::sync::mpsc::channel();
let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
std::thread::spawn(move || {
    for event in event_rx {
        println!("{:?}", event);
    }
});

let (sequence_index, ((), (validation_warnings, initialization_warnings, execution_warnings))) = andean_condor.process_all(event_tx, cancelled).expect("Andean Condor failed to process all sequences");

println!("Great job getting all the way down here!");
```

</details>

## Developing

See [Developing and Contributing](https://rust-av.github.io/Av1an/contributing) for a guide on developing Av1an and preparing for a Pull Request. Additions to Condor would be greatly appreciated, especially if they are related to merging back to Av1an.

## Av1an

Besides being library and a TUI, Condor offers several features not found in Av1an. Before discussing those, let's look at what Av1an currently has to offer over Condor:

* Decoding with FFmpeg
* Filtering with FFmpeg - *Planned.*
* Encoding audio with FFmpeg - *Planned.*
* Thread affinity
* Multithreaded decoding
* Scene sorting
* Target Quality - *Planned.*
* VMAF graphing - *Planned.*
* Specifying cache path - *Planned.*
* Force keyframes
* Max tries
* Zones - *Planned (California Condor).*
* docker - *Planned (California Condor).*
* loglevel - *Planned (California Condor).*
* no defaults - *Planned (California Condor).*
* VapourSynth arguments - *Planned (California Condor).*

Some of the unplanned features may no longer be necessary or desired in Condor, such as max tries and multithreaded decoding. Condor has been made more robust and may not need a retry mechanism for encoding. Condor decodes frames in a single thread and delivers it to encoders in multiple threads so while multithreaded decoding is theoretically faster, it is not necessary and uses more of system resources like RAM or VRAM for no realizable benefit.

Below are all the features Condor currently has over Av1an:

* VapourSynth Plugins - Several core plugins and features of VapourSynth are available in Andean Condor.
* Modify VapourSynth Inputs with VapourSynth Plugins like Trim, Crop, Bicubic, etc.
* VapourSynth Script Builder - Programmatically build a VapourSynth script text. Also adds support for Plugins and libraries written exclusively for Python such as RescaleBuilder and vs-jetpack.
* Decoding VapourSynth and FFMS2 without external binaries. Input can decode and stream a YUV4MPEG2 video. This makes the decoding simpler, robust, and less resource-intensive.
* Adds FFmpeg and VVenC as Encoder alternatives.
* Specify custom encoder executables by providing a path.
* Encoder parameters are stored as a HashMap of CLIParameters. A generic parser is also provided.
* Ability to encode a specific pass.
* Photon Noise: Besides ISO, chroma, width, and height, users can also specify a chroma ISO, and AR coefficients for luma, Cb, and Cr. This allows users to apply a custom noise table.
* Sequence, a core component of Andean Condor that provides an extensible module for working with Condor.
* No requirement to use any of the built-in Sequences. Make and use custom Sequences for unique workflows.
* Benchmarker Sequence to determine the optimal amount of workers for the Parallel Encoder Sequence.
* Better process cancellation for graceful shutdown.
* Progress feedback delivered via multi-producer-single-consumer channels.
* TUI - California Condor provides a TUI during scene detection and encoding.
* California Condor provides progress feedback when piped, allowing users to write scripts directly interfacing the CLI and receive real-time feedback.