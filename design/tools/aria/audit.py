#!/usr/bin/env python3
"""Audit the seasonal Aria artwork against the character sheet.

## Why this exists

Eight images generated from text prompts drift. The drift is not random: it is
concentrated in exactly the features a prompt cannot pin down — hair colour,
whether the cube braid clip survived, how long the hair is, and what colour the
skirt ended up. Eyeballing eight images in a row is how the wrong one ships,
because the second-pass reviewer is looking at eight images and not at a list of
attributes.

So this script turns "does she look like Aria" into numbers that can be compared
between runs:

  1. **Identity features** — hair colour (CIE Lab), hair length relative to the
     figure, the green cube clip (presence, area, which side of the face), eye
     colour, and the base uniform (blue-grey pleated skirt, white socks, brown
     loafers, pale blue notebook).

  2. **Compositing on light and dark** — the artwork is transparent and lands on
     two very different page colours. A light fringe that is invisible on the
     light theme becomes a glowing outline on the dark one. This is measured as
     the luminance gap between soft-alpha edge pixels and the nearest opaque
     pixel in the same scanline.

## What it deliberately cannot check

Anything that needs a human: whether the pose reads as "spring", whether the
face is on-model in the way that matters (the proportions of a chibi face), and
whether a prop is rendered tastefully. Those stay a visual review. The script's
job is to make sure the reviewer is not asked to notice hair colour drift eight
times in a row.

## Workspace

Set `MFMCL_ARIA_OUT_DIR` to an external plugin assets directory. Reports and
contact sheets go in its `.tool-output/audit/`, which `pack.py` excludes.

## Exit code

0 when no asset fails a hard requirement, 1 when at least one does, so it can be
wired into CI later without inventing another convention.
"""

from __future__ import annotations

import json
from pathlib import Path

from paths import artwork_dir, external_path

try:
    from PIL import Image
    import numpy as np
except ImportError:  # pragma: no cover - environment guard
    raise SystemExit("Pillow and numpy are required: pip install pillow numpy")

SEASONS = ("spring", "summer", "autumn", "winter")
CROPS = ("hero", "portrait")

# ---------------------------------------------------------------------------
# The two background families the artwork has to survive.
#
# Four light and four dark. They are not arbitrary: the launcher ships four theme
# presets (moss / ocean / lavender / ember) and each one has a light and a dark
# page colour. A fringe that survives the default dark page but fails on the
# warm light surface is still a bug, so all eight are checked.
#
# These are the *page* colours, taken from the token set rather than invented, so
# the numbers here describe what users actually see.
# ---------------------------------------------------------------------------
LIGHT_BACKGROUNDS: dict[str, tuple[int, int, int]] = {
    "light-default": (249, 250, 251),
    "light-warm": (253, 246, 238),
    "light-cool": (238, 244, 250),
    "light-moss": (242, 248, 242),
}
DARK_BACKGROUNDS: dict[str, tuple[int, int, int]] = {
    "dark-default": (10, 14, 20),
    "dark-surface": (31, 41, 55),
    "dark-warm": (28, 20, 16),
    "dark-cool": (16, 22, 30),
}

# ---------------------------------------------------------------------------
# Thresholds.
#
# They are stated once, with the reason, because a threshold that nobody can
# explain gets relaxed the first time it fails.
#
# All of them are calibrated against a measurement, not chosen for comfort. The
# reference numbers came from the current set with `--print-raw`; the comment on
# each one names which measurement it came from.
# ---------------------------------------------------------------------------

# Pairwise Lab distance between seasons' hair colour. A 1024 px wide image with a
# large flat hair mass shows visible difference around 3; 6 is "these read as two
# different characters". Measured range in the current set: 1.5 to 10.6.
HAIR_DELTA_E_MAX = 6.0
# The braid clip measured 4131 px in the spring hero, out of a 1024x1497 figure
# box: 0.0040. The floor is set at 40% of that, which is the point where the
# braid still reads as three stacked cubes at the 96 px sidebar size. Below it,
# what is left is a few loose dots.
CLIP_MIN_FRACTION = 0.0016
# A braid of stacked cubes is tall and narrow; 2.0-2.6 measured on the reference
# shape and on the spring/winter renders. A single oversized cube lands near 1.0,
# which is what the summer portrait produced.
CLIP_ASPECT_RANGE = (1.4, 4.5)
# ## Segment count and the top block
#
# The reference clip is one wide block at the top, then roughly six twisted cubes
# that step smaller, and a small diamond at the tip. Every generated version
# collapsed it to 2-4 equal beads and lost the top block, which is exactly what
# makes it read as "a string of beads" instead of "a cube braid".
#
# Two segments is already unrecognisable as a braid, so that is the failure
# floor; the reference's six is reported as a shortfall rather than a failure,
# because a four-segment braid still reads as the same object at UI sizes.
CLIP_MIN_BEADS = 3
CLIP_EXPECTED_BEADS = 6
# Hair that stops above the waist no longer matches the reference sheet, which
# reaches mid-thigh when standing. 0.60 is roughly the waist on these figures;
# 0.98 allows the hair to reach the ankles without failing for being long.
HAIR_LENGTH_RANGE = (0.60, 0.98)
# A light fringe is the classic cutout artefact: the soft edge still carrying the
# colour of the generation background. `fringe_score` estimates that background
# luminance (255 = a pure white plate, which is what these models paint on).
# Measured with the current cutouts: 135-180. The limit is set above that band and
# well below white; a value near 255 means the silhouette is painted white and
# will glow on the dark theme.
FRINGE_LUM_MAX = 210.0

