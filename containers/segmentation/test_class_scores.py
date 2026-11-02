"""Tests for segment.class_scores on fake OneFormer outputs (torch on CPU, no model).

Run: uv run --no-project --index https://download.pytorch.org/whl/cpu --with torch==2.14.1 \
    --with numpy==2.5.4 --with pillow==13.0.0 --with scipy==1.18.1 --with pytest==9.2.0 \
    pytest -q test_class_scores.py
"""

from types import SimpleNamespace

import numpy as np
import pytest

torch = pytest.importorskip("torch")
import segment  # noqa: E402  (needs torch, imported after the skip)

QUERIES, CLASSES = 12, 40  # 40 classes span three CHUNK=16 chunks
SIZE = (37, 53)  # odd, so the upsampling factor is not an integer


def fake_outputs(seed: int) -> SimpleNamespace:
    """OneFormer-shaped outputs: class logits with a trailing no-object class, mask logits."""
    gen = torch.Generator().manual_seed(seed)
    return SimpleNamespace(
        class_queries_logits=torch.randn(1, QUERIES, CLASSES + 1, generator=gen) * 3,
        masks_queries_logits=torch.randn(1, QUERIES, 9, 13, generator=gen) * 3,
    )


def reference(outputs, size, water_ids):
    """All class maps upsampled at once, as post_process_semantic_segmentation does."""
    classes = outputs.class_queries_logits.softmax(dim=-1)[..., :-1]
    masks = outputs.masks_queries_logits.sigmoid()
    scores = torch.einsum("bqc, bqhw -> bchw", classes, masks)
    up = torch.nn.functional.interpolate(scores, size=size, mode="bilinear", align_corners=False)[0]
    total = up.sum(dim=0)
    water = up[list(water_ids)].sum(dim=0) if water_ids else torch.zeros(size)
    return up.argmax(dim=0).numpy(), (water / total).numpy()


# One id in each chunk, including the first id of the second chunk.
@pytest.mark.parametrize("water_ids", [[3, 16, 33], [15], [39]])
@pytest.mark.parametrize("seed", [0, 1, 2])
def test_matches_full_resolution_reference(water_ids, seed):
    outputs = fake_outputs(seed)
    best, prob = segment.class_scores(outputs, SIZE, water_ids)
    ref_best, ref_prob = reference(outputs, SIZE, water_ids)
    assert best.shape == prob.shape == SIZE
    np.testing.assert_array_equal(best, ref_best)
    np.testing.assert_allclose(prob, ref_prob, atol=1e-5)


def test_no_water_ids_give_zero_probability():
    best, prob = segment.class_scores(fake_outputs(0), SIZE, [])
    assert best.shape == SIZE
    np.testing.assert_array_equal(prob, np.zeros(SIZE))


def test_probability_is_a_share_of_the_scores():
    _, prob = segment.class_scores(fake_outputs(3), SIZE, list(range(CLASSES)))
    np.testing.assert_allclose(prob, np.ones(SIZE), atol=1e-5)
