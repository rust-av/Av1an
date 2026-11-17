# Downloaded native artifacts

Release staging verifies SHA-256 **before** archive extraction, source builds,
or publishing a DLL/model into its destination. Docker verifies ZooMVTools and
its Zig toolchain before installing/unpacking them. Rejected bytes leave any
existing destination unchanged at the download boundary. HTTP errors are
failures, not archives. MSYS2 mirrors may only
supply the same filename and expected digest; a checksum failure may try the next
mirror but cannot select a newer package.

## Trust and versions

These pins were audited on 2026-11-13 against official upstream HTTPS endpoints.
FFMS2, Vship, MSYS2 archives and VMAF archive/model hashes are **maintainer-audited
known-byte pins (trust on first use)**, not invented author signatures. The audit
downloaded bytes independently of installation and never executed native assets.
This protects later installs from changed bytes; it does not prove the original
release, build toolchain, or administrator installation was uncompromised.

- [FFMS2 release 5.0](https://api.github.com/repos/FFMS/ffms2/releases/tags/5.0):
  asset `170589300`, size matches metadata; upstream's digest is null. The
  official GitHub download redirects to `release-assets.githubusercontent.com`.
- [Vship v5.1.2](https://codeberg.org/api/v1/repos/Line-fr/Vship/releases/tags/v5.1.2):
  Vulkan asset `2005842`, NVIDIA `2005839`. No AMD archive is published for this
  release. The Linux source build separately checks commit
  `5a627933274196219f782b3b74e13fad47cd6370` before building.
- [VMAF v3.2.1](https://api.github.com/repos/Netflix/vmaf/git/ref/tags/v3.2.1):
  source commit `f85a853692a8c730d0270cd733c8bb30b5b93b7c`. The official source
  archive redirects to `codeload.github.com`. **Release 3.2.1 reports runtime
  3.2.0**; do not change the loader's 3.2.0 admission to the release number.
- MSYS2 archive bytes were audited at `https://repo.msys2.org/mingw/mingw64/`.
  Current package metadata may describe another version; never reuse its hash
  for an older pinned filename. No MSYS2 signing-key verification is claimed here.
- [ZooMVTools project 78027771](https://gitlab.com/api/v4/projects/78027771)
  is `shssoichiro/vapoursynth-zoomvtools`. GitLab's `x-checksum-sha256` for its
  v2.0.2 generic Linux x86_64 package matches the independently downloaded bytes.

Native archive/DLL pins live in `.github/.env` or their single Docker consumer.
Standalone Windows installers embed their required pins: downloading one script
does not require a checkout or a hidden adjacent manifest. The regression checks
keep those defaults in sync with `.env`.

| Exact file | Bytes | SHA-256 |
| --- | ---: | --- |
| `ffms2-5.0-msvc.7z` | 8588155 | `e867a3df7262865107df40f230f5b8e1455905eba9b8852e6f35b1227537caeb` |
| `libvship_VULKAN.dll` | 3325875 | `3bcfe626c9c41cb16d18be1ed4a89c58977c54761c988bfe04de0dfca0a37558` |
| `libvship_NVIDIA.zip` | 6338737 | `b9715b881b52764d97444f660d63398ecc754c4e335cb572b671fbc9e2807e9e` |
| `v3.2.1.tar.gz` (Netflix/vmaf tag archive) | 18590364 | `5df7386911bc15fd1ca783132528748d219768ae4fc5f8e0b61184f041648092` |
| `mingw-w64-x86_64-vmaf-3.2.1-1-any.pkg.tar.zst` | 422789 | `ae55f009377b3bd447a466109de70650e65296cff329d6bc699e6b74612e3f28` |
| `mingw-w64-x86_64-libgcc-16.2.0-4-any.pkg.tar.zst` | 77173 | `d615f6a8536ca16b1f049daea1fa440a3b7a405449ec0e93ad6106db43682d54` |
| `mingw-w64-x86_64-libstdc%2B%2B-16.2.0-4-any.pkg.tar.zst` | 797348 | `3d4c3faf4c2c5c7a851ff12214ddcbf8c0d6df0964fc1d3ebd8c38f227183034` |
| `mingw-w64-x86_64-libwinpthread-git-12.0.0.r747.g1a99f8514-1-any.pkg.tar.zst` | 30860 | `38e0a33fc8b3b6b761d4958fa69afef6e0804e4ef7ea64018004da02f9b2294f` |
| `vapoursynth-zoomvtools-v2.0.2-linux-x86_64.so` | 6591648 | `c3446598c810eaa55334b6238de134d9f3e3041e5cf3e44775249f01d00d4d56` |
| `zig-x86_64-linux-0.16.0.tar.xz` | 55478392 | `70e49664a74374b48b51e6f3fdfbf437f6395d42509050588bd49abe52ba3d00` |

Canonical binary URLs are the exact release filenames under
`https://github.com/FFMS/ffms2/releases/download/5.0/`,
`https://codeberg.org/Line-fr/Vship/releases/download/v5.1.2/`, the MSYS2 base
above, and
`https://gitlab.com/api/v4/projects/78027771/packages/generic/vapoursynth-zoomvtools/v2.0.2/`.
The source archive URL is
`https://github.com/Netflix/vmaf/archive/refs/tags/v3.2.1.tar.gz`.

All nine fetched models use
`https://raw.githubusercontent.com/Netflix/vmaf/v3.2.1/model/<name>.json`:

| Name | Bytes | SHA-256 |
| --- | ---: | --- |
| `vmaf_v0.6.1` | 19101 | `5950d61fa1f861bd45d8149d80539ed9f3376cfc2495b8f0fa8e9f57cb131ee3` |
| `vmaf_v0.6.1neg` | 19519 | `1fcda193aaa2a3136471b7516b3c5dc344546c05fcde2d58609f8c60a2387392` |
| `vmaf_4k_v0.6.1` | 22910 | `73b187001309703c89d57cf58baab01660bd11e4ea6fac62bc064c5f5da6dac8` |
| `vmaf_4k_v0.6.1neg` | 23328 | `f92048bf94401a5b915f551eb248709275c61df5f20d062a3ae010dcf4d4e8fc` |
| `vmaf_b_v0.6.3` | 408883 | `34f620dbaff662fe7d33dd80326553c2cc3910e4135349ba4c367171d3a3cfbd` |
| `vmaf_float_v0.6.1` | 19045 | `26573aa333346f1109226e7283969e8b3581119a1ab7f18897767351dd963ff8` |
| `vmaf_float_v0.6.1neg` | 19463 | `8ac8e9bd6153e57fed4e9809c49711ea7654da2a14ea701423187905e914597a` |
| `vmaf_float_4k_v0.6.1` | 22854 | `2af34f3068174720a35126807dbe8d0b19c5143fee328a9fc2331c15c54e4433` |
| `vmaf_float_b_v0.6.3` | 407707 | `e32df73119386b124836910956fe8e0b0583a35638d2c518719e5a5069d0248b` |

Locally supplied/distro VMAF models and libraries remain an administrator/package
manager trust boundary, not newly verified downloads. TransNetV2's existing
`c4d54a682bace32f25136ef83ca2c9d403e8f8193775efeb995172a0d95a8e0c`
pin remains enforced in staging and its standalone installers.

## Installed Vship and overrides

The Windows Vship installer first looks for an existing candidate in configured
Vship directories, the VapourSynth runtime/plugins and extra-plugin directories,
system/PATH directories, and the destination. It leaves candidate bytes and the
destination untouched, makes no download attempt, and reports **no package was
installed**. Configure `VSHIP_LIB_DIR` if needed. Finding a file is not proof of
ABI/GPU compatibility: Av1an's existing loader must still admit version (5,1,2),
backend and required symbols, and retains its normal errors/fallbacks.
Windows release staging also preserves an existing stage DLL, or reuses this
installer's candidate result. A local candidate copied into a new release stage
is explicitly labeled local/unverified-by-download and must pass the existing
clean-extracted native bundle gate; its source bytes are not overwritten.

With no local candidate, the installer uses the exact v5.1.2 Vulkan/NVIDIA pins.
For AMD or another unpinned version, it fails before directory creation/download
with version, variant and local-install guidance. An explicit
`-ExpectedSha256 <independently-audited-digest>` can authorize different bytes for
the requested version/variant URL; this is caller trust, not an official release
attestation. It must not be derived from the same incoming installer request.
The VMAF installer refuses versions without an exact package/model pin set;
changing `VMAF_VERSION` cannot silently disable checks or pick latest packages.

## Zig build toolchain

Docker's `metrics` stage downloads exactly Zig 0.16.0 from
`https://ziglang.org/download/0.16.0/zig-x86_64-linux-0.16.0.tar.xz`.
Authority: the official [Zig download index](https://ziglang.org/download/index.json),
0.16.0 / x86_64-linux entry, rechecked on 2026-11-13 (release date 2026-04-13).
The table's digest and size are upstream-published HTTPS metadata, not a claim of
signature verification. The existing verified-download helper checks SHA-256
before tar or executable use; the installed executable must report exactly the
expected version. Zig lives in `/opt/zig`, linked from `/usr/local/bin/zig`, only
in that build stage; the runtime image copies only the two metric libraries.

`.github/.env` holds `FMETRICS_ZIG_VERSION` and the Linux x86_64 archive digest.
CI setup and both standalone fmetrics installer defaults use the same exact
0.16.0 pin. Newer 0.17 releases removed `b.build_root` used by the source pin.
Both installers reject mismatched, malformed, empty or unsuccessful version
output before fetching source/dependencies, building, or changing the destination.
`-ZigPath` (Windows) and `PATH` select an executable, not its expected version.
An administrator can deliberately set `FMETRICS_ZIG_VERSION` to another exact
0.16 patch; this is caller authorization, not automatic compatibility detection.
The 0.16 compatibility ceiling remains enforced even with that override.

When updating, independently verify the exact release/platform URL, size and
SHA-256 in the official index, audit bytes without executing them, and update
the central pins, both standalone defaults, CI setup versions and this table
together. Require real Docker and native Windows acceptance separately. This
pin does not lock Arch packages, base images or Zig's fetched dependency graph,
and does not make native builds reproducible.

## Python wheels in Docker and CPU CI

Both CPU jobs install exactly the official PyPI
`vapoursynth_vszip-24.0.0-py3-none-manylinux_2_17_x86_64.whl`, SHA-256
`abf1be51f3b4733500eca04f7179236f8383e09b298ac4ca2c8e8959a93a92da`,
6,082,465 bytes. Authority:
`https://pypi.org/pypi/vapoursynth-vszip/24.0.0/json`. PyPI reports Python >=3.12
and VapourSynth >=75; the wheel's Python tag covers the CI Python 3.14, but actual
Arch native compatibility must still pass CI. `--no-index --no-deps` preserves
Arch's VapourSynth and prevents a newly preferred wheel changing the payload.

Both CPU jobs also install the SHA-256-pinned `vapoursynth-lsmas` 1282.0.0.post2
wheel from `.github/docker-requirements.txt`. Its bundled FFmpeg libraries avoid
the CI image's unmanaged lsmas plugin breaking when Arch upgrades FFmpeg's ABI.
Update the lsmas URL/hash in both `.github/workflows/tests.yml` setup blocks
together with the Docker requirements entry.

`.github/docker-requirements.txt` retains `vsjetpack[full]` 2.2.4 and `vsfgs`
0.7.0 and binds their Linux native plugin dependencies to exact wheel URLs and
SHA-256 fragments. Pip checks each fragment before installing. These are existing
extras, not new functionality. Previously unbounded native dependency versions
are constrained to the published minimum requirements where available; lsmas
1282.0.0.post2 and znedi3 2.2 are the earliest published PyPI versions satisfying
the 1282.0.0 / 2.1 lower bounds. No requested direct release was upgraded.

PyPI wheel authority is the version-specific JSON metadata at
`https://pypi.org/pypi/<package>/<version>/json`, with exact package/version,
filename, URL and hash recorded in each requirements entry. The two exceptions
are JET's Linux `vapoursynth-ffms2` 5.2.0 and `vapoursynth-fmtconv` 31 wheels:
their official project-linked indexes at
`https://jaded-encoding-thaumaturgy.github.io/vs-wheels/simple/<package>/`
publish the exact GitHub release URLs and SHA-256 fragments. They are explicit
per-package origins, not a competing global package index.

Remaining transitive Python/runtime packages resolve only from PyPI, using
`pip --isolated --index-url https://pypi.org/simple --only-binary=:all:`. This is
not a complete hash-locked Python dependency closure or a reproducible Arch
image. Native source builds via pip are refused rather than permitting build
backends to fetch unpinned native sources. OS repositories, the Rust/Zig build
dependency graphs and base images have separate trust policies; these pins do
not authenticate all transitive internet content. The image remains x86_64, as
already required by its ZooMVTools binary. Docker/Windows acceptance is CI work,
not proved by a Linux source check.

## Updating pins

1. Confirm the exact release, immutable source commit where available, filename,
   platform and authority at the official project. Prefer published signed
   checksums or official PyPI/MSYS2 metadata. Do not infer a binary's source commit
   from a mutable release branch name.
2. Independently audit bytes over canonical upstream HTTPS, check size/metadata,
   and compute SHA-256 without executing assets. Record the URL, release commit
   where evidenced, filename, byte count, digest, authority and audit date. If no
   signed checksum exists, label this known-byte/HTTPS trust, not an author
   signature. Archive re-compression or a replacement asset requires review even
   when its tag/version did not change.
3. Update `.env`, standalone defaults, the relevant Docker requirement/consumer,
   and this table together. A mirror must use the same exact expected digest;
   never change a filename to whatever is latest or learn expected hashes during
   installation. Keep VMAF release and runtime version fields distinct.
4. Run the offline release-script and Docker policy checks, workflow lint,
   relevant Bash syntax checks, compile-only MSRV check, and full default
   `just precommit`. Require native Windows and Docker CI acceptance separately.