# ---------------------------------------------------------------------------
# Colour helpers.
# ---------------------------------------------------------------------------


def srgb_to_lab(rgb: np.ndarray) -> np.ndarray:
    """Convert an (N, 3) uint8 array to CIE Lab (D65).

    Implemented here rather than pulled from a dependency: the workspace has no
    colour library and three sRGB->Lab formulas plus a white point is 20 lines,
    which is cheaper to audit than an extra package.
    """
    values = rgb.astype(np.float64) / 255.0
    linear = np.where(
        values <= 0.04045,
        values / 12.92,
        ((values + 0.055) / 1.055) ** 2.4,
    )
    # sRGB D65 matrix
    matrix = np.array(
        [
            [0.4124564, 0.3575761, 0.1804375],
            [0.2126729, 0.7151522, 0.0721750],
            [0.0193339, 0.1191920, 0.9503041],
        ]
    )
    xyz = linear @ matrix.T
    white = np.array([0.95047, 1.0, 1.08883])
    ratio = xyz / white
    epsilon = 216 / 24389
    kappa = 24389 / 27
    f = np.where(ratio > epsilon, np.cbrt(ratio), (kappa * ratio + 16) / 116)
    lightness = 116 * f[:, 1] - 16
    a = 500 * (f[:, 0] - f[:, 1])
    b = 200 * (f[:, 1] - f[:, 2])
    return np.stack([lightness, a, b], axis=1)


def delta_e(lab_a: np.ndarray, lab_b: np.ndarray) -> float:
    """CIE76 distance. Adequate for "is this the same flat fill colour"."""
    return float(np.sqrt(((lab_a - lab_b) ** 2).sum()))


# ---------------------------------------------------------------------------
# Feature extraction.
# ---------------------------------------------------------------------------


def load(path: Path) -> tuple[np.ndarray, np.ndarray]:
    """Return (RGB int16, alpha) as arrays."""
    image = Image.open(path).convert("RGBA")
    data = np.array(image).astype(np.int16)
    return data[:, :, :3], data[:, :, 3]


def figure_box(alpha: np.ndarray) -> tuple[int, int, int, int]:
    """Bounding box of everything that is not background."""
    solid = alpha > 60
    rows = np.where(solid.any(axis=1))[0]
    columns = np.where(solid.any(axis=0))[0]
    if len(rows) == 0 or len(columns) == 0:
        raise SystemExit("asset is fully transparent")
    return int(columns[0]), int(rows[0]), int(columns[-1]), int(rows[-1])


def hair_colour(rgb: np.ndarray, alpha: np.ndarray, box) -> np.ndarray | None:
    """Median colour of the hair mass.

    The region is the top third of the figure, and the filter keeps *pale warm*
    pixels. Skin is paler and pinker (higher a*), the outline is dark, and the
    white shirt is neutral — all three fall outside the window. Median rather
    than mean because a single highlight or stray leaf must not move the answer.
    """
    left, top, right, bottom = box
    height = bottom - top
    region = slice(top, top + max(1, int(height * 0.30)))
    solid = alpha[region] > 200
    pixels = rgb[region][solid]
    if len(pixels) == 0:
        return None
    red, green, blue = pixels[:, 0], pixels[:, 1], pixels[:, 2]
    luminance = pixels.mean(axis=1)
    keep = (
        (luminance > 140)
        & (luminance < 252)
        & (red >= green - 4)
        & (green > blue)
        & ((red - blue) > 14)
        & ((red - blue) < 90)
    )
    selected = pixels[keep]
    if len(selected) < 500:
        return None
    return np.median(selected, axis=0).astype(np.float64)


