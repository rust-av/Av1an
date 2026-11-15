"""Offline workflows contracts."""

from copy import deepcopy
from pathlib import Path
import re
import runpy
from types import SimpleNamespace
import unittest


class WorkflowTests(unittest.TestCase):
    def test_docker_and_ci_share_build_only_zig_pin(self) -> None:
        """Supplement runtime proof: a floating Arch tool must not enter any stage."""
        root = Path(__file__).resolve().parents[4]
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
            steps = [step for job in self._workflow(workflow)["jobs"].values()
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

    def _check_pin_policies(self, policies: set[str]) -> None:
        """Supplement runtime checks: consumers must not detach from verified boundaries."""
        root = Path(__file__).resolve().parents[4]
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

        if "release" in policies:
            with self.subTest(policy="release-pin"):
                release_pin_policy(stage)
        if "filesystem" in policies:
            with self.subTest(policy="filesystem-only"):
                filesystem_policy(vship)
        if "destination" in policies:
            with self.subTest(policy="destination-boundary"):
                destination_policy(vship)
            with self.assertRaises(AssertionError):
                destination_policy(vship.replace(normalize_destination, ""))
            with self.assertRaises(AssertionError):
                destination_policy(vship.replace(normalize_destination, "").replace("$Sha256 = ", normalize_destination + "\n$Sha256 = ", 1))
            with self.assertRaises(AssertionError):
                destination_policy(vship.replace(normalize_destination, normalize_destination + "\n" + normalize_destination))
        if "release" in policies:
            with self.assertRaises(AssertionError):
                release_pin_policy(stage.replace(".Hash -ne (Get-Pin 'VSHIP_VULKAN_SHA256')", ".Hash -eq (Get-Pin 'VSHIP_VULKAN_SHA256')"))
        if "filesystem" in policies:
            with self.assertRaises(AssertionError):
                filesystem_policy(vship.replace(anchored_candidate, "(Join-Path $directory $name)"))
            with self.assertRaises(AssertionError):
                filesystem_policy(vship.replace(anchored_candidate, "[IO.Path]::Combine($directory, $name)"))
            with self.assertRaises(AssertionError):
                filesystem_policy(vship.replace("[IO.Path]::GetDirectoryName($env:VSSCRIPT_PATH.Trim().Trim('\"'))", "(Split-Path -Parent $env:VSSCRIPT_PATH)"))
        if "pins" not in policies:
            return
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
        if "ffms" not in policies:
            return
        steps = self._workflow("windows-build")["jobs"]["build"]["steps"]
        ffms = next(s["run"] for s in steps if s.get("name") == "FFMS2 installation")
        self.assertLess(ffms.index("Get-FileHash"), ffms.index("7z x"))
        self.assertIn("throw", ffms[ffms.index("Get-FileHash"):ffms.index("7z x")])
        if "docker" not in policies:
            return
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
        jobs = self._workflow("tests")["jobs"]
        for job in ("unit-tests", "code-coverage"):
            setup = next(s["run"] for s in jobs[job]["steps"] if s.get("name") == "Setup CPU-native test dependencies")
            self.assertIn("--no-deps --no-index", setup)
            self.assertRegex(setup, r"vapoursynth_vszip-24\.0\.0-py3-none-manylinux_2_17_x86_64\.whl#sha256=[0-9a-f]{64}")

    def test_vship_release_digest_policy_rejects_inverted_guard(self) -> None:
        self._check_pin_policies({"release"})

    def test_vship_candidate_policy_uses_filesystem_only_powershell_anchored_paths(self) -> None:
        self._check_pin_policies({"filesystem"})

    def test_vship_destination_policy_requires_one_early_normalization(self) -> None:
        self._check_pin_policies({"destination"})

    def test_native_installer_pins_match_central_values_and_all_download_calls_pass_sha256(self) -> None:
        self._check_pin_policies({"pins"})

    def test_windows_ffms2_download_is_hashed_before_unpack(self) -> None:
        self._check_pin_policies({"pins", "ffms"})

    def test_docker_and_cpu_ci_use_only_pinned_binary_wheels_and_verified_downloads(self) -> None:
        self._check_pin_policies({"pins", "ffms", "docker"})

    def _workflows(self) -> dict:
        names = {"tests", "linux-build", "windows-build", "release", "mdbook", "miri-grain"}
        directory = Path(__file__).resolve().parents[2]
        self.assertEqual({p.stem for p in directory.glob("*.yml")}, names)
        return {name: self._workflow(name) for name in sorted(names)}

    def _assert_policy(self, workflows: dict) -> None:
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

    def _assert_deploy_gate(self, workflow: dict) -> None:
        expression = runpy.run_path(str(Path(__file__).resolve().parents[3] / "scripts/check-docker-policy.py"))["expression"]
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

    def test_workflows_enforce_least_privilege_immutable_actions_and_explicit_credential_consumers(self) -> None:
        """Global Pages write, mutable uses, persisted checkout or token exports fail."""
        self._assert_policy(self._workflows())
        directory = Path(__file__).resolve().parents[2]
        for path in directory.glob("*.yml"):
            for line in path.read_text().splitlines():
                if "uses:" in line and "./.github/workflows/" not in line:
                    self.assertRegex(line, r"@[0-9a-f]{40} # (v\d[^\s]*|stable)$")

    def _check_pages_and_fork_policy(self, fork_permissions: bool) -> None:
        """A fork PR sees neither writable jobs nor explicit publication credentials."""
        workflows = self._workflows()
        self._assert_deploy_gate(workflows["mdbook"])
        if not fork_permissions:
            return
        expression = runpy.run_path(str(Path(__file__).resolve().parents[3] / "scripts/check-docker-policy.py"))["expression"]
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

    def test_pages_deploy_gate_allows_only_upstream_master_push(self) -> None:
        self._check_pages_and_fork_policy(False)

    def test_fork_pr_runnable_jobs_are_read_only_and_runnable_steps_have_no_publish_credentials(self) -> None:
        self._check_pages_and_fork_policy(True)

    def _check_policy_mutations(self, mutations: tuple[str, ...], gates: bool = False) -> None:
        """Exercise the same production-policy assertions on in-memory unsafe edits."""
        workflows = self._workflows()
        for mutation in mutations:
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
                with self.assertRaises(AssertionError): self._assert_policy(changed)
        if not gates:
            return
        for gate in ("True", "github.event_name == 'push' && github.ref == 'refs/heads/master'",
                     "github.repository == 'rust-av/Av1an' && github.ref == 'refs/heads/master'"):
            with self.subTest(gate=gate):
                changed = deepcopy(workflows["mdbook"])
                changed["jobs"]["deploy"]["if"] = gate
                with self.assertRaises(AssertionError): self._assert_deploy_gate(changed)

    def test_workflow_policy_rejects_write_permission_mutations(self) -> None:
        self._check_policy_mutations(("global-write", "native-write"))

    def test_workflow_policy_rejects_mutable_actions_or_missing_checkout_toolchain_contracts(self) -> None:
        self._check_policy_mutations(("checkout", "mutable", "toolchain"))

    def test_workflow_policy_rejects_token_exports_arguments_outputs_or_inherited_secrets(self) -> None:
        self._check_policy_mutations(("token-env", "token-argument", "token-output", "inherit"))

    def test_pages_metadata_policy_rejects_site_enablement(self) -> None:
        self._check_policy_mutations(("metadata-write",))

    def test_pages_deploy_gate_rejects_missing_repository_event_or_ref_restrictions(self) -> None:
        self._check_policy_mutations((), gates=True)

    def _workflow(self, name: str) -> dict:
        import yaml

        path = Path(__file__).resolve().parents[2] / f"{name}.yml"
        self.assertTrue(path.is_file(), f"missing production workflow: {path.name}")
        return yaml.load(path.read_text(), Loader=yaml.BaseLoader)


if __name__ == "__main__":
    unittest.main()
