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
cargo +stable test --profile ci -p av-metrics-vmaf -p av-metrics-vship -p av-metrics-fmetrics -- loader_policy_tests bare_vsscript_does_not_select_media_cwd qualified_relative_vsscript_directory_becomes_absolute --include-ignored --nocapture
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

The eight behavior-named DLL tests under
`loader_policy_tests::internal_integration::windows_loader` are ignored in ordinary
suites because they require this compiler; `--include-ignored` runs all eight for
each wrapper. They compile only on Windows. Pure diagnostic checks live under
`loader_policy_tests::unit`; host filesystem policy checks live under
`loader_policy_tests::internal_integration`. On Linux these check path selection,
not Windows DLL loading or dependency precedence.
Native Windows execution is required before claiming Windows loader acceptance.

## Runtime-only native discovery retention (Unix)

```sh
cargo +stable test --profile ci -p av-metrics-vmaf -p av-metrics-vship -p av-metrics-fmetrics --test ffi -- \
  vship_plugin_path_loads_fixture_without_bundled_sibling \
  vship_lib_dir_loads_fixture_without_bundled_sibling \
  staged_vship_loads_via_runtime_loader_path_from_separate_media_cwd \
  vmaf_lib_dir_loads_fixture_without_bundled_sibling \
  staged_vmaf_loads_via_runtime_loader_path_from_separate_media_cwd \
  fmetrics_lib_path_loads_fixture_without_bundled_sibling \
  fmetrics_lib_dir_loads_fixture_without_bundled_sibling \
  staged_fmetrics_loads_executable_sibling_without_loader_path_from_media_cwd --nocapture
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
python3 -B -m unittest discover -s .github/workflows/tests -p 'test_linux_build.py' -v
```

This offline regression executes the actual workflow version command and its
environment setup against an environment-reporting probe. Eleven poisoned
overrides must be absent while unrelated environment survives. It verifies that
command boundary, not archive contents or a hosted Actions run.

## Complete Python suite

Run both discovery roots from the repository root on Linux with Python 3, Bash,
Git and the existing PyYAML dependency available:

```sh
python3 -B -m unittest discover -s .github/scripts/tests -p 'test_*.py' -v
python3 -B -m unittest discover -s .github/workflows/tests -p 'test_*.py' -v
```

Both roots recurse into `unit/` and `integration/`. Unit suites check in-memory
parsers and read-only source/workflow policies; integrations execute scripts,
Git, downloads through owned doubles, or real archive I/O. Source-policy checks
are not native execution evidence.

Run a boundary separately without changing the discovery/import root:

```sh
python3 -B -m unittest discover -s .github/scripts/tests -p 'test_*.py' -k 'unit.*' -v
python3 -B -m unittest discover -s .github/scripts/tests -p 'test_*.py' -k 'integration.*' -v
python3 -B -m unittest discover -s .github/workflows/tests -p 'test_*.py' -k 'unit.*' -v
python3 -B -m unittest discover -s .github/workflows/tests -p 'test_*.py' -k 'integration.*' -v
```

Native-Windows installer cases are skipped on Linux. Run the Windows CI selections
on native Windows and the separate Windows DLL gate above before claiming Windows
acceptance; these discovery commands do not replace those gates.
The exact selections in `.github/workflows/windows-build.yml` run from
`.github/scripts/tests` and import only stdlib-based suites. They do not replace
the subsequent clean-extracted native archive gate.
