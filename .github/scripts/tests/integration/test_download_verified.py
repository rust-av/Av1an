"""Offline download verified contracts."""

import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class DownloadVerifiedTests(unittest.TestCase):
    def _check_download(self, indices: tuple[int, ...]) -> None:
        """Bypassing the production digest or HTTP check must publish bad bytes."""
        helper = Path(__file__).resolve().parents[2] / "download-verified.sh"
        self.assertTrue(helper.is_file(), "missing production verified-download boundary")
        with tempfile.TemporaryDirectory(prefix="download download ") as directory:
            base = Path(directory)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            curl = bin_dir / "curl"
            curl.write_text(f"#!{sys.executable}\n" + r'''
import os, sys
from pathlib import Path
a = sys.argv[1:]
assert "-o" in a and any("f" in v for v in a if v.startswith("-")), a
url = a[-1]
with Path(os.environ["CALLS"]).open("a") as out: out.write(url + "\n")
Path(a[a.index("-o")+1]).write_bytes({"good": b"verified download bytes\n", "bad": b"corrupt", "short": b"ver", "http": b"<html>503</html>"}[url.rsplit("/",1)[-1]])
sys.exit(22 if url.endswith("/http") else 0)
''')
            curl.chmod(0o755)
            destination = base / "payload"
            calls = base / "calls"
            digest = hashlib.sha256(b"verified download bytes\n").hexdigest()
            cases = [(digest, ["good"], True), (digest, ["bad"], False),
                     (digest, ["short"], False), (digest, ["http"], False),
                     (digest, ["http", "bad", "good"], True),
                     (digest, ["bad", "short"], False), ("", ["good"], False),
                     ("not-a-hash", ["good"], False)]
            for expected, sources, accepted in [cases[index] for index in indices]:
                with self.subTest(expected=expected, sources=sources):
                    destination.write_bytes(b"existing destination")
                    calls.unlink(missing_ok=True)
                    result = subprocess.run(["bash", str(helper), expected, str(destination),
                                             *[f"https://override.invalid/{s}" for s in sources]],
                                            env={"PATH": f"{bin_dir}:/usr/bin:/bin", "CALLS": str(calls)}, capture_output=True, text=True)
                    self.assertEqual(result.returncode == 0, accepted, result.stdout + result.stderr)
                    self.assertEqual(destination.read_bytes(), b"verified download bytes\n" if accepted else b"existing destination")
                    if accepted:
                        self.assertEqual(destination.stat().st_mode & 0o777, 0o644, "Docker's runtime user must be able to read verified plugins")
                    if expected in ("", "not-a-hash"):
                        self.assertFalse(calls.exists(), "missing/invalid expected hash must fail before downloading")
                    self.assertEqual(list(base.glob("*.part.*")), [], "partial payload leaked")

    def test_verified_download_matching_override_url_publishes_readable_bytes(self) -> None:
        self._check_download((0,))

    def test_verified_download_http_or_digest_failure_preserves_destination_and_removes_parts(self) -> None:
        self._check_download((1, 2, 3, 5))

    def test_verified_download_falls_through_failed_mirrors_to_verified_bytes(self) -> None:
        self._check_download((4,))

    def test_verified_download_missing_or_malformed_digest_never_invokes_curl(self) -> None:
        self._check_download((6, 7))


if __name__ == "__main__":
    unittest.main()
