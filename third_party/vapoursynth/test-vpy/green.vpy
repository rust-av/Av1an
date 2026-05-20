import vapoursynth as vs
from vapoursynth import core
video = core.std.BlankClip(width = 1920,
                           height = 1080,
                           format = vs.RGB24,
                           length = 100,
                           fpsnum = 60,
                           fpsden = 1,
                           color = [0, 255, 0])
#  video = core.resize.Bicubic(video, format = vs.YUV444P8, matrix_s = '709')
video.set_output()
