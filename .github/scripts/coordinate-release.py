#!/usr/bin/env python3
"""Run-scoped platform handoff and the release.yml serialized publication step.

Requires the runner's Git/GH CLI and Python stdlib. Publication is deliberately
not transactional across GitHub's ref, release, and asset APIs; failures surface
and must be retried, never hidden by a successful partial-platform build.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from urllib.parse import quote
import zipfile


UPSTREAM = "rust-av/Av1an"
SCHEMA = "configuration.schema.json"
PLATFORMS = ("linux", "windows")


def run(*args: str) -> str:
    """Run a concrete CLI command; retain stderr on failure at the entrypoint."""
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def provenance(platform: str) -> dict[str, str]:
    """Bind a platform handoff to the checked-out event SHA, run, and attempt."""
    sha = os.environ["GITHUB_SHA"]
    run_id = os.environ["GITHUB_RUN_ID"]
    attempt = os.environ["GITHUB_RUN_ATTEMPT"]
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        raise ValueError("Invalid source SHA")
    if not run_id.isdecimal() or not attempt.isdecimal():
        raise ValueError("Invalid run ID/attempt")
    if run("git", "rev-parse", "--verify", "HEAD^{commit}").strip() != sha:
        raise ValueError("Checkout HEAD does not match source SHA")
    return {"platform": platform, "sha": sha, "run_id": run_id,
            "run_attempt": attempt}


def bundle_schema(directory: Path, platform: str) -> dict:
    """Require a nonempty archive and matching root schema without repacking it."""
    archive = directory / f"av1an-{platform}-x64.zip"
    if not archive.is_file() or archive.stat().st_size == 0:
        raise ValueError(f"Missing/empty {platform} archive: {archive}")
    schema = json.loads((directory / SCHEMA).read_text(encoding="utf-8-sig"))
    with zipfile.ZipFile(archive) as bundle:
        if bundle.namelist().count(SCHEMA) != 1:
            raise ValueError(f"{platform} archive needs exactly one root schema")
        archived = json.loads(bundle.read(SCHEMA).decode("utf-8-sig"))
    if not isinstance(schema, dict) or archived != schema:
        raise ValueError(f"{platform} archive schema differs from handoff schema")
    return schema


def stage(platform: str) -> None:
    """Stage only the archive/schema/provenance after the platform's smoke gate."""
    manifest = provenance(platform)
    directory = Path("dist")
    directory.mkdir()  # Refuse leftover output instead of merging stale handoffs.
    shutil.copyfile(f"target/av1an-{platform}-x64.zip",
                    directory / f"av1an-{platform}-x64.zip")
    shutil.copyfile(Path("california-condor") / SCHEMA, directory / SCHEMA)
    bundle_schema(directory, platform)
    (directory / "provenance.json").write_text(json.dumps(manifest) + "\n")


def api(endpoint: str, *args: str, optional: bool = False) -> dict | None:
    """Read/write one upstream API resource; only an explicit 404 means absent."""
    command = ("gh", "api", f"repos/{UPSTREAM}/{endpoint}", *args)
    result = subprocess.run(command, capture_output=True, text=True)
    if optional and result.returncode and "(HTTP 404)" in result.stderr:
        return None
    result.check_returncode()
    return json.loads(result.stdout) if result.stdout.strip() else None


def publish() -> None:
    """Publish both platforms only inside release.yml's non-cancelling job lock."""
    ref = os.environ.get("GITHUB_REF", "")
    if (os.environ.get("GITHUB_REPOSITORY") != UPSTREAM
            or os.environ.get("GITHUB_EVENT_NAME") != "push"
            or not (ref == "refs/heads/master" or ref.startswith("refs/tags/v"))):
        print("Build only: untrusted or manual event; publication skipped")
        return

    schemas = []
    assets = []
    for platform in PLATFORMS:
        directory = Path("handoff") / platform
        expected = provenance(platform)
        manifest = json.loads((directory / "provenance.json").read_text())
        if manifest != expected:
            raise ValueError(f"{platform} provenance SHA/run/attempt/platform mismatch")
        schemas.append(bundle_schema(directory, platform))
        assets.append(str(directory / f"av1an-{platform}-x64.zip"))
    if schemas[0] != schemas[1]:
        raise ValueError("Linux/Windows configuration schema mismatch for source SHA")
    assets.append(str(Path("handoff/linux") / SCHEMA))
    sha = os.environ["GITHUB_SHA"]
    latest = ref == "refs/heads/master"
    tag = "latest" if latest else ref.removeprefix("refs/tags/")

    with tempfile.TemporaryDirectory(prefix="av1an-release-") as temp:
        output = Path(temp) / "changelog-output"
        scripts = Path(__file__).resolve().parent
        subprocess.run(
            ["bash", str(scripts / "generate-changelog.sh"),
             str(scripts / "linux-release-notes.md")], check=True,
            # SECURITY: R07 only reads local Git history; it needs no publish token.
            env={**{key: value for key, value in os.environ.items()
                    if key not in {"GH_TOKEN", "GITHUB_TOKEN"}},
                 "GITHUB_OUTPUT": str(output)},
        )
        body = output.read_text()
        prefix, suffix = "body<<CHANGELOG_EOF\n", "CHANGELOG_EOF\n"
        if not body.startswith(prefix) or not body.endswith(suffix):
            raise ValueError("Unexpected R07 changelog body protocol")
        notes = Path(temp) / "body.md"
        notes.write_text(body[len(prefix):-len(suffix)])

        # SECURITY: Check the trusted live source inside the same lock as ALL
        # writes. An older build may arrive here after a newer publisher finished.
        if latest:
            master = api("git/ref/heads/master")
            if master["object"]["sha"] != sha:
                print(f"Stale master build {sha}; publication skipped")
                return
        elif api(f"commits/tags/{quote(tag, safe='')}")["sha"] != sha:
            raise ValueError("Version tag no longer matches source SHA; refusing publication")

        existing = api(f"releases/tags/{quote(tag, safe='')}", optional=True)
        if latest:
            current = api("git/ref/tags/latest", optional=True)
            # target_commitish does NOT move an existing Git tag. Move exactly
            # this managed ref; immutable user version tags are never written.
            if current is None:
                api("git/refs", "--method", "POST", "-f", "ref=refs/tags/latest", "-f", f"sha={sha}")
            else:
                api("git/refs/tags/latest", "--method", "PATCH", "-f", f"sha={sha}", "-F", "force=true")

        action = "edit" if existing is not None else "create"
        arguments = ["gh", "release", action, tag, "--repo", UPSTREAM,
                     "--title", tag, "--notes-file", str(notes),
                     f"--prerelease={'true' if latest else 'false'}"]
        if action == "create":
            arguments.append("--verify-tag")
        run(*arguments)
        # Remove only the previous publishers' misleading loose executables.
        for asset in (existing or {}).get("assets", []):
            if asset["name"] in ("condor", "condor.exe"):
                api(f"releases/assets/{asset['id']}", "--method", "DELETE")
        run("gh", "release", "upload", tag, "--repo", UPSTREAM, "--clobber", "--", *assets)
        print(f"Published {tag} from {sha}: both archives and one schema")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("stage").add_argument("platform", choices=PLATFORMS)
    commands.add_parser("publish")
    args = parser.parse_args()
    try:
        if args.command == "stage":
            stage(args.platform)
        else:
            publish()
    except (ValueError, OSError, KeyError, zipfile.BadZipFile,
            subprocess.CalledProcessError) as error:
        parser.exit(1, f"Release coordination failed: {error}\n{getattr(error, 'stderr', '') or ''}")