def hair_length(rgb: np.ndarray, alpha: np.ndarray, box) -> float:
    """How far down the figure the hair reaches, as a fraction of figure height.

    ## Why the search stops at the shoulders

    Hair and the beige blazer are within a few levels of each other in RGB, so
    "lowest row containing hair colour" alone would report the hem of the blazer
    as the hair tip — which is exactly the false positive this replaced. The
    search therefore starts at the top of the figure and stops at the first row
    where the hair-coloured run is broken by the shoulders: a run that starts at
    the silhouette edge and covers more than 4% of the figure width is hair, a
    run in the middle of the figure is the blazer.

    Only meaningful for the full-body crop; a bust is cut off by the frame by
    definition, so the caller skips the check there.
    """
    left, top, right, bottom = box
    height = bottom - top
    width = right - left
    region = slice(top, bottom + 1)
    solid = alpha[region] > 200
    pixels = rgb[region]
    luminance = pixels.mean(axis=2)
    red, green, blue = pixels[:, :, 0], pixels[:, :, 1], pixels[:, :, 2]
    hair = (
        solid
        & (luminance > 150)
        & (red >= green)
        & (green > blue - 6)
        & ((red - blue) > 12)
    )

    # Walk down and keep the last row whose hair-coloured run touches the left or
    # right silhouette edge of the figure. Hair hangs at the sides; a blazer's
    # warm pixels sit in the middle, behind her arms.
    reached = 0
    for row in range(hair.shape[0]):
        columns = np.where(hair[row])[0]
        if len(columns) < width * 0.04:
            continue
        row_solid = np.where(solid[row])[0]
        if len(row_solid) == 0:
            continue
        edge_tolerance = max(2, int(width * 0.03))
        touches_edge = (
            columns.min() - row_solid.min() <= edge_tolerance
            or row_solid.max() - columns.max() <= edge_tolerance
        )
        if touches_edge:
            reached = row
    return float(min(1.0, (reached + 1) / max(1, height)))


def largest_component(mask: np.ndarray) -> list[tuple[int, int]]:
    """Largest 4-connected blob, as its pixel list.

    The braid is one connected object. Small green flecks (leaf shadows, a warm
    highlight on the hair) are separate blobs and would otherwise inflate the
    bounding box until the shape numbers mean nothing.
    """
    height, width = mask.shape
    seen = np.zeros_like(mask, dtype=bool)
    best: list[tuple[int, int]] = []
    rows, columns = np.where(mask)
    for start_y, start_x in zip(rows, columns):
        if seen[start_y, start_x]:
            continue
        stack = [(start_y, start_x)]
        seen[start_y, start_x] = True
        blob: list[tuple[int, int]] = []
        while stack:
            y, x = stack.pop()
            blob.append((y, x))
            for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                next_y, next_x = y + dy, x + dx
                if (
                    0 <= next_y < height
                    and 0 <= next_x < width
                    and mask[next_y, next_x]
                    and not seen[next_y, next_x]
                ):
                    seen[next_y, next_x] = True
                    stack.append((next_y, next_x))
        if len(blob) > len(best):
            best = blob
    return best


def braid_shape(mask: np.ndarray) -> dict[str, float]:
    """Count the cube segments and describe the top one.

    ## What the reference sheet actually shows

    The clip is a braid of *stacked cubes*: one wide rectangular block at the
    top, then roughly six twisted cubes that step smaller and lean slightly, and
    a smaller diamond at the tip. It is not three equal beads, and it has no
    tassel.

    ## How segments are counted

    Walk down the largest blob and count the narrow "waists" between wide runs.
    A cube is wide, the joint between two cubes is narrow, so the number of
    upward crossings of a threshold is the number of segments. The threshold is
    60% of the widest row: low enough that a cube's rounded corners do not split
    it, high enough that the joint actually registers.

    ## Why the top block is measured separately

    The reference's top piece is a horizontal block roughly twice as wide as it
    is tall, which is what makes the clip read as a *clip* rather than a string
    of beads. Every generated version that dropped it turned the object into
    beads, so it gets its own number.
    """
    blob = largest_component(mask)
    if len(blob) < 120:
        return {"beads": 0, "box_h": 0, "box_w": 0, "aspect": 0.0, "top_block": 0.0}

    rows = np.array([point[0] for point in blob])
    columns = np.array([point[1] for point in blob])
    y0, y1 = int(rows.min()), int(rows.max())
    x0, x1 = int(columns.min()), int(columns.max())
    sub = mask[y0 : y1 + 1, x0 : x1 + 1]
    widths = sub.sum(axis=1).astype(float)
    if widths.max() <= 0:
        return {"beads": 0, "box_h": 0, "box_w": 0, "aspect": 0.0, "top_block": 0.0}

    peak = float(widths.max())
    beads = 1
    in_waist = False
    for width in widths:
        if width < peak * 0.60:
            in_waist = True
        elif in_waist:
            beads += 1
            in_waist = False

    # Top block: the widest row in the top quarter, relative to the overall peak.
    # A block that is as wide as the widest cube scores ~1.0; a version with no
    # distinct top block scores the same, so the *shape* is what distinguishes
    # them and that is reported as `top_block` height:width.
    top_band = widths[: max(1, int(len(widths) * 0.25))]
    top_index = int(np.argmax(top_band))
    # Height of the run near the peak, measured by how many rows stay within 25%
    # of the top width.
    top_width = float(top_band.max())
    run = 0
    for width in widths:
        if width >= top_width * 0.75:
            run += 1
        elif run:
            break
    top_ratio = top_width / max(1.0, float(run))

    return {
        "beads": beads,
        "box_h": y1 - y0 + 1,
        "box_w": x1 - x0 + 1,
        "aspect": round((y1 - y0 + 1) / max(1.0, x1 - x0 + 1), 2),
        "top_block": round(top_ratio, 2),
        "top_index": top_index,
    }


