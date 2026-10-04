"""Checks gnm_head.py's poses and geometric labels on the real GNM head.

    python tools/face-synth/test_gnm_head.py

Needs NumPy and the GNM head file: `VRFT_GNM`, else `.local/gnm/gnm_head.npz`
(render_face.py downloads it there). Skipped without it.
"""

import os
import sys
import unittest
from pathlib import Path

import numpy as np

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import gnm_head as gh  # noqa: E402

PATH = Path(os.environ.get("VRFT_GNM", HERE.parents[1] / ".local" / "gnm" / "gnm_head.npz"))


@unittest.skipUnless(PATH.is_file(), f"no GNM head at {PATH}")
class GnmHeadTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.gnm = gh.Gnm(PATH)
        cls.prototypes = gh.Prototypes(cls.gnm)
        cls.people = []
        for seed in range(3):
            identity = np.clip(np.random.default_rng(seed).normal(0, 1, cls.gnm.identity_dim), -2.5, 2.5)
            neutral = cls.gnm.neutral(identity.astype(np.float32))
            marks = gh.Landmarks(cls.gnm, neutral)
            deformers = gh.Deformers(cls.gnm, neutral, marks)
            gh.straight_out(cls.gnm, neutral, marks, cls.prototypes, deformers)
            cls.people.append((neutral, marks, deformers, cls.gnm.normals(neutral)))

    def measure(self, person, weights=None, deform=None):
        neutral, marks, deformers, normals = person
        posed = self.gnm.posed(neutral, self.prototypes.mix(weights or {}))
        posed = deformers.apply(posed, deform or {})
        return gh.labels(posed, neutral, np.eye(3), marks, normals), posed

    def test_arrays_are_float32_and_whitened_blocks_line_up(self):
        self.assertEqual(self.gnm.expression_basis.dtype, np.float32)
        self.assertEqual(self.gnm.expression_names[350], "tongue_mean")
        self.assertEqual(self.gnm.expression_names[gh.LOWER_FACE[0]], "lower_face_region_000")
        for vector in self.prototypes.vectors.values():
            self.assertLessEqual(np.linalg.norm(vector), 12.0 + 1e-3)

    def test_a_neutral_face_labels_nothing(self):
        for person in self.people:
            labels, posed = self.measure(person)
            self.assertEqual(labels["visibility"], 0.0)
            self.assertLess(labels["tongue_past_lips_mm"], -10)
            for name, value in labels.items():
                if name != "tongue_past_lips_mm":
                    self.assertAlmostEqual(value, 0.0, places=6, msg=name)
            self.assertEqual(gh.tongue_depth(posed, self.gnm.normals(posed), person[1]), 0.0)

    def test_each_brow_moves_on_its_own_side(self):
        for side, other in (("left", "right"), ("right", "left")):
            labels, _ = self.measure(self.people[0], {f"brow_inner_up_{side}": 1.5})
            self.assertGreater(labels[f"brow_inner_up_{side}"], 0.5)
            self.assertLess(labels[f"brow_inner_up_{other}"], 0.1)
            labels, _ = self.measure(self.people[0], {f"brow_lowerer_{side}": 1.5})
            self.assertGreater(labels[f"brow_lowerer_{side}"], 0.5)
            self.assertEqual(labels[f"brow_inner_up_{side}"], 0.0)

    def test_cheeks_and_jaw(self):
        labels, _ = self.measure(self.people[1], deform={"puff_left": 1.0})
        self.assertEqual((labels["cheek_puff_left"], labels["cheek_puff_right"]), (1.0, 0.0))
        labels, _ = self.measure(self.people[1], deform={"suck_left": 1.0, "suck_right": 1.0})
        self.assertEqual((labels["cheek_suck_left"], labels["cheek_suck_right"]), (1.0, 1.0))
        labels, _ = self.measure(self.people[1], {"jaw_open": 1.2})
        self.assertGreater(labels["jaw_open"], 0.4)
        self.assertEqual(labels["cheek_suck_left"], 0.0, "an open jaw isn't a suck")

    def test_the_tongue_reads_its_direction(self):
        for person in self.people:
            labels, _ = self.measure(person, gh.STRAIGHT_OUT, gh.STRAIGHT_STRETCH)
            self.assertEqual(labels["visibility"], 1.0)
            self.assertGreater(labels["extension"], 0.8)
            self.assertLess(abs(labels["horizontal"]), 0.2)
            self.assertLess(abs(labels["vertical"]), 0.2)
            right, _ = self.measure(person, gh.STRAIGHT_OUT, {"bend": 0.9, "stretch": 0.006})
            left, _ = self.measure(person, gh.STRAIGHT_OUT, {"bend": -0.9, "stretch": 0.006})
            self.assertGreater(right["horizontal"], 0.5)
            self.assertLess(left["horizontal"], -0.5)
            up, _ = self.measure(person, {**gh.STRAIGHT_OUT, "tongue_up": 0.7}, {"lift": 0.6, "stretch": 0.006})
            self.assertGreater(up["vertical"], 0.5)

    def test_the_head_sits_by_its_eyes(self):
        centres = self.gnm.eye_centres(np.zeros(self.gnm.identity_dim, np.float32))
        r, t = gh.placement(centres, 5.0, -3.0, 2.0, (0.0, -0.002, -0.02))
        np.testing.assert_allclose(r @ centres.mean(0) + t, (0.0, -0.002, -0.02), atol=1e-6)
        np.testing.assert_allclose(r @ r.T, np.eye(3), atol=1e-6)


if __name__ == "__main__":
    unittest.main()
