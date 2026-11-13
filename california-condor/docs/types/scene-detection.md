# Scene Detection

`METHOD` selects the scene detection algorithm. Used by `detect-scenes --method` (see [detect-scenes]\(../commands/detect-scenes.md\)).

| Value       | Behavior                                                        |
| ----------- | --------------------------------------------------------------- |
| `none`      | No detection; chunks scenes by maximum length                   |
| `fast`      | av-scenechange fast algorithm                                   |
| `standard`  | av-scenechange standard algorithm                               |
| `transnetv2` | TransNetV2 neural network via ONNX Runtime                     |

### TransNetV2

`transnetv2` runs the TransNetV2 shot boundary network with ONNX Runtime.
Providers are picked at runtime and fall back to CPU automatically:
DirectML on Windows (AMD/Intel/NVIDIA), CoreML on macOS, plus opt-in build
features `cuda`, `rocm`, `webgpu`, and `openvino` for other GPU stacks.

The model (~31 MB) is never fetched at runtime. The release ships it at
`model/transnetv2.onnx` beside the executable, the same way the native metric
libraries ship beside it. For source builds, download it once from
<https://huggingface.co/elya5/transnetv2> — or run the installer, which
verifies the pinned SHA-256, downloads the model into the current directory,
and prints the `TRANSNETV2_MODEL_PATH` to set:

- Windows: `andean-condor/scripts/install-transnetv2-model-windows.ps1`
- Linux/macOS: `andean-condor/scripts/install-transnetv2-model-linux.sh`

Resolution order when no `model_path` is set on the TransNetV2 method:

1. `TRANSNETV2_MODEL_PATH` — the file itself
2. `model/transnetv2.onnx` beside the running executable
3. `transnetv2.onnx` beside the running executable
4. the user cache directory (`%LOCALAPPDATA%\condor\models` on Windows,
   `~/Library/Caches/condor/models` on macOS, `$XDG_CACHE_HOME` or
   `~/.cache/condor/models` elsewhere)

An explicit `model_path` on the method (or in `condor.json`) wins over all
of these and must exist.

Each run logs the execution provider the session actually runs on, e.g.
`TransNetV2 execution provider: DirectML`, or `CPU (fallback)` when no
provider could be registered — a provider the runtime lacks or whose device
cannot be initialized is logged by name as unavailable.

## Defaults

- `--method`: `standard`
- `--min-scene-seconds`: `1`
- `--max-scene-seconds`: `10`

Scene length bounds apply to all methods, including `none` (which cuts purely by maximum length).

## Examples

- `> av1an detect-scenes --method fast --min-scene-seconds 1 --max-scene-seconds 10`
- `> av1an detect-scenes --method none --max-scene-seconds 5`