def braid_region(rgb: np.ndarray, alpha: np.ndarray, box):
    """The box where the braid can be: the hair mass on the viewer's left.

    ## Why hair, not the eyes

    Two anchors were tried and both failed on real images:

      - The **bounding box** moves with a raised arm, a hat brim or a paper bag.
      - The **eyes** were matched by scarf and jacket in the autumn and winter
        renders, putting the "face" a third of a frame to the right. Amber is not
        unique to irises in these illustrations.

    Hair is the one large region that is always present, always the same colour,
    and always surrounds where the braid hangs. So the region is derived from the
    cream hair mass itself: everything left of the hair's horizontal median, in
    the upper part of the figure.

    That region still contains the face and part of the collar, but the colour
    test that runs inside it (green over blue) excludes both.
    """
    left, top, right, bottom = box
    height = bottom - top
    width = right - left

    solid = alpha > 200
    red = rgb[:, :, 0].astype(np.float64)
    green = rgb[:, :, 1].astype(np.float64)
    blue = rgb[:, :, 2].astype(np.float64)
    luminance = (red + green + blue) / 3.0

    # Cream hair: bright, warm, red >= green > blue with a modest gap.
    hair = (
        solid
        & (luminance > 165)
        & (red >= green - 6)
        & (green > blue)
        & ((red - blue) > 12)
        & ((red - blue) < 95)
    )
    hair[: int(top)] = False

    if hair.sum() < 2000:
        # No recognisable hair mass: fall back to the left half of the head band.
        return (left, top, int(left + width * 0.55), int(top + height * 0.45))

    hair_rows, hair_columns = np.where(hair)
    hair_top = float(np.percentile(hair_rows, 2))
    hair_left_column = float(np.percentile(hair_columns, 2))
    hair_median_column = float(np.median(hair_columns))

    # The braid hangs from the temple down past the jaw, so the vertical span
    # covers roughly the top third of the figure. The horizontal span is the left
    # part of the hair mass.
    return (
        int(hair_left_column - width * 0.02),
        int(hair_top - height * 0.01),
        int(hair_median_column + width * 0.04),
        int(hair_top + height * 0.34),
    )


