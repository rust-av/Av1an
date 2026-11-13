#!/bin/bash
# .SYNOPSIS
#     Packs the staged Linux release into a zip with the layout the binary needs.
#
# .DESCRIPTION
#     The Linux mirror of package-release.ps1. The release assets form a
#     directory, not a pile of loose files, because the runtime resolves parts
#     of it by relative path:
#
#       * VMAF models must sit in `model/` beside the executable.
#         av-metrics-vmaf searches `<exe dir>/model`, so a flat layout
#         silently fails to find them and VMAF is reported unavailable with
#         no other symptom.
#       * The metric libraries must sit beside the executable. fmetrics probes
#         the executable's directory itself, but vmaf and vship do not: they
#         try environment overrides and then bare names, and the ELF loader
#         never searches the executable's own directory. A `$ORIGIN` runpath
#         is therefore written into the binary before packing, which makes the
#         extracted directory work with no environment variables set.
#
#     The executable bit is the other thing a zip does not carry by default.
#     Entries are written one at a time with the Unix mode in the external
#     attribute field, and the archive is verified afterward the same way the
#     PowerShell script verifies its own.
#
# .PARAMETER StageDir
#     Directory holding the staged release. Defaults to `target/release`.
#
# .PARAMETER OutputPath
#     Where to write the archive. Defaults to `target/av1an-linux-x64.zip`.
#
# .EXAMPLE
#     ./.github/scripts/package-release-linux.sh
#     ./.github/scripts/package-release-linux.sh target/release target/av1an-linux-x64.zip
set -euo pipefail

StageDir="${1:-target/release}"
OutputPath="${2:-target/av1an-linux-x64.zip}"
SchemaPath="${3:-california-condor/configuration.schema.json}"

step() { echo "==> $1"; }

# Everything the runtime loads by relative path. `model/` is a directory
# rather than a file, which is the whole reason this archive exists.
Contents=(
    'av1an'
    'libvmaf.so*'
    'libvship.so'
    'libfmetrics.so'
    'model'
)

step 'Checking the staged release'
missing=()
for entry in "${Contents[@]}"; do
    # A glob with no match must fail the build, not expand to itself.
    if [ "$entry" = 'libvmaf.so*' ]; then
        compgen -G "$StageDir/$entry" >/dev/null || missing+=("$entry")
    else
        [ -e "$StageDir/$entry" ] || missing+=("$entry")
    fi
done
if (( ${#missing[@]} )); then
    echo "Missing from $StageDir: ${missing[*]}" >&2
    exit 1
fi

schema=''
if [ -f "$SchemaPath" ]; then
    schema="$SchemaPath"
    echo "    including $SchemaPath"
fi

models="$(find "$StageDir/model" -name '*.json' | wc -l)"
echo "    ${#Contents[@]} entries plus $models model files"

# The loader never searches the executable's directory, so the binary carries
# its own search path. DT_RUNPATH is what patchelf writes by default; it is
# overridden by LD_LIBRARY_PATH, which is fine here: a user who sets that
# variable is deliberately pointing somewhere else.
step "Setting the runpath to \$ORIGIN"
patchelf --set-rpath '$ORIGIN' "$StageDir/av1an"

step "Writing $OutputPath"
mkdir -p "$(dirname "$OutputPath")"
rm -f "$OutputPath"

# Written entry by entry so the archive root stays flat and the binary keeps
# its executable bit. Plain `zip` records Unix permissions in the external
# attribute field, but being explicit keeps the guarantee from depending on
# which archiver is installed.
python3 - "$StageDir" "$OutputPath" "$schema" <<'PY'
import os
import stat
import sys
import zipfile

stage, output, schema = sys.argv[1], sys.argv[2], sys.argv[3]
contents = ["av1an", "libvmaf.so*", "libvship.so", "libfmetrics.so", "model"]


def add(zf, source, entry_name, mode):
    info = zipfile.ZipInfo.from_file(source, entry_name)
    info.external_attr = mode << 16
    info.create_system = 3  # UNIX
    with open(source, "rb") as handle:
        zf.writestr(info, handle.read())


with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as zf:
    for entry in contents:
        if entry.endswith("*"):
            import glob
            for source in sorted(glob.glob(os.path.join(stage, entry))):
                add(zf, source, os.path.basename(source), 0o644)
        elif os.path.isdir(os.path.join(stage, entry)):
            for root, _, files in os.walk(os.path.join(stage, entry)):
                for name in files:
                    path = os.path.join(root, name)
                    add(zf, path, os.path.join(entry, name),
                        stat.S_IMODE(os.stat(path).st_mode))
        else:
            # The binary is the only entry that needs the executable bit; the
            # libraries are data the process maps, not commands it runs.
            mode = 0o755 if entry == "av1an" else 0o644
            add(zf, os.path.join(stage, entry), entry, mode)

    if schema:
        # Sits at the archive root, where `av1an init` looks for it.
        add(zf, schema, os.path.basename(schema), 0o644)
PY

step 'Verifying the archive'
python3 - "$OutputPath" <<'PY'
import os
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as zf:
    names = zf.namelist()
    mode = (zf.getinfo("av1an").external_attr >> 16) & 0o777

# A model outside model/ would resolve to nothing at runtime, so the layout is
# asserted rather than assumed.
flat = [n for n in names if n.startswith("vmaf_") and n.endswith(".json")]
if flat:
    raise SystemExit(f"Models landed at the archive root, where they cannot be found: {flat}")

if not mode & 0o111:
    raise SystemExit(f"av1an is not executable in the archive (mode {mode:o})")

# libvmaf may be staged as libvmaf.so or libvmaf.so.3 (both are in the
# crate's candidate list). Accept either.
vmaf_ok = any(n.endswith("libvmaf.so") or n.endswith("libvmaf.so.3") for n in names)
if not vmaf_ok:
    raise SystemExit(f"The archive is missing libvmaf.so or libvmaf.so.3. It contains: {names}")

expected = ["av1an", "libvship.so", "libfmetrics.so",
            "model/vmaf_v0.6.1.json"]
for name in expected:
    leaf = os.path.basename(name)
    if not any(n.endswith(leaf) for n in names):
        raise SystemExit(f"The archive is missing {name}. It contains: {names}")

print(f"    {len(names)} entries, av1an mode {mode:o}")
PY

size="$(du -h "$OutputPath" | cut -f1)"
echo "Archive ready: $OutputPath ($size)"
