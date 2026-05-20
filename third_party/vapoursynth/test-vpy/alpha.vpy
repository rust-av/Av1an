import vapoursynth as vs
from vapoursynth import core
video = core.std.BlankClip(width = 1920,
                           height = 1080,
                           format = vs.RGB24,
                           length = 100,
                           fpsnum = 60,
                           fpsden = 1,
                           color = [0, 255, 0])
alpha = core.std.BlankClip(width = 1920,
                           height = 1080,
                           format = vs.GRAY8,
                           length = 100,
                           fpsnum = 60,
                           fpsden = 1,
                           color = [128])
video.set_output(alpha = alpha)