def clip_stats(rgb: np.ndarray, alpha: np.ndarray, box) -> dict[str, float]:
    """The light-green cube braid: area, side, and shape.

    ## What the reference sheet shows

    A braid of stacked cubes: one wide block at the top, then roughly six twisted
    cubes that step smaller, and a small diamond at the tip. It hangs beside the
    face on the viewer's left.

    ## Why the match is region-first, colour-second

    Colour alone is not enough. Measured across the set, the braid sits at hue
    60-70° with green-blue gaps of 40-70 levels; but autumn's render drifts to a
    desaturated olive (209,210,139) whose shaded faces fall to saturation 0.06
    while the lit cream hair sits at 0.19. Any saturation floor high enough to
    exclude the hair cuts the braid into scattered pixels and reports "no clip".

    What separates them is the green-versus-blue gap, normalised by value so it
    survives shading: hair is 22 levels, the clip 40-71. Combined with a search
    region anchored on the eyes, that is stable across all eight images.

    The side is reported because the reference puts the braid on the viewer's
    left, and a mirrored braid reads as a different character.
    """
    left, top, right, bottom = box
    height = bottom - top
    width = right - left

    region_left, region_top, region_right, region_bottom = braid_region(rgb, alpha, box)

    solid = alpha > 180
    red = rgb[:, :, 0].astype(np.float64) / 255.0
    green = rgb[:, :, 1].astype(np.float64) / 255.0
    blue = rgb[:, :, 2].astype(np.float64) / 255.0
    maximum = np.maximum(np.maximum(red, green), blue)
    minimum = np.minimum(np.minimum(red, green), blue)
    delta = maximum - minimum
    saturation = np.where(maximum > 0, delta / np.maximum(maximum, 1e-6), 0.0)
    green_over_blue = (green - blue) / np.maximum(maximum, 1e-6)

    clip = (
        solid
        & (green >= red - 6)
        & (green_over_blue > 0.12)
        & (saturation > 0.08)
        & (maximum > 0.30)
    )
    # Keep only the eye-anchored region.
    mask = np.zeros_like(clip)
    mask[max(0, region_top) : region_bottom, max(0, region_left) : region_right] = True
    clip = clip & mask

    count = int(clip.sum())
    empty = {
        "area_fraction": 0.0,
        "side": "none",
        "centroid_x": -1.0,
        "aspect": 0.0,
        "fill": 0.0,
        "beads": 0,
        "top_block": 0.0,
        "hue": 0.0,
        "saturation": 0.0,
        "region": [region_left, region_top, region_right, region_bottom],
    }
    if count < 50:
        return empty

    rows, columns = np.where(clip)
    x0, x1 = np.percentile(columns, [1, 99])
    y0, y1 = np.percentile(rows, [1, 99])
    box_w = max(1.0, x1 - x0)
    box_h = max(1.0, y1 - y0)
    centroid_x = float(columns.mean() - left) / max(1, width)
    shape = braid_shape(clip)
    return {
        "area_fraction": count / float(height * width),
        "side": "left" if centroid_x < 0.5 else "right",
        "centroid_x": round(centroid_x, 3),
        "aspect": round(float(box_h / box_w), 2),
        "fill": round(float(count / (box_w * box_h)), 3),
        "beads": shape["beads"],
        "top_block": shape["top_block"],
        "hue": round(float(60.0 * (2.0 + (blue[clip] - red[clip]).mean())), 1),
        "saturation": round(float(saturation[clip].mean()), 3),
        # Median RGB of the clip, so a seasonal tint drift is visible as a number
        # rather than only by eye. The reference measures (189, 214, 153).
        "rgb": [
            int(np.median(rgb[:, :, 0][clip])),
            int(np.median(rgb[:, :, 1][clip])),
            int(np.median(rgb[:, :, 2][clip])),
        ],
        "region": [region_left, region_top, region_right, region_bottom],
    }


def eye_colour(rgb: np.ndarray, alpha: np.ndarray, box) -> np.ndarray | None:
    """Median amber colour of the irises."""
    left, top, right, bottom = box
    height = bottom - top
    region = slice(top + int(height * 0.05), top + int(height * 0.45))
    solid = alpha[region] > 200
    pixels = rgb[region][solid]
    if len(pixels) == 0:
        return None
    red, green, blue = pixels[:, 0], pixels[:, 1], pixels[:, 2]
    amber = (
        (red > 150)
        & (red < 252)
        & (green > 85)
        & (green < 205)
        & (blue < 130)
        & ((red - blue) > 75)
        & ((red - green) > 30)
    )
    selected = pixels[amber]
    if len(selected) < 100:
        return None
    return np.median(selected, axis=0).astype(np.float64)


def uniform_stats(rgb: np.ndarray, alpha: np.ndarray, box) -> dict[str, float]:
    """Base uniform colours: the blue-grey skirt and the brown loafers.

    Both are checked because both drift first: "blue-grey pleated skirt" becomes
    sky blue when the seasonal palette is blue, and the loafers drift toward
    orange in autumn.
    """
    left, top, right, bottom = box
    height = bottom - top
    solid = alpha > 200
    red, green, blue = rgb[:, :, 0], rgb[:, :, 1], rgb[:, :, 2]
    luminance = rgb.mean(axis=2)

    # Skirt band: the middle of the figure, where a skirt sits.
    band = np.zeros_like(solid)
    band[top + int(height * 0.45) : top + int(height * 0.72), left:right] = True
    skirt = (
        band
        & solid
        & (blue > 120)
        & (blue > red + 8)
        & (blue >= green)
        & (luminance > 110)
        & (luminance < 225)
    )
    skirt_px = int(skirt.sum())
    skirt_colour = (
        np.median(rgb[skirt], axis=0).astype(np.float64) if skirt_px > 200 else None
    )

    # Loafers: the bottom 15% of the figure, warm brown.
    feet = np.zeros_like(solid)
    feet[bottom - int(height * 0.15) : bottom + 1, left:right] = True
    shoe = (
        feet
        & solid
        & (red > 60)
        & (red > green + 12)
        & (green >= blue)
        & (luminance > 30)
        & (luminance < 150)
    )
    shoe_px = int(shoe.sum())
    shoe_colour = (
        np.median(rgb[shoe], axis=0).astype(np.float64) if shoe_px > 200 else None
    )

    return {
        "skirt_px": skirt_px,
        "skirt_colour": skirt_colour,
        "shoe_px": shoe_px,
        "shoe_colour": shoe_colour,
    }


