"""Water gating for the segment tool: pure numpy, no torch.

OneFormer sometimes labels wet gravel as water (#12). Such pixels tend to
carry a low share of the water classes' score, or to form small regions away
from the bottom of the frame, where the shoreline footage has its water.
`combine` drops them and leaves every other class as plain argmax.

Real water can carry a low share too: the band along a shoreline, or water
seen through a railing. Cutting by probability first splits such water from
the main body, so `combine` uses the cut only to find seeds and then grows
them back over every argmax-water pixel connected to them (hysteresis), with
thin occluders bridged. Distant water cut off by a dock or a treeline has no
cut seed, but unlike the gravel it is confident on average, so a region whose
mean water probability is high seeds itself.
"""

from collections.abc import Collection

import numpy as np
from scipy import ndimage


def gate_by_probability(candidate: np.ndarray, prob: np.ndarray, min_prob: float) -> np.ndarray:
    """Candidates whose probability is at least `min_prob`.

    `candidate` is a boolean mask (pixels whose argmax class is water) and
    `prob` the per-pixel water probability. Pixels outside `candidate` are
    never added. With `min_prob == 0` every candidate is kept.
    """
    return np.logical_and(candidate, prob >= min_prob)


def keep_connected(mask: np.ndarray, min_area: float) -> np.ndarray:
    """Components of the 2-D `mask` that touch the bottom row or are large.

    Components are 4-connected (diagonal neighbours are separate). A
    component is kept if any of its pixels is in the last row, or if its area
    is at least `min_area` (a fraction of the image area). Water in shoreline
    footage reaches the bottom of the frame or covers a large region; small
    inland regions are the misclassified gravel. With `min_area == 0` every
    component is kept.
    """
    if mask.ndim != 2:
        raise ValueError(f"mask must be 2-D, got shape {mask.shape}")
    if mask.size == 0:
        return mask.astype(bool, copy=True)
    labels, n = ndimage.label(mask)
    areas = np.bincount(labels.ravel(), minlength=n + 1)
    keep = areas >= min_area * mask.size
    keep[np.unique(labels[-1])] = True
    keep[0] = False
    return keep[labels]


def bridge(mask: np.ndarray, radius: float) -> np.ndarray:
    """`mask` closed with a disk of `radius` pixels (dilate, then erode).

    Gaps narrower than about 2 * radius, such as railing bars across water,
    are filled; the result contains `mask`. Pixels near the image border are
    not eroded from outside, so the closing does not shrink at the edges.
    With `radius <= 0` the mask is returned unchanged.
    """
    if radius <= 0 or not mask.any():
        return mask.astype(bool, copy=True)
    dilated = ndimage.distance_transform_edt(~mask) <= radius
    if dilated.all():  # edt of an array with no background is not a distance map
        return dilated
    return ndimage.distance_transform_edt(dilated) > radius


def grow(
    seed: np.ndarray, candidate: np.ndarray, radius: float, prob: np.ndarray, min_mean: float
) -> np.ndarray:
    """`seed` plus every `candidate` region that holds a seed or is confident.

    Regions are 4-connected on `candidate` closed by `bridge(candidate,
    radius)`, so a candidate region separated from a seed only by a gap
    narrower than the bridge still joins it. A region is kept if it holds a
    seed pixel, or if the mean of `prob` over its candidate pixels is at least
    `min_mean`. The bridging pixels themselves are never returned: the result
    is `seed | (candidate & kept)`.
    """
    if not seed.shape == candidate.shape == prob.shape:
        raise ValueError(f"seed {seed.shape}, candidate {candidate.shape} and prob {prob.shape} differ in shape")
    labels, n = ndimage.label(bridge(candidate | seed, radius))
    kept = np.zeros(n + 1, dtype=bool)
    kept[np.unique(labels[seed])] = True
    inside = labels[candidate]
    count = np.bincount(inside, minlength=n + 1)
    total = np.bincount(inside, weights=prob[candidate], minlength=n + 1)
    kept |= (count > 0) & (total >= min_mean * count)
    kept[0] = False
    return seed | (candidate & kept[labels])


def combine(
    best_class: np.ndarray,
    water_prob: np.ndarray,
    water_ids: Collection[int],
    other_ids: Collection[int],
    min_prob: float,
    min_area: float,
    bridge_frac: float,
    seed_prob: float,
) -> np.ndarray:
    """The final class mask: water grown from confident seeds, plus argmax other classes.

    `best_class` is the per-pixel argmax class id and `water_prob` the
    per-pixel share of the water classes' score. Seeds are the water pixels
    (argmax in `water_ids`) that pass `gate_by_probability` and then
    `keep_connected`. `grow` then adds every argmax-water pixel connected to a
    seed, bridging gaps up to `bridge_frac` of the image width, and every
    argmax-water region whose mean water probability is at least `seed_prob`.
    A low-confidence water region with no seed, such as wet gravel away from
    the water, is dropped; a pixel whose argmax is in `other_ids` is always
    kept. With `min_prob` and `min_area` both 0, or `seed_prob` 0, the result
    equals `np.isin(best_class, water_ids + other_ids)`.
    """
    candidate = np.isin(best_class, list(water_ids))
    seed = keep_connected(gate_by_probability(candidate, water_prob, min_prob), min_area)
    water = grow(seed, candidate, bridge_frac * best_class.shape[-1], water_prob, seed_prob)
    return water | np.isin(best_class, list(other_ids))
