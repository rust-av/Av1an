"""Execute source archive verification through the actual staging script."""

import hashlib
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest


class StageNativeLibsTests(unittest.TestCase):
    def test_vmaf_staging_only_matching_source_digest_reaches_tar(self) -> None:
        """Removing the source checksum must expose the real staging tar boundary."""
        root = Path(__file__).resolve().parents[4]
        with tempfile.TemporaryDirectory(prefix="download stage ") as directory:
            base = Path(directory)
            scripts = base / ".github/scripts"
            scripts.mkdir(parents=True)
            for name in ("stage-native-libs.sh", "download-verified.sh"):
                source = root / ".github/scripts" / name
                if source.exists():
                    shutil.copyfile(source, scripts / name)
            good = b"verified download bytes\n"
            digest = hashlib.sha256(good).hexdigest()
            pins = (root / ".github/.env").read_text()
            pins = re.sub(r"^TRANSNETV2_MODEL_SHA256=.*$", f"TRANSNETV2_MODEL_SHA256={digest}", pins, flags=re.M)
            pins = re.sub(r"^VMAF_SOURCE_SHA256=.*\n?", "", pins, flags=re.M)
            (base / "model").mkdir()
            (base / "model/vmaf_v0.6.1.json").write_text("{}")
            bin_dir = base / "bin"
            bin_dir.mkdir()
            curl = bin_dir / "curl"
            curl.write_text(f"#!{sys.executable}\n" + r'''
import os, sys
from pathlib import Path
a = sys.argv[1:]
assert "-o" in a and any("f" in v for v in a if v.startswith("-")), a
Path(a[a.index("-o")+1]).write_bytes(b"verified download bytes\n" if "transnetv2" in a[-1] else bytes.fromhex(os.environ["PAYLOAD"]))
''')
            curl.chmod(0o755)
            for name, body in {"ldconfig": "exit 0", "tar": 'touch "$MARKER"; exit 91'}.items():
                path = bin_dir / name
                path.write_text("#!/bin/bash\n" + body + "\n")
                path.chmod(0o755)
            env = {"PATH": f"{bin_dir}:/usr/bin:/bin", "TMPDIR": str(base),
                   "MARKER": str(base / "unpacked"), "GITHUB_WORKSPACE": str(base)}
            for case, payload in (("good", good), ("corrupt", b"changed"), ("truncated", good[:3]), ("nohash", good)):
                with self.subTest(case=case):
                    marker = base / "unpacked"
                    marker.unlink(missing_ok=True)
                    (scripts.parent / ".env").write_text(pins + (f"\nVMAF_SOURCE_SHA256={digest}\n" if case != "nohash" else ""))
                    result = subprocess.run(["bash", str(scripts / "stage-native-libs.sh"), str(base / "stage")],
                                            cwd=base, env={**env, "PAYLOAD": payload.hex()}, capture_output=True, text=True)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(marker.exists(), case == "good", result.stdout + result.stderr)
