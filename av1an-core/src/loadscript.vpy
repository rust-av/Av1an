import os
from vapoursynth import core

source = os.environ.get("AV1AN_SOURCE", None)
chunk_method = os.environ.get("AV1AN_CHUNK_METHOD", None)
perform_scene_detection = globals().get("AV1AN_PERFORM_SCENE_DETECTION", None)
cache_mode = os.environ.get("AV1AN_CACHE_MODE", None)
cache_file = os.environ.get("AV1AN_CACHE_FILE", None)

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
        if cache_mode == "temp":
            video = core.lsmas.LWLibavSource(source, cachefile=cache_file)
        else:
            video = core.lsmas.LWLibavSource(source)
    case "ffms2":
        if cache_mode == "temp":
            video = core.ffms2.Source(source, cachefile=cache_file)
        else:
            video = core.ffms2.Source(source)
    case "dgdecnv":
        video = core.dgdecodenv.DGSource(source)
    case "bestsource":
        if cache_mode == "temp":
            video = core.bs.VideoSource(source, cachepath=cache_file, cachemode=4)
        else:
            # bestsource has the default behavior to store its index files in a user-specific directory
            # but for consistency, this setting makes it store the index file next to the video
            # as all the other source filters do
            video = core.bs.VideoSource(source, cachepath="/")

if perform_scene_detection is None:
    # Limit decoder resources when encoding since we will have multiple workers running
    core.num_threads = 1
    core.max_cache_size = 1024


# Output video
video.set_output()
