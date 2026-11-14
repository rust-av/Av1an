"""Offline check docker policy contracts."""

import os
import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


class CheckDockerPolicyTests(unittest.TestCase):
    @unittest.skipIf(os.name == "nt", "Docker RUN boundary uses Bash")
    def test_docker_download_verifies_before_tar_and_version(self) -> None:
        """Run the production RUN with verified-download hashing; bad bytes never reach tar/Zig."""
        root = Path(__file__).resolve().parents[3]
        metrics = (root / "Dockerfile").read_text().split("FROM base AS metrics\n", 1)[1].split("FROM base AS runtime", 1)[0]
        run = next(line[4:] for line in metrics.replace("\\\n", "").splitlines()
                   if line.startswith("RUN ") and "/tmp/cicd.env" in line)
        self.assertIn("download-verified.sh", run, "must execute the production verified download")
        for case in ("good", "corrupt", "truncated", "http", "missing-hash", "wrong-version"):
            with self.subTest(case=case), tempfile.TemporaryDirectory(prefix="toolchain docker ") as directory:
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

    def test_run_step_uses_selected_tmpdir_and_bash(self) -> None:
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
