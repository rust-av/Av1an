"""Offline native installers contracts."""

import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from support import git, init_repo


class NativeInstallerTests(unittest.TestCase):
    def _package_native(self, case: str) -> None:
        root = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="package native ") as directory:
            base = Path(directory)
            temp = base / "private temp"
            temp.mkdir()
            result = subprocess.run(
                ["pwsh", "-NoProfile", "-File", str(Path(__file__).parents[1] / "fixtures/native_installers.ps1"),
                 "-PackageCase", case], cwd=base, capture_output=True, text=True,
                env={**os.environ, "REPO": str(root), "FIXTURE": str(base),
                     "TEMP": str(temp), "TMP": str(temp)})
            logs = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, logs)
            self.assertIn(f"Package {case} verified", result.stdout)
            self.assertEqual(list(temp.iterdir()), [])

    @unittest.skipUnless(os.name == "nt", "native PowerShell pinned lookup fixture runs on Windows CI")
    def test_windows_missing_archive_never_uses_newer_index(self) -> None:
        """All mirrors 404; an available newer directory listing is never read."""
        self._package_native("missing-archive")

    @unittest.skipUnless(os.name == "nt", "native PowerShell pin validation fixture runs on Windows CI")
    def test_windows_vmaf_missing_or_invalid_package_digest_refuses_before_download_or_unpack(self) -> None:
        """A missing or malformed expected digest must not reach transport/tar."""
        for case in ("missing-pin", "invalid-pin"):
            with self.subTest(case=case):
                self._package_native(case)

    @unittest.skipUnless(os.name == "nt", "native PowerShell package hashing fixture runs on Windows CI")
    def test_windows_package_hash_precedes_unpack(self) -> None:
        """Keep real Save-File SHA256 checking with good/bad inert package bytes."""
        for case in ("good", "hash"):
            with self.subTest(case=case):
                self._package_native(case)

    @unittest.skipIf(os.name == "nt", "Linux Vship installer uses Bash")
    def test_linux_vship_cleans_early_failure_and_retries(self) -> None:
        """A late EXIT trap leaks the missing-tool checkout; all retries stay owned."""
        installer = Path(__file__).resolve().parents[4] / "av-metrics-vship/scripts/install-libvship-linux.sh"
        with tempfile.TemporaryDirectory(prefix="installer linux ") as directory:
            base = Path(directory)
            env, commits = self._installer_fixture(base)
            with patch.dict(os.environ, env, clear=True):
                git(base / "remote", "tag", "fixture", commits["pin"])
            tools = base / "bin"
            tools.mkdir()
            for name in ("mktemp", "rm", "git", "mkdir", "install", "grep", "stat"):
                executable = shutil.which(name)
                self.assertIsNotNone(executable, f"host prerequisite missing: {name}")
                (tools / name).symlink_to(executable)
            (tools / "clang").write_text("#!/bin/bash\nexit 91\n")
            (tools / "nm").write_text("#!/bin/bash\nprintf '000 T Vship_GetVersion\\n'; for n in {1..9}; do printf '000 T Vship_%s\\n' \"$n\"; done\n")
            make = tools / "make"
            body = '''#!/bin/bash
set -eu
test "$1" = -C && test "$3" = build && test "$4" = BACKEND=Vulkan
if [ "$MODE" = build ]; then echo 'installer fixture inert build failure' >&2; exit 79; fi
printf 'installer fixture inert library bytes\\n' > "$2/libvship.so"
'''
            for tool in (tools / "clang", tools / "nm"):
                tool.chmod(0o755)
            dl = base / "destination/dl"
            dl.mkdir()
            (dl / "sentinel").write_bytes(b"preexisting dl\x00")
            env.update(PATH=str(tools), VULKAN_SDK="fixture unused SDK", VSHIP_VERSION="fixture",
                       VSHIP_COMMIT=commits["pin"], VSHIP_REPO=str(base / "remote"))
            for mode, expected, error in (("missing", 1, "make not found"), ("fetch", 128, "does not exist"),
                                           ("build", 79, "installer fixture inert build failure"), ("success", 0, "Done.")):
                with self.subTest(mode=mode):
                    if mode != "missing":
                        make.write_text(body)
                        make.chmod(0o755)
                    selected = {**env, "MODE": mode}
                    if mode == "fetch":
                        selected["VSHIP_REPO"] = str(base / "absent remote")
                    result = subprocess.run(["/bin/bash", str(installer), "-Destination", str(base / "destination"), "-Quiet"],
                                            cwd=base, env=selected, capture_output=True, text=True)
                    logs = result.stdout + result.stderr
                    self.assertEqual(result.returncode, expected, logs)
                    self.assertIn(error, logs)
                    self.assertEqual(list((base / "temp").iterdir()), [], logs)
                    self.assertEqual((dl / "sentinel").read_bytes(), b"preexisting dl\x00")
                    self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                    target = base / "destination/libvship.so"
                    if mode == "success":
                        self.assertEqual(target.read_bytes(), b"installer fixture inert library bytes\n")
                    else:
                        self.assertFalse(target.exists(), logs)

    @unittest.skipUnless(os.name == "nt", "native PowerShell ownership fixture runs on Windows CI")
    def test_windows_installers_owned_scratch_survives_failures_retry_and_creation_collisions(self) -> None:
        """Real Get-Asset/Save-File and filesystem; only HTTP/unpack are inert doubles."""
        root = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="installer native ") as directory:
            base = Path(directory)
            temp = base / "private temp"
            temp.mkdir()
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(Path(__file__).parents[1] / "fixtures/native_installers.ps1")],
                                    cwd=base, capture_output=True, text=True,
                                    env={**os.environ, "REPO": str(root), "FIXTURE": str(base),
                                         "TEMP": str(temp), "TMP": str(temp)})
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(list(temp.iterdir()), [])

    @unittest.skipUnless(os.name == "nt", "native PowerShell PID ownership fixture runs on Windows CI")
    def test_windows_fmetrics_preserves_old_pid_checkout(self) -> None:
        """An unknown immutable pin must not adopt/delete an old same-PID directory."""
        root = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="installer pid ") as directory:
            base = Path(directory)
            env, _ = self._installer_fixture(base)
            zig = base / "zig.cmd"
            zig.write_text('''@echo off
if "%~1"=="version" (
  echo 0.16.0
  exit /b 0
)
exit /b 93
''')
            # Check the native stub independently before any installer/ownership path.
            version = subprocess.run([str(zig), "version"], cwd=base, env=env,
                                     capture_output=True, text=True)
            self.assertEqual(version.returncode, 0, version.stdout + version.stderr)
            self.assertEqual(version.stdout, "0.16.0\n")
            runner = base / "run.ps1"
            runner.write_text(r'''$ErrorActionPreference = 'Stop'
$old = Join-Path ([IO.Path]::GetTempPath()) "fmetrics-build-$PID"
New-Item -ItemType Directory -Path $old | Out-Null
$sentinel = Join-Path $old 'sentinel'
[IO.File]::WriteAllText($sentinel, 'old PID checkout')
try {
    try {
        & (Join-Path $env:REPO 'av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1') -Destination (Join-Path $env:FIXTURE 'destination') -ZigPath (Join-Path $env:FIXTURE 'zig.cmd') -Commit ('1' * 40)
        throw 'accepted unknown commit'
    } catch { if ($_.Exception.Message -notmatch 'Failed to fetch fmetrics commit') { throw } }
    if ([IO.File]::ReadAllText($sentinel) -ne 'old PID checkout') { throw 'old PID sentinel changed' }
    if (@(Get-ChildItem -LiteralPath ([IO.Path]::GetTempPath()) -Force).Count -ne 1) { throw 'new scratch leaked' }
} finally { Remove-Item -LiteralPath $old -Recurse -Force }
''')
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(runner)], cwd=base,
                                    env={**env, "REPO": str(root), "FIXTURE": str(base)}, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
            self.assertEqual(list((base / "temp").iterdir()), [])

    @unittest.skipIf(os.name == "nt", "Linux installer uses Bash and ELF tooling")
    def test_linux_requires_exact_zig_before_fetch(self) -> None:
        """Accepting another patch, malformed output or failed version must not fetch."""
        root = Path(__file__).resolve().parents[4]
        installer = root / "av-metrics-fmetrics/scripts/install-fmetrics-linux.sh"
        with tempfile.TemporaryDirectory(prefix="toolchain version ") as directory:
            base = Path(directory)
            env, commits = self._installer_fixture(base)
            tools = base / "bin"
            tools.mkdir()
            (tools / "zig").write_text('''#!/bin/bash
set -eu
if [ "$1" = version ]; then
    printf '%s' "$ZIG_FIXTURE_VERSION"
    exit "$ZIG_VERSION_EXIT"
fi
test "$*" = 'build --release=fast' || exit 93
{ git rev-parse HEAD; cat payload; } > "$MARKER"
echo 'toolchain fixture compiler boundary; no native build' >&2
exit 79
''')
            (tools / "gcc").write_text('#!/bin/bash\ntouch "$FETCH_MARKER"; exit 81\n')
            (tools / "git").write_text('''#!/bin/bash
printf '%s\\n' "$*" >> "$GIT_MARKER"
exec "$FIXTURE_GIT" "$@"
''')
            for tool in tools.iterdir():
                tool.chmod(0o755)
            env["PATH"] = f"{tools}:/usr/bin:/bin"
            env.update(FIXTURE_GIT=shutil.which("git"), GIT_MARKER=str(base / "git-calls"))
            for version, expected, status, accepted in self._zig_versions():
                with self.subTest(version=version, expected=expected, status=status):
                    marker = base / "compiler"
                    marker.unlink(missing_ok=True)
                    (base / "git-calls").unlink(missing_ok=True)
                    selected = {**env, "ZIG_FIXTURE_VERSION": version,
                                "ZIG_VERSION_EXIT": str(status)}
                    if expected is not None:
                        selected["FMETRICS_ZIG_VERSION"] = expected
                    result = subprocess.run(
                        ["bash", str(installer), "-Destination", str(base / "destination"), "-Quiet"],
                        cwd=base, env=selected, capture_output=True, text=True)
                    logs = result.stdout + result.stderr
                    if accepted:
                        self.assertEqual(result.returncode, 79, logs)
                        self.assertEqual(marker.read_text().splitlines(), [commits["pin"], "pinned"])
                        self.assertIn("fetch --quiet --no-tags", (base / "git-calls").read_text())
                    else:
                        self.assertNotEqual(result.returncode, 0, logs)
                        self.assertIn("expected", logs.lower())
                        self.assertIn(expected or "0.16.0", logs)
                        if expected is None:
                            self.assertIn(version or "<no version>", logs)
                        self.assertNotIn("Fetching fmetrics", logs)
                        self.assertFalse(marker.exists(), logs)
                        self.assertFalse((base / "git-calls").exists(), "invalid version must not invoke Git")
                    self.assertFalse((base / "dependencies").exists(), "linker must not run")
                    self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                    self.assertEqual(list((base / "destination").iterdir()), [base / "destination/sentinel"])
                    self.assertEqual(list((base / "temp").iterdir()), [], logs)

    def _zig_versions(self) -> list[tuple[str, str | None, int, bool]]:
        return [
            ("0.16.0", None, 0, True),
            ("0.16.0", "0.16.0", 0, True),
            ("0.16.1", None, 0, False),
            ("0.17.0", None, 0, False),
            ("0.17.1", None, 0, False),
            ("0x16.0", None, 0, False),
            ("0.16.0-dev.1", None, 0, False),
            ("0.16.0\njunk", None, 0, False),
            ("", None, 0, False),
            ("0.16.0", None, 9, False),
            ("0.16.0", "0.16.1", 0, False),
            ("0.16.1", "0.16.1", 0, True),
            ("0.17.0", "0.17.0", 0, False),
            ("0.16.0", "0x16.0", 0, False),
        ]

    @unittest.skipUnless(os.name == "nt", "native PowerShell version fixture runs on Windows CI")
    def test_windows_requires_exact_zig_before_fetch(self) -> None:
        """Run actual PS with external -ZigPath/PATH tools, never a native compiler."""
        root = Path(__file__).resolve().parents[4]
        installer = root / "av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1"
        with tempfile.TemporaryDirectory(prefix="toolchain native ") as directory:
            base = Path(directory)
            env, commits = self._installer_fixture(base)
            runner = base / "run.ps1"
            runner.write_text('''param([string] $Installer, [string] $Destination, [string] $ZigPath)
$parameters = @{ Destination = $Destination }
if ($ZigPath) { $parameters.ZigPath = $ZigPath }
try { & $Installer @parameters }
catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }
''')
            zig = base / "zig.cmd"
            zig.write_text(r'''@echo off
if "%~1"=="version" (
  "%FIXTURE_PYTHON%" -c "import os; print(os.environ['ZIG_FIXTURE_VERSION'], end='')"
  exit /b %ZIG_VERSION_EXIT%
)
if not "%*"=="build --fetch" exit /b 93
(
  git rev-parse HEAD
  type payload
)> "%MARKER%"
echo toolchain fixture dependency boundary; no native build
exit /b 79
''')
            # Let Git trace itself: batch forwarding changes HEAD^{commit}.
            trace = (base / "git-trace").resolve()
            env.update(PATH=str(base) + os.pathsep + env["PATH"], FIXTURE_PYTHON=sys.executable,
                       GIT_TRACE=str(trace))
            for version, expected, status, accepted in self._zig_versions():
                for explicit in (False, True):
                    with self.subTest(version=version, expected=expected, status=status, explicit=explicit):
                        marker = base / "compiler"
                        marker.unlink(missing_ok=True)
                        trace.unlink(missing_ok=True)
                        selected = {**env, "ZIG_FIXTURE_VERSION": version,
                                    "ZIG_VERSION_EXIT": str(status)}
                        if expected is not None:
                            selected["FMETRICS_ZIG_VERSION"] = expected
                        arguments = ["-ZigPath", str(zig)] if explicit else []
                        result = subprocess.run(
                            ["pwsh", "-NoProfile", "-File", str(runner), "-Installer", str(installer),
                             "-Destination", str(base / "destination"), *arguments],
                            cwd=base, env=selected, capture_output=True, text=True)
                        logs = result.stdout + result.stderr
                        self.assertNotEqual(result.returncode, 0, logs)
                        if accepted:
                            self.assertTrue(trace.is_file(), logs)
                            self.assertGreater(trace.stat().st_size, 0, logs)
                            self.assertIn("fetching the Zig dependencies failed", logs)
                            self.assertEqual(marker.read_text().splitlines(), [commits["pin"], "pinned"])
                        else:
                            self.assertIn("expected", logs.lower())
                            self.assertIn(expected or "0.16.0", logs)
                            if expected is None:
                                self.assertIn(version or "<no version>", logs)
                            self.assertNotIn("Fetching fmetrics", logs)
                            self.assertFalse(marker.exists(), logs)
                            self.assertFalse(trace.exists(), "invalid version must not invoke Git")
                        self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                        self.assertEqual(list((base / "destination").iterdir()), [base / "destination/sentinel"])
                        self.assertEqual(list((base / "temp").iterdir()), [], logs)

    def _installer_fixture(self, base: Path) -> tuple[dict, dict]:
        # SECURITY: Only owned Git repositories/configs; no inherited Git routing,
        # shell startup, credentials or global config may affect these fetches.
        env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT", "COMSPEC", "PATHEXT")
               if key in os.environ}
        temp = base / "temp"
        temp.mkdir()
        env.update(TMPDIR=str(temp), TEMP=str(temp), TMP=str(temp),
                   GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
                   GIT_CEILING_DIRECTORIES=str(base), GIT_ALLOW_PROTOCOL="file",
                   GIT_TERMINAL_PROMPT="0")
        repo = base / "remote"
        with patch.dict(os.environ, env, clear=True):
            init_repo(repo)
            (repo / "payload").write_text("base\n")
            git(repo, "add", ".")
            git(repo, "commit", "-m", "installer fixture base")
            upstream = git(repo, "rev-parse", "HEAD")
            (repo / "payload").write_text("pinned\n")
            (repo / "patches").mkdir()
            (repo / "patches/0001-windows-portability.patch").write_bytes(
                b"diff --git a/payload.txt b/payload.txt\n"
                b"--- a/payload.txt\n+++ b/payload.txt\n@@ -1 +1 @@\n-old\n+patched\n")
            git(repo, "add", ".")
            git(repo, "commit", "-m", "installer fixture pin")
            pinned = git(repo, "rev-parse", "HEAD")
            (repo / "payload").write_text("advanced\n")
            git(repo, "commit", "-am", "installer fixture advanced branch")
            advanced = git(repo, "rev-parse", "HEAD")
            other = base / "other remote"
            init_repo(other)
            git(other, "commit", "--allow-empty", "-m", "installer fixture unreachable")
            unreachable = git(other, "rev-parse", "HEAD")
        destination = base / "destination"
        destination.mkdir()
        (destination / "sentinel").write_text("preserve destination\n")
        env.update(FMETRICS_REPO=str(repo), FMETRICS_COMMIT=pinned,
                   FMETRICS_BRANCH="main", FMETRICS_UPSTREAM_BASE=upstream,
                   MARKER=str(base / "compiler"), FETCH_MARKER=str(base / "dependencies"))
        return env, {"base": upstream, "pin": pinned, "advanced": advanced,
                     "unreachable": unreachable}

    @unittest.skipIf(os.name == "nt", "Linux installer uses Bash and ELF tooling")
    def test_linux_fetches_exact_commit_before_compilation(self) -> None:
        """Branch-head checkout, ignored overrides or a fail-open fetch must fail."""
        installer = Path(__file__).resolve().parents[4] / "av-metrics-fmetrics/scripts/install-fmetrics-linux.sh"
        with tempfile.TemporaryDirectory(prefix="installer fetch ") as directory:
            base = Path(directory)
            env, commits = self._installer_fixture(base)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            for name, body in {
                "zig": '''if [ "$1" = version ]; then echo 0.16.0; exit 0; fi
test "$*" = 'build --release=fast' || exit 93
{ git rev-parse HEAD; cat payload; git rev-parse --abbrev-ref HEAD; } > "$MARKER"
echo 'installer fixture compiler boundary; no native build' >&2
exit 79
''',
                "gcc": 'touch "$FETCH_MARKER"; exit 81\n',
            }.items():
                tool = bin_dir / name
                tool.write_text("#!/bin/bash\nset -eu\n" + body)
                tool.chmod(0o755)
            env["PATH"] = f"{bin_dir}:/usr/bin:/bin"
            cases = [
                ("advanced-branch", commits["pin"], "main", [], "pinned", ""),
                ("absent-branch", commits["pin"], "absent", [], "pinned", ""),
                ("environment", commits["advanced"], "absent", [], "advanced", ""),
                ("explicit-override", commits["advanced"], "absent", ["-Commit", commits["pin"]], "pinned", ""),
                ("short", commits["pin"][:12], "main", [], None, "full.*commit"),
                ("branch-as-pin", "main", "main", [], None, "full.*commit"),
                ("nonhex", "z" * 40, "main", [], None, "full.*commit"),
                ("newline", commits["pin"] + "\n", "main", [], None, "full.*commit"),
                ("unknown", "1" * 40, "main", [], None, "failed to fetch fmetrics commit " + "1" * 40),
                ("unreachable", commits["unreachable"], "main", [], None, "failed to fetch fmetrics commit " + commits["unreachable"]),
            ]
            for case, selected, branch, arguments, payload, error in cases:
                with self.subTest(case=case):
                    marker = base / "compiler"
                    marker.unlink(missing_ok=True)
                    result = subprocess.run(
                        ["bash", str(installer), "-Destination", str(base / "destination"), "-Quiet", *arguments],
                        cwd=base, env={**env, "FMETRICS_COMMIT": selected, "FMETRICS_BRANCH": branch},
                        capture_output=True, text=True)
                    logs = result.stdout + result.stderr
                    if payload:
                        self.assertEqual(result.returncode, 79, logs)
                        expected = commits["advanced"] if payload == "advanced" else commits["pin"]
                        self.assertEqual(marker.read_text().splitlines(), [expected, payload, "HEAD"], logs)
                        self.assertIn("installer fixture compiler boundary", logs)
                    else:
                        self.assertNotEqual(result.returncode, 0, logs)
                        self.assertRegex(logs.lower(), error)
                        self.assertFalse(marker.exists(), logs)
                        if case in ("short", "branch-as-pin", "nonhex", "newline"):
                            self.assertNotIn("Fetching fmetrics", logs, "invalid pin must fail before fetching")
                    self.assertFalse((base / "dependencies").exists(), "linker must never run")
                    self.assertEqual(list((base / "destination").iterdir()), [base / "destination/sentinel"])
                    self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                    self.assertEqual(list((base / "temp").iterdir()), [], logs)

    def _check_windows_fetch(self, selected_cases: tuple[str, ...]) -> None:
        """Run actual PS/Git/apply; fake Zig never compiles or installs fixture bytes."""
        installer = Path(__file__).resolve().parents[4] / "av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1"
        with tempfile.TemporaryDirectory(prefix="installer native ") as directory:
            base = Path(directory)
            env, commits = self._installer_fixture(base)
            runner = base / "run.ps1"
            runner.write_text('''param([string] $Installer, [string] $Destination, [string] $ZigPath, [string] $Commit)
$parameters = @{ Destination = $Destination; ZigPath = $ZigPath }
if ($PSBoundParameters.ContainsKey('Commit')) { $parameters.Commit = $Commit }
try { & $Installer @parameters }
catch { [Console]::Error.WriteLine($_.Exception.Message); exit 1 }
''')
            zig = base / "zig.cmd"
            # Batch positional parameters split on '='; preserve the raw invocation.
            zig.write_text(r'''@echo off
if "%~1"=="version" (
  echo 0.16.0
  exit /b 0
)
if not "%~1"=="build" exit /b 93
if "%~2"=="--fetch" (
  echo dependencies> "%FETCH_MARKER%"
  mkdir zig-pkg\fcvvdp-fixture
  "%FIXTURE_PYTHON%" -c "import os; from pathlib import Path; Path('zig-pkg/fcvvdp-fixture/payload.txt').write_bytes(b'incompatible\n' if os.environ['BAD_DEP']=='1' else b'old\n')"
  if errorlevel 1 exit /b 95
  exit /b 0
)
if not "%*"=="build --release=fast -Dshared=true" exit /b 94
(
  git rev-parse HEAD
  type payload
  git rev-parse --abbrev-ref HEAD
  type zig-pkg\fcvvdp-fixture\payload.txt
  echo %*
)> "%MARKER%"
echo installer fixture compiler boundary; no native build
exit /b 79
''')
            cases = [
                ("advanced-branch", commits["pin"], "main", [], commits["base"], False, "pinned", "build failed"),
                ("absent-branch", commits["pin"], "absent", [], commits["base"], False, "pinned", "build failed"),
                ("environment", commits["advanced"], "absent", [], commits["base"], False, "advanced", "build failed"),
                ("explicit-override", commits["advanced"], "absent", ["-Commit", commits["pin"]], commits["base"], False, "pinned", "build failed"),
                ("short", commits["pin"][:12], "main", [], commits["base"], False, None, "full.*commit"),
                ("branch-as-pin", "main", "main", [], commits["base"], False, None, "full.*commit"),
                ("nonhex", "z" * 40, "main", [], commits["base"], False, None, "full.*commit"),
                ("newline", commits["pin"] + "\n", "main", [], commits["base"], False, None, "full.*commit"),
                ("unknown", "1" * 40, "main", [], commits["base"], False, None, "failed to fetch fmetrics commit " + "1" * 40),
                ("unreachable", commits["unreachable"], "main", [], commits["base"], False, None, "failed to fetch fmetrics commit " + commits["unreachable"]),
                ("wrong-ancestry", commits["pin"], "main", [], commits["advanced"], False, None, "not built on.*upstream"),
                ("missing-ancestry", commits["pin"], "main", [], "2" * 40, False, None, "not built on.*upstream"),
                ("missing-patch", commits["base"], "main", [], commits["base"], False, None, "no patches/0001-windows-portability.patch"),
                ("apply-failure", commits["pin"], "main", [], commits["base"], True, None, "applying.*patch failed"),
            ]
            for case, selected, branch, arguments, upstream, bad_dep, payload, error in cases:
                if case not in selected_cases:
                    continue
                with self.subTest(case=case):
                    marker, fetched = base / "compiler", base / "dependencies"
                    marker.unlink(missing_ok=True)
                    fetched.unlink(missing_ok=True)
                    result = subprocess.run(
                        ["pwsh", "-NoProfile", "-File", str(runner), "-Installer", str(installer), "-Destination", str(base / "destination"),
                         "-ZigPath", str(zig), *arguments], cwd=base,
                        env={**env, "FMETRICS_COMMIT": selected, "FMETRICS_BRANCH": branch,
                             "FMETRICS_UPSTREAM_BASE": upstream, "BAD_DEP": "1" if bad_dep else "0",
                             "FIXTURE_PYTHON": sys.executable},
                        capture_output=True, text=True)
                    logs = result.stdout + result.stderr
                    self.assertNotEqual(result.returncode, 0, logs)
                    self.assertRegex(logs.lower(), error)
                    self.assertEqual(fetched.exists(), bool(payload) or bad_dep, logs)
                    if payload:
                        expected = commits["advanced"] if payload == "advanced" else commits["pin"]
                        self.assertEqual(marker.read_text().splitlines(),
                                         [expected, payload, "HEAD", "patched", "build --release=fast -Dshared=true"], logs)
                        self.assertIn("installer fixture compiler boundary", logs)
                    else:
                        self.assertFalse(marker.exists(), logs)
                        if case in ("short", "branch-as-pin", "nonhex", "newline"):
                            self.assertNotIn("Fetching fmetrics", logs, "invalid pin must fail before fetching")
                    self.assertEqual(list((base / "destination").iterdir()), [base / "destination/sentinel"])
                    self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                    self.assertEqual(list((base / "temp").iterdir()), [], logs)

    @unittest.skipUnless(os.name == "nt", "native PowerShell fetch fixture runs on Windows CI")
    def test_windows_fmetrics_fetches_exact_detached_commit_and_applies_portability_patch_before_build(self) -> None:
        self._check_windows_fetch(("advanced-branch", "absent-branch", "environment", "explicit-override"))

    @unittest.skipUnless(os.name == "nt", "native PowerShell fetch fixture runs on Windows CI")
    def test_windows_fmetrics_invalid_or_unreachable_commit_never_builds(self) -> None:
        self._check_windows_fetch(("short", "branch-as-pin", "nonhex", "newline", "unknown", "unreachable"))

    @unittest.skipUnless(os.name == "nt", "native PowerShell fetch fixture runs on Windows CI")
    def test_windows_fmetrics_wrong_or_missing_upstream_ancestry_never_fetches_dependencies(self) -> None:
        self._check_windows_fetch(("wrong-ancestry", "missing-ancestry"))

    @unittest.skipUnless(os.name == "nt", "native PowerShell fetch fixture runs on Windows CI")
    def test_windows_fmetrics_missing_or_inapplicable_patch_never_compiles(self) -> None:
        self._check_windows_fetch(("missing-patch", "apply-failure"))

    @unittest.skipUnless(os.name == "nt", "native PowerShell installer acceptance runs on Windows CI")
    def test_windows_vship_candidate_discovery_and_verified_downloads_preserve_existing_bytes(self) -> None:
        """Run real PS functions and installer; never load/execute fixture DLL bytes."""
        root = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="download native ") as directory:
            base = Path(directory)
            script = base / "check.ps1"
            script.write_text(r'''
$ErrorActionPreference = 'Stop'
$global:Downloads = 0
function curl.exe {
    $global:Downloads++
    $out = $args[[array]::IndexOf($args, '-o') + 1]
    $out = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($out)
    [IO.File]::WriteAllText($out, $env:PAYLOAD)
    $global:LASTEXITCODE = if ($env:PAYLOAD -eq 'http') { 22 } else { 0 }
}
$vship = Join-Path $env:REPO 'av-metrics-vship/scripts/install-libvship-windows.ps1'
$env:VSHIP_PLUGIN_PATH = ''
$env:VSSCRIPT_PATH = ''
$env:VAPOURSYNTH_EXTRA_PLUGIN_PATH = ''
$env:PATH = ''
$dest = Join-Path $env:FIXTURE 'destination'
$local = Join-Path $env:FIXTURE 'local'
New-Item -ItemType Directory $local | Out-Null
$candidate = Join-Path $local 'vship.dll'
[IO.File]::WriteAllText($candidate, 'local candidate, not ABI proof')
$env:VSHIP_LIB_DIR = $local
$found = & $vship -Version 0.0.0 -Variant amd -Destination $dest
if (-not $found.Path -or $found.Downloaded) { throw 'local candidate misreported as a download' }
if ($Downloads -ne 0) { throw 'local candidate attempted a download' }
if (Test-Path $dest) { throw 'fallback mutated destination' }
if ([IO.File]::ReadAllText($candidate) -ne 'local candidate, not ABI proof') { throw 'candidate changed' }
$savedLocation = (Get-Location -PSProvider FileSystem).ProviderPath
$processCwd = [Environment]::CurrentDirectory
$savedEnvironment = @{}
foreach ($variable in @('VSHIP_PLUGIN_PATH', 'VSHIP_LIB_DIR', 'VSSCRIPT_PATH', 'VAPOURSYNTH_EXTRA_PLUGIN_PATH', 'PATH', 'PAYLOAD')) {
    $savedEnvironment[$variable] = [Environment]::GetEnvironmentVariable($variable)
    [Environment]::SetEnvironmentVariable($variable, '')
}
$ownedDrive = $null
try {
    $working = Join-Path $env:FIXTURE 'powershell location'
    $relative = "relative vship $([guid]::NewGuid().ToString('N'))"
    $relativeDirectory = Join-Path $working $relative
    New-Item -ItemType Directory -Path $relativeDirectory -Force | Out-Null
    $relativeCandidate = Join-Path $relativeDirectory 'vship.dll'
    [IO.File]::WriteAllText($relativeCandidate, 'preserve relative existing candidate')
    Set-Location -LiteralPath $working
    if ((Get-Location -PSProvider FileSystem).ProviderPath -eq $processCwd) { throw 'fixture needs distinct PS and process locations' }
    $env:PAYLOAD = 'good'
    foreach ($case in @('destination', 'lib-dir', 'path')) {
        $env:VSHIP_LIB_DIR = if ($case -eq 'lib-dir') { $relative } else { '' }
        $env:PATH = if ($case -eq 'path') { '"' + $relative + '"' } else { '' }
        $target = if ($case -eq 'destination') { $relative } else { 'unused destination' }
        $found = & $vship -Version 5.1.2 -Variant vulkan -Destination $target -ExpectedSha256 '770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c'
        if ($found.Downloaded -or $found.Path -ne $relativeCandidate -or -not [IO.Path]::IsPathRooted($found.Path)) { throw "relative candidate not preserved: $case" }
        if ([IO.File]::ReadAllText($found.Path) -ne 'preserve relative existing candidate') { throw "relative candidate overwritten: $case" }
        if ($Downloads -ne 0 -or (Test-Path -LiteralPath 'unused destination')) { throw 'relative discovery downloaded or changed destination' }
        if ([Environment]::CurrentDirectory -ne $processCwd) { throw 'fixture changed process CWD' }
    }
    $env:VSHIP_LIB_DIR = ''
    $env:PATH = ''
    $driveLetter = 90..68 | ForEach-Object { [string][char]$_ } | Where-Object { -not (Get-PSDrive -Name $_ -ErrorAction SilentlyContinue) } | Select-Object -First 1
    if (-not $driveLetter) { throw 'fixture needs an unused PSDrive letter' }
    $driveRoot = Join-Path $env:FIXTURE 'owned psdrive'
    $driveWorking = Join-Path $driveRoot 'working'
    $rootRelative = "root vship $([guid]::NewGuid().ToString('N'))"
    $driveRelative = "drive vship $([guid]::NewGuid().ToString('N'))"
    $rootDirectory = Join-Path $driveRoot $rootRelative
    $driveDirectory = Join-Path $driveWorking $driveRelative
    New-Item -ItemType Directory -Path $driveWorking, $rootDirectory, $driveDirectory -Force | Out-Null
    # A session-only drive keeps root-relative fixtures inside owned temporary storage.
    $ownedDrive = New-PSDrive -Name $driveLetter -PSProvider FileSystem -Root $driveRoot
    Set-Location -LiteralPath "$($ownedDrive.Name):\working"
    foreach ($case in @(
        @{ Destination = '\' + $rootRelative; Path = (Join-Path $rootDirectory 'vship.dll') },
        @{ Destination = "$($ownedDrive.Name):$driveRelative"; Path = (Join-Path $driveDirectory 'vship.dll') }
    )) {
        [IO.File]::WriteAllText($case.Path, 'preserve PSDrive existing candidate')
        # An unavailable pin makes a discovery regression fail before download or writes.
        $found = & $vship -Version 0.0.0 -Variant amd -Destination $case.Destination
        if ($found.Downloaded -or $found.Path -ne $case.Path -or -not [IO.Path]::IsPathRooted($found.Path)) { throw "PSDrive candidate not preserved: $($case.Destination)" }
        if ([IO.File]::ReadAllText($found.Path) -ne 'preserve PSDrive existing candidate') { throw 'PSDrive candidate overwritten' }
        if ($Downloads -ne 0 -or (Test-Path -LiteralPath $dest) -or (Test-Path -LiteralPath (Join-Path $driveWorking 'unused destination'))) { throw 'PSDrive discovery downloaded or changed unused destination' }
        if (@(Get-ChildItem -LiteralPath ([IO.Path]::GetDirectoryName($case.Path)) -Force).Count -ne 1) { throw 'PSDrive discovery left extra files' }
        if ([Environment]::CurrentDirectory -ne $processCwd) { throw 'PSDrive fixture changed process CWD' }
    }
} finally {
    Set-Location -LiteralPath $savedLocation
    if ($ownedDrive) { Remove-PSDrive -Name $ownedDrive.Name }
    foreach ($variable in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($variable, $savedEnvironment[$variable]) }
}
$letter = 90..68 | ForEach-Object { [string][char]$_ } | Where-Object { -not (Get-PSDrive -Name $_ -ErrorAction SilentlyContinue) } | Select-Object -First 1
if (-not $letter) { throw 'fixture needs an unmapped drive letter' }
$missing = "$($letter):\unmapped"
$env:VSHIP_PLUGIN_PATH = $missing
$env:VSHIP_LIB_DIR = ''
$env:VSSCRIPT_PATH = '"' + $missing + '\vsscript.dll"'
$env:VAPOURSYNTH_EXTRA_PLUGIN_PATH = $missing
$env:PATH = $missing + ';"' + (Join-Path $env:FIXTURE 'absent path') + '";"' + $local + '"'
$found = & $vship -Version 0.0.0 -Variant amd -Destination $dest
if ($found.Path -ne $candidate -or $found.Downloaded) { throw 'bad PATH entry prevented the quoted valid candidate' }
if ($Downloads -ne 0 -or (Test-Path $dest)) { throw 'candidate probing mutated the destination or downloaded' }
if ([IO.File]::ReadAllText($candidate) -ne 'local candidate, not ABI proof') { throw 'PATH candidate changed' }
Remove-Item $candidate
$env:VSHIP_LIB_DIR = ''
try { & $vship -Version 0.0.0 -Variant amd -Destination $dest; throw 'accepted unavailable pin' }
catch { if ($_.Exception.Message -notmatch 'unavailable.*pin|pin.*unavailable') { throw } }
if (Test-Path $dest) { throw 'unavailable pin mutated destination' }
if ($Downloads -ne 0) { throw 'unavailable pin attempted a download' }
$sha = '770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c'
function Expand-Archive {
    param($Path, $DestinationPath, [switch]$Force)
    [IO.File]::WriteAllText((Join-Path $env:FIXTURE 'unpacked'), 'marker')
    throw 'verified-download unpack marker; fixture is not an executable archive'
}
foreach ($payload in @('good', 'corrupt', 'g', 'http')) {
    $env:PAYLOAD = $payload
    $marker = Join-Path $env:FIXTURE 'unpacked'
    if (Test-Path $marker) { Remove-Item $marker }
    try { & $vship -Version 0.0.0 -Variant nvidia -Destination (Join-Path $env:FIXTURE $payload) -ExpectedSha256 $sha }
    catch { }
    if ((Test-Path $marker) -ne ($payload -eq 'good')) { throw "unsafe archive boundary: $payload" }
}
foreach ($relative in @('av-metrics-vship/scripts/install-libvship-windows.ps1', 'av-metrics-vmaf/scripts/install-libvmaf-windows.ps1', '.github/scripts/stage-native-libs.ps1')) {
    $errors = $null; $tokens = $null
    $ast = [Management.Automation.Language.Parser]::ParseFile((Join-Path $env:REPO $relative), [ref]$tokens, [ref]$errors)
    if ($errors.Count) { throw ($errors | Out-String) }
    $function = $ast.Find({param($node) $node -is [Management.Automation.Language.FunctionDefinitionAst] -and $node.Name -in @('Get-Asset', 'Save-File', 'Invoke-Download')}, $true)
    . ([scriptblock]::Create($function.Extent.Text))
    $UserAgent = 'offline'
    $MinimumBytes = 0
    $file = Join-Path $env:FIXTURE 'download'
    $sha = '770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c'
    foreach ($payload in @('good', 'corrupt', 'g', 'http')) {
        $env:PAYLOAD = $payload
        [IO.File]::WriteAllText($file, 'preserve')
        $accepted = $false
        try { & $function.Name -Uri 'https://fixture.invalid' -OutFile $file -Sha256 $sha; $accepted = $true } catch { }
        if ($accepted -ne ($payload -eq 'good')) { throw "wrong acceptance: $relative / $payload" }
        if ([IO.File]::ReadAllText($file) -ne $(if ($accepted) {'good'} else {'preserve'})) { throw 'destination changed on rejection' }
    }
    if ($relative -eq '.github/scripts/stage-native-libs.ps1') {
        $guard = $ast.Find({param($node) $node -is [Management.Automation.Language.IfStatementAst] -and $node.Clauses[0].Item1.Extent.Text -match 'Get-FileHash.*\$vshipStaging'}, $true)
        if (-not $guard) { throw 'missing final release Vship digest guard' }
        function Get-Pin([string] $Name) {
            if ($Name -ne 'VSHIP_VULKAN_SHA256') { throw 'unexpected release pin' }
            return $sha
        }
        $vshipStaging = Join-Path $env:FIXTURE 'release-vship.dll'
        $VshipVersion = 'fixture-version'
        foreach ($payload in @('good', 'wrong-variant', 'g')) {
            [IO.File]::WriteAllText($vshipStaging, $payload)
            $published = $false
            try { . ([scriptblock]::Create($guard.Extent.Text)); $published = $true }
            catch { if ($_.Exception.Message -notmatch 'release.*Vship|Vship.*release') { throw } }
            if ($published -ne ($payload -eq 'good')) { throw "unsafe release boundary: $payload" }
            if ([IO.File]::ReadAllText($vshipStaging) -ne $payload) { throw 'release guard changed candidate bytes' }
        }
    }
}
''')
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(script)], capture_output=True, text=True,
                                    env={**os.environ, "REPO": str(root), "FIXTURE": str(base)})
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("candidate", result.stdout.lower())
            self.assertIn("runtime", result.stdout.lower())


if __name__ == "__main__":
    unittest.main()
