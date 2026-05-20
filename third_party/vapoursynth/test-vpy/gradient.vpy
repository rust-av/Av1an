import vapoursynth as vs
from vapoursynth import core

def pixel(color):
    return core.std.BlankClip(width = 1,
                              height = 1,
                              format = vs.RGB24,
                              length = 1,
                              color = color)

def row(red):
    return core.std.StackHorizontal(
        [pixel([red, green * 16, 0]) for green in range(16)])

clip = core.std.StackVertical([row(red * 16) for red in range(16)])
#  clip = core.resize.Lanczos(clip, format = vs.YUV444P8, matrix_s = '709')
clip.set_output()
