#!/usr/bin/env python3
"""Pack a plugin directory into a `.mflp` archive.

## Why the package layout is fixed

The installer (`src-tauri/src/plugin_package.rs`) requires `manifest.json` at the
archive root. Everything else is free-form as long as the manifest points at it.
That single constraint is what makes a package inspectable: unzip it in a file
manager and the first thing you see is the manifest.

## Why a fixed timestamp

ZIP entries use a fixed timestamp and permissions, without filesystem-specific
extra attributes, so identical inputs give the same archive and SHA-256.

## Usage

    python design/tools/aria/pack.py --source <external artwork dir> --out <external file.mflp>

Both arguments are required and must resolve outside the launcher repository.
`--source` is the external artwork directory (hero/portrait files and `pages/`),
not the plugin root. Artwork is archived under `assets/` beside `manifest.json`.
The launcher does not ship Aria or any other non-built-in plugin.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import zipfile
from pathlib import Path

from paths import external_path, external_tree

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
    source = external_tree(source, "--source")
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
        "name": {"key": "plugin.ariaWithYou.name"},
        "description": {"key": "plugin.ariaWithYou.description"},
        "author": "Aria Project",
        "version_note": "Artwork generated with gpt-image-2.6-sunburst; included only in the independent plugin package, never fetched at runtime.",
        "glyph": "palette",
        "accent": "#E39BD4",
        "hooks": ["theme", "background", "sidebar"],
        "permissions": ["theme", "uiSlot", "storage"],
        "enabledByDefault": False,
        # Declared so the UI can say "this one does nothing at launch" rather than
        # leaving the user to wonder. Aria is pure presentation.
        "contributions": [],
        "assets": {
            # Relative to the installed package root, matching assetBase/assets.
            "base": "assets",
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
    source = external_tree(source, "--source")
    out = external_path(out, "--out")
    if not source.is_dir():
        raise ValueError(f"source directory not found: {source}")
    if out.is_relative_to(source):
        raise ValueError("--out must not be inside --source")
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
            add(f"assets/{relative.as_posix()}", (source / relative).read_bytes())

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
    parser.add_argument("--source", type=Path, required=True, help="external artwork directory")
    parser.add_argument("--out", type=Path, required=True, help="external .mflp destination")
    parser.add_argument("--version", default="1.0.0")
    args = parser.parse_args(argv[1:])

    try:
        source = external_tree(args.source, "--source")
        out = external_path(args.out, "--out")
        manifest = build_manifest(source)
        manifest["version"] = args.version
        summary = write_archive(source, out, manifest)
    except (OSError, RuntimeError, ValueError) as error:
        parser.error(str(error))

    print(json.dumps(summary, indent=2))
    print(f"\npackage: {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
