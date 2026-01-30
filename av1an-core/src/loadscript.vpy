import os
import vapoursynth as vs

core = vs.core

source = os.environ.get("AV1AN_SOURCE", None)
chunk_method = os.environ.get("AV1AN_CHUNK_METHOD", None)
perform_scene_detection = globals().get("AV1AN_PERFORM_SCENE_DETECTION", None)
cache_mode = os.environ.get("AV1AN_CACHE_MODE", None)
cache_file = os.environ.get("AV1AN_CACHE_FILE", None)
pix_fmt = os.environ.get("AV1AN_PIXEL_FORMAT", None)

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
            try:
                video = core.bs.VideoSource(source, cachepath=cache_file, cachemode=4)
            except Exception:
                video = core.bs.VideoSource(source, cachepath=cache_file)
        else:
            # bestsource has the default behavior to store its index files in a user-specific directory
            # but for consistency, this setting makes it store the index file next to the video
            # as all the other source filters do
            video = core.bs.VideoSource(source, cachepath="/")

if perform_scene_detection is None:
    # Limit decoder resources when encoding since we will have multiple workers running
    core.num_threads = 1
    core.max_cache_size = 1024

if pix_fmt is not None:
    video = video.resize.Bicubic(format=vs.PresetVideoFormat[pix_fmt])

# Output video
video.set_output()
