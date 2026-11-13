"""Validate release ZIP layout; --native also checks imports and decodes on Windows."""

import argparse
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tempfile
import zipfile


REQUIRED = (
    "av1an.exe", "ffms2.dll", "libvmaf.dll", "libvship.dll", "fmetrics.dll",
    "vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll", "DirectML.dll",
    "libgcc_s_seh-1.dll", "libstdc++-6.dll", "libwinpthread-1.dll",
    "model/vmaf_v0.6.1.json", "model/transnetv2.onnx",
)
# Windows inbox DLLs only: VC redistributables, ORT and DirectML must be bundled.
SYSTEM_DLLS = set("""
advapi32.dll bcrypt.dll bcryptprimitives.dll cfgmgr32.dll combase.dll comctl32.dll
comdlg32.dll crypt32.dll cryptbase.dll cryptsp.dll d3d11.dll d3d12.dll d3dcompiler_47.dll
dbghelp.dll dnsapi.dll dwmapi.dll dxcore.dll dxgi.dll gdi32.dll imm32.dll iphlpapi.dll
kernel32.dll kernelbase.dll mpr.dll msasn1.dll msvcrt.dll ncrypt.dll netapi32.dll
normaliz.dll ntdll.dll ole32.dll oleaut32.dll powrprof.dll propsys.dll psapi.dll
rpcrt4.dll secur32.dll setupapi.dll shell32.dll shlwapi.dll user32.dll userenv.dll
ucrtbase.dll usp10.dll uxtheme.dll version.dll winhttp.dll wininet.dll winmm.dll
wintrust.dll ws2_32.dll wtsapi32.dll
""".split())
# sysinfo 0.39.6 Windows CPU refresh calls PdhOpenQueryW; windows 0.62.2
# links that API to inbox pdh.dll (Win32/System/Performance), not a redistributable.
SYSTEM_DLLS.add("pdh.dll")


def verify_archive(path: Path, expected: list[str] | tuple[str, ...] = ()) -> None:
    """Require exact nonempty relative entries, before extracting into owned scratch."""
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        names = [entry.filename for entry in entries]
        if len({name.casefold() for name in names}) != len(names):
            raise ValueError("Duplicate ZIP entries")
        for entry in entries:
            name = entry.filename
            if ("\\" in name or ":" in name or name.startswith("/")
                    or ".." in PurePosixPath(name).parts
                    or (entry.external_attr >> 16) & 0o170000 == 0o120000):
                raise ValueError(f"Not a relative byte-file ZIP entry: {name}")
        for name in (*REQUIRED, *expected):
            if name not in names or archive.getinfo(name).file_size == 0:
                raise ValueError(f"Missing or empty archive entry: {name}")


def import_names(output: str) -> list[str]:
    """Read llvm-readobj --coff-imports normal AND delay-load import blocks."""
    if "Format: COFF-x86-64" not in output:
        raise ValueError("Expected llvm-readobj x64 PE output")
    blocks = re.findall(r"^(?:Import|DelayImport) \{\r?\n(.*?)^\}", output, re.M | re.S)
    names = []
    for block in blocks:
        match = re.search(r"^  Name: ([^\r\n]+\.dll)\r?$", block, re.M | re.I)
        if not match:
            raise ValueError("Unrecognized PE import block")
        names.append(match[1])
    if len(blocks) != len(re.findall(r"^(?:Import|DelayImport) \{", output, re.M)):
        raise ValueError("Incomplete PE import output")
    return names


def check_imports(imports: dict[str, list[str]], bundled: set[str]) -> None:
    """Fail closed on non-inbox imports, except libvship's optional Vulkan driver."""
    bundled = {name.casefold() for name in bundled}
    for source, dependencies in imports.items():
        for dependency in dependencies:
            name = dependency.casefold()
            if name in bundled or name in SYSTEM_DLLS:
                continue
            if name.startswith(("api-ms-win-", "ext-ms-win-")) and name.endswith(".dll"):
                continue
            if source.casefold() == "libvship.dll" and name == "vulkan-1.dll":
                print("libvship.dll: vulkan-1.dll is an optional host GPU driver prerequisite")
                continue
            raise ValueError(f"Missing required import: {source} -> {dependency}")


