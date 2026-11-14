"""Offline verify windows bundle contracts."""

from pathlib import Path
import runpy
import tempfile
import unittest
import zipfile


class VerifyWindowsBundleTests(unittest.TestCase):
    def test_archive_rejects_missing_empty_and_wrong_relative_entries(self) -> None:
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

    def test_real_import_policy_fails_closed(self) -> None:
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


if __name__ == "__main__":
    unittest.main()
