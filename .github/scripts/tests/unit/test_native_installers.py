"""Source policies supplement, but do not execute, native installer acceptance."""

from pathlib import Path
import unittest


class NativeInstallerPolicyTests(unittest.TestCase):
    def test_vmaf_windows_installer_uses_only_exact_pinned_package_urls(self) -> None:
        root = Path(__file__).resolve().parents[4]
        source = (root / "av-metrics-vmaf/scripts/install-libvmaf-windows.ps1").read_text()
        self.assertNotRegex(source, r"(?i)Get-LatestPackage|\$matches\b")
        self.assertIn("$PackageHashes.ContainsKey($package)", source)
        self.assertIn("Save-File -Uri $candidates -OutFile $archive -Sha256 $PackageHashes[$package]", source)

    def test_windows_installer_sources_create_guid_scratch_before_entering_cleanup(self) -> None:
        root = Path(__file__).resolve().parents[4]
        for relative, variable in (("av-metrics-vship/scripts/install-libvship-windows.ps1", "stageDir"),
                                   ("av-metrics-vmaf/scripts/install-libvmaf-windows.ps1", "StageDir"),
                                   ("av-metrics-fmetrics/scripts/install-fmetrics-windows.ps1", "work")):
            with self.subTest(installer=relative):
                source = (root / relative).read_text()
                self.assertNotRegex(source, r"Join-Path \$(?:Destination|InstallDir) 'dl'|fmetrics-build-\$PID")
                self.assertRegex(source, rf"\${variable} = Join-Path \(\[IO.Path\]::GetTempPath\(\)\).*NewGuid")
                self.assertRegex(source, rf"New-Item -ItemType Directory -Path \${variable} \| Out-Null\s+try \{{")
                self.assertRegex(source, rf"finally \{{\s*(?:#[^\n]*\n\s*)?if \(Test-Path -LiteralPath \${variable}\) \{{ Remove-Item -LiteralPath \${variable} -Recurse -Force")
