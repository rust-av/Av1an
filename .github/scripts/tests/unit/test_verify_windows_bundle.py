"""In-memory readobj parsing and Windows dependency policies; no DLL loading."""

from pathlib import Path
import runpy
import unittest


class VerifyWindowsBundleTests(unittest.TestCase):
    def _check_import_policy(self, phase: int) -> None:
        assert 1 <= phase <= 3
        helper = Path(__file__).resolve().parents[2] / "verify-windows-bundle.py"
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
        if phase == 1:
            return
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
        if phase == 2:
            return
        try:
            check({"av1an.exe": ["pdh.dll"]}, {"av1an.exe"})
        except ValueError as error:
            self.fail(f"Windows inbox PDH must be accepted: {error}")

    def test_import_names_parses_normal_and_delay_imports_and_rejects_unrecognized_output(self) -> None:
        self._check_import_policy(1)

    def test_import_policy_requires_bundled_runtime_dependencies_and_rejects_unknown_dlls(self) -> None:
        self._check_import_policy(2)

    def test_import_policy_allows_vulkan_only_for_libvship_and_accepts_windows_inbox_dlls(self) -> None:
        self._check_import_policy(3)
