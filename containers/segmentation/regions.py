"""Water gating for the segment tool: pure numpy, no torch.

OneFormer sometimes labels wet gravel as water (#12). Such pixels tend to
carry a low share of the water classes' score, or to form small regions away
from the bottom of the frame, where the shoreline footage has its water.
`combine` drops both and leaves every other class as plain argmax.
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


def combine(
    best_class: np.ndarray,
    water_prob: np.ndarray,
    water_ids: Collection[int],
    other_ids: Collection[int],
    min_prob: float,
    min_area: float,
) -> np.ndarray:
    """The final class mask: gated, connected water plus argmax other classes.

    `best_class` is the per-pixel argmax class id and `water_prob` the
    per-pixel share of the water classes' score. A water pixel (argmax in
    `water_ids`) is kept if it passes `gate_by_probability` and then
    `keep_connected`; a pixel whose argmax is in `other_ids` is always kept.
    With both thresholds 0 the result equals `np.isin(best_class, water_ids
    + other_ids)`.
    """
    water = gate_by_probability(np.isin(best_class, list(water_ids)), water_prob, min_prob)
    return keep_connected(water, min_area) | np.isin(best_class, list(other_ids))
