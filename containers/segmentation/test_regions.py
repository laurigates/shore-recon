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


# bridge


def test_bridge_fills_a_gap_narrower_than_the_disk():
    mask = grid(["###.###", "###.###", "###.###"])
    np.testing.assert_array_equal(regions.bridge(mask, 1.0), np.ones((3, 7), dtype=bool))


def test_bridge_keeps_a_wide_gap():
    mask = grid(["##.....##", "##.....##"])
    np.testing.assert_array_equal(regions.bridge(mask, 1.0), mask)


def test_bridge_does_not_shrink_at_the_border():
    mask = grid(["##..", "##..", "...."])
    out = regions.bridge(mask, 1.0)
    assert out[mask].all()


def test_bridge_of_a_full_mask_is_full():
    # scipy's edt of an array with no background is not a distance map.
    full = np.ones((3, 4), dtype=bool)
    np.testing.assert_array_equal(regions.bridge(full, 2.0), full)


def test_bridge_zero_radius_and_empty_mask():
    mask = grid(["#.#"])
    np.testing.assert_array_equal(regions.bridge(mask, 0.0), mask)
    np.testing.assert_array_equal(regions.bridge(np.zeros((2, 2), dtype=bool), 3.0), np.zeros((2, 2), dtype=bool))


# grow


def test_grow_adds_candidates_connected_to_a_seed():
    seed = grid(["....", "....", "####"])
    candidate = grid(["#...", "####", "####"])
    np.testing.assert_array_equal(regions.grow(seed, candidate, 0.0, np.zeros(seed.shape), 1.0), candidate)


def test_grow_drops_candidates_with_no_seed():
    seed = grid([".....", ".....", "#####"])
    candidate = grid(["#....", ".....", "#####"])
    np.testing.assert_array_equal(regions.grow(seed, candidate, 0.0, np.zeros(seed.shape), 1.0), seed)


def test_grow_bridges_a_thin_occluder_but_never_returns_it():
    # A railing bar (column 2) separates the candidate on the left from the seed.
    seed = grid(["...##", "...##"])
    candidate = grid(["##.##", "##.##"])
    np.testing.assert_array_equal(regions.grow(seed, candidate, 0.0, np.zeros(seed.shape), 1.0), seed)
    np.testing.assert_array_equal(regions.grow(seed, candidate, 1.0, np.zeros(seed.shape), 1.0), candidate)


def test_grow_keeps_a_confident_region_without_a_seed():
    seed = grid([".....", ".....", "#####"])
    candidate = grid(["##...", ".....", "#####"])
    prob = np.zeros((3, 5))
    prob[0, :2] = [0.8, 0.7]  # mean 0.75
    np.testing.assert_array_equal(regions.grow(seed, candidate, 0.0, prob, 0.75), candidate)
    np.testing.assert_array_equal(regions.grow(seed, candidate, 0.0, prob, 0.76), seed)


def test_grow_mean_counts_only_candidate_pixels():
    # The bridged gap pixel (column 1, prob 0) must not lower the region's mean.
    seed = np.zeros((1, 3), dtype=bool)
    candidate = grid(["#.#"])
    prob = np.array([[0.8, 0.0, 0.8]])
    np.testing.assert_array_equal(regions.grow(seed, candidate, 1.0, prob, 0.8), candidate)


def test_grow_rejects_mismatched_shapes():
    with pytest.raises(ValueError):
        regions.grow(np.zeros((2, 2), dtype=bool), np.zeros((2, 3), dtype=bool), 0.0, np.zeros((2, 2)), 1.0)


# combine

WATER, SKY, GRAVEL = 21, 2, 13


def test_low_prob_water_dropped_but_low_prob_sky_kept():
    # The low-prob water pixel touches the confident one only diagonally, so it is not grown back.
    best = np.array([[WATER, SKY], [SKY, WATER]])
    prob = np.array([[0.2, 0.2], [0.9, 0.9]])
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.0, 0.0, 1.0)
    np.testing.assert_array_equal(out, [[False, True], [True, True]])


def test_low_prob_water_touching_confident_water_is_grown_back():
    best = np.array([[WATER, SKY], [WATER, WATER]])
    prob = np.array([[0.2, 0.2], [0.9, 0.9]])
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.0, 0.0, 1.0)
    np.testing.assert_array_equal(out, [[True, True], [True, True]])


