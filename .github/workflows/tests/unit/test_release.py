"""Read-only release handoff, publisher and gate policies; no publication occurs."""

from pathlib import Path
import runpy
from types import SimpleNamespace
import unittest


class ReleaseWorkflowTests(unittest.TestCase):
    def _workflow(self, name: str) -> dict:
        import yaml

        path = Path(__file__).resolve().parents[2] / f"{name}.yml"
        self.assertTrue(path.is_file(), f"missing production workflow: {path.name}")
        return yaml.load(path.read_text(), Loader=yaml.BaseLoader)

    def _check_handoff(self, phase: int) -> None:
        assert 1 <= phase <= 3
        for platform in ("linux", "windows"):
            workflow = self._workflow(f"{platform}-build")
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
        if phase == 1:
            return
        workflow = self._workflow("release")
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
        if phase == 2:
            return
        self.assertEqual(publishers[0]["env"], {"GH_TOKEN": "${{ github.token }}"})
        self.assertTrue(all("GH_TOKEN" not in str(s) for s in steps[:-1]))

    def test_platform_workflows_stage_run_bound_artifacts_after_required_smoke_gate(self) -> None:
        self._check_handoff(1)

    def test_release_workflow_waits_for_both_platforms_and_serializes_the_only_publisher(self) -> None:
        self._check_handoff(2)

    def test_release_workflow_exposes_publish_token_only_to_final_publish_step(self) -> None:
        self._check_handoff(3)

    def test_release_gate_allows_only_successful_upstream_master_or_version_tag_pushes(self) -> None:
        script = Path(__file__).resolve().parents[3] / "scripts/check-docker-policy.py"
        expression = runpy.run_path(str(script))["expression"]
        gate = " ".join(self._workflow("release")["jobs"]["release"]["if"].split())
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
