# Required Windows DLL search acceptance gate

Run on native Windows with Rust stable (MSRV 1.97) and a native MinGW
GCC-compatible C compiler/linker matching the Rust process architecture.
The compiler must support `-shared` and `-Wl,--out-implib,<path>`; point
`AV_METRICS_WINDOWS_CC` at its executable. No metric library, GPU, or downloaded
DLL fixture is needed. Keep the compiler's own runtime/tool directories on the
parent shell's PATH. The gate fails with prerequisites/compilation diagnostics
if the compiler is missing or incompatible; it does not silently skip.

From the repository root in PowerShell:

```powershell
$env:AV_METRICS_WINDOWS_CC = 'C:\msys64\mingw64\bin\gcc.exe'
cargo +stable test --profile ci -p av-metrics-vmaf -p av-metrics-vship -p av-metrics-fmetrics windows_loader -- --include-ignored --nocapture
```

Each wrapper tests its **actual production load helper** against freshly built
harmless marker DLLs. Eight bounded fresh child processes check a private sibling
dependency without PATH, rejection of media-CWD and ordinary-PATH DLL decoys,
absolute and relative explicit directory overrides through production candidate
builders, rejection of a DLL filename used as a directory override, original OS
error 126 for a missing dependency, and rejection of a
same-name media-CWD dependency shadow. A bare `VSSCRIPT_PATH=vsscript.dll` must
not create relative directory candidates. Child-only CWD/PATH/environment settings
avoid process-global mutations in the test runner. Temporary sources, import
libraries, and DLLs are removed when the parent test exits normally or unwinds.

The native test is ignored in ordinary suites because it requires this compiler;
`--include-ignored` makes it a required, actionable gate on Windows. It is compiled
only on Windows. Linux `loader_policy_tests` tests exercise production **path/policy selection**
using native Linux paths, not Windows DLL loading or dependency precedence.
Native Windows execution is required before claiming Windows loader acceptance.

## Runtime-only native discovery retention (Unix)

```sh
cargo +stable test --profile ci -p av-metrics-vmaf -p av-metrics-vship -p av-metrics-fmetrics runtime_overrides_and_staged_release -- --nocapture
```

These tests reuse the pinned C ABI fixtures and production scorers. Fresh children
run copied test executables from a separate media directory with cleared inherited
environment and poisoned, unconsumed build-host variables. Each runtime directory
override is tested alone, with no fixture beside the executable or loader path.
Then the fixture moves beside the executable and all overrides are unset:
fmetrics must discover that sibling directly; VMAF/vship use only the staged
runtime `LD_LIBRARY_PATH` (`DYLD_LIBRARY_PATH` on macOS) for bare-name loading.
Temporary libraries and executables are removed by the parent. These are retention
checks, not reproductions of a runtime failure: the removed build hints were inert.
Native Windows loader acceptance still requires the separate gate above.

### Linux archive smoke environment

The Linux release archive uses the packaged executable's `$ORIGIN` runpath,
not the test artifacts' injected `LD_LIBRARY_PATH`. Its workflow smoke check
clears native-library, model, and loader-path overrides locally before invoking
`./av1an --version`; build-tree exports remain available to other steps.
This removes runner-path overrides, not the platform's system-library fallback.

```sh
python3 test-support/m24_archive_smoke.py
```

This offline regression executes the actual workflow version command and its
environment setup against an environment-reporting probe. Eleven poisoned
overrides must be absent while unrelated environment survives. It verifies that
command boundary, not archive contents or a hosted Actions run.
