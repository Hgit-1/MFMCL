#!/usr/bin/env python3
"""Hash-locked aria2 source acquisition, auditable packaging, and Tauri staging.

Uses Python's standard library and curl; no third-party Python packages.
"""
import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import tarfile
import tempfile

SCRIPTS = Path(__file__).resolve().parent
LOCK = json.loads((SCRIPTS / "source.lock.json").read_text())
VERSION = "1.37.0"
EPOCH = LOCK["source_date_epoch"]
if LOCK["version"] != VERSION:
    raise RuntimeError("Unsupported source version")


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def check_source(path):
    if digest(path) != LOCK["sha256"]:
        raise ValueError("Official source SHA-256 mismatch; refusing extraction/build")


def extract(archive, dest, prefix=None):
    """Reject traversal, links, devices, duplicates, and unexpected archive roots."""
    with tarfile.open(archive, "r:*") as tar:
        members = tar.getmembers()
        seen = set()
        for member in members:
            name = Path(member.name)
            if (name.is_absolute() or ".." in name.parts or "\\" in member.name
                    or ":" in member.name or not (member.isdir() or member.isfile())
                    or member.name in seen):
                raise ValueError(f"Unsafe archive member: {member.name}")
            if prefix and (not name.parts or name.parts[0] != prefix):
                raise ValueError("Unexpected source archive root")
            seen.add(member.name)
        # All entries were checked, including on Python versions predating filter=.
        for member in members:
            target = dest / member.name
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with tar.extractfile(member) as src, target.open("wb") as dst:
                    shutil.copyfileobj(src, dst)
                target.chmod(member.mode & 0o777)
                # Release tarball mtimes prevent unintended autotools regeneration.
                os.utime(target, (member.mtime, member.mtime))


def prepare(work):
    work = work.resolve()
    if work.exists() and any(work.iterdir()):
        raise ValueError("Use a fresh empty work directory; existing builds are never reused")
    work.mkdir(parents=True, exist_ok=True)
    archive = work / LOCK["filename"]
    cached = os.environ.get("ARIA2_SOURCE_ARCHIVE")
    if cached:
        shutil.copyfile(cached, archive)
    else:
        errors = []
        for url in (LOCK["url"], LOCK["api_url"]):
            try:
                subprocess.run([
                    "curl", "--fail", "--location", "--show-error", "--silent",
                    "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2",
                    "--connect-timeout", "15", "--max-time", "300", "--retry", "2",
                    "-H", "Accept: application/octet-stream", "-o", str(archive), url,
                ], check=True)
                check_source(archive)
                break
            except (subprocess.CalledProcessError, ValueError) as error:
                errors.append(str(error))
        else:
            raise ValueError("Official download failed: " + "; ".join(errors))
    check_source(archive)
    extract(archive, work / "source", f"aria2-{VERSION}")
    (work / "build").mkdir()
    print(f"Verified official source sha256:{digest(archive)}", flush=True)


def native_key():
    systems = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}
    arches = {"x86_64": "x86_64", "AMD64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}
    key = f"{systems[platform.system()]}-{arches[platform.machine()]}"
    if key not in {"linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64", "windows-x86_64"}:
        raise ValueError("Unsupported native build platform: " + key)
    return key


def binary_key(path):
    data = Path(path).read_bytes()
    if data[:4] == b"\x7fELF":
        if data[4:6] != b"\x02\x01":
            raise ValueError("Expected 64-bit little-endian ELF")
        arch = {62: "x86_64", 183: "aarch64"}[struct.unpack_from("<H", data, 18)[0]]
        return "linux-" + arch
    if data[:4] == b"\xcf\xfa\xed\xfe":
        arch = {0x1000007: "x86_64", 0x100000C: "aarch64"}[struct.unpack_from("<I", data, 4)[0]]
        return "macos-" + arch
    if data[:2] == b"MZ":
        offset = struct.unpack_from("<I", data, 0x3C)[0]
        if data[offset:offset + 4] != b"PE\0\0":
            raise ValueError("Invalid PE executable")
        if struct.unpack_from("<H", data, offset + 4)[0] != 0x8664:
            raise ValueError("Only x86_64 Windows is supported")
        return "windows-x86_64"
    raise ValueError("Unrecognized native executable")


def deterministic_archive(root, output):
    with output.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=EPOCH) as gz:
        with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
            for path in sorted(root.rglob("*")):
                if not path.is_file() or path.is_symlink():
                    continue
                info = tar.gettarinfo(str(path), path.relative_to(root).as_posix())
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mtime = EPOCH
                info.mode = 0o755 if path.name in {"aria2c", "aria2c.exe"} else 0o644
                with path.open("rb") as stream:
                    tar.addfile(info, stream)


