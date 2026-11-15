"""Required native Windows gates and exact stdlib-only test selections."""

from pathlib import Path
import unittest


class WindowsBuildPolicyTests(unittest.TestCase):
    def _check_native_gate(self, selections: bool) -> None:
        import yaml

        scripts = Path(__file__).resolve().parents[3] / "scripts"
        workflow = yaml.safe_load((scripts.parent / "workflows/windows-build.yml").read_text())
        steps = workflow["jobs"]["build"]["steps"]
        packaging = next(i for i, s in enumerate(steps) if "package-release.ps1" in s.get("run", ""))
        gate = next((i for i, s in enumerate(steps) if "verify-windows-bundle.py" in s.get("run", "") and "--native" in s["run"]), None)
        self.assertIsNotNone(gate, "release needs a native clean-extracted Windows gate")
        self.assertGreater(gate, packaging)
        self.assertNotIn("if", steps[gate])
        self.assertFalse(steps[gate].get("continue-on-error", False))
        if selections:
            selected = [line.split()[5:] for line in steps[gate]["run"].splitlines()
                        if line.strip().startswith("python -B -m unittest -v ")]
            self.assertEqual(selected, [
                ["integration.test_verify_windows_bundle.VerifyWindowsBundleTests.test_archive_rejects_missing_empty_and_wrong_relative_entries",
                 "unit.test_verify_windows_bundle.VerifyWindowsBundleTests.test_import_names_parses_normal_and_delay_imports_and_rejects_unrecognized_output",
                 "unit.test_verify_windows_bundle.VerifyWindowsBundleTests.test_import_policy_requires_bundled_runtime_dependencies_and_rejects_unknown_dlls",
                 "unit.test_verify_windows_bundle.VerifyWindowsBundleTests.test_import_policy_allows_vulkan_only_for_libvship_and_accepts_windows_inbox_dlls",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_vship_candidate_discovery_and_verified_downloads_preserve_existing_bytes",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_fmetrics_fetches_exact_detached_commit_and_applies_portability_patch_before_build",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_fmetrics_invalid_or_unreachable_commit_never_builds",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_fmetrics_wrong_or_missing_upstream_ancestry_never_fetches_dependencies",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_fmetrics_missing_or_inapplicable_patch_never_compiles",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_requires_exact_zig_before_fetch",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_installers_owned_scratch_survives_failures_retry_and_creation_collisions",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_fmetrics_preserves_old_pid_checkout"],
                ["integration.test_native_installers.NativeInstallerTests.test_windows_missing_archive_never_uses_newer_index",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_vmaf_missing_or_invalid_package_digest_refuses_before_download_or_unpack",
                 "integration.test_native_installers.NativeInstallerTests.test_windows_package_hash_precedes_unpack"],
            ])
        for i, step in enumerate(steps):
            if "action-gh-release" in step.get("uses", ""):
                self.assertGreater(i, gate)

    def test_windows_workflow_requires_clean_extracted_native_gate_after_packaging(self) -> None:
        self._check_native_gate(False)

    def test_windows_native_gate_selects_exact_stdlib_only_release_contract_methods(self) -> None:
        self._check_native_gate(True)

    def test_windows_native_gate_selects_all_three_pinned_package_methods(self) -> None:
        root = Path(__file__).resolve().parents[4]
        workflow = (root / ".github/workflows/windows-build.yml").read_text()
        for method in ("test_windows_missing_archive_never_uses_newer_index",
                       "test_windows_vmaf_missing_or_invalid_package_digest_refuses_before_download_or_unpack",
                       "test_windows_package_hash_precedes_unpack"):
            self.assertIn(f"integration.test_native_installers.NativeInstallerTests.{method}", workflow)

    def test_windows_native_gate_selects_scratch_and_pid_ownership_methods(self) -> None:
        root = Path(__file__).resolve().parents[4]
        workflow = (root / ".github/workflows/windows-build.yml").read_text()
        for method in ("test_windows_installers_owned_scratch_survives_failures_retry_and_creation_collisions", "test_windows_fmetrics_preserves_old_pid_checkout"):
            self.assertIn(f"integration.test_native_installers.NativeInstallerTests.{method}", workflow)
