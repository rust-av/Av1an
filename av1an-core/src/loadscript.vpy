import os
import vapoursynth
from vapoursynth import core

# Set cache size to 1GB
core.max_cache_size = 1024

source = os.environ.get("AV1AN_SOURCE", None)
chunk_method = os.environ.get("AV1AN_CHUNK_METHOD", None)
cache_file = os.environ.get("AV1AN_CACHE_FILE", None)

# Scene Detection
perform_scene_detection = os.environ.get("AV1AN_PERFORM_SCENE_DETECTION", None)
downscale_height = os.environ.get("AV1AN_DOWNSCALE_HEIGHT", None)
pixel_format = os.environ.get("AV1AN_PIXEL_FORMAT", None)
scaler = os.environ.get("AV1AN_SCALER", None)

# Default valid chunk methods
VALID_CHUNK_METHODS: list[str] = ["lsmash", "ffms2", "dgdecnv", "bestsource"]

# Ensure chunk_method is valid
if chunk_method not in VALID_CHUNK_METHODS:
    raise ValueError(f"Invalid chunk method: {chunk_method}")

# Check if source is provided
if not source:
    raise ValueError("Source path not provided")

# Ensure source exists
if not os.path.exists(source):
    raise ValueError("Source path does not exist")

# Import video
match (chunk_method):  # type: ignore
    case "lsmash":
        video = core.lsmas.LWLibavSource(source, cachefile=cache_file)
    case "ffms2":
        video = core.ffms2.Source(source, cachefile=cache_file)
    case "dgdecnv":
        video = core.dgdecodenv.DGSource(source)
    case "bestsource":
        # Different versions of BestSource have different behaviors on Windows
        # Versions R1 and older support absolute paths just as Av1an originally expected (with .json extension)
        # Versions R8 and newer support absolute paths for cache files, but require setting cachemode to 4
        # Versions since ~R2 attempt to create a path stemming from CWD but using the path of the source and also appends the track index and a .bsindex extension, which is unexpected for Av1an
        # Unfortunately, BestSource is not keeping the reported version number updated properly so we cannot reliably determine if it supports absolute paths or not
        # At best, we can wrap an attempt in a try/except block as previous versions of BestSource should throw an exception if an invalid cachemode value is provided
        try:
            video = core.bs.VideoSource(source, cachepath=cache_file, cachemode=4)
        except Exception:
            # Installed BestSource version does not support absolute paths, fallback to default behavior
            video = core.bs.VideoSource(source, cachepath=cache_file)

# Scene Detection
# Bicubic is based on FFmpeg defaults. See https://ffmpeg.org/ffmpeg-scaler.html#toc-Scaler-Options
scaler_function = core.resize.Bicubic
# Map scaler parameter to equivalent vapoursynth scaler. See https://www.vapoursynth.com/doc/functions/video/resize.html#resize
if scaler is not None:
    match (scaler.lower()):
        case "fast_bilinear" | "bilinear": scaler_function = core.resize.Bilinear
        case "neighbor": scaler_function = core.resize.Point
        case _: scaler_function = core.resize.Bicubic

# Map pixel format to equivalent vapoursynth pixel format (Needs expansion)
if pixel_format is not None:
    match (pixel_format.lower()):
        case "yuv420p": pixel_format = vapoursynth.YUV420P8
        case "yuv420p10le": pixel_format = vapoursynth.YUV420P10
        case _: pixel_format = None

# Apply Scene Detection Processing
if perform_scene_detection is not None and downscale_height is not None or pixel_format is not None:
    # Ensure downscale_height is not greater than video height
    if downscale_height is not None:
        try:
            downscale_height = int(downscale_height)
        finally:
            downscale_height = min(downscale_height, video.height)
    # Maintain aspect ratio and ensure width is divisible by 2
    video = scaler_function(
        video,
        width=int(((video.width / video.height) * int(downscale_height)) // 2 * 2) if downscale_height is not None else video.width,
        height=int(downscale_height or video.height),
        format=pixel_format,
    )
else:
    # Limit to one thread when encoding
    core.num_threads = 1


# Output video
video.set_output()