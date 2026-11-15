"""Offline stage native libs contracts."""

from pathlib import Path
import re
import unittest


class StageNativeLibsTests(unittest.TestCase):
    def _check_crt_policy(self, phase: int) -> None:
        assert 1 <= phase <= 3
        """Require selected VS app-local CRT staging, not a host CRT or static FFMS claim."""
        root = Path(__file__).resolve().parents[4]
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
        if phase == 1:
            return
        for name in ("vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"):
            with self.subTest(runtime=name):
                self.assertIn(f"'{name}'", stage)
                self.assertIn(f"'{name}'", package)
        if phase == 2:
            return
        for path in ("california-condor/docs/compiling.md", "av-metrics-vship/README.md",
                     ".github/scripts/stage-native-libs.sh"):
            with self.subTest(path=path):
                text = (root / path).read_text()
                self.assertNotRegex(text, r"(?is)FFMS2.{0,100}static|links FFMS2\s+statically")
                self.assertNotIn("FFMS2 and VapourSynth are linked into the executable", text)

    def test_windows_staging_sources_app_local_crt_from_selected_visual_studio_redist(self) -> None:
        self._check_crt_policy(1)

    def test_windows_stage_and_package_require_all_three_crt_dlls(self) -> None:
        self._check_crt_policy(2)

    def test_docs_and_linux_staging_do_not_claim_static_ffms2_linkage(self) -> None:
        self._check_crt_policy(3)

    def test_windows_stage_and_package_include_nonempty_ffms2_and_all_build_dlls(self) -> None:
        """Dropping FFMS2, build DLLs, or the pre-upload native gate breaks this contract."""
        scripts = Path(__file__).resolve().parents[2]
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


if __name__ == "__main__":
    unittest.main()