def native_smoke(path: Path, readobj: Path) -> None:
    """Inspect actual bundled PEs, then run only extracted Condor with a clean environment."""
    if os.name != "nt":
        raise ValueError("--native requires Windows; offline fixtures are not native proof")
    import ctypes

    # Prevent a missing-DLL negative check from opening an interactive error dialog.
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    old_error_mode = kernel32.SetErrorMode(0x0001 | 0x0002)
    try:
        with tempfile.TemporaryDirectory(prefix="av1an-windows-smoke-") as scratch:
            root = Path(scratch)
            extracted, work = root / "bundle", root / "work"
            work.mkdir()
            with zipfile.ZipFile(path) as archive:
                archive.extractall(extracted)
            binaries = [extracted / "av1an.exe", *extracted.glob("*.dll")]
            imports = {}
            for binary in binaries:
                result = subprocess.run(
                    [str(readobj), "--coff-imports", str(binary)],
                    check=True, capture_output=True, text=True, timeout=60,
                )
                imports[binary.name] = import_names(result.stdout)
                print(f"{binary.name}: {', '.join(imports[binary.name])}")
            check_imports(imports, {binary.name for binary in binaries})
            system_root = os.environ["SystemRoot"]
            # Allowlist, not subtraction: no build PATH, ORT/VSSCRIPT/model/plugin overrides.
            env = {
                "SystemRoot": system_root, "WINDIR": system_root,
                "PATH": str(Path(system_root) / "System32"),
                "TEMP": str(work), "TMP": str(work), "USERPROFILE": str(work),
                "HOME": str(work), "APPDATA": str(work), "LOCALAPPDATA": str(work),
            }
            exe = str(extracted / "av1an.exe")

            def run(*args: str, check: bool = True) -> subprocess.CompletedProcess:
                result = subprocess.run(
                    [exe, *args], cwd=work, env=env, capture_output=True,
                    text=True, timeout=120, check=False,
                )
                print(f"av1an {' '.join(args)}: exit {result.returncode}")
                print(result.stdout + result.stderr)
                if check:
                    result.check_returncode()
                return result

            ffms = extracted / "ffms2.dll"
            hidden = extracted / "ffms2.dll.disabled"
            ffms.rename(hidden)
            try:
                if run("--version", check=False).returncode == 0:
                    raise ValueError("Missing-FFMS2 negative loader check unexpectedly succeeded")
            finally:
                hidden.rename(ffms)
            run("--version")
            # Y4M is a tiny real video container; FFMS2 must index/open/decode its 24 frames.
            video = work / "tiny.y4m"
            with video.open("wb") as stream:
                stream.write(b"YUV4MPEG2 W64 H64 F24:1 Ip A1:1 C420jpeg\n")
                for frame in range(24):
                    stream.write(b"FRAME\n" + bytes([16 + frame * 8]) * 4096 + bytes([128]) * 2048)
            config = work / "condor.json"
            run("--config-file", str(config), "--temp", str(work / "temp"),
                "init", str(video), str(work / "output.ivf"), "--decoder", "ffms2", "--concat", "ivf")
            run("--config-file", str(config), "detect-scenes", "--method", "standard")
            scenes = json.loads(config.read_text())["condor"]["scenes"]
            if not scenes or scenes[-1]["end_frame"] != 24:
                raise ValueError("Native scene detector did not consume the tiny FFMS2 input")
            print("Native clean-extracted version + FFMS2 frame decode passed")
    finally:
        kernel32.SetErrorMode(old_error_mode)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--expect", nargs="*", default=[])
    parser.add_argument("--native", action="store_true")
    parser.add_argument("--readobj", type=Path)
    args = parser.parse_args()
    verify_archive(args.archive, args.expect)
    if args.native:
        if not args.readobj or not args.readobj.is_file():
            parser.error("--native requires an explicit installed llvm-readobj executable")
        native_smoke(args.archive.resolve(), args.readobj.resolve())
    print("Windows archive contracts passed")