def run_output(args):
    return subprocess.check_output(args, stderr=subprocess.STDOUT, text=True)


def package(work, binary):
    work, binary = work.resolve(), binary.resolve()
    key = binary_key(binary)
    if key != native_key():
        raise ValueError("Cross or mislabeled binary; use the native runner for this target")
    version = run_output([str(binary), "--version"])
    if version.splitlines()[0].strip() != f"aria2 version {VERSION}" or "HTTPS" not in version:
        raise ValueError("Unexpected aria2 version or missing HTTPS")
    check_source(work / LOCK["filename"])
    os_name, arch = key.split("-", 1)
    filename = "aria2c.exe" if os_name == "windows" else "aria2c"
    payload = work / "payload"
    output = payload / "aria2" / os_name / arch
    legal = output / "distribution"
    legal.mkdir(parents=True)
    shutil.copyfile(binary, output / filename)
    (output / filename).chmod(0o755)
    shutil.copyfile(work / LOCK["filename"], legal / LOCK["filename"])
    source = work / "source" / f"aria2-{VERSION}"
    for name in ("COPYING", "LICENSE.OpenSSL", "AUTHORS", "README.rst"):
        if (source / name).is_file():
            shutil.copyfile(source / name, legal / name)
    if not (legal / "COPYING").is_file():
        raise ValueError("Upstream GPL license missing")
    shutil.copytree(SCRIPTS, legal / "build-scripts", ignore=shutil.ignore_patterns("__pycache__"))
    for name in ("configure.log", "make.log", "toolchain.txt", "version.txt"):
        shutil.copyfile(work / name, legal / name)
    for name in ("config.log", "config.status", "config.h"):
        if (work / "build" / name).is_file():
            shutil.copyfile(work / "build" / name, legal / name)
    if os_name == "linux":
        dependencies = run_output(["ldd", str(binary)])
        if "not found" in dependencies:
            raise ValueError("Unresolved runtime libraries")
    elif os_name == "macos":
        dependencies = run_output(["otool", "-L", str(binary)])
    else:
        dependencies = run_output(["objdump", "-p", str(binary)])
        # A self-contained Windows build must not need MinGW/MSYS runtime DLLs.
        for forbidden in ("libstdc++", "libgcc_s", "libwinpthread", "msys-2.0", "libssl", "libcrypto"):
            if any("DLL Name:" in line and forbidden in line for line in dependencies.splitlines()):
                raise ValueError("Unexpected runtime DLL: " + forbidden)
    (legal / "dependencies.txt").write_text(dependencies, encoding="utf-8")
    binary_hash = digest(output / filename)
    metadata = {
        "version": VERSION, "platform": key, "binary_sha256": binary_hash,
        "source": LOCK, "source_modified": False,
        "distribution": "GPL-2.0-or-later; see COPYING and upstream OpenSSL exception",
        "reproducibility": "Fixed source/configuration/epoch; exact compiler, SDK and system TLS library versions must also match. Hosted runners/packages are not immutable.",
        "signing": "macOS ad-hoc only; no publisher signature or notarization" if os_name == "macos" else "Unsigned; no publisher signature",
    }
    write_json(legal / "build-info.json", metadata)
    (legal / "NOTICE.txt").write_text(
        f"aria2 {VERSION} — https://aria2.github.io/\n"
        "Copyright: upstream aria2 authors (see AUTHORS and source headers).\n"
        "License: GNU GPL version 2 or later, with upstream OpenSSL exception; see COPYING.\n"
        "This separate executable is unmodified upstream source. Corresponding source,\n"
        "license, build scripts, configuration and logs accompany it in this directory.\n"
        "System libraries and compiler/SDK are not redistributed by this sidecar package.\n"
        "Redistributors must retain these files and review their own license obligations.\n",
        encoding="utf-8",
    )
    write_json(payload / "resources" / "aria2-manifest.json", {
        "version": VERSION,
        "platforms": {key: {"sha256": binary_hash}},
    })
    # Inventory includes every distributed byte except the checksum file itself.
    entries = [f"{digest(p)}  {p.relative_to(payload).as_posix()}" for p in sorted(payload.rglob("*")) if p.is_file()]
    (payload / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="utf-8")
    archive = work / f"aria2-{VERSION}-{key}.tar.gz"
    deterministic_archive(payload, archive)
    archive.with_suffix(archive.suffix + ".sha256").write_text(f"{digest(archive)}  {archive.name}\n")
    print(json.dumps({"platform": key, "sha256": binary_hash, "archive": str(archive)}), flush=True)


