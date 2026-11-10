#!/usr/bin/env python3
"""Pack a plugin directory into a `.mflp` archive.

## Why the package layout is fixed

The installer (`src-tauri/src/plugin_package.rs`) requires `manifest.json` at the
archive root. Everything else is free-form as long as the manifest points at it.
That single constraint is what makes a package inspectable: unzip it in a file
manager and the first thing you see is the manifest.

## Why `-X` and a fixed timestamp

`zip` stores modification times per entry, so packing the same directory twice
produces different bytes and a different SHA-256. A source index pins a hash, and
a hash that changes on every build is a hash nobody can verify. `-X` drops the
extra attributes and the mtime is pinned via `SOURCE_DATE_EPOCH`, so the same
inputs give the same archive — and therefore the same pin.

## Usage

    python design/tools/aria/pack.py --source <plugin dir> --out <file.mflp>

The default source is the Aria asset directory, because that is the one plugin
this repository ships.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_SOURCE = HERE.parent.parent / "flowlaunch-ui" / "public" / "plugins" / "aria-with-you"

# Files that are build output rather than plugin content.
#
# `manifest.json` in the asset directory is the *generation record* (model,
# prompts, hashes), not the plugin manifest, so it must not ship: the installer
# would read it and reject the package for having no `id`. The plugin manifest is
# written separately by `write_archive`.
EXCLUDE_NAMES = {"manifest.json", "pages.json"}


def collect(source: Path) -> list[Path]:
    """Every file that belongs in the package.

    `pages/` is included: the per-page artwork is part of the plugin's behaviour,
    not a build artefact. `pages.json` (the generation record for those) is not,
    for the same reason as `manifest.json`.
    """
    files: list[Path] = []
    for path in sorted(source.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(source)
        if relative.name in EXCLUDE_NAMES:
            continue
        if any(part.startswith(".") for part in relative.parts):
            continue
        files.append(relative)
    return files


def build_manifest(source: Path) -> dict[str, object]:
    """The plugin manifest, assembled from the plugin's own identity.

    Kept in the packing script rather than in the asset directory so the asset
    directory stays pure artwork: `manifest.json` there already means the
    generation record, and two files with the same name in one folder is how the
    wrong one gets shipped.
    """
    return {
        "id": "aria-with-you",
        "version": "1.0.0",
        "name": {
            "zh-CN": "Aria with You",
            "en": "Aria with You",
            "ja": "Aria with You",
        },
        "description": {
            "zh-CN": "按真实日期切换春夏秋冬的 Aria 立绘与配色。四季各有不同的穿搭、动作与道具。",
            "en": "Switches Aria's artwork and the accent colour between spring, summer, autumn and winter, following the date.",
            "ja": "日付に合わせて Aria の春夏秋冬の立ち絵と配色を切り替えます。",
        },
        "author": "Aria Project",
        "version_note": "Artwork generated with gpt-image-2.6-sunburst; bundled, never fetched at runtime.",
        "glyph": "palette",
        "accent": "#E39BD4",
        "hooks": ["theme", "background", "sidebar"],
        "permissions": ["theme", "uiSlot", "storage"],
        "enabledByDefault": False,
        # Declared so the UI can say "this one does nothing at launch" rather than
        # leaving the user to wonder. Aria is pure presentation.
        "contributions": [],
        "assets": {
            # The prefix the frontend should load artwork from. The installer
            # rewrites it to the plugin's asset base after extraction.
            "base": ".",
            "seasons": {
                season: {
                    "hero": f"aria-{season}-hero.webp",
                    "portrait": f"aria-{season}-portrait.webp",
                    "thumb": f"aria-{season}-portrait-thumb.webp",
                }
                for season in ("spring", "summer", "autumn", "winter")
            },
            # Per-page poses. Same season treatment, different gesture per page,
            # so five pages do not show the same drawing.
            "pages": {
                f"{page}-{season}": {
                    "hero": f"pages/aria-{page}-{season}.webp",
                    "thumb": f"pages/aria-{page}-{season}-thumb.webp",
                }
                for page in ("library", "store", "downloads", "plugins", "instance-detail")
                for season in ("spring", "summer", "autumn", "winter")
            },
        },
    }


def write_archive(source: Path, out: Path, manifest: dict[str, object]) -> dict[str, object]:
    files = collect(source)
    out.parent.mkdir(parents=True, exist_ok=True)
    # Pinned timestamp: see the module docstring.
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "1735689600"))  # 2025-01-01T00:00:00Z
    stamp = (1980, 1, 1, 0, 0, 0)  # the earliest a ZIP can represent

    with zipfile.ZipFile(out, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        def add(name: str, data: bytes) -> None:
            info = zipfile.ZipInfo(name, date_time=stamp)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            archive.writestr(info, data)

        add("manifest.json", (json.dumps(manifest, indent=2, ensure_ascii=False) + "\n").encode("utf-8"))
        for relative in files:
            add(relative.as_posix(), (source / relative).read_bytes())

    data = out.read_bytes()
    return {
        "file": out.name,
        "sha256": hashlib.sha256(data).hexdigest(),
        "bytes": len(data),
        "entries": len(files) + 1,
        "epoch": epoch,
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--out", type=Path)
    parser.add_argument("--version", default="1.0.0")
    args = parser.parse_args(argv[1:])

    if not args.source.is_dir():
        print(f"source directory not found: {args.source}", file=sys.stderr)
        return 2

    manifest = build_manifest(args.source)
    manifest["version"] = args.version
    out = args.out or (args.source.parent / f"aria-with-you-{args.version}.mflp")
    summary = write_archive(args.source, out, manifest)

    print(json.dumps(summary, indent=2))
    print(f"\npackage: {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