def test_inland_water_blob_dropped_bottom_water_kept():
    best = np.full((6, 6), GRAVEL)
    best[1, 1] = WATER  # wet gravel: small and away from the bottom
    best[4:, :] = WATER  # the sea along the bottom edge
    best[0, 4:] = SKY
    prob = np.ones((6, 6))
    prob[1, 1] = 0.6  # passes the cut but is too small, and not confident enough to seed itself
    expected = np.zeros((6, 6), dtype=bool)
    expected[4:, :] = True
    expected[0, 4:] = True
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.1, 0.0, 0.7)
    np.testing.assert_array_equal(out, expected)


def test_zero_thresholds_equal_plain_argmax():
    best = np.array([[WATER, GRAVEL, SKY], [GRAVEL, WATER, GRAVEL]])
    prob = np.zeros((2, 3))
    out = regions.combine(best, prob, [WATER], [SKY], 0.0, 0.0, 0.0, 1.0)
    np.testing.assert_array_equal(out, np.isin(best, [WATER, SKY]))


def test_water_split_by_the_probability_cut_is_grown_back():
    # Lauri's 00044 failure in miniature: the sea along the bottom, a seam whose
    # probability is under min_prob across it, and a small water region above.
    best = np.full((8, 8), GRAVEL)
    best[1:, 2:6] = WATER
    prob = np.full((8, 8), 0.9)
    prob[1:5, :] = 0.3  # the seam and the region above it
    out = regions.combine(best, prob, [WATER], [], 0.5, 0.5, 0.0, 1.0)
    expected = np.zeros((8, 8), dtype=bool)
    expected[1:, 2:6] = True
    np.testing.assert_array_equal(out, expected)


def test_low_prob_gravel_away_from_the_water_stays_dropped():
    best = np.full((10, 10), GRAVEL)
    best[0:2, 0:2] = WATER  # wet gravel, top left
    best[7:, :] = WATER  # the sea
    prob = np.full((10, 10), 0.9)
    prob[0:2, 0:2] = 0.3
    out = regions.combine(best, prob, [WATER], [], 0.5, 0.2, 0.2, 1.0)  # bridge 2 px
    expected = np.zeros((10, 10), dtype=bool)
    expected[7:, :] = True
    np.testing.assert_array_equal(out, expected)


def test_water_behind_a_railing_is_bridged():
    best = np.full((6, 10), GRAVEL)
    best[:, 0:4] = WATER  # low-prob water seen past the railing
    best[:, 5:] = WATER  # the sea, touching the bottom
    prob = np.full((6, 10), 0.9)
    prob[:, 0:4] = 0.3
    no_bridge = regions.combine(best, prob, [WATER], [], 0.5, 0.5, 0.0, 1.0)
    bridged = regions.combine(best, prob, [WATER], [], 0.5, 0.5, 0.1, 1.0)  # 1 px
    np.testing.assert_array_equal(no_bridge, (best == WATER) & (np.arange(10) >= 5))
    np.testing.assert_array_equal(bridged, best == WATER)


def test_confident_water_cut_off_at_the_top_is_kept_and_gravel_is_not():
    # Distant water beyond a dock (top right, confident) and wet gravel (top left, not).
    best = np.full((10, 10), GRAVEL)
    best[0:2, 0:3] = WATER
    best[0:2, 6:] = WATER
    best[6:, :] = WATER
    prob = np.full((10, 10), 0.9)
    prob[0:2, 0:3] = 0.4
    prob[0:2, 6:] = 0.8
    out = regions.combine(best, prob, [WATER], [], 0.5, 0.2, 0.0, 0.7)
    expected = best == WATER
    expected[0:2, 0:3] = False
    np.testing.assert_array_equal(out, expected)


def test_zero_seed_prob_equals_plain_argmax():
    best = np.array([[WATER, GRAVEL, SKY], [GRAVEL, WATER, GRAVEL]])
    prob = np.zeros((2, 3))
    out = regions.combine(best, prob, [WATER], [SKY], 0.5, 0.5, 0.0, 0.0)
    np.testing.assert_array_equal(out, np.isin(best, [WATER, SKY]))


def test_empty_water_ids():
    best = np.array([[WATER, SKY]])
    out = regions.combine(best, np.ones((1, 2)), [], [SKY], 0.5, 0.02, 0.0, 1.0)
    np.testing.assert_array_equal(out, [[False, True]])


def test_empty_other_ids():
    best = np.array([[WATER, SKY]])
    out = regions.combine(best, np.ones((1, 2)), [WATER], [], 0.5, 0.02, 0.0, 1.0)
    np.testing.assert_array_equal(out, [[True, False]])
