"""Offline check of the workflow's actual archive version-command environment."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap


def main() -> None:
    """Catch runner-path overrides reaching the archive smoke command."""
    workflow = Path(__file__).resolve().parents[1] / ".github/workflows/linux-build.yml"
    step = workflow.read_text().split("      - name: Smoke-test the archive\n", 1)[1]
    step = step.split("      - name:", 1)[0]
    body = textwrap.dedent(step.split("        run: |\n", 1)[1])
    # Execute the actual command and its environment setup, not unpacking or
    # verdict checks: the probe below observes only the process boundary.
    command = body[body.index("\n", body.index("unzip ")) + 1 :]
    command = command.split("cat version.txt", 1)[0]
    variables = (
        "FMETRICS_LIB_PATH",
        "FMETRICS_LIB_DIR",
        "VSHIP_PLUGIN_PATH",
        "VSHIP_LIB_DIR",
        "VSSCRIPT_PATH",
        "VAPOURSYNTH_EXTRA_PLUGIN_PATH",
        "VMAF_LIB_DIR",
        "VMAF_MODEL_PATH",
        "LD_LIBRARY_PATH",
        "DYLD_LIBRARY_PATH",
        "DYLD_FALLBACK_LIBRARY_PATH",
    )
    environment = dict(os.environ)
    environment.update(dict.fromkeys(variables, "/m24-build-host/target/release"))
    environment.update(
        M24_CHECK_VARIABLES=":".join(variables), M24_KEEP_ENV="preserved"
    )
    with tempfile.TemporaryDirectory(prefix="m24-archive-") as directory:
        root = Path(directory)
        probe = root / "av1an"
        probe.write_text(
            f"#!{sys.executable}\n"
            "import json, os, sys\n"
            "print(json.dumps({'args': sys.argv[1:], 'leaked': [name for name in "
            "os.environ['M24_CHECK_VARIABLES'].split(':') if name in os.environ], "
            "'sentinel': os.environ.get('M24_KEEP_ENV')}))\n"
        )
        probe.chmod(0o700)
        subprocess.run(
            ["bash", "-euo", "pipefail", "-c", command],
            cwd=root,
            env=environment,
            check=True,
            timeout=10,
        )
        observed = json.loads((root / "version.txt").read_text())
        assert observed["args"] == ["--version"], observed
        assert not observed["leaked"], (
            f"build-host overrides reached smoke: {observed['leaked']}"
        )
        assert observed["sentinel"] == "preserved", "unrelated environment must survive"
    print(
        f"M24 archive command: {len(variables)} poisoned overrides absent; "
        "--version executed"
    )


if __name__ == "__main__":
    main()