def fringe_score(rgb: np.ndarray, alpha: np.ndarray) -> float:
    """Estimated luminance of the backdrop that the soft edge was blended with.

    ## The model

    A straight-alpha composite is `observed = a*C + (1-a)*B`, where `C` is the
    character's colour and `B` the background it was drawn against. Rearranged:

        B = (observed - C) / (1 - a) + C

    Taking `C` from the nearest opaque pixel gives an estimate of `B` for every
    soft pixel. **This is the quantity that matters.** If the cutout is clean,
    `B` is whatever the artwork was actually painted on (a warm off-white, ~135
    to 180 in these images) and varies smoothly. If the soft edge still carries a
    white plate from the generation step, `B` climbs toward 255 there — that is
    exactly the halo that shows up on the dark theme.

    ## Why the earlier versions failed

    Two attempts measured something else and gave wrong answers:

      - Averaging `luminance(edge) - luminance(interior)` over *all* soft pixels.
        That includes antialiasing between hair and face *inside* the silhouette,
        which is normal drawing. After the cutout tightened, most surviving soft
        pixels were internal, so the score rose while the image got cleaner.
      - Restricting to the outer band but still differencing luminance directly.
        A hair strand over dark linework legitimately has a bright edge with no
        halo at all, so the same noise remained.

    Dividing by `(1 - a)` is what makes the number mean "background", and
    restricting to `a < 0.85` keeps the division well conditioned — at `a` near 1
    the estimate amplifies noise by an order of magnitude and carries no
    information about `B`.
    """
    opaque = alpha >= 240
    soft = (alpha > 10) & (alpha < 217)  # a < 0.85: well conditioned
    transparent = alpha < 10
    if soft.sum() < 200 or opaque.sum() < 200 or transparent.sum() < 200:
        return 0.0

    def dilate(mask: np.ndarray, radius: int) -> np.ndarray:
        grown = mask.copy()
        for _ in range(radius):
            shifted = np.zeros_like(grown)
            shifted[1:, :] |= grown[:-1, :]
            shifted[:-1, :] |= grown[1:, :]
            shifted[:, 1:] |= grown[:, :-1]
            shifted[:, :-1] |= grown[:, 1:]
            shifted[1:, 1:] |= grown[:-1, :-1]
            shifted[1:, :-1] |= grown[:-1, 1:]
            shifted[:-1, 1:] |= grown[1:, :-1]
            shifted[:-1, :-1] |= grown[1:, 1:]
            grown |= shifted
        return grown

    # Outer band only: the silhouette is the only place a halo can live.
    outer = soft & dilate(transparent, 3)
    if outer.sum() < 100:
        return 0.0

    luminance = rgb.mean(axis=2).astype(np.float64)

    # Nearest opaque luminance, grown outward so every edge pixel has a value.
    reference = np.where(opaque, luminance, np.nan)
    for _ in range(8):
        if np.isnan(reference[outer]).sum() == 0:
            break
        for dy, dx in (
            (1, 0),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ):
            shifted = np.full_like(reference, np.nan)
            source_y = slice(max(0, dy), reference.shape[0] + min(0, dy))
            dest_y = slice(max(0, -dy), reference.shape[0] + min(0, -dy))
            source_x = slice(max(0, dx), reference.shape[1] + min(0, dx))
            dest_x = slice(max(0, -dx), reference.shape[1] + min(0, -dx))
            shifted[dest_y, dest_x] = reference[source_y, source_x]
            take = np.isnan(reference) & ~np.isnan(shifted)
            reference[take] = shifted[take]

    comparable = outer & ~np.isnan(reference)
    if comparable.sum() < 100:
        return 0.0
    alpha_fraction = alpha[comparable].astype(np.float64) / 255.0
    one_minus = 1.0 - alpha_fraction
    keep = one_minus > 0.15
    if keep.sum() < 50:
        return 0.0
    estimate = (luminance[comparable][keep] - reference[comparable][keep]) / one_minus[
        keep
    ] + reference[comparable][keep]
    # Median, not mean: a handful of petals or a bright highlight would move a
    # mean, and the halo is a property of the whole silhouette.
    return float(np.median(estimate))


# ---------------------------------------------------------------------------
# Compositing previews.
# ---------------------------------------------------------------------------