def validate_payload(payload):
    manifest = json.loads((payload / "resources" / "aria2-manifest.json").read_text())
    if manifest["version"] != VERSION or len(manifest["platforms"]) != 1:
        raise ValueError("Expected a single-platform pinned distribution")
    key, pin = next(iter(manifest["platforms"].items()))
    os_name, arch = key.split("-", 1)
    if key not in {"linux-x86_64", "linux-aarch64", "macos-x86_64", "macos-aarch64", "windows-x86_64"}:
        raise ValueError("Unsupported manifest platform")
    folder = payload / "aria2" / os_name / arch
    binary = folder / ("aria2c.exe" if os_name == "windows" else "aria2c")
    if digest(binary) != pin["sha256"] or binary_key(binary) != key:
        raise ValueError("Binary pin/architecture mismatch")
    check_source(folder / "distribution" / LOCK["filename"])
    if not (folder / "distribution" / "COPYING").is_file():
        raise ValueError("License missing")
    seen = set()
    for line in (payload / "SHA256SUMS").read_text().splitlines():
        expected, relative = line.split("  ", 1)
        path = Path(relative)
        if path.is_absolute() or ".." in path.parts or "\\" in relative or ":" in relative or relative in seen:
            raise ValueError("Unsafe checksum path")
        if digest(payload / path) != expected:
            raise ValueError("Distribution inventory mismatch: " + relative)
        seen.add(relative)
    actual = {p.relative_to(payload).as_posix() for p in payload.rglob("*") if p.is_file() and p != payload / "SHA256SUMS"}
    if seen != actual:
        raise ValueError("Distribution inventory does not cover every file")
    return key, pin, folder


def stage(archive, tauri, expected):
    # This expected hash must come from the reviewed CI/release record, not the same download.
    if len(expected) != 64 or digest(archive) != expected.lower():
        raise ValueError("Distribution archive does not match reviewed SHA-256")
    with tempfile.TemporaryDirectory(prefix="aria2-stage-") as temp:
        payload = Path(temp)
        extract(archive, payload)
        key, pin, folder = validate_payload(payload)
        manifest_path = tauri / "resources" / "aria2-manifest.json"
        manifest = json.loads(manifest_path.read_text())
        if manifest["version"] != VERSION:
            raise ValueError("Existing Tauri manifest version differs")
        # Update only one platform; retain all other reviewed pins.
        destination = tauri / "resources" / "aria2" / folder.parent.name / folder.name
        if destination.exists():
            shutil.rmtree(destination)
        shutil.copytree(folder, destination)
        binary = destination / ("aria2c.exe" if key.startswith("windows-") else "aria2c")
        binary.chmod(0o755)
        manifest["platforms"][key] = pin
        manifest["note"] = "Generated from reviewed official-source builds. See each platform distribution directory for source, licenses and audit records."
        write_json(manifest_path, manifest)
        print(f"Staged {key}: sha256:{pin['sha256']}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    prep = sub.add_parser("prepare")
    prep.add_argument("--work", type=Path, required=True)
    pack = sub.add_parser("package")
    pack.add_argument("--work", type=Path, required=True)
    pack.add_argument("--binary", type=Path, required=True)
    staging = sub.add_parser("stage")
    staging.add_argument("--archive", type=Path, required=True)
    staging.add_argument("--archive-sha256", required=True)
    staging.add_argument("--tauri-dir", type=Path, required=True)
    args = parser.parse_args()
    if args.command == "prepare":
        prepare(args.work)
    elif args.command == "package":
        package(args.work, args.binary)
    else:
        stage(args.archive, args.tauri_dir, args.archive_sha256)


if __name__ == "__main__":
    main()
