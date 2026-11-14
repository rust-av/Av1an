"""Offline stage native libs contracts."""

import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


class StageNativeLibsTests(unittest.TestCase):
    def test_vmaf_stage_rejects_bytes_before_unpack(self) -> None:
        """Removing the source checksum must expose the real staging tar boundary."""
        root = Path(__file__).resolve().parents[3]
        with tempfile.TemporaryDirectory(prefix="download stage ") as directory:
            base = Path(directory)
            scripts = base / ".github/scripts"
            scripts.mkdir(parents=True)
            for name in ("stage-native-libs.sh", "download-verified.sh"):
                source = root / ".github/scripts" / name
                if source.exists():
                    shutil.copyfile(source, scripts / name)
            good = b"verified download bytes\n"
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
Path(a[a.index("-o")+1]).write_bytes(b"verified download bytes\n" if "transnetv2" in a[-1] else bytes.fromhex(os.environ["PAYLOAD"]))
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

    def test_msvc_redist_and_dynamic_ffms_contract(self) -> None:
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

    def test_staging_packaging_and_native_gate_are_connected(self) -> None:
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
        selected = [line.split()[5:] for line in steps[gate]["run"].splitlines()
                    if line.strip().startswith("python -B -m unittest -v ")]
        self.assertEqual(selected, [
            ["test_verify_windows_bundle.VerifyWindowsBundleTests.test_archive_rejects_missing_empty_and_wrong_relative_entries",
             "test_verify_windows_bundle.VerifyWindowsBundleTests.test_real_import_policy_fails_closed",
             "test_native_installers.NativeInstallerTests.test_windows_installer_boundaries_and_local_candidate",
             "test_native_installers.NativeInstallerTests.test_windows_fetch_patch_and_ancestry_before_compilation",
             "test_native_installers.NativeInstallerTests.test_windows_requires_exact_zig_before_fetch",
             "test_native_installers.NativeInstallerTests.test_windows_installers_private_scratch_and_retry",
             "test_native_installers.NativeInstallerTests.test_windows_fmetrics_preserves_old_pid_checkout"],
            ["test_native_installers.NativeInstallerTests.test_windows_missing_archive_never_uses_newer_index",
             "test_native_installers.NativeInstallerTests.test_windows_missing_or_invalid_pin_refuses_before_use",
             "test_native_installers.NativeInstallerTests.test_windows_package_hash_precedes_unpack"],
        ])
        for i, step in enumerate(steps):
            if "action-gh-release" in step.get("uses", ""):
                self.assertGreater(i, gate)


if __name__ == "__main__":
    unittest.main()