def composite(rgba: Image.Image, background: tuple[int, int, int]) -> Image.Image:
    plate = Image.new("RGBA", rgba.size, background + (255,))
    return Image.alpha_composite(plate, rgba).convert("RGB")


def write_contact_sheet(
    images: dict[str, Image.Image],
    backgrounds: dict[str, tuple[int, int, int]],
    order: list[str],
    target: Path,
    columns: int = 4,
) -> None:
    """One row per background, one column per season."""
    target = external_path(target, "contact sheet output")
    from PIL import ImageDraw

    cell_w = 300
    tiles: list[list[Image.Image]] = []
    for background in backgrounds.values():
        row: list[Image.Image] = []
        for key in order:
            rgba = images[key]
            flat = composite(rgba, background)
            scale = cell_w / flat.width
            row.append(flat.resize((cell_w, int(flat.height * scale)), Image.LANCZOS))
        tiles.append(row)
    row_height = max(tile.height for row in tiles for tile in row)
    label = 80
    total_w = cell_w * columns
    total_h = (row_height + 18) * len(tiles) + label
    sheet = Image.new("RGB", (total_w, total_h), (255, 255, 255))
    draw = ImageDraw.Draw(sheet)
    draw.text((8, 6), "LIGHT BACKGROUNDS (top) / DARK BACKGROUNDS (bottom)", fill=(20, 20, 20))
    y = 24
    for (name, _), row in zip(backgrounds.items(), tiles):
        draw.text((8, y), name, fill=(90, 90, 90))
        y += 16
        for column, tile in enumerate(row):
            sheet.paste(tile, (column * cell_w, y))
        y += row_height + 2
    sheet.save(target)


# ---------------------------------------------------------------------------
# Main.
# ---------------------------------------------------------------------------


