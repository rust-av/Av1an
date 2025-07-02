import os
import vapoursynth
from vapoursynth import core

# Set cache size to 1GB
core.max_cache_size = 1024
# Limit to one thread
core.num_threads = 1

source = os.environ.get('AV1AN_SOURCE', None)
chunk_method = os.environ.get('AV1AN_CHUNK_METHOD', None)
cache_file = os.environ.get('AV1AN_CACHE_FILE', None)

# Default valid chunk methods
VALID_CHUNK_METHODS: list[str] = ['lsmash', 'ffms2', 'dgdecnv', 'bestsource']

# Ensure chunk_method is valid
if chunk_method not in VALID_CHUNK_METHODS:
    raise ValueError(f'Invalid chunk method: {chunk_method}')

# Check if source is provided
if not source:
    raise ValueError('Source path not provided')

# Ensure source exists
if not os.path.exists(source):
    raise ValueError('Source path does not exist')

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

# Output video
video.set_output()