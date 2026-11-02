"""Tests for regions.py on synthetic arrays (no torch, no model).

Run: uv run --with numpy --with scipy --with pytest pytest -q containers/segmentation/test_regions.py
"""

import numpy as np
import pytest

import regions


def grid(rows: list[str]) -> np.ndarray:
    """A boolean mask from strings: '#' is True, '.' is False."""
    return np.array([[c == "#" for c in row] for row in rows], dtype=bool)


# gate_by_probability


def test_gate_keeps_equal_and_drops_lower():
    candidate = np.ones((1, 3), dtype=bool)
    prob = np.array([[0.5, 0.49, 0.9]])
    out = regions.gate_by_probability(candidate, prob, 0.5)
    np.testing.assert_array_equal(out, [[True, False, True]])


def test_gate_never_adds_non_candidates():
    candidate = np.array([[True, False]])
    prob = np.array([[1.0, 1.0]])
    out = regions.gate_by_probability(candidate, prob, 0.5)
    np.testing.assert_array_equal(out, [[True, False]])


def test_gate_zero_keeps_every_candidate():
    candidate = np.array([[True, False], [False, True]])
    prob = np.zeros((2, 2))
    out = regions.gate_by_probability(candidate, prob, 0.0)
    np.testing.assert_array_equal(out, candidate)


# keep_connected


def test_tiny_component_touching_bottom_row_is_kept():
    mask = grid(["....", "....", "....", "..#."])
    out = regions.keep_connected(mask, 0.5)
    np.testing.assert_array_equal(out, mask)


def test_tiny_inland_blob_is_dropped():
    mask = grid([".....", ".#...", ".....", ".....", "....."])
    out = regions.keep_connected(mask, 0.05)  # 1 of 25 pixels = 0.04
    np.testing.assert_array_equal(out, np.zeros((5, 5), dtype=bool))


def test_large_inland_blob_is_kept_by_area():
    mask = np.zeros((10, 10), dtype=bool)
    mask[2:8, 2:8] = True  # 36 of 100 pixels
    out = regions.keep_connected(mask, 0.02)
    np.testing.assert_array_equal(out, mask)


def test_blob_exactly_at_area_threshold_is_kept():
    mask = grid(["##..", "##..", "....", "...."])  # 4 of 16 pixels = 0.25
    np.testing.assert_array_equal(regions.keep_connected(mask, 0.25), mask)
    np.testing.assert_array_equal(regions.keep_connected(mask, 0.26), np.zeros((4, 4), dtype=bool))


def test_min_area_zero_keeps_everything():
    mask = grid(["#...#", ".....", "..#..", ".....", "#...."])
    out = regions.keep_connected(mask, 0.0)
    np.testing.assert_array_equal(out, mask)


def test_one_component_kept_one_dropped():
    mask = grid(
        [
            "#.....",
            "......",
            "...###",
            "...###",
        ]
    )
    expected = grid(
        [
            "......",
            "......",
            "...###",
            "...###",
        ]
    )
    out = regions.keep_connected(mask, 0.25)
    np.testing.assert_array_equal(out, expected)


def test_diagonal_neighbours_are_separate_components():
    # The top-left pixel touches the bottom-row component only diagonally.
    mask = grid(["#..", ".#.", ".#."])
    expected = grid(["...", ".#.", ".#."])
    out = regions.keep_connected(mask, 0.5)
    np.testing.assert_array_equal(out, expected)


def test_components_on_top_and_side_edges_are_dropped():
    # Only the bottom row anchors a component; the top row and the side columns do not.
    mask = grid(["##....", "......", "#....#", "#....#", "......", "......"])
    out = regions.keep_connected(mask, 0.1)  # each component is 2 of 36 pixels
    np.testing.assert_array_equal(out, np.zeros((6, 6), dtype=bool))


def test_empty_array():
    out = regions.keep_connected(np.zeros((0, 0), dtype=bool), 0.02)
    assert out.shape == (0, 0)
    assert out.dtype == bool


def test_rejects_non_2d_input():
    with pytest.raises(ValueError):
        regions.keep_connected(np.zeros((2, 2, 2), dtype=bool), 0.0)


def test_output_is_bool_with_input_shape():
    mask = grid(["#..", "...", "..#"])
    out = regions.keep_connected(mask, 0.5)
    assert out.dtype == bool
    assert out.shape == mask.shape


# combine

WATER, SKY, GRAVEL = 21, 2, 13


def test_low_prob_water_dropped_but_low_prob_sky_kept():
    best = np.array([[WATER, SKY], [WATER, WATER]])
    prob = np.array([[0.2, 0.2], [0.9, 0.9]])
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.0)
    np.testing.assert_array_equal(out, [[False, True], [True, True]])


def test_inland_water_blob_dropped_bottom_water_kept():
    best = np.full((6, 6), GRAVEL)
    best[1, 1] = WATER  # wet gravel: small and away from the bottom
    best[4:, :] = WATER  # the sea along the bottom edge
    best[0, 4:] = SKY
    prob = np.ones((6, 6))
    expected = np.zeros((6, 6), dtype=bool)
    expected[4:, :] = True
    expected[0, 4:] = True
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.1)
    np.testing.assert_array_equal(out, expected)


def test_zero_thresholds_equal_plain_argmax():
    best = np.array([[WATER, GRAVEL, SKY], [GRAVEL, WATER, GRAVEL]])
    prob = np.zeros((2, 3))
    out = regions.combine(best, prob, [WATER], [SKY], 0.0, 0.0)
    np.testing.assert_array_equal(out, np.isin(best, [WATER, SKY]))


def test_empty_water_ids():
    best = np.array([[WATER, SKY]])
    out = regions.combine(best, np.ones((1, 2)), [], [SKY], 0.5, 0.02)
    np.testing.assert_array_equal(out, [[False, True]])


def test_empty_other_ids():
    best = np.array([[WATER, SKY]])
    out = regions.combine(best, np.ones((1, 2)), [WATER], [], 0.5, 0.02)
    np.testing.assert_array_equal(out, [[True, False]])
