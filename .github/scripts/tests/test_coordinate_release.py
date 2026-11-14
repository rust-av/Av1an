"""Offline coordinate release contracts."""

import os
import json
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zipfile

from support import git, init_repo


class CoordinateReleaseTests(unittest.TestCase):
    def test_changelog_subprocess_does_not_inherit_publish_tokens(self) -> None:
        """Run the real publisher with the history child rejecting both token variables."""
        with tempfile.TemporaryDirectory(prefix="credentials child ") as directory:
            repo, env = self._release_fixture(Path(directory))
            script = repo / ".github/scripts/generate-changelog.sh"
            script.write_text('test "${GH_TOKEN+x}${GITHUB_TOKEN+x}" = "" || exit 91\n' + script.read_text())
            env["GITHUB_TOKEN"] = "OFFLINE_CREDENTIAL_BOUNDARY_INERT"
            result = self._publish_release(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def _workflow(self, name: str) -> dict:
        import yaml

        path = Path(__file__).resolve().parents[2] / f"workflows/{name}.yml"
        self.assertTrue(path.is_file(), f"missing production workflow: {path.name}")
        return yaml.load(path.read_text(), Loader=yaml.BaseLoader)

    def test_workflow_handoff_and_single_trusted_publisher(self) -> None:
        """An independent publisher, ungated upload, or cross-run download must fail."""
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
        self.assertEqual(publishers[0]["env"], {"GH_TOKEN": "${{ github.token }}"})
        self.assertTrue(all("GH_TOKEN" not in str(s) for s in steps[:-1]))

    def test_actual_workflow_gate_denies_failure_manual_and_forks(self) -> None:
        """Evaluate the production gate, not a copied release decision table."""
        script = Path(__file__).resolve().parents[1] / "check-docker-policy.py"
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

    def _release_fixture(self, base: Path) -> tuple[Path, dict]:
        scripts = Path(__file__).resolve().parents[1]
        helper = scripts / "coordinate-release.py"
        self.assertTrue(helper.is_file(), "missing production handoff/publisher helper")
        repo = base / "repo"
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            init_repo(repo)
        destination = repo / ".github/scripts"
        destination.mkdir(parents=True)
        for name in ("coordinate-release.py", "generate-changelog.sh", "linux-release-notes.md"):
            shutil.copyfile(scripts / name, destination / name)
        with patch.dict(os.environ, {"PATH": "/usr/bin:/bin"}, clear=True):
            git(repo, "commit", "--allow-empty", "-m", "RELEASE_OLD")
            git(repo, "tag", "v0.5.1")
            git(repo, "commit", "--allow-empty", "-m", "RELEASE_NEW")
            git(repo, "tag", "v1.2.3")
            git(repo, "tag", "-a", "v1.2.3+雪#%", "-m", "annotated fixture")
            git(repo, "branch", "v1.2.3", "v0.5.1")
            sha = git(repo, "rev-parse", "HEAD")
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
               "GIT_CEILING_DIRECTORIES": str(base), "GH_TOKEN": "OFFLINE_RELEASE_INERT",
               "GITHUB_SHA": sha, "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "2",
               "GITHUB_REPOSITORY": "rust-av/Av1an", "GITHUB_EVENT_NAME": "push",
               "GITHUB_REF": "refs/heads/master", "FAKE_STATE": str(base / "state.json")}
        self._stage_release(repo, env)
        return repo, env

    def _stage_release(self, repo: Path, env: dict) -> None:
        for platform in ("linux", "windows"):
            (repo / "target").mkdir(exist_ok=True)
            schema = repo / "california-condor/configuration.schema.json"
            schema.parent.mkdir(exist_ok=True)
            schema.write_bytes(b'\xef\xbb\xbf{\r\n  "properties": {}, "type": "object"\r\n}\r\n'
                               if platform == "windows" else b'{"type":"object","properties":{}}\n')
            with zipfile.ZipFile(repo / f"target/av1an-{platform}-x64.zip", "w") as archive:
                archive.writestr("configuration.schema.json", schema.read_bytes())
                archive.writestr("av1an.exe" if platform == "windows" else "av1an", platform + env["GITHUB_SHA"])
            steps = self._workflow(f"{platform}-build")["jobs"]["build"]["steps"]
            stage = next(s["run"] for s in steps if "coordinate-release.py stage" in s.get("run", ""))
            # Windows uses the same stdlib CLI; PowerShell/native acceptance remains CI-only.
            result = subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", stage], cwd=repo, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            target = repo / f"handoff/{platform}"
            target.parent.mkdir(exist_ok=True)
            (repo / "dist").rename(target)

    def _publish_release(self, repo: Path, env: dict) -> subprocess.CompletedProcess[str]:
        steps = self._workflow("release")["jobs"]["release"]["steps"]
        command = next(s["run"] for s in steps if "coordinate-release.py publish" in s.get("run", ""))
        return subprocess.run(["bash", "--noprofile", "--norc", "-e", "-c", command], cwd=repo, env=env, capture_output=True, text=True)

    def test_publication_body_assets_tag_and_serialized_stale_order(self) -> None:
        """Run production commands with real owned Git history and an inert GH boundary."""
        with tempfile.TemporaryDirectory(prefix="release publish ") as directory:
            base = Path(directory)
            repo, env = self._release_fixture(base)
            state_path = base / "state.json"
            tag_endpoints = {
                "refs/tags/v1.2.3": "repos/rust-av/Av1an/commits/tags/v1.2.3",
                "refs/tags/v1.2.3+雪#%": "repos/rust-av/Av1an/commits/tags/v1.2.3%2B%E9%9B%AA%23%25",
            }
            with patch.dict(os.environ, env, clear=True):
                self.assertEqual(git(repo, "cat-file", "-t", "refs/tags/v1.2.3"), "commit")
                self.assertEqual(git(repo, "cat-file", "-t", "refs/tags/v1.2.3+雪#%"), "tag")
                self.assertEqual(git(repo, "rev-parse", "refs/tags/v1.2.3+雪#%^{commit}"), env["GITHUB_SHA"])
            for ref in ("refs/heads/master", *tag_endpoints):
                with self.subTest(ref=ref):
                    env["GITHUB_REF"] = ref
                    result = self._publish_release(repo, env)
                    self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                    state = json.loads(state_path.read_text())
                    tag = "latest" if ref.endswith("master") else ref.removeprefix("refs/tags/")
                    release = state["releases"][tag]
                    self.assertIn(f"Latest build from {env['GITHUB_SHA']}\n", release["body"])
                    self.assertIn("- RELEASE_NEW\n", release["body"])
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
                git(repo, "commit", "--allow-empty", "-m", "RELEASE_NEWER")
                env["GITHUB_SHA"] = git(repo, "rev-parse", "HEAD")
            env["GITHUB_RUN_ID"] = "124"
            env["GITHUB_REF"] = "refs/heads/master"
            self._stage_release(repo, env)
            state = json.loads(state_path.read_text())
            state["master"] = env["GITHUB_SHA"]
            state_path.write_text(json.dumps(state))
            result = self._publish_release(repo, env)
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
                git(repo, "checkout", "--detach", old_env["GITHUB_SHA"])
            result = self._publish_release(repo, old_env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertIn("stale", result.stdout.lower())
            state = json.loads(state_path.read_text())
            self.assertEqual(state["releases"], before)
            self.assertEqual(state["calls"], [["api", "repos/rust-av/Av1an/git/ref/heads/master"]])
            self.assertEqual(list((base / "temp").iterdir()), [])

    def test_rejects_invalid_handoff_and_untrusted_runs_before_write(self) -> None:
        """Removing a provenance/schema/HEAD/policy check must expose a fake write."""
        cases = ("mixed-sha", "wrong-run", "wrong-attempt", "wrong-platform", "missing-platform",
                 "schema-mismatch", "checkout-sha", "tag-moved", "tag-missing", "tag-api-failure", "api-failure", "manual-master", "manual-tag", "manual-feature", "fork", "fork-tag")
        with tempfile.TemporaryDirectory(prefix="release reject ") as directory:
            for case in cases:
                with self.subTest(case=case):
                    base = Path(directory) / case
                    base.mkdir()
                    repo, env = self._release_fixture(base)
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
                    result = self._publish_release(repo, env)
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

    def test_create_latest_and_surface_partial_remote_failure(self) -> None:
        """A missing managed tag is created; failed uploads never report success."""
        with tempfile.TemporaryDirectory(prefix="release remote ") as directory:
            base = Path(directory)
            repo, env = self._release_fixture(base)
            path = base / "state.json"
            state = json.loads(path.read_text())
            state.update(latest=None, releases={})
            path.write_text(json.dumps(state))
            result = self._publish_release(repo, env)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            state = json.loads(path.read_text())
            self.assertEqual(state["latest"], env["GITHUB_SHA"])
            self.assertEqual(len(state["releases"]["latest"]["assets"]), 3)
            state["fail_upload"] = True
            path.write_text(json.dumps(state))
            result = self._publish_release(repo, env)
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("Published", result.stdout)
            self.assertEqual(list((base / "temp").iterdir()), [])


if __name__ == "__main__":
    unittest.main()
