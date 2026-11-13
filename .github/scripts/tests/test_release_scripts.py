"""Offline release-script regressions: python3 -m unittest discover -s .github/scripts/tests."""

import os
import hashlib
import json
from copy import deepcopy
from pathlib import Path
import re
import runpy
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile


class ReleaseScriptTests(unittest.TestCase):
    def test_r12_pinned_lookup_source_and_required_native_gate(self) -> None:
        """Supplement only: remove the dead automatic-variable/index lookup."""
        root = Path(__file__).resolve().parents[3]
        source = (root / "av-metrics-vmaf/scripts/install-libvmaf-windows.ps1").read_text()
        self.assertNotRegex(source, r"(?i)Get-LatestPackage|\$matches\b")
        self.assertIn("$PackageHashes.ContainsKey($package)", source)
        self.assertIn("Save-File -Uri $candidates -OutFile $archive -Sha256 $PackageHashes[$package]", source)
        workflow = (root / ".github/workflows/windows-build.yml").read_text()
        for method in ("test_r12_windows_missing_archive_never_uses_newer_index",
                       "test_r12_windows_missing_or_invalid_pin_refuses_before_use",
                       "test_r12_windows_package_hash_precedes_unpack"):
            self.assertIn(f"ReleaseScriptTests.{method}", workflow)

    def _r12_native(self, case: str) -> None:
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="r12 native ") as directory:
            base = Path(directory)
            temp = base / "private temp"
            temp.mkdir()
            result = subprocess.run(
                ["pwsh", "-NoProfile", "-File", str(Path(__file__).with_name("r11-installers.ps1")),
                 "-R12Case", case], cwd=base, capture_output=True, text=True,
                env={**os.environ, "REPO": str(root), "FIXTURE": str(base),
                     "TEMP": str(temp), "TMP": str(temp)})
            logs = result.stdout + result.stderr
            self.assertEqual(result.returncode, 0, logs)
            self.assertIn(f"R12 {case} verified", result.stdout)
            self.assertEqual(list(temp.iterdir()), [])

    @unittest.skipUnless(os.name == "nt", "native PowerShell pinned lookup fixture runs on Windows CI")
    def test_r12_windows_missing_archive_never_uses_newer_index(self) -> None:
        """All mirrors 404; an available newer directory listing is never read."""
        self._r12_native("missing-archive")

    @unittest.skipUnless(os.name == "nt", "native PowerShell pin validation fixture runs on Windows CI")
    def test_r12_windows_missing_or_invalid_pin_refuses_before_use(self) -> None:
        """A missing or malformed expected digest must not reach transport/tar."""
        for case in ("missing-pin", "invalid-pin"):
            with self.subTest(case=case):
                self._r12_native(case)

    @unittest.skipUnless(os.name == "nt", "native PowerShell package hashing fixture runs on Windows CI")
    def test_r12_windows_package_hash_precedes_unpack(self) -> None:
        """Keep real Save-File SHA256 checking with good/bad inert package bytes."""
        for case in ("good", "hash"):
            with self.subTest(case=case):
                self._r12_native(case)

    @unittest.skipIf(os.name == "nt", "Linux Vship installer uses Bash")
    def test_r11_linux_vship_cleans_early_failure_and_retries(self) -> None:
        """A late EXIT trap leaks the missing-tool checkout; all retries stay owned."""
        installer = Path(__file__).resolve().parents[3] / "av-metrics-vship/scripts/install-libvship-linux.sh"
        with tempfile.TemporaryDirectory(prefix="r11 linux ") as directory:
            base = Path(directory)
            env, commits = self._r06_fixture(base)
            with patch.dict(os.environ, env, clear=True):
                self._r07_git(base / "remote", "tag", "fixture", commits["pin"])
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
if [ "$MODE" = build ]; then echo 'R11 inert build failure' >&2; exit 79; fi
printf 'R11 inert library bytes\\n' > "$2/libvship.so"
'''
            for tool in (tools / "clang", tools / "nm"):
                tool.chmod(0o755)
            dl = base / "destination/dl"
            dl.mkdir()
            (dl / "sentinel").write_bytes(b"preexisting dl\x00")
            env.update(PATH=str(tools), VULKAN_SDK="fixture unused SDK", VSHIP_VERSION="fixture",
                       VSHIP_COMMIT=commits["pin"], VSHIP_REPO=str(base / "remote"))
            for mode, expected, error in (("missing", 1, "make not found"), ("fetch", 128, "does not exist"),
                                           ("build", 79, "R11 inert build failure"), ("success", 0, "Done.")):
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
                        self.assertEqual(target.read_bytes(), b"R11 inert library bytes\n")
                    else:
                        self.assertFalse(target.exists(), logs)

    @unittest.skipUnless(os.name == "nt", "native PowerShell ownership fixture runs on Windows CI")
    def test_r11_windows_installers_private_scratch_and_retry(self) -> None:
        """Real Get-Asset/Save-File and filesystem; only HTTP/unpack are inert doubles."""
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="r11 native ") as directory:
            base = Path(directory)
            temp = base / "private temp"
            temp.mkdir()
            result = subprocess.run(["pwsh", "-NoProfile", "-File", str(Path(__file__).with_name("r11-installers.ps1"))],
                                    cwd=base, capture_output=True, text=True,
                                    env={**os.environ, "REPO": str(root), "FIXTURE": str(base),
                                         "TEMP": str(temp), "TMP": str(temp)})
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(list(temp.iterdir()), [])

    @unittest.skipUnless(os.name == "nt", "native PowerShell PID ownership fixture runs on Windows CI")
    def test_r11_windows_fmetrics_preserves_old_pid_checkout(self) -> None:
        """An unknown immutable pin must not adopt/delete an old same-PID directory."""
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="r11 pid ") as directory:
            base = Path(directory)
            env, _ = self._r06_fixture(base)
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

    def test_r11_powershell_ownership_source_and_required_gate(self) -> None:
        """Supplement only: native Windows execution is mandatory CI, not Linux proof."""
        root = Path(__file__).resolve().parents[3]
        for relative, variable in (("av-metrics-vship/scripts/install-libvship-windows.ps1", "stageDir"),
                                   ("av-metrics-vmaf/scripts/install-libvmaf-windows.ps1", "StageDir"),
                                   ("av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1", "work")):
            with self.subTest(installer=relative):
                source = (root / relative).read_text()
                self.assertNotRegex(source, r"Join-Path \$(?:Destination|InstallDir) 'dl'|fmetrics-build-\$PID")
                self.assertRegex(source, rf"\${variable} = Join-Path \(\[IO.Path\]::GetTempPath\(\)\).*NewGuid")
                self.assertRegex(source, rf"New-Item -ItemType Directory -Path \${variable} \| Out-Null\s+try \{{")
                self.assertRegex(source, rf"finally \{{\s*(?:#[^\n]*\n\s*)?if \(Test-Path -LiteralPath \${variable}\) \{{ Remove-Item -LiteralPath \${variable} -Recurse -Force")
        workflow = (root / ".github/workflows/windows-build.yml").read_text()
        for method in ("test_r11_windows_installers_private_scratch_and_retry", "test_r11_windows_fmetrics_preserves_old_pid_checkout"):
            self.assertIn(f"ReleaseScriptTests.{method}", workflow)

    @unittest.skipIf(os.name == "nt", "Linux installer uses Bash and ELF tooling")
    def test_r10_linux_requires_exact_zig_before_fetch(self) -> None:
        """Accepting another patch, malformed output or failed version must not fetch."""
        root = Path(__file__).resolve().parents[3]
        installer = root / "av-metrics-fmetrics/scripts/install-fmetrics-linux.sh"
        with tempfile.TemporaryDirectory(prefix="r10 version ") as directory:
            base = Path(directory)
            env, commits = self._r06_fixture(base)
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
echo 'R10 compiler boundary; no native build' >&2
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
            for version, expected, status, accepted in self._r10_versions():
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

    def _r10_versions(self) -> list[tuple[str, str | None, int, bool]]:
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
    def test_r10_windows_requires_exact_zig_before_fetch(self) -> None:
        """Run actual PS with external -ZigPath/PATH tools, never a native compiler."""
        root = Path(__file__).resolve().parents[3]
        installer = root / "av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1"
        with tempfile.TemporaryDirectory(prefix="r10 native ") as directory:
            base = Path(directory)
            env, commits = self._r06_fixture(base)
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
echo R10 dependency boundary; no native build
exit /b 79
''')
            # Let Git trace itself: batch forwarding changes HEAD^{commit}.
            trace = (base / "git-trace").resolve()
            env.update(PATH=str(base) + os.pathsep + env["PATH"], FIXTURE_PYTHON=sys.executable,
                       GIT_TRACE=str(trace))
            for version, expected, status, accepted in self._r10_versions():
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

    def test_r10_docker_and_ci_share_build_only_zig_pin(self) -> None:
        """Supplement runtime proof: a floating Arch tool must not enter any stage."""
        root = Path(__file__).resolve().parents[3]
        docker = (root / "Dockerfile").read_text()
        metrics = docker.split("FROM base AS metrics\n", 1)[1].split("FROM base AS runtime", 1)[0]
        packages = re.search(r"RUN pacman .*?(?=\n\n)", metrics, re.S)[0]
        self.assertNotRegex(packages, r"\bzig\b", "Arch Zig floats independently of the fmetrics pin")
        pins = dict(re.findall(r"^(\w+)=(\S+)$", (root / ".github/.env").read_text(), re.M))
        self.assertEqual(pins["FMETRICS_ZIG_VERSION"], "0.16.0")
        self.assertEqual(pins["FMETRICS_ZIG_LINUX_X86_64_SHA256"],
                         "70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00")
        self.assertIn('https://ziglang.org/download/${FMETRICS_ZIG_VERSION}/zig-x86_64-linux-${FMETRICS_ZIG_VERSION}.tar.xz', metrics)
        for workflow in ("linux-build", "windows-build", "mdbook", "tests"):
            steps = [step for job in self._r03_workflow(workflow)["jobs"].values()
                     for step in job.get("steps", []) if "mlugg/setup-zig@" in step.get("uses", "")]
            self.assertTrue(steps, workflow)
            for step in steps:
                self.assertEqual(step["with"]["version"], pins["FMETRICS_ZIG_VERSION"], workflow)
        for platform in ("linux.sh", "windows.ps1"):
            source = (root / f"av-metrics-fmetrics/scripts/install-fmetrics-{platform}").read_text()
            self.assertIn("FMETRICS_ZIG_VERSION", source)
            self.assertIn("0.16.0", source)
        staging = (root / ".github/scripts/stage-native-libs.ps1").read_text()
        self.assertRegex(staging, r"\$env:FMETRICS_ZIG_VERSION\s*= Get-Pin 'FMETRICS_ZIG_VERSION'",
                         "Windows staging must propagate the central expected pin")
        other_stages = docker.replace(metrics, "")
        self.assertNotIn("/opt/zig", other_stages)
        self.assertNotRegex(other_stages, r"COPY --from=metrics .*zig")

    @unittest.skipIf(os.name == "nt", "Docker RUN boundary uses Bash")
    def test_r10_docker_download_verifies_before_tar_and_version(self) -> None:
        """Run the production RUN with R05 hashing; bad bytes never reach tar/Zig."""
        root = Path(__file__).resolve().parents[3]
        metrics = (root / "Dockerfile").read_text().split("FROM base AS metrics\n", 1)[1].split("FROM base AS runtime", 1)[0]
        run = next(line[4:] for line in metrics.replace("\\\n", "").splitlines()
                   if line.startswith("RUN ") and "/tmp/cicd.env" in line)
        self.assertIn("download-verified.sh", run, "must execute the production verified download")
        for case in ("good", "corrupt", "truncated", "http", "missing-hash", "wrong-version"):
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="r10 docker ") as directory:
                base = Path(directory)
                tools, local, scratch = base / "bin", base / "usr/local/bin", base / "tmp"
                for path in (tools, local, scratch):
                    path.mkdir(parents=True)
                # SECURITY: Every Docker absolute write path is remapped before any execution.
                script = re.sub(r"/(?:usr/local/bin|usr/lib|opt/zig|tmp/)",
                                lambda match: '"' + str(base) + match[0] + '"', run)
                self.assertNotRegex(script.replace(str(base), "OWNED"), r"(?<!\w)/(?:usr|opt|tmp)/", "unmapped Docker path")
                payload = base / "payload"
                payload.write_bytes(b"owned offline Zig archive fixture\n")
                digest = hashlib.sha256(payload.read_bytes()).hexdigest()
                pins = (root / ".github/.env").read_text()
                pins = re.sub(r"(?m)^FMETRICS_ZIG_LINUX_X86_64_SHA256=.*$",
                              "FMETRICS_ZIG_LINUX_X86_64_SHA256=" + ("" if case == "missing-hash" else digest), pins)
                (scratch / "cicd.env").write_bytes(pins.replace("\n", "\r\n").encode())
                shutil.copyfile(root / ".github/scripts/download-verified.sh", local / "download-verified.sh")
                for installer in ("install-libvship-linux.sh", "install-fmetrics-linux.sh"):
                    (local / installer).write_bytes(b'''#!/bin/bash\r
set -eu\r
test "$FMETRICS_ZIG_VERSION" = 0.16.0\r
test "$FMETRICS_COMMIT" = ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac\r
test "$VSHIP_COMMIT" = 5a627933274196219f782b3b74e13fad47cd6370\r
printf '%s\\n' "$0" >> "$FIXTURE/installers"\r
''')
                bodies = {
                    "curl": '''test "$*" = "-sSfL --proto =https --proto-redir =https --retry 3 --retry-delay 2 -o ${11} https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz" || exit 93
touch "$FIXTURE/downloaded"
case "$CASE" in
http) exit 22 ;;
corrupt) printf 'wrong bytes' > "${11}" ;;
truncated) head -c 5 "$FIXTURE/payload" > "${11}" ;;
*) cp "$FIXTURE/payload" "${11}" ;;
esac
''',
                    "tar": '''test "$1" = -xJf && test "$3" = --strip-components=1 && test "$4" = -C || exit 94
cmp "$2" "$FIXTURE/payload"
touch "$FIXTURE/unpacked"
cp "$FIXTURE/fake-zig" "$5/zig"
''',
                }
                for name, body in bodies.items():
                    (tools / name).write_text("#!/bin/bash\nset -eu\n" + body)
                    (tools / name).chmod(0o755)
                # No host package manager, compiler, real Zig, or network executable is available.
                for name in ("sed", "mkdir", "mktemp", "rm", "ln", "bash", "sha256sum", "awk",
                             "chmod", "mv", "head", "cp", "cmp", "touch"):
                    (tools / name).symlink_to(shutil.which(name))
                zig = base / "fake-zig"
                zig.write_text('''#!/bin/bash
set -eu
test "$*" = version || exit 95
touch "$FIXTURE/executed"
if [ "$CASE" = wrong-version ]; then echo 0.16.1; else echo 0.16.0; fi
''')
                zig.chmod(0o755)
                result = subprocess.run(
                    [shutil.which("bash"), "--noprofile", "--norc", "-c", script], cwd=base,
                    env={"PATH": f"{base / 'usr/local/bin'}:{tools}", "FIXTURE": str(base), "CASE": case},
                    capture_output=True, text=True)
                logs = result.stdout + result.stderr
                verified = case in ("good", "wrong-version")
                self.assertEqual(result.returncode == 0, case == "good", logs)
                self.assertEqual((base / "downloaded").exists(), case != "missing-hash", logs)
                self.assertEqual((base / "unpacked").exists(), verified, logs)
                self.assertEqual((base / "executed").exists(), verified, logs)
                self.assertEqual((base / "installers").exists(), case == "good", logs)
                if case == "good":
                    self.assertEqual(len((base / "installers").read_text().splitlines()), 2)
                else:
                    self.assertIn({"corrupt": "SHA-256 mismatch", "truncated": "SHA-256 mismatch",
                                   "http": "no mirror supplied", "missing-hash": "independently verified SHA-256",
                                   "wrong-version": "expected exactly 0.16.0"}[case], logs)
                self.assertEqual(list(scratch.iterdir()), [] if case == "good" else [scratch / "cicd.env"], logs)

    def _r06_fixture(self, base: Path) -> tuple[dict, dict]:
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
            self._r07_repo(repo)
            (repo / "payload").write_text("base\n")
            self._r07_git(repo, "add", ".")
            self._r07_git(repo, "commit", "-m", "R06 base")
            upstream = self._r07_git(repo, "rev-parse", "HEAD")
            (repo / "payload").write_text("pinned\n")
            (repo / "patches").mkdir()
            (repo / "patches/0001-windows-portability.patch").write_bytes(
                b"diff --git a/payload.txt b/payload.txt\n"
                b"--- a/payload.txt\n+++ b/payload.txt\n@@ -1 +1 @@\n-old\n+patched\n")
            self._r07_git(repo, "add", ".")
            self._r07_git(repo, "commit", "-m", "R06 pin")
            pinned = self._r07_git(repo, "rev-parse", "HEAD")
            (repo / "payload").write_text("advanced\n")
            self._r07_git(repo, "commit", "-am", "R06 advanced branch")
            advanced = self._r07_git(repo, "rev-parse", "HEAD")
            other = base / "other remote"
            self._r07_repo(other)
            self._r07_git(other, "commit", "--allow-empty", "-m", "R06 unreachable")
            unreachable = self._r07_git(other, "rev-parse", "HEAD")
        destination = base / "destination"
        destination.mkdir()
        (destination / "sentinel").write_text("preserve destination\n")
        env.update(FMETRICS_REPO=str(repo), FMETRICS_COMMIT=pinned,
                   FMETRICS_BRANCH="main", FMETRICS_UPSTREAM_BASE=upstream,
                   MARKER=str(base / "compiler"), FETCH_MARKER=str(base / "dependencies"))
        return env, {"base": upstream, "pin": pinned, "advanced": advanced,
                     "unreachable": unreachable}

    @unittest.skipIf(os.name == "nt", "Linux installer uses Bash and ELF tooling")
    def test_r06_linux_fetches_exact_commit_before_compilation(self) -> None:
        """Branch-head checkout, ignored overrides or a fail-open fetch must fail."""
        installer = Path(__file__).resolve().parents[3] / "av-metrics-fmetrics/scripts/install-fmetrics-linux.sh"
        with tempfile.TemporaryDirectory(prefix="r06 fetch ") as directory:
            base = Path(directory)
            env, commits = self._r06_fixture(base)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            for name, body in {
                "zig": '''if [ "$1" = version ]; then echo 0.16.0; exit 0; fi
test "$*" = 'build --release=fast' || exit 93
{ git rev-parse HEAD; cat payload; git rev-parse --abbrev-ref HEAD; } > "$MARKER"
echo 'R06 compiler boundary; no native build' >&2
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
                        self.assertIn("R06 compiler boundary", logs)
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

    @unittest.skipUnless(os.name == "nt", "native PowerShell fetch fixture runs on Windows CI")
    def test_r06_windows_fetch_patch_and_ancestry_before_compilation(self) -> None:
        """Run actual PS/Git/apply; fake Zig never compiles or installs fixture bytes."""
        installer = Path(__file__).resolve().parents[3] / "av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1"
        with tempfile.TemporaryDirectory(prefix="r06 native ") as directory:
            base = Path(directory)
            env, commits = self._r06_fixture(base)
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
echo R06 compiler boundary; no native build
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
                        self.assertIn("R06 compiler boundary", logs)
                    else:
                        self.assertFalse(marker.exists(), logs)
                        if case in ("short", "branch-as-pin", "nonhex", "newline"):
                            self.assertNotIn("Fetching fmetrics", logs, "invalid pin must fail before fetching")
                    self.assertEqual(list((base / "destination").iterdir()), [base / "destination/sentinel"])
                    self.assertEqual((base / "destination/sentinel").read_text(), "preserve destination\n")
                    self.assertEqual(list((base / "temp").iterdir()), [], logs)

    def test_r05_vmaf_stage_rejects_bytes_before_unpack(self) -> None:
        """Removing the source checksum must expose the real staging tar boundary."""
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="r05 stage ") as directory:
            base = Path(directory)
            scripts = base / ".github/scripts"
            scripts.mkdir(parents=True)
            for name in ("stage-native-libs.sh", "download-verified.sh"):
                source = root / ".github/scripts" / name
                if source.exists():
                    shutil.copyfile(source, scripts / name)
            good = b"R05 verified bytes\n"
            digest = hashlib.sha256(good).hexdigest()
            pins = (root / ".github/.env").read_text()
            pins = re.sub(r"^TRANSNETV2_MODEL_SHA256=.*$", f"TRANSNETV2_MODEL_SHA256={digest}", pins, flags=re.M)
            pins = re.sub(r"^VMAF_SOURCE_SHA256=.*\n?", "", pins, flags=re.M)
            (base / "model").mkdir()
            (base / "model/vmaf_v0.6.1.json").write_text("{}")
            bin_dir = base / "bin"
            bin_dir.mkdir()
            curl = bin_dir / "curl"
            curl.write_text(f"#!{sys.executable}\n" + r'''
import os, sys
from pathlib import Path
a = sys.argv[1:]
assert "-o" in a and any("f" in v for v in a if v.startswith("-")), a
Path(a[a.index("-o")+1]).write_bytes(b"R05 verified bytes\n" if "transnetv2" in a[-1] else bytes.fromhex(os.environ["PAYLOAD"]))
''')
            curl.chmod(0o755)
            for name, body in {"ldconfig": "exit 0", "tar": 'touch "$MARKER"; exit 91'}.items():
                path = bin_dir / name
                path.write_text("#!/bin/bash\n" + body + "\n")
                path.chmod(0o755)
            env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "TMPDIR": str(base),
                   "MARKER": str(base / "unpacked"), "GITHUB_WORKSPACE": str(base)}
            for case, payload in (("good", good), ("corrupt", b"changed"), ("truncated", good[:3]), ("nohash", good)):
                with self.subTest(case=case):
                    marker = base / "unpacked"
                    marker.unlink(missing_ok=True)
                    (scripts.parent / ".env").write_text(pins + (f"\nVMAF_SOURCE_SHA256={digest}\n" if case != "nohash" else ""))
                    result = subprocess.run(["bash", str(scripts / "stage-native-libs.sh"), str(base / "stage")],
                                            cwd=base, env={**env, "PAYLOAD": payload.hex()}, capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(marker.exists(), case == "good", result.stdout + result.stderr)

    def test_r05_download_boundary_mirrors_http_overrides_and_preservation(self) -> None:
        """Bypassing the production digest or HTTP check must publish bad bytes."""
        helper = Path(__file__).resolve().parents[1] / "download-verified.sh"
        self.assertTrue(helper.is_file(), "missing production verified-download boundary")
        with tempfile.TemporaryDirectory(prefix="r05 download ") as directory:
            base = Path(directory)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            curl = bin_dir / "curl"
            curl.write_text(f"#!{sys.executable}\n" + r'''
import os, sys
from pathlib import Path
a = sys.argv[1:]
assert "-o" in a and any("f" in v for v in a if v.startswith("-")), a
url = a[-1]
with Path(os.environ["CALLS"]).open("a") as out: out.write(url + "\n")
Path(a[a.index("-o")+1]).write_bytes({"good": b"R05 verified bytes\n", "bad": b"corrupt", "short": b"R05", "http": b"<html>503</html>"}[url.rsplit("/",1)[-1]])
sys.exit(22 if url.endswith("/http") else 0)
''')
            curl.chmod(0o755)
            destination = base / "payload"
            calls = base / "calls"
            digest = hashlib.sha256(b"R05 verified bytes\n").hexdigest()
            cases = [(digest, ["good"], True), (digest, ["bad"], False),
                     (digest, ["short"], False), (digest, ["http"], False),
                     (digest, ["http", "bad", "good"], True),
                     (digest, ["bad", "short"], False), ("", ["good"], False),
                     ("not-a-hash", ["good"], False)]
            for expected, sources, accepted in cases:
                with self.subTest(expected=expected, sources=sources):
                    destination.write_bytes(b"existing destination")
                    calls.unlink(missing_ok=True)
                    result = subprocess.run(["bash", str(helper), expected, str(destination),
                                             *[f"https://override.invalid/{s}" for s in sources]],
                                            env={"PATH": f"{bin_dir}:/usr/bin:/bin", "CALLS": str(calls)}, capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)
                    self.assertEqual(destination.read_bytes(), b"R05 verified bytes\n" if accepted else b"existing destination")
                    if accepted:
                        self.assertEqual(destination.stat().st_mode & 0o777, 0o644, "Docker's runtime user must be able to read verified plugins")
                    if expected in ("", "not-a-hash"):
                        self.assertFalse(calls.exists(), "missing/invalid expected hash must fail before downloading")
                    self.assertEqual(list(base.glob("*.part.*")), [], "partial payload leaked")

    @unittest.skipUnless(os.name == "nt", "native PowerShell installer acceptance runs on Windows CI")
    def test_r05_windows_installer_boundaries_and_local_candidate(self) -> None:
        """Run real PS functions and installer; never load/execute fixture DLL bytes."""
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="r05 native ") as directory:
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
    throw 'R05 unpack marker; fixture is not an executable archive'
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

    def test_r05_pin_propagation_and_download_consumers(self) -> None:
        """Supplement runtime checks: consumers must not detach from verified boundaries."""
        root = Path(__file__).resolve().parents[3]
        pins = dict(re.findall(r"^([A-Za-z0-9_]+)=(\S+)$", (root / ".github/.env").read_text(), re.M))
        vmaf = (root / "av-metrics-vmaf/scripts/install-libvmaf-windows.ps1").read_text()
        vship = (root / "av-metrics-vship/scripts/install-libvship-windows.ps1").read_text()
        stage = (root / ".github/scripts/stage-native-libs.ps1").read_text()

        # Static policy/mutation checks only; the native fixture above executes PS.
        anchored_candidate = "[IO.Path]::Combine((Get-Location -PSProvider FileSystem).ProviderPath, $directory, $name)"
        normalize_destination = "$Destination = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Destination)"

        def release_pin_policy(source: str) -> None:
            release = source[source.index("$vshipStaging = "):source.index("if (-not $SkipModels)")]
            self.assertRegex(release, r"(?m)^if \(\(Get-FileHash -Algorithm SHA256 -LiteralPath \$vshipStaging\)\.Hash -ne \(Get-Pin 'VSHIP_VULKAN_SHA256'\)\) \{\n\s+throw [^\n]+\n\}",
                             "release bytes need an unconditional final Vulkan digest guard")

        def filesystem_policy(source: str) -> None:
            scan = source[source.index("$directories = "):source.index("$Sha256 = ")]
            self.assertNotRegex(scan, r"\b(?:Join-Path|Split-Path|Test-Path)\b", "candidate probes must not resolve PowerShell providers")
            self.assertNotIn("GetUnresolvedProviderPathFromPSPath(", scan, "ambient candidates must stay filesystem-only")
            for operation in ("[IO.Path]::GetDirectoryName(", "[IO.Path]::Combine(", "[IO.File]::Exists("):
                self.assertIn(operation, scan)
            self.assertIn(anchored_candidate, scan, "relative candidate probes must use the PowerShell filesystem location")

        def destination_policy(source: str) -> None:
            self.assertEqual(source.count("GetUnresolvedProviderPathFromPSPath("), 1, "resolve Destination exactly once")
            self.assertIn(normalize_destination, source)
            for operation in ("$directories = ", "Move-Item ", "New-Item ", "Copy-Item "):
                self.assertLess(source.index(normalize_destination), source.index(operation), "resolve Destination before discovery and writes")

        with self.subTest(policy="release-pin"):
            release_pin_policy(stage)
        with self.subTest(policy="filesystem-only"):
            filesystem_policy(vship)
        with self.subTest(policy="destination-boundary"):
            destination_policy(vship)
        with self.assertRaises(AssertionError):
            destination_policy(vship.replace(normalize_destination, ""))
        with self.assertRaises(AssertionError):
            destination_policy(vship.replace(normalize_destination, "").replace("$Sha256 = ", normalize_destination + "\n$Sha256 = ", 1))
        with self.assertRaises(AssertionError):
            destination_policy(vship.replace(normalize_destination, normalize_destination + "\n" + normalize_destination))
        with self.assertRaises(AssertionError):
            release_pin_policy(stage.replace(".Hash -ne (Get-Pin 'VSHIP_VULKAN_SHA256')", ".Hash -eq (Get-Pin 'VSHIP_VULKAN_SHA256')"))
        with self.assertRaises(AssertionError):
            filesystem_policy(vship.replace(anchored_candidate, "(Join-Path $directory $name)"))
        with self.assertRaises(AssertionError):
            filesystem_policy(vship.replace(anchored_candidate, "[IO.Path]::Combine($directory, $name)"))
        with self.assertRaises(AssertionError):
            filesystem_policy(vship.replace("[IO.Path]::GetDirectoryName($env:VSSCRIPT_PATH.Trim().Trim('\"'))", "(Split-Path -Parent $env:VSSCRIPT_PATH)"))
        self.assertIn("install-libvship-windows.ps1", stage, "staging must reuse installer candidate-preservation policy")
        self.assertIn("$vshipCandidate.Downloaded", stage, "local staging must not claim a newly verified download")
        for key, value in pins.items():
            if key.startswith(("VMAF_MODEL_", "MSYS2_")) or key == "VMAF_PACKAGE_SHA256":
                self.assertIn(value, vmaf, f"standalone installer pin drift: {key}")
                self.assertIn(key if not key.startswith("VMAF_MODEL_") else "VMAF_MODEL_", stage)
            elif key in ("VSHIP_VULKAN_SHA256", "VSHIP_NVIDIA_SHA256"):
                self.assertIn(value, vship, f"standalone Vship pin drift: {key}")
        for source in (vmaf, vship, stage):
            for line in source.splitlines():
                if re.match(r"\s*(?:\$(?:\w+|null) = )?(?:Invoke-Download|Get-Asset|Save-File) -Uri", line):
                    self.assertIn("-Sha256 ", line, "download consumer skipped expected digest")
        steps = self._r03_workflow("windows-build")["jobs"]["build"]["steps"]
        ffms = next(s["run"] for s in steps if s.get("name") == "FFMS2 installation")
        self.assertLess(ffms.index("Get-FileHash"), ffms.index("7z x"))
        self.assertIn("throw", ffms[ffms.index("Get-FileHash"):ffms.index("7z x")])
        docker = (root / "Dockerfile").read_text()
        self.assertNotIn("--extra-index-url", docker)
        self.assertIn("--index-url https://pypi.org/simple --only-binary=:all:", docker)
        self.assertIn("COPY .github/docker-requirements.txt", docker)
        self.assertIn("COPY .github/scripts/download-verified.sh", docker)
        self.assertIn("bash /usr/local/bin/download-verified.sh", docker)
        requirements = (root / ".github/docker-requirements.txt").read_text()
        entries = [line for line in requirements.splitlines() if line and not line.startswith("#")]
        self.assertTrue(any(line.startswith("vsjetpack[full] @ ") for line in entries))
        self.assertTrue(any(line.startswith("dfttest2[cpu] @ ") for line in entries))
        for line in entries:
            self.assertRegex(line, r" @ https://(?:files\.pythonhosted\.org/packages/|github\.com/Jaded-Encoding-Thaumaturgy/vs-wheels/releases/download/).*\.whl#sha256=[0-9a-f]{64}$")
        jobs = self._r03_workflow("tests")["jobs"]
        for job in ("unit-tests", "code-coverage"):
            setup = next(s["run"] for s in jobs[job]["steps"] if s.get("name") == "Setup CPU-native test dependencies")
            self.assertIn("--no-deps --no-index", setup)
            self.assertRegex(setup, r"vapoursynth_vszip-24\.0\.0-py3-none-manylinux_2_17_x86_64\.whl#sha256=[0-9a-f]{64}")

    def _r04_workflows(self) -> dict:
        names = {"tests", "linux-build", "windows-build", "release", "mdbook", "miri-grain"}
        directory = Path(__file__).resolve().parents[2] / "workflows"
        self.assertEqual({p.stem for p in directory.glob("*.yml")}, names)
        return {name: self._r03_workflow(name) for name in sorted(names)}

    def _r04_assert_policy(self, workflows: dict) -> None:
        # These are the only explicit credential consumers, never native/build commands.
        allowed = {
            ("release", "release", "Publish coordinated release", "env", "GH_TOKEN"): "${{ github.token }}",
            ("tests", "docker-publish", "Check publishing credentials", "env", "DOCKERHUB_USERNAME"): "${{ secrets.DOCKERHUB_USERNAME }}",
            ("tests", "docker-publish", "Check publishing credentials", "env", "DOCKERHUB_TOKEN"): "${{ secrets.DOCKERHUB_TOKEN }}",
            ("tests", "docker-publish", "Docker meta", "with", "images"): "${{ secrets.DOCKERHUB_USERNAME }}/av1an",
            ("tests", "docker-publish", "Login to DockerHub", "with", "username"): "${{ secrets.DOCKERHUB_USERNAME }}",
            ("tests", "docker-publish", "Login to DockerHub", "with", "password"): "${{ secrets.DOCKERHUB_TOKEN }}",
            ("tests", "code-coverage", "Upload coverage to Codecov", "with", "token"): "${{ secrets.CODECOV_TOKEN }}",
        }
        sensitive = re.compile(r"secrets\.|github\.token|github_token|GH_TOKEN|GITHUB_TOKEN", re.I)

        def credentials(value: object, path: tuple) -> None:
            if isinstance(value, dict):
                for key, child in value.items():
                    self.assertNotEqual((key, child), ("secrets", "inherit"), path)
                    credentials(child, (*path, key))
            elif isinstance(value, list):
                for index, child in enumerate(value):
                    credentials(child, (*path, index))
            elif sensitive.search(str(value)) or sensitive.search(str(path[-1])):
                self.assertIn(path, allowed, f"credential propagation: {path}")
                self.assertEqual(value, allowed[path])

        for name, workflow in workflows.items():
            default = workflow.get("permissions")
            self.assertIsInstance(default, dict, name)
            self.assertTrue(set(default) <= {"contents"}, f"workflow permission leak: {name}")
            self.assertTrue(all(v in {"read", "none"} for v in default.values()))
            self.assertNotIn("pull_request_target", workflow["on"])
            for key, value in workflow.items():
                if key != "jobs":
                    credentials(value, (name, key))
            for job_name, job in workflow["jobs"].items():
                self.assertNotIn("secrets", job, "builds must not inherit publishing secrets")
                permissions = job.get("permissions", default)
                wanted = ({"contents": "write"} if (name, job_name) == ("release", "release") else
                          {"pages": "write", "id-token": "write"} if (name, job_name) == ("mdbook", "deploy") else
                          {"contents": "read", "pages": "read"} if (name, job_name) == ("mdbook", "build") else
                          {"contents": "read"})
                self.assertEqual(permissions, wanted, f"job permission leak: {name}/{job_name}")
                for key, value in job.items():
                    if key != "steps":
                        credentials(value, (name, job_name, key))
                uses = job.get("uses", "")
                if uses:
                    self.assertIn(uses, {"./.github/workflows/linux-build.yml", "./.github/workflows/windows-build.yml"})
                for step in job.get("steps", []):
                    credentials(step, (name, job_name, step.get("name", step.get("uses", ""))))
                    uses = step.get("uses", "")
                    if not uses:
                        continue
                    self.assertRegex(uses, r"^[\w.-]+/[\w.-]+@[0-9a-f]{40}$", "mutable action ref")
                    inputs = step.get("with", {})
                    if uses.startswith("actions/checkout@"):
                        self.assertEqual(inputs.get("persist-credentials"), "false", "checkout persistence")
                    if uses.startswith("dtolnay/rust-toolchain@"):
                        self.assertEqual(inputs.get("toolchain"), "stable", "SHA is not a toolchain name")
                    if uses.startswith("actions/configure-pages@"):
                        self.assertEqual(inputs.get("enablement"), "false", "metadata must not create a site")
                    if uses.startswith("docker/build-push-action@"):
                        self.assertFalse(set(inputs) & {"secrets", "secret-envs", "secret-files", "github-token"})
                        self.assertNotIn("TOKEN", inputs.get("build-args", "").upper())
        docs = workflows["mdbook"]["jobs"]
        self.assertEqual(docs["build"]["needs"], "screenshots")
        self.assertEqual(docs["deploy"]["needs"], "build")
        self.assertEqual(docs["deploy"]["environment"]["name"], "github-pages")

    def _r04_assert_deploy_gate(self, workflow: dict) -> None:
        expression = runpy.run_path(str(Path(__file__).resolve().parents[1] / "check-docker-policy.py"))["expression"]
        gate = " ".join(workflow["jobs"]["deploy"].get("if", "True").split())
        cases = [
            ("rust-av/Av1an", "push", "refs/heads/master", True),
            ("rust-av/Av1an", "push", "refs/heads/feature", False),
            ("rust-av/Av1an", "push", "refs/tags/v1.2.3", False),
            ("rust-av/Av1an", "workflow_dispatch", "refs/heads/master", False),
            ("fork/Av1an", "push", "refs/heads/master", False),
            ("fork/Av1an", "workflow_dispatch", "refs/heads/master", False),
            ("rust-av/Av1an", "pull_request", "refs/pull/1/merge", False),
            ("fork/Av1an", "pull_request", "refs/pull/1/merge", False),
        ]
        for repo, event, ref, wanted in cases:
            context = {"github": SimpleNamespace(repository=repo, event_name=event, ref=ref)}
            self.assertEqual(bool(expression(gate, context)), wanted, (repo, event, ref))

    def test_r04_actual_least_privilege_pins_and_credential_boundaries(self) -> None:
        """Global Pages write, mutable uses, persisted checkout or token exports fail."""
        self._r04_assert_policy(self._r04_workflows())
        directory = Path(__file__).resolve().parents[2] / "workflows"
        for path in directory.glob("*.yml"):
            for line in path.read_text().splitlines():
                if "uses:" in line and "./.github/workflows/" not in line:
                    self.assertRegex(line, r"@[0-9a-f]{40} # (v\d[^\s]*|stable)$")

    def test_r04_actual_pages_gate_denies_manual_forks_and_prs(self) -> None:
        """A fork PR sees neither writable jobs nor explicit publication credentials."""
        workflows = self._r04_workflows()
        self._r04_assert_deploy_gate(workflows["mdbook"])
        expression = runpy.run_path(str(Path(__file__).resolve().parents[1] / "check-docker-policy.py"))["expression"]
        context = {"github": SimpleNamespace(repository="fork/Av1an", event_name="pull_request", ref="refs/pull/1/merge"),
                   "needs": SimpleNamespace(linux=SimpleNamespace(result="success"), windows=SimpleNamespace(result="success"))}
        for name, workflow in workflows.items():
            for job in workflow["jobs"].values():
                if not expression(" ".join(job.get("if", "True").split()), context):
                    continue
                self.assertNotIn("write", job.get("permissions", workflow["permissions"]).values(), name)
                for step in job.get("steps", []):
                    if expression(" ".join(step.get("if", "True").split()), context):
                        self.assertNotIn("secrets.", str(step), name)
                        self.assertNotIn("github.token", str(step), name)

    def test_r04_policy_mutations_are_rejected(self) -> None:
        """Exercise the same production-policy assertions on in-memory unsafe edits."""
        workflows = self._r04_workflows()
        for mutation in ("global-write", "native-write", "checkout", "mutable", "toolchain", "token-env", "token-argument", "token-output", "inherit", "metadata-write"):
            with self.subTest(mutation=mutation):
                changed = deepcopy(workflows)
                job = changed["linux-build"]["jobs"]["build"]
                checkout = next(s for s in job["steps"] if s.get("uses", "").startswith("actions/checkout@"))
                if mutation == "global-write": changed["mdbook"]["permissions"]["pages"] = "write"
                elif mutation == "native-write": job["permissions"] = {"contents": "write"}
                elif mutation == "checkout": checkout["with"].pop("persist-credentials", None)
                elif mutation == "mutable": checkout["uses"] = "actions/checkout@v6"
                elif mutation == "toolchain":
                    next(s for s in job["steps"] if s.get("uses", "").startswith("dtolnay/"))["with"].pop("toolchain", None)
                elif mutation == "token-env": job["env"]["GH_TOKEN"] = "${{ github.token }}"
                elif mutation == "token-argument": job["steps"].append({"run": "cargo build --token '${{ github.token }}'"})
                elif mutation == "token-output": job["outputs"] = {"publish": "${{ secrets.PUBLISH_TOKEN }}"}
                elif mutation == "inherit": changed["release"]["jobs"]["linux"]["secrets"] = "inherit"
                else:
                    next(s for s in changed["mdbook"]["jobs"]["build"]["steps"] if s.get("id") == "pages")["with"]["enablement"] = "true"
                with self.assertRaises(AssertionError): self._r04_assert_policy(changed)
        for gate in ("True", "github.event_name == 'push' && github.ref == 'refs/heads/master'",
                     "github.repository == 'rust-av/Av1an' && github.ref == 'refs/heads/master'"):
            with self.subTest(gate=gate):
                changed = deepcopy(workflows["mdbook"])
                changed["jobs"]["deploy"]["if"] = gate
                with self.assertRaises(AssertionError): self._r04_assert_deploy_gate(changed)

    def test_r04_changelog_subprocess_does_not_inherit_publish_tokens(self) -> None:
        """Run the real publisher with the R07 child rejecting both token variables."""
        with tempfile.TemporaryDirectory(prefix="r04 child ") as directory:
            repo, env = self._r03_fixture(Path(directory))
            script = repo / ".github/scripts/generate-changelog.sh"
            script.write_text('test "${GH_TOKEN+x}${GITHUB_TOKEN+x}" = "" || exit 91\n' + script.read_text())
            env["GITHUB_TOKEN"] = "OFFLINE_R04_INERT"
            result = self._r03_publish(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def _r03_workflow(self, name: str) -> dict:
        import yaml

        path = Path(__file__).resolve().parents[2] / f"workflows/{name}.yml"
        self.assertTrue(path.is_file(), f"missing production workflow: {path.name}")
        return yaml.load(path.read_text(), Loader=yaml.BaseLoader)

    def test_r03_workflow_handoff_and_single_trusted_publisher(self) -> None:
        """An independent publisher, ungated upload, or cross-run download must fail."""
        for platform in ("linux", "windows"):
            workflow = self._r03_workflow(f"{platform}-build")
            self.assertEqual(set(workflow["on"]), {"workflow_call", "workflow_dispatch"})
            self.assertEqual(workflow["permissions"], {"contents": "read"})
            self.assertEqual(set(workflow["jobs"]), {"build"})
            self.assertNotIn("secrets.", str(workflow))
            self.assertNotIn("action-gh-release", str(workflow))
            steps = workflow["jobs"]["build"]["steps"]
            checkout = next(s for s in steps if "actions/checkout@" in s.get("uses", ""))
            self.assertEqual(checkout["with"]["ref"], "${{ github.sha }}")
            upload = next(i for i, s in enumerate(steps) if "actions/upload-artifact@" in s.get("uses", ""))
            stage = next(i for i, s in enumerate(steps) if "coordinate-release.py stage" in s.get("run", ""))
            self.assertLess(stage, upload)
            self.assertNotIn("if", steps[stage])
            self.assertNotIn("continue-on-error", steps[stage])
            self.assertNotIn("if", steps[upload])
            self.assertEqual(steps[upload]["with"], {
                "name": f"av1an-{platform}-x64-${{{{ github.sha }}}}-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}",
                "path": "dist/", "retention-days": "1", "if-no-files-found": "error",
            })
            if platform == "windows":
                gate = next(i for i, s in enumerate(steps) if "--native" in s.get("run", ""))
            else:
                gate = next(i for i, s in enumerate(steps) if s.get("name") == "Smoke-test the archive")
            self.assertLess(gate, stage)
            self.assertNotIn("if", steps[gate])
            self.assertNotIn("continue-on-error", steps[gate])

        workflow = self._r03_workflow("release")
        self.assertEqual(workflow["on"]["push"], {"branches": ["master"], "tags": ["v*"]})
        self.assertIn("workflow_dispatch", workflow["on"])
        self.assertNotIn("concurrency", workflow, "must not cancel an in-flight publisher")
        self.assertEqual(workflow["permissions"], {"contents": "read"})
        jobs = workflow["jobs"]
        self.assertEqual(set(jobs), {"linux", "windows", "release"})
        for platform in ("linux", "windows"):
            self.assertEqual(jobs[platform], {"uses": f"./.github/workflows/{platform}-build.yml"})
        release = jobs["release"]
        self.assertEqual(set(release["needs"]), {"linux", "windows"})
        self.assertEqual(release["permissions"], {"contents": "write"})
        self.assertEqual(release["concurrency"], {
            "group": "condor-release-${{ github.repository }}-${{ github.ref == 'refs/heads/master' && 'latest' || github.ref }}",
            "cancel-in-progress": "false",
        })
        steps = release["steps"]
        self.assertEqual(steps[0]["with"], {"fetch-depth": "0", "ref": "${{ github.sha }}", "persist-credentials": "false"})
        downloads = [s["with"] for s in steps if "actions/download-artifact@" in s.get("uses", "")]
        self.assertEqual(downloads, [{
            "name": f"av1an-{p}-x64-${{{{ github.sha }}}}-${{{{ github.run_id }}}}-${{{{ github.run_attempt }}}}",
            "path": f"handoff/{p}",
        } for p in ("linux", "windows")])
        publishers = [s for s in steps if "coordinate-release.py publish" in s.get("run", "")]
        self.assertEqual(len(publishers), 1)
        self.assertEqual(steps[-1], publishers[0])
        self.assertEqual(publishers[0]["env"], {"GH_TOKEN": "${{ github.token }}"})
        self.assertTrue(all("GH_TOKEN" not in str(s) for s in steps[:-1]))

    def test_r03_actual_workflow_gate_denies_failure_manual_and_forks(self) -> None:
        """Evaluate the production gate, not a copied release decision table."""
        script = Path(__file__).resolve().parents[1] / "check-docker-policy.py"
        expression = runpy.run_path(str(script))["expression"]
        gate = " ".join(self._r03_workflow("release")["jobs"]["release"]["if"].split())
        cases = [
            ("rust-av/Av1an", "push", "refs/heads/master", "success", "success", True),
            ("rust-av/Av1an", "push", "refs/tags/v1.2.3", "success", "success", True),
            ("rust-av/Av1an", "push", "refs/heads/master", "failure", "success", False),
            ("rust-av/Av1an", "push", "refs/heads/master", "success", "cancelled", False),
            ("rust-av/Av1an", "push", "refs/heads/master", "skipped", "success", False),
            ("fork/Av1an", "push", "refs/heads/master", "success", "success", False),
            ("fork/Av1an", "push", "refs/tags/v1.2.3", "success", "success", False),
        ]
        cases += [("rust-av/Av1an", "workflow_dispatch", ref, "success", "success", False)
                  for ref in ("refs/heads/feature", "refs/heads/master", "refs/tags/v1.2.3")]
        cases += [("rust-av/Av1an", "push", "refs/heads/feature", "success", "success", False)]
        for repo, event, ref, linux, windows, wanted in cases:
            with self.subTest(repo=repo, event=event, ref=ref, linux=linux, windows=windows):
                context = {"github": SimpleNamespace(repository=repo, event_name=event, ref=ref),
                           "needs": SimpleNamespace(linux=SimpleNamespace(result=linux), windows=SimpleNamespace(result=windows))}
                self.assertEqual(bool(expression(gate, context)), wanted)

    def _r03_fixture(self, base: Path) -> tuple[Path, dict]:
        scripts = Path(__file__).resolve().parents[1]
        helper = scripts / "coordinate-release.py"
        self.assertTrue(helper.is_file(), "missing production handoff/publisher helper")
        repo = base / "repo"
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            self._r07_repo(repo)
        destination = repo / ".github/scripts"
        destination.mkdir(parents=True)
        for name in ("coordinate-release.py", "generate-changelog.sh", "linux-release-notes.md"):
            shutil.copyfile(scripts / name, destination / name)
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_OLD")
            self._r07_git(repo, "tag", "v0.5.1")
            self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_NEW")
            self._r07_git(repo, "tag", "v1.2.3")
            self._r07_git(repo, "tag", "-a", "v1.2.3+雪#%", "-m", "annotated fixture")
            self._r07_git(repo, "branch", "v1.2.3", "v0.5.1")
            sha = self._r07_git(repo, "rev-parse", "HEAD")
        bin_dir = base / "bin"
        bin_dir.mkdir()
        # The only remote boundary is this owned, fail-closed executable. No real gh.
        gh = bin_dir / "gh"
        gh.write_text(f"#!{sys.executable}\n" + r'''
import json, os, sys
from pathlib import Path
a = sys.argv[1:]
p = Path(os.environ["FAKE_STATE"])
s = json.loads(p.read_text())
s["calls"].append(a)
def save(): p.write_text(json.dumps(s))
def value(flag): return a[a.index(flag) + 1]
if a[0] == "api":
    if s.get("deny_api"):
        save(); print("gh: Forbidden (HTTP 403)", file=sys.stderr); sys.exit(1)
    endpoint = a[1]
    prefix = "repos/rust-av/Av1an/"
    assert endpoint.startswith(prefix), a
    endpoint = endpoint[len(prefix):]
    method = value("--method") if "--method" in a else "GET"
    if endpoint == "git/ref/heads/master" and method == "GET":
        print(json.dumps({"object": {"sha": s["master"]}}))
    # Independent literal oracle: Get a commit documents tags/TAG_NAME, not refs/tags/.
    elif endpoint in ("commits/tags/v1.2.3", "commits/tags/v1.2.3%2B%E9%9B%AA%23%25") and method == "GET":
        if s.get("tag_error"):
            save(); print(f"gh: tag lookup failed (HTTP {s['tag_error']})", file=sys.stderr); sys.exit(1)
        print(json.dumps({"sha": s["tag_commit"]}))
    elif endpoint == "git/ref/tags/latest" and method == "GET":
        if s["latest"] is None:
            save(); print("gh: Not Found (HTTP 404)", file=sys.stderr); sys.exit(1)
        print(json.dumps({"object": {"sha": s["latest"]}}))
    elif endpoint in ("git/refs/tags/latest", "git/refs") and method in ("PATCH", "POST"):
        fields = [a[i + 1] for i, v in enumerate(a[:-1]) if v in ("-f", "-F")]
        if method == "PATCH": assert "force=true" in fields, a
        else: assert "ref=refs/tags/latest" in fields, a
        s["latest"] = next(v[4:] for v in fields if v.startswith("sha="))
    elif endpoint.startswith("releases/tags/") and method == "GET":
        tag = endpoint.removeprefix("releases/tags/")
        if tag not in s["releases"]:
            save(); print("gh: Not Found (HTTP 404)", file=sys.stderr); sys.exit(1)
        print(json.dumps({"assets": [{"id": 7, "name": "condor.exe"}]}))
    elif endpoint == "releases/assets/7" and method == "DELETE":
        pass
    else:
        save(); raise AssertionError(a)
elif a[:2] in (["release", "create"], ["release", "edit"]):
    assert value("--repo") == "rust-av/Av1an", a
    if a[1] == "create": assert "--verify-tag" in a, a
    s["releases"][a[2]] = {"body": Path(value("--notes-file")).read_text()}
elif a[:2] == ["release", "upload"]:
    assert value("--repo") == "rust-av/Av1an" and "--clobber" in a, a
    if s.get("fail_upload"):
        save(); sys.exit(2)
    files = a[a.index("--") + 1:]
    s["releases"][a[2]]["assets"] = {Path(f).name: Path(f).read_bytes().hex() for f in files}
else: raise AssertionError(a)
save()
''')
        gh.chmod(0o755)
        (base / "state.json").write_text(json.dumps({
            "master": sha, "tag_commit": sha, "latest": "0" * 40,
            "releases": {"latest": {}}, "calls": [],
        }))
        temp = base / "temp"
        temp.mkdir()
        # No BASH_ENV, exported functions, credentials, HOME config, or GIT_* routing.
        env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "TMPDIR": str(temp),
               "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
               "GIT_CEILING_DIRECTORIES": str(base), "GH_TOKEN": "OFFLINE_R03_INERT",
               "GITHUB_SHA": sha, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
               "GITHUB_REPOSITORY": "rust-av/Av1an", "GITHUB_EVENT_NAME": "push",
               "GITHUB_REF": "refs/heads/master", "FAKE_STATE": str(base / "state.json")}
        self._r03_stage(repo, env)
        return repo, env

    def _r03_stage(self, repo: Path, env: dict) -> None:
        for platform in ("linux", "windows"):
            (repo / "target").mkdir(exist_ok=True)
            schema = repo / "california-condor/configuration.schema.json"
            schema.parent.mkdir(exist_ok=True)
            schema.write_bytes(b'\xef\xbb\xbf{\r\n  "properties": {}, "type": "object"\r\n}\r\n'
                               if platform == "windows" else b'{"type":"object","properties":{}}\n')
            with zipfile.ZipFile(repo / f"target/av1an-{platform}-x64.zip", "w") as archive:
                archive.writestr("configuration.schema.json", schema.read_bytes())
                archive.writestr("av1an.exe" if platform == "windows" else "av1an", platform + env["GITHUB_SHA"])
            steps = self._r03_workflow(f"{platform}-build")["jobs"]["build"]["steps"]
            stage = next(s["run"] for s in steps if "coordinate-release.py stage" in s.get("run", ""))
            # Windows uses the same stdlib CLI; PowerShell/native acceptance remains CI-only.
            result = subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", stage], cwd=repo, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            target = repo / f"handoff/{platform}"
            target.parent.mkdir(exist_ok=True)
            (repo / "dist").rename(target)

    def _r03_publish(self, repo: Path, env: dict) -> subprocess.CompletedProcess[str]:
        steps = self._r03_workflow("release")["jobs"]["release"]["steps"]
        command = next(s["run"] for s in steps if "coordinate-release.py publish" in s.get("run", ""))
        return subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", command], cwd=repo, env=env, capture_output=True, text=True)

    def test_r03_publication_body_assets_tag_and_serialized_stale_order(self) -> None:
        """Run production commands with real owned Git history and an inert GH boundary."""
        with tempfile.TemporaryDirectory(prefix="r03 publish ") as directory:
            base = Path(directory)
            repo, env = self._r03_fixture(base)
            state_path = base / "state.json"
            tag_endpoints = {
                "refs/tags/v1.2.3": "repos/rust-av/Av1an/commits/tags/v1.2.3",
                "refs/tags/v1.2.3+雪#%": "repos/rust-av/Av1an/commits/tags/v1.2.3%2B%E9%9B%AA%23%25",
            }
            with patch.dict(os.environ, env, clear=True):
                self.assertEqual(self._r07_git(repo, "cat-file", "-t", "refs/tags/v1.2.3"), "commit")
                self.assertEqual(self._r07_git(repo, "cat-file", "-t", "refs/tags/v1.2.3+雪#%"), "tag")
                self.assertEqual(self._r07_git(repo, "rev-parse", "refs/tags/v1.2.3+雪#%^{commit}"), env["GITHUB_SHA"])
            for ref in ("refs/heads/master", *tag_endpoints):
                with self.subTest(ref=ref):
                    env["GITHUB_REF"] = ref
                    result = self._r03_publish(repo, env)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    state = json.loads(state_path.read_text())
                    tag = "latest" if ref.endswith("master") else ref.removeprefix("refs/tags/")
                    release = state["releases"][tag]
                    self.assertIn(f"Latest build from {env['GITHUB_SHA']}\n", release["body"])
                    self.assertIn("- R03_NEW\n", release["body"])
                    notes = (repo / ".github/scripts/linux-release-notes.md").read_text()
                    self.assertIn(notes, release["body"])
                    self.assertEqual(set(release["assets"]), {"av1an-linux-x64.zip", "av1an-windows-x64.zip", "configuration.schema.json"})
                    for platform in ("linux", "windows"):
                        name = f"av1an-{platform}-x64.zip"
                        self.assertEqual(release["assets"][name], (repo / f"handoff/{platform}/{name}").read_bytes().hex())
                    self.assertEqual(state["latest"], env["GITHUB_SHA"])
                    if ref.startswith("refs/tags/"):
                        self.assertEqual(state["calls"][0], ["api", tag_endpoints[ref]])
                        self.assertFalse(any("git/refs" in " ".join(a) for a in state["calls"]))
                    state["calls"] = []
                    state_path.write_text(json.dumps(state))

            # A newer run publishes first, then the slow old build enters the lock.
            old_env = env.copy()
            old_env["GITHUB_REF"] = "refs/heads/master"
            (repo / "handoff").rename(repo / "old-handoff")
            with patch.dict(os.environ, env, clear=True):
                self._r07_git(repo, "commit", "--allow-empty", "-m", "R03_NEWER")
                env["GITHUB_SHA"] = self._r07_git(repo, "rev-parse", "HEAD")
            env["GITHUB_RUN_ID"] = "124"
            env["GITHUB_REF"] = "refs/heads/master"
            self._r03_stage(repo, env)
            state = json.loads(state_path.read_text())
            state["master"] = env["GITHUB_SHA"]
            state_path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = json.loads(state_path.read_text())
            self.assertEqual(state["latest"], env["GITHUB_SHA"])
            self.assertIn(f"Latest build from {env['GITHUB_SHA']}\n", state["releases"]["latest"]["body"])
            for platform in ("linux", "windows"):
                name = f"av1an-{platform}-x64.zip"
                self.assertEqual(state["releases"]["latest"]["assets"][name], (repo / f"handoff/{platform}/{name}").read_bytes().hex())
            before = state["releases"]
            state["calls"] = []
            state_path.write_text(json.dumps(state))
            (repo / "handoff").rename(repo / "new-handoff")
            (repo / "old-handoff").rename(repo / "handoff")
            with patch.dict(os.environ, env, clear=True):
                self._r07_git(repo, "checkout", "--detach", old_env["GITHUB_SHA"])
            result = self._r03_publish(repo, old_env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("stale", result.stdout.lower())
            state = json.loads(state_path.read_text())
            self.assertEqual(state["releases"], before)
            self.assertEqual(state["calls"], [["api", "repos/rust-av/Av1an/git/ref/heads/master"]])
            self.assertEqual(list((base / "temp").iterdir()), [])

    def test_r03_rejects_invalid_handoff_and_untrusted_runs_before_write(self) -> None:
        """Removing a provenance/schema/HEAD/policy check must expose a fake write."""
        cases = ("mixed-sha", "wrong-run", "wrong-attempt", "wrong-platform", "missing-platform",
                 "schema-mismatch", "checkout-sha", "tag-moved", "tag-missing", "tag-api-failure", "api-failure", "manual-master", "manual-tag", "manual-feature", "fork", "fork-tag")
        with tempfile.TemporaryDirectory(prefix="r03 reject ") as directory:
            for case in cases:
                with self.subTest(case=case):
                    base = Path(directory) / case
                    base.mkdir()
                    repo, env = self._r03_fixture(base)
                    manifest_path = repo / "handoff/windows/provenance.json"
                    manifest = json.loads(manifest_path.read_text())
                    key = {"mixed-sha": "sha", "wrong-run": "run_id", "wrong-attempt": "run_attempt", "wrong-platform": "platform"}.get(case)
                    if key:
                        manifest[key] = "0" * 40 if key == "sha" else "wrong"
                        manifest_path.write_text(json.dumps(manifest))
                    elif case == "missing-platform":
                        (repo / "handoff/windows/av1an-windows-x64.zip").unlink()
                    elif case == "schema-mismatch":
                        (repo / "handoff/windows/configuration.schema.json").write_text('{"type":"string"}')
                        with zipfile.ZipFile(repo / "handoff/windows/av1an-windows-x64.zip", "w") as archive:
                            archive.writestr("configuration.schema.json", '{"type":"string"}')
                    elif case == "checkout-sha":
                        env["GITHUB_SHA"] = "0" * 40
                    elif case in ("tag-moved", "tag-missing", "tag-api-failure", "api-failure"):
                        state_path = base / "state.json"
                        state = json.loads(state_path.read_text())
                        if case.startswith("tag-"):
                            env["GITHUB_REF"] = "refs/tags/v1.2.3"
                            if case == "tag-moved":
                                state["tag_commit"] = "f" * 40
                            else:
                                state["tag_error"] = 404 if case == "tag-missing" else 503
                        else:
                            state["deny_api"] = True
                        state_path.write_text(json.dumps(state))
                    elif case.startswith("manual"):
                        env["GITHUB_EVENT_NAME"] = "workflow_dispatch"
                        env["GITHUB_REF"] = {"manual-master": "refs/heads/master", "manual-tag": "refs/tags/v1.2.3", "manual-feature": "refs/heads/feature"}[case]
                    elif case.startswith("fork"):
                        env["GITHUB_REPOSITORY"] = "fork/Av1an"
                        if case == "fork-tag":
                            env["GITHUB_REF"] = "refs/tags/v1.2.3"
                    result = self._r03_publish(repo, env)
                    if case.startswith(("manual", "fork")):
                        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    else:
                        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                        self.assertRegex(result.stderr.lower(), "sha|run|attempt|platform|schema|archive|tag|provenance|403")
                    state = json.loads((base / "state.json").read_text())
                    if case.startswith("tag-"):
                        self.assertEqual(state["calls"], [["api", "repos/rust-av/Av1an/commits/tags/v1.2.3"]])
                    self.assertEqual(state["releases"], {"latest": {}})
                    self.assertFalse(any("--method" in a or a[0] == "release" for a in state["calls"]))
                    self.assertEqual(list((base / "temp").iterdir()), [])

    def test_r03_create_latest_and_surface_partial_remote_failure(self) -> None:
        """A missing managed tag is created; failed uploads never report success."""
        with tempfile.TemporaryDirectory(prefix="r03 remote ") as directory:
            base = Path(directory)
            repo, env = self._r03_fixture(base)
            path = base / "state.json"
            state = json.loads(path.read_text())
            state.update(latest=None, releases={})
            path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = json.loads(path.read_text())
            self.assertEqual(state["latest"], env["GITHUB_SHA"])
            self.assertEqual(len(state["releases"]["latest"]["assets"]), 3)
            state["fail_upload"] = True
            path.write_text(json.dumps(state))
            result = self._r03_publish(repo, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Published", result.stdout)
            self.assertEqual(list((base / "temp").iterdir()), [])

    def _r07_git(self, repo: Path, *args: str) -> str:
        # SECURITY: Hook/wrapper Git routing must never redirect fixture writes.
        return subprocess.run(
            ["git", *args], cwd=repo, check=True, capture_output=True, text=True,
            env={**{key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}, "GIT_CONFIG_NOSYSTEM": "1",
                 "GIT_CONFIG_GLOBAL": os.devnull,
                 "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve()),
                 "GIT_AUTHOR_DATE": "2000-01-01T00:00:00+0000",
                 "GIT_COMMITTER_DATE": "2000-01-01T00:00:00+0000"},
        ).stdout.strip()

    def _r07_repo(self, repo: Path) -> None:
        repo.mkdir()
        self._r07_git(repo, "init", "--initial-branch=main")
        self._r07_git(repo, "config", "user.name", "Release Fixture")
        self._r07_git(repo, "config", "user.email", "fixture@example.invalid")
        self._r07_git(repo, "config", "commit.gpgsign", "false")
        self._r07_git(repo, "config", "tag.gpgsign", "false")

    def _r07_changelog(
        self, repo: Path, output: Path, *args: str,
    ) -> subprocess.CompletedProcess[str]:
        script = Path(__file__).resolve().parents[1] / "generate-changelog.sh"
        return subprocess.run(
            ["bash", str(script), *args], cwd=repo, capture_output=True, text=True,
            env={**{key: value for key, value in os.environ.items()
                    if not key.startswith("GIT_")}, "GITHUB_SHA": "OFFLINE_R07_SHA",
                 "GITHUB_OUTPUT": str(output), "GIT_CONFIG_NOSYSTEM": "1",
                 "GIT_CONFIG_GLOBAL": os.devnull,
                 "GIT_CEILING_DIRECTORIES": str(repo.parent.resolve())},
        )

    def test_r07_helpers_isolate_git_environment_and_parent_discovery(self) -> None:
        """Inherited routing must not write to a containing repo or leak its history."""
        with tempfile.TemporaryDirectory(prefix="r07 isolation ") as directory:
            base = Path(directory)
            env = {key: os.environ[key] for key in ("PATH", "SYSTEMROOT")
                   if key in os.environ}
            env["TMPDIR"] = str(base)
            # SECURITY: Never route the unsafe RED helpers to the actual project.
            with patch.dict(os.environ, env, clear=True):
                outer = base / "outer repo"
                self._r07_repo(outer)
                (outer / "sentinel").write_text("preserve outer index and worktree\n")
                self._r07_git(outer, "add", "--", "sentinel")
                self._r07_git(outer, "commit", "-m", "R07_OUTER")
                self._r07_git(outer, "tag", "v9.0.0")
                self._r07_git(outer, "config", "user.name", "Outer Sentinel")

                def outer_state() -> dict[str, bytes]:
                    git_dir = outer / ".git"
                    paths = [git_dir / name for name in
                             ("HEAD", "config", "index", "packed-refs")]
                    paths.extend((git_dir / "refs").rglob("*"))
                    return {str(path.relative_to(git_dir)): path.read_bytes()
                            for path in paths if path.is_file()}

                before = outer_state()
                inner = outer / "inner repo"
                output = base / "inner output"
                polluted = {
                    "GIT_DIR": str(outer / ".git"),
                    "GIT_COMMON_DIR": str(outer / ".git"),
                    "GIT_WORK_TREE": str(outer),
                    "GIT_INDEX_FILE": str(outer / ".git/index"),
                    "GIT_OBJECT_DIRECTORY": str(outer / ".git/objects"),
                    "GIT_ALTERNATE_OBJECT_DIRECTORIES": str(outer / ".git/objects"),
                    "GIT_CONFIG": str(outer / ".git/config"),
                    "GIT_CONFIG_COUNT": "1",
                    "GIT_CONFIG_KEY_0": "user.name",
                    "GIT_CONFIG_VALUE_0": "Injected Outer Identity",
                    "GIT_AUTHOR_NAME": "Injected Outer Author",
                    "GIT_CEILING_DIRECTORIES": str(base),
                }
                with patch.dict(os.environ, polluted):
                    self._r07_repo(inner)
                    self._r07_git(inner, "commit", "--allow-empty", "-m", "R07_INNER")
                    self._r07_git(inner, "tag", "v0.5.1")
                    result = self._r07_changelog(inner, output)

                self.assertEqual(outer_state(), before, "outer HEAD/config/index/refs changed")
                self.assertTrue((inner / ".git").is_dir())
                self.assertEqual(self._r07_git(inner, "log", "-1", "--format=%s"), "R07_INNER")
                self.assertEqual(self._r07_git(inner, "tag", "--list"), "v0.5.1")
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                body = output.read_text()
                self.assertEqual(
                    [line[2:] for line in body.splitlines() if line.startswith("- ")],
                    ["R07_INNER"],
                )
                self.assertIn(": R07_INNER (Release Fixture)\n", body)

                not_git = outer / "not git"
                not_git.mkdir()
                with self.assertRaises(subprocess.CalledProcessError):
                    self._r07_git(not_git, "rev-parse", "--show-toplevel")
                output.write_text("existing=true\n")
                result = self._r07_changelog(not_git, output)
                self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertEqual(output.read_text(), "existing=true\n")
                self.assertEqual(outer_state(), before)

    def test_r07_previous_reachable_version_excludes_head_aliases_and_latest(self) -> None:
        """Current, moving, non-version, or numerically largest tags are not baselines."""
        root = ("commit", "--allow-empty", "-m", "R07_ROOT")
        change = ("commit", "--allow-empty", "-m", "R07_CHANGE")
        head = ("commit", "--allow-empty", "-m", "R07_HEAD")
        previous = ("tag", "-a", "v0.5.1", "-m", "previous release")
        cases = (
            ("firsttag", [root, change, ("tag", "v0.5.1")],
             ["R07_CHANGE", "R07_ROOT"], "the beginning of history"),
            ("secondtag", [root, previous, change, ("tag", "v0.5.2")],
             ["R07_CHANGE"], "v0.5.1"),
            ("masteraftertag", [root, previous, change, head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("stale-latest-ancestor", [root, previous, change, ("tag", "latest"), head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("moving-latest-head", [root, previous, change, ("tag", "latest")],
             ["R07_CHANGE"], "v0.5.1"),
            ("multiple-head-aliases", [root, previous, change,
             ("tag", "v0.5.2"), ("tag", "-a", "v0.5.3", "-m", "alias"),
             ("tag", "latest")], ["R07_CHANGE"], "v0.5.1"),
            ("untagged-first-history", [root, change],
             ["R07_CHANGE", "R07_ROOT"], "the beginning of history"),
            ("ignore-nonrelease-tags", [root, previous, change,
             ("tag", "build-2026"), ("tag", "v9.9.9-preview"),
             ("tag", "v8.8.8junk"), ("tag", "v7.7.7.7"), head],
             ["R07_HEAD", "R07_CHANGE"], "v0.5.1"),
            ("nearest-not-largest", [root, ("tag", "v9.0.0"), change, previous, head],
             ["R07_HEAD"], "v0.5.1"),
        )
        with tempfile.TemporaryDirectory(prefix="r07 git ") as directory:
            base = Path(directory)
            for name, commands, expected, baseline in cases:
                with self.subTest(history=name):
                    repo = base / name
                    self._r07_repo(repo)
                    for command in commands:
                        self._r07_git(repo, *command)
                    output = base / f"{name} output"

                    result = self._r07_changelog(repo, output)

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        expected,
                    )
                    self.assertIn(f"Changes since {baseline}:\n", body)
                    self.assertNotIn("No changes", body)

    def test_r07_merge_uses_release_on_second_parent_not_unreachable_tag(self) -> None:
        """Walking only HEAD's first parent loses the reachable side-branch release."""
        with tempfile.TemporaryDirectory(prefix="r07 merge ") as directory:
            base = Path(directory)
            repo = base / "repo"
            self._r07_repo(repo)
            for command in (
                ("commit", "--allow-empty", "-m", "R07_ROOT"),
                ("branch", "unreachable"),
                ("checkout", "-b", "side"),
                ("commit", "--allow-empty", "-m", "R07_PREVIOUS"),
                ("tag", "v0.5.1"),
                ("commit", "--allow-empty", "-m", "R07_SIDE"),
                ("checkout", "main"),
                ("commit", "--allow-empty", "-m", "R07_MAIN"),
                ("merge", "--no-ff", "side", "-m", "R07_MERGE"),
                ("tag", "v0.5.2"),
                ("checkout", "unreachable"),
                ("commit", "--allow-empty", "-m", "R07_UNREACHABLE"),
                ("tag", "v99.0.0"),
                ("checkout", "main"),
            ):
                self._r07_git(repo, *command)
            output = base / "output"

            result = self._r07_changelog(repo, output)

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            body = output.read_text()
            self.assertCountEqual(
                [line[2:] for line in body.splitlines() if line.startswith("- ")],
                ["R07_MERGE", "R07_MAIN", "R07_SIDE"],
            )
            self.assertIn("Changes since v0.5.1:\n", body)

    def test_r07_body_protocol_notes_and_first_ten_commit_boundary(self) -> None:
        """Keep every subject exactly once, correct overflow counts, and spaced paths."""
        with tempfile.TemporaryDirectory(prefix="r07 body ") as directory:
            base = Path(directory)
            for count in (1, 10, 11, 12):
                with self.subTest(commits=count):
                    repo = base / f"repo {count}"
                    self._r07_repo(repo)
                    self._r07_git(repo, "commit", "--allow-empty", "-m", "R07_BASE")
                    self._r07_git(repo, "tag", "v0.5.1")
                    subjects = [f"R07 change {i:02d} `quotes` & spaces" for i in range(count)]
                    for subject in subjects:
                        self._r07_git(repo, "commit", "--allow-empty", "-m", subject)
                    notes = base / "release notes with spaces.md"
                    notes.write_text("Offline release notes\nverbatim, no final newline")
                    output = base / f"output {count}"
                    output.write_text("existing=true\n")

                    result = self._r07_changelog(repo, output, str(notes))

                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    body = output.read_text()
                    self.assertTrue(body.startswith(
                        "existing=true\nbody<<CHANGELOG_EOF\nLatest build from OFFLINE_R07_SHA\n"
                    ))
                    self.assertIn(f": {subjects[-1]} (Release Fixture)\n", body)
                    self.assertEqual(
                        [line[2:] for line in body.splitlines() if line.startswith("- ")],
                        subjects[::-1],
                    )
                    inline = body.split("<details>")[0]
                    self.assertEqual(inline.count("\n- R07 change"), min(count, 10))
                    if count > 10:
                        self.assertIn(f"<summary>... and {count - 10} more commits</summary>", body)
                        self.assertEqual(body.count("</details>"), 1)
                    else:
                        self.assertNotIn("<details>", body)
                    self.assertTrue(body.endswith(f"\n---\n\n{notes.read_text()}\nCHANGELOG_EOF\n"))

    def test_r07_shallow_and_git_errors_fail_before_output_mutation(self) -> None:
        """Shallow roots must not masquerade as first releases, nor Git errors as success."""
        with tempfile.TemporaryDirectory(prefix="r07 errors ") as directory:
            base = Path(directory)
            source = base / "source"
            self._r07_repo(source)
            self._r07_git(source, "commit", "--allow-empty", "-m", "R07_ROOT")
            self._r07_git(source, "tag", "v0.5.1")
            self._r07_git(source, "commit", "--allow-empty", "-m", "R07_CHANGE")
            self._r07_git(source, "clone", "--depth=1", source.as_uri(), str(base / "shallow"))
            self._r07_repo(base / "empty")
            (base / "not-git").mkdir()
            for name in ("shallow", "empty", "not-git"):
                with self.subTest(history=name):
                    output = base / f"{name} output"
                    output.write_text("existing=true\n")

                    result = self._r07_changelog(base / name, output)

                    self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
                    self.assertEqual(output.read_text(), "existing=true\n")
                    self.assertNotIn("No changes", result.stdout + result.stderr)
                    if name == "shallow":
                        self.assertIn("shallow", result.stderr.lower())
                        self.assertIn("fetch-depth: 0", result.stderr)

    def test_r02_msvc_redist_and_dynamic_ffms_contract(self) -> None:
        """Require selected VS app-local CRT staging, not a host CRT or static FFMS claim."""
        root = Path(__file__).resolve().parents[3]
        stage = (root / ".github/scripts/stage-native-libs.ps1").read_text()
        package = (root / ".github/scripts/package-release.ps1").read_text()
        with self.subTest(contract="VS redist source"):
            self.assertIn("vswhere.exe", stage)
            self.assertIn("Microsoft.VCRedistVersion.default.txt", stage)
            self.assertIn("Microsoft.VC14*.CRT", stage)
            self.assertIn("VC\\Redist\\MSVC", stage)
            self.assertIn("Get-ChildItem -LiteralPath $crtDir -File -Filter '*.dll'", stage)
            self.assertIn("Copy-Item -LiteralPath $runtime.FullName", stage)
            self.assertIn("$runtime.Length -eq 0", stage)
        for name in ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
            with self.subTest(runtime=name):
                self.assertIn(f"'{name}'", stage)
                self.assertIn(f"'{name}'", package)
        for path in ("california-condor/docs/compiling.md", "av-metrics-vship/README.md",
                     ".github/scripts/stage-native-libs.sh"):
            with self.subTest(path=path):
                text = (root / path).read_text()
                self.assertNotRegex(text, r"(?is)FFMS2.{0,100}static|links FFMS2\s+statically")
                self.assertNotIn("FFMS2 and VapourSynth are linked into the executable", text)

    def test_r02_staging_packaging_and_native_gate_are_connected(self) -> None:
        """Dropping FFMS2, build DLLs, or the pre-upload native gate breaks this contract."""
        import yaml

        scripts = Path(__file__).resolve().parents[1]
        stage = (scripts / "stage-native-libs.ps1").read_text()
        package = (scripts / "package-release.ps1").read_text()
        contents = re.search(r"\$Contents = @\((.*?)\n\)", package, re.S)[1]
        self.assertIn("'ffms2.dll'", contents)
        self.assertIn("$env:FFMS_LIB_DIR", stage)
        self.assertIn("'ffms2.dll'", stage)
        self.assertIn(".Length -eq 0", stage)
        self.assertRegex(package, r"Get-ChildItem .*?-Filter ['\"]\*\.dll['\"]")
        self.assertIn("verify-windows-bundle.py", package)
        self.assertNotIn('"*$leaf"', package)
        workflow = yaml.safe_load((scripts.parent / "workflows/windows-build.yml").read_text())
        steps = workflow["jobs"]["build"]["steps"]
        packaging = next(i for i, s in enumerate(steps) if "package-release.ps1" in s.get("run", ""))
        gate = next((i for i, s in enumerate(steps) if "verify-windows-bundle.py" in s.get("run", "") and "--native" in s["run"]), None)
        self.assertIsNotNone(gate, "release needs a native clean-extracted Windows gate")
        self.assertGreater(gate, packaging)
        self.assertNotIn("if", steps[gate])
        self.assertFalse(steps[gate].get("continue-on-error", False))
        for i, step in enumerate(steps):
            if "action-gh-release" in step.get("uses", ""):
                self.assertGreater(i, gate)

    def test_r02_archive_rejects_missing_empty_and_wrong_relative_entries(self) -> None:
        """Leaf aliases and absent runtime DLLs must not pass production validation."""
        helper = Path(__file__).resolve().parents[1] / "verify-windows-bundle.py"
        self.assertTrue(helper.is_file(), "missing production archive verifier")
        verify = runpy.run_path(str(helper))["verify_archive"]
        entries = {
            "av1an.exe": b"exe", "ffms2.dll": b"dll", "libvmaf.dll": b"dll",
            "libvship.dll": b"dll", "fmetrics.dll": b"dll",
            "libgcc_s_seh-1.dll": b"dll", "libstdc++-6.dll": b"dll",
            "libwinpthread-1.dll": b"dll", "model/vmaf_v0.6.1.json": b"{}",
            "model/transnetv2.onnx": b"model", "onnxruntime.dll": b"ort",
            "DirectML.dll": b"directml", "model/nested/example.json": b"{}",
            "vcruntime140.dll": b"crt", "vcruntime140_1.dll": b"crt",
            "msvcp140.dll": b"cpp", "msvcp140_2.dll": b"companion",
        }
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "bundle.zip"
            for change in (None, "missing", "wrong-layout", "wrong-dll-layout", "empty", "missing-ort", "missing-directml",
                           "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
                with self.subTest(change=change):
                    files = entries.copy()
                    if change == "missing":
                        del files["ffms2.dll"]
                    elif change == "wrong-layout":
                        files["wrong/vmaf_v0.6.1.json"] = files.pop("model/vmaf_v0.6.1.json")
                    elif change == "wrong-dll-layout":
                        files["wrong/ffms2.dll"] = files.pop("ffms2.dll")
                    elif change == "empty":
                        files["ffms2.dll"] = b""
                    elif change == "missing-ort":
                        del files["onnxruntime.dll"]
                    elif change == "missing-directml":
                        del files["DirectML.dll"]
                    elif change in ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
                        del files[change]
                    with zipfile.ZipFile(archive, "w") as bundle:
                        for name, content in files.items():
                            bundle.writestr(name, content)
                    if change is None:
                        verify(archive, ["onnxruntime.dll", "DirectML.dll", "model/nested/example.json"])
                    else:
                        with self.assertRaises(ValueError):
                            verify(archive, ["onnxruntime.dll"])

    def test_r02_real_import_policy_fails_closed(self) -> None:
        """Missing ORT/DirectML/CRT imports fail; Vulkan is driver-only for libvship."""
        helper = Path(__file__).resolve().parents[1] / "verify-windows-bundle.py"
        self.assertTrue(helper.is_file(), "missing production import verifier")
        module = runpy.run_path(str(helper))
        output = """File: av1an.exe
Format: COFF-x86-64
Import {
  Name: ffms2.dll
  Symbol: FFMS_Init (0)
}
DelayImport {
  Name: onnxruntime.dll
  Symbol: OrtGetApiBase (0)
}
"""
        self.assertEqual(module["import_names"](output), ["ffms2.dll", "onnxruntime.dll"])
        with self.assertRaises(ValueError):
            module["import_names"]("unrecognized tool output")
        imports = {"av1an.exe": ["ffms2.dll", "onnxruntime.dll", "KERNEL32.dll", "MSVCP140.dll", "VCRUNTIME140_1.dll"],
                   "onnxruntime.dll": ["DirectML.dll"], "DirectML.dll": ["VCRUNTIME140.dll"],
                   "ffms2.dll": ["WS2_32.dll", "ole32.dll", "USER32.dll", "bcrypt.dll",
                                 "CRYPT32.dll", "KERNEL32.dll", "ADVAPI32.dll"],
                   "libvship.dll": ["vulkan-1.dll", "KERNEL32.dll"]}
        check = module["check_imports"]
        bundled = {"av1an.exe", "ffms2.dll", "onnxruntime.dll", "DirectML.dll", "vcruntime140.dll", "libvship.dll", "msvcp140.dll", "vcruntime140_1.dll"}
        check(imports, bundled)
        for missing in ("ffms2.dll", "onnxruntime.dll", "DirectML.dll", "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
            with self.subTest(missing=missing), self.assertRaisesRegex(ValueError, "[Mm]issing"):
                check(imports, bundled - {missing})
        for dependency in ("unknown.dll", "vulkan-1.dll", "avcodec.dll"):
            with self.subTest(dependency=dependency), self.assertRaises(ValueError):
                check({"av1an.exe": [dependency]}, {"av1an.exe"})
        try:
            check({"av1an.exe": ["pdh.dll"]}, {"av1an.exe"})
        except ValueError as error:
            self.fail(f"Windows inbox PDH must be accepted: {error}")

    def test_r01_run_step_uses_selected_tmpdir_and_bash(self) -> None:
        """Catch host-specific paths without changing any host files or environment."""
        bash = shutil.which("bash")
        self.assertIsNotNone(bash, "Bash must be installed to run this regression")
        checker = Path(__file__).resolve().parents[1] / "check-docker-policy.py"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            temp = root / "temp"
            temp.mkdir()
            bin_dir = root / "bin"
            bin_dir.mkdir()
            (bin_dir / "bash").symlink_to(bash)
            # A fresh interpreter isolates tempfile's cached default and the dummy env.
            result = subprocess.run(
                [sys.executable, "-B", "-c", r'''
import os
from pathlib import Path
import runpy
import shutil
import sys

checker = runpy.run_path(sys.argv[1])
output, logs = checker["run_step"]({
    "env": {"TOKEN": checker["TOKEN"]},
    "run": """
test "${HOST_ONLY+x}" = ""
test "$TOKEN" = "OFFLINE_TOKEN_DO_NOT_PRINT"
printf 'ready=true\\n' >> "$GITHUB_OUTPUT"
printf '%s\\n' "$GITHUB_OUTPUT" "$BASH"
""",
}, {})
assert output == "ready=true\n", output
output_path, bash_path = logs.splitlines()
temp = Path(os.environ["TMPDIR"])
assert Path(output_path).parent.parent == temp, "run_step ignored selected TMPDIR"
assert bash_path == shutil.which("bash"), "run_step ignored Bash on PATH"
assert list(temp.iterdir()) == [], "run_step left temporary files behind"
assert checker["TOKEN"] not in output + logs, "dummy credential leaked"
''', str(checker)],
                env={"TMPDIR": str(temp), "PATH": str(bin_dir),
                     "HOST_ONLY": "OFFLINE_SENTINEL"},
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


if __name__ == "__main__":
    unittest.main()