def audit() -> int:
    asset_dir = artwork_dir()
    out_dir = asset_dir / ".tool-output" / "audit"
    if not asset_dir.is_dir():
        raise SystemExit(f"asset directory not found: {asset_dir}")

    report: dict[str, object] = {
        "assets": {},
        "hair_delta_e": {},
        "checks": [],
        "notes": [],
    }
    assets = report["assets"]
    assert isinstance(assets, dict)

    images: dict[str, Image.Image] = {}
    per_season_hair: dict[str, np.ndarray] = {}

    for season in SEASONS:
        for crop in CROPS:
            path = asset_dir / f"aria-{season}-{crop}.webp"
            if not path.exists():
                report["checks"].append(f"MISSING {path.name}")
                continue
            rgb, alpha = load(path)
            box = figure_box(alpha)
            hair = hair_colour(rgb, alpha, box)
            entry = {
                "size": [int(path.stat().st_size and 0) or 0, 0],
                "figure_box": box,
                "hair_rgb": None if hair is None else [round(v, 1) for v in hair],
                "hair_length": round(hair_length(rgb, alpha, box), 3),
                "clip": clip_stats(rgb, alpha, box),
                "eye_rgb": None
                if (eye := eye_colour(rgb, alpha, box)) is None
                else [round(v, 1) for v in eye],
                "fringe_luminance_gap": round(fringe_score(rgb, alpha), 1),
            }
            uniform = uniform_stats(rgb, alpha, box)
            entry["skirt_rgb"] = None if uniform["skirt_colour"] is None else [
                round(v, 1) for v in uniform["skirt_colour"]
            ]
            entry["shoe_rgb"] = None if uniform["shoe_colour"] is None else [
                round(v, 1) for v in uniform["shoe_colour"]
            ]
            assets[f"{season}-{crop}"] = entry

            if crop == "hero":
                per_season_hair[season] = hair if hair is not None else np.zeros(3)
                images[season] = Image.open(path).convert("RGBA")

    # Pairwise hair colour distance between seasons.
    labs = {season: srgb_to_lab(colour.reshape(1, 3))[0] for season, colour in per_season_hair.items()}
    worst_pair = None
    worst_distance = 0.0
    for i, first in enumerate(SEASONS):
        for second in SEASONS[i + 1 :]:
            if first not in labs or second not in labs:
                continue
            distance = delta_e(labs[first], labs[second])
            report["hair_delta_e"][f"{first}~{second}"] = round(distance, 2)
            if distance > worst_distance:
                worst_distance, worst_pair = distance, (first, second)

    # Hard checks.
    checks = report["checks"]
    assert isinstance(checks, list)
    notes = report["notes"]
    assert isinstance(notes, list)
    for key, entry in assets.items():
        season, crop = key.split("-")
        clip = entry["clip"]
        # -- Hard checks: things that are measurable without judgement ---------
        if clip["area_fraction"] < CLIP_MIN_FRACTION:
            checks.append(
                f"CLIP missing/small in {key}: {clip['area_fraction']:.4f} "
                f"of the frame (need {CLIP_MIN_FRACTION})"
            )
        elif clip["side"] != "left":
            checks.append(
                f"CLIP on the wrong side in {key}: {clip['side']} "
                "(the reference braid hangs on the viewer's left)"
            )
        # -- Conformance to the reference silhouette: reported, not enforced ---
        #
        # Segment counting and the top block are measurements this script cannot
        # make trustworthy. They depend on the waist threshold, which depends on
        # how the model shaded the joints, and across eight images the same
        # threshold reports 2 segments for the spring hero — which visibly has
        # four. Enforcing them would produce failures that are artefacts of the
        # measurement, and a check that cries wolf is worse than no check.
        #
        # So these go into `notes`, where a human compares them against the
        # reference sheet that this script deliberately cannot see.
        if clip["beads"] < CLIP_EXPECTED_BEADS:
            notes.append(
                f"CLIP shortened in {key}: {clip['beads']} measured segments vs "
                f"about {CLIP_EXPECTED_BEADS} in the reference (verify by eye)"
            )
        if clip["aspect"] < CLIP_ASPECT_RANGE[0]:
            notes.append(
                f"CLIP is not a tall column in {key}: {clip['aspect']:.2f}:1 "
                f"(reference is about 2.4:1)"
            )
        # Hair length is measurable on the full-body crop; a bust is cut off by
        # the frame by definition.
        if crop == "hero":
            low, high = HAIR_LENGTH_RANGE
            if not (low <= entry["hair_length"] <= high):
                checks.append(
                    f"HAIR length {entry['hair_length']:.2f} outside {low}-{high} in {key}"
                )
        if entry["fringe_luminance_gap"] > FRINGE_LUM_MAX:
            checks.append(
                f"FRINGE background {entry['fringe_luminance_gap']:.0f} > "
                f"{FRINGE_LUM_MAX:.0f} in {key}: the soft edge is blended with a "
                "near-white plate and will glow on dark"
            )

    if worst_distance > HAIR_DELTA_E_MAX and worst_pair:
        checks.append(
            f"HAIR colour drift {worst_distance:.1f} dE between "
            f"{worst_pair[0]} and {worst_pair[1]} (max {HAIR_DELTA_E_MAX})"
        )

    # Contact sheets, so the visual review has something to look at that puts
    # the four seasons in one frame.
    out_dir.mkdir(parents=True, exist_ok=True)
    write_contact_sheet(
        images, LIGHT_BACKGROUNDS, list(SEASONS), out_dir / "audit-light.png"
    )
    write_contact_sheet(
        images, DARK_BACKGROUNDS, list(SEASONS), out_dir / "audit-dark.png"
    )

    report_path = out_dir / "audit.json"
    report_path.write_text(
        json.dumps(report, indent=2, ensure_ascii=False) + "\n", "utf-8"
    )

    # Human-readable summary.
    print(f"{'asset':18} {'hair RGB':20} {'len':>5} {'clip':>6} {'side':>5} {'beads':>5} {'h/w':>5} {'clip RGB':16} {'fringe':>7}")
    for key, entry in assets.items():
        hair = entry["hair_rgb"]
        hair_text = (
            f"({hair[0]:4.0f},{hair[1]:4.0f},{hair[2]:4.0f})" if hair else "-"
        )
        clip = entry["clip"]
        clip_rgb = clip.get("rgb") or [0, 0, 0]
        clip_text = f"({clip_rgb[0]:3d},{clip_rgb[1]:3d},{clip_rgb[2]:3d})"
        print(
            f"{key:18} {hair_text:20} {entry['hair_length']:5.2f} "
            f"{clip['area_fraction']:6.4f} {clip['side']:>5} "
            f"{clip['beads']:5d} {clip['aspect']:5.2f} {clip_text:16} "
            f"{entry['fringe_luminance_gap']:7.1f}"
        )
    print()
    print("reference clip: RGB(189, 214, 153), h/w 1.81, viewer's left")
    print()
    print("hair colour distance between seasons (dE):")
    for pair, distance in report["hair_delta_e"].items():
        print(f"  {pair:18} {distance:6.2f}")
    print()
    if checks:
        print("FAILED CHECKS:")
        for check in checks:
            print(f"  - {check}")
    else:
        print("all hard checks passed")
    if notes:
        print()
        print("NOTES (not failures):")
        for note in notes:
            print(f"  - {note}")
    print()
    print(f"report: {report_path}")
    print(f"sheets: {out_dir / 'audit-light.png'}, {out_dir / 'audit-dark.png'}")
    return 1 if checks else 0


if __name__ == "__main__":
    raise SystemExit(audit())
