#!/bin/bash
# .SYNOPSIS
#     Builds fmetrics from source and installs the shared library for Linux.
#
# .DESCRIPTION
#     Installs fmetrics so av-metrics-fmetrics can load it at runtime with
#     `libloading`. fmetrics publishes no prebuilt Linux binaries and nothing
#     installable from a package mirror, so this fetches a pinned commit and
#     compiles it with Zig.
#
#     That makes this the one install script here that needs a toolchain: Zig
#     0.16.0, Git and a C compiler. Zig 0.17 is refused because it removed
#     `b.build_root`, which the pinned `build.zig` still uses.
#
#     Built from BoatsMcGee/fmetrics rather than halidecx/fmetrics, which
#     carries the shared-library target upstream lacks. The fcvvdp patch the
#     Windows script applies is not needed here: it works around POSIX
#     `sysconf()`, which does not compile on Windows and does on Linux.
#
#     `zig build -Dshared=true` is deliberately not used. It links the static
#     archive the ordinary way, so the linker pulls in only those members that
#     resolve an undefined symbol, and nothing references them, which leaves a
#     library exporting nothing at all. Windows is unaffected because
#     `win32_module_definition` (src/fmetrics.def) forces the members in; ELF
#     gets no export list, so the failure is Linux-only. Zig 0.16 offers no
#     whole-archive option, so the library is linked here instead.
#
#     This script is standalone: it needs no repository checkout and nothing
#     beside itself. The release pipeline overrides the pins below with
#     environment variables instead of editing the defaults.
#
# .PARAMETER Destination
#     Where to place `libfmetrics.so`. Defaults to `$PWD/fmetrics`.
#
# .PARAMETER Commit
#     The full commit ID to build. Defaults to the standalone pin below.
#
# .PARAMETER Quiet
#     Suppress the environment-variable guidance printed at the end.
#
# .EXAMPLE
#     ./install-fmetrics-linux.sh
#
# .EXAMPLE
#     ./install-fmetrics-linux.sh -Destination /usr/lib -Quiet
set -euo pipefail

# Defaults, overridable from the environment so the release pipeline can pass
# its pins without editing this script.
Destination="$PWD/fmetrics"
Commit="${FMETRICS_COMMIT:-ae87c8e5607063f0dcc8241b52cb143f6c9ad4ac}"
Repository="${FMETRICS_REPO:-https://github.com/BoatsMcGee/fmetrics.git}"
# Descriptive only; fetching the immutable pin does not require this branch.
PatchRef="${FMETRICS_BRANCH:-add-windows-shared-build}"
RequiredZigVersion="${FMETRICS_ZIG_VERSION:-0.16.0}"
Quiet=0

while [ $# -gt 0 ]; do
    case "$1" in
        -Destination) Destination="$2"; shift 2 ;;
        -Commit)      Commit="$2";      shift 2 ;;
        -Quiet)       Quiet=1;          shift ;;
        -h|--help)    awk 'NR > 1 { if (/^#/) print; else exit }' "$0"; exit 0 ;;
        *)            echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ ! "$Commit" =~ ^[0-9a-fA-F]{40}$ ]]; then
    echo "Expected a full 40-hex commit ID, not '$Commit'." >&2
    exit 1
fi
Commit="${Commit,,}"

library=libfmetrics.so

step() { echo "==> $1"; }

require() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "$2" >&2
        exit 1
    }
}

if [[ ! "$RequiredZigVersion" =~ ^0\.16\.(0|[1-9][0-9]*)$ ]]; then
    echo "Expected FMETRICS_ZIG_VERSION '$RequiredZigVersion' must be an exact 0.16 patch: 0.17 removed b.build_root." >&2
    exit 1
fi
require zig "zig not found. Install exactly Zig $RequiredZigVersion from https://ziglang.org/download/."
if zig_version="$(zig version)" && [ "$zig_version" = "$RequiredZigVersion" ]; then
    echo "    zig $zig_version"
else
    echo "zig '${zig_version:-<no version>}' found, but expected exactly $RequiredZigVersion (successful zig version); refusing fetch/build." >&2
    exit 1
fi
require git "git not found; it is needed to fetch a pinned commit."
require gcc "gcc not found; it links the shared library."

work="$(mktemp -d "${TMPDIR:-/tmp}/fmetrics-build-XXXXXX")"
trap 'rm -rf "$work"' EXIT

step "Fetching fmetrics commit $Commit (descriptive branch: $PatchRef)"
git init --quiet "$work" || { echo 'git init failed for fmetrics checkout.' >&2; exit 1; }
# No depth limit: Windows also needs the pin's ancestry, not today's branch head.
if ! git -C "$work" fetch --quiet --no-tags -- "$Repository" "$Commit"; then
    echo "Failed to fetch fmetrics commit $Commit from $Repository." >&2
    exit 1
fi
if ! git -C "$work" checkout --quiet --detach "$Commit" --; then
    echo "Failed to check out fmetrics commit $Commit." >&2
    exit 1
fi

if ! actual="$(git -C "$work" rev-parse --verify 'HEAD^{commit}')"; then
    echo "Failed to verify fmetrics HEAD for commit $Commit." >&2
    exit 1
fi
if [ "$actual" != "$Commit" ]; then
    echo "fmetrics HEAD is $actual, not the expected commit $Commit." >&2
    exit 1
fi

step 'Building libfmetrics.a'
(cd "$work" && zig build --release=fast)

mkdir -p "$Destination"
target="$Destination/$library"

step "Linking $target"
# -lm and -lpthread are what the archive's cvvdp and workspace code need.
# Without -lm the library still loads, but fails on an undefined logf.
gcc -shared -fPIC -o "$target" \
    -Wl,--whole-archive "$work/zig-out/lib/libfmetrics.a" \
    -Wl,--no-whole-archive \
    -lm -lpthread

# The failure this link works around is a silently empty library, so the exports
# are asserted rather than assumed.
exports="$(nm -D --defined-only "$target" | grep -c ' T fmetrics_' || true)"
if [ "$exports" -lt 10 ]; then
    echo "$target exports only $exports fmetrics_* symbols." >&2
    echo "The whole-archive link did not pull the API in." >&2
    exit 1
fi

size="$(stat -c %s "$target")"

echo
echo 'Done.'
echo "  $library : $target ($size bytes, $exports exports)"
echo "  commit   : $Commit"

if [ "$Quiet" -eq 0 ]; then
    cat <<EOF

Set this environment variable to use the install:

  export FMETRICS_LIB_DIR='$Destination'

Not needed when the library sits beside the av1an binary, which is where a
release stages it; the crate searches there on its own.

Verify with:

  cargo test -p av-metrics-fmetrics

Those tests skip themselves when the library is absent, so a pass alone does not
prove it loaded.
EOF
fi
