"""Renders synthetic Quest Pro mouth-camera recordings with Blender.

Run headless (see README.md):

    blender -b --factory-startup -P tools/tongue-synth/render_tongue.py -- --count 200

Writes a recording in the same `vrft-tongue-capture-v1` format the daemon
saves, under `.local/tongue-captures/<unixms>-synthetic-<pid>/`, so it shows
up in the Train tab beside real recordings. Every frame is a random face,
mouth and tongue with its exact label.

Faces are MakeHuman heads from the MPFB extension, varied in build, age,
ancestry and face shape, with MakeHuman's expression units moving the mouth.
The tongue is procedural, so its extension and direction are exact. The
scene is seen through the headset's own mouth cameras: their lens model and
their pose in the headset come from the headset's factory calibration
(`--calibration`), or from nominal values rounded from one Quest Pro.

The scene is built in the calibration's headset frame, in metres: x is the
person's left, y up and z forward, away from the face. The head is placed in
it by its eyes. Everything that moves with the head (tongue, mouth cavity)
is built in MPFB's own frame, where the face looks down -Y, +Z is up and the
person's left is +X, under the head's transform.
"""

import argparse
import importlib
import json
import math
import os
import re
import shutil
import sys
import tempfile
import time
from pathlib import Path

import addon_utils
import bpy
import numpy as np
from mathutils import Matrix, Vector
from mathutils.kdtree import KDTree

SIZE = 400
TARGETS = [
    "visibility", "extension", "horizontal", "vertical", "curl_up", "bend_down",
    "roll", "flat", "squish", "twist", "cheek_puff_left", "cheek_puff_right",
]
# Pinhole render that the fisheye views are resampled from: it covers the
# lens's corners (about 60 degrees off axis) at a little more than the
# lens's own resolution in the middle. `--fast` halves it: the model reads
# each view at 224 px, well under the lens's 400.
PINHOLE = 700
PINHOLE_TAN = 1.32
SAMPLES = 16
# Tongue: rings along the centreline, points around each ring.
T_ALONG, T_AROUND = 44, 28
JAW_MAX_DEGREES = 22.0
# How far a shown tongue's part past the lips may sink under the skin: lips
# wrapping it press in a few millimetres. Deeper, it's passing through a
# cheek, a lip or the chin, and the frame's pose is drawn again, up to
# TONGUE_ATTEMPTS times before settling for a tongue-in pose.
TONGUE_DEPTH = 0.004
TONGUE_ATTEMPTS = 8
# Headset frame from MPFB's frame, before the head's own rotation.
MPFB_TO_HEADSET = Matrix(((1, 0, 0), (0, 0, 1), (0, -1, 0)))
# How close the face may come to a camera's centre: the lens and its housing.
CLEARANCE = 0.02
# Where the eyeballs' centres sit in the headset frame on average: behind
# the lenses, 1.2 cm behind the eye-tracking calibration's eye box. A stereo
# fit of a real recording to the default MPFB head agreed to within a few mm.
EYES = (0.0, -0.002, -0.02)

# Rounded from one Quest Pro's /persist/calibration/ft_calib.scio.json, in
# its layout. The strip's left half is the camera on the person's left.
NOMINAL_CALIBRATION = [
    {
        "Id": "cam07_left_mouth", "ImageSize": [400, 400],
        "DeviceFromCamera": [-0.9389, -0.0645, -0.3382, 0.0300,
                             0.1150, 0.8671, -0.4847, -0.0437,
                             0.3245, -0.4940, -0.8066, 0.0392,
                             0, 0, 0, 1],
        "Projection": {"Model": "PinholeSymmetric", "Coefficients": [218.4, 197.9, 207.3]},
        "Distortion": {"Model": "Fisheye62Lut",
                       "Coefficients": [0.3579, -0.121, -0.1787, 0.854, -1.1574, 0.5015, -0.0004, 0.0109]},
        "HorizontalFlip": False, "VerticalFlip": True,
    },
    {
        "Id": "cam08_right_mouth", "ImageSize": [400, 400],
        "DeviceFromCamera": [-0.9456, 0.0611, 0.3194, -0.0302,
                             -0.1079, 0.8677, -0.4852, -0.0437,
                             -0.3068, -0.4933, -0.8140, 0.0391,
                             0, 0, 0, 1],
        "Projection": {"Model": "PinholeSymmetric", "Coefficients": [215.1, 204.1, 200.2]},
        "Distortion": {"Model": "Fisheye62Lut",
                       "Coefficients": [0.355, -0.1456, 0.2677, -0.4877, 0.3375, -0.0696, 0.0006, 0.0097]},
        "HorizontalFlip": True, "VerticalFlip": True,
    },
]
MOUTH_CAMERAS = ("cam07_left_mouth", "cam08_right_mouth")

# MakeHuman expression units that move the mouth (data/targets/expression/units).
UNITS = [
    "mouth-compression", "mouth-corner-puller", "mouth-depression", "mouth-depression-retraction",
    "mouth-elevation", "mouth-eversion", "mouth-open", "mouth-parling", "mouth-part-later",
    "mouth-protusion", "mouth-pursing", "mouth-retraction", "mouth-upward-retraction",
]
# Face-shape targets varied per person, as opposite pairs, with the spread
# of each folder's weights. The lip-middle pairs are left out: pushing one
# lip's middle into the other tears the mouth open.
SHAPE_FOLDERS = {"mouth": 0.18, "chin": 0.22, "cheek": 0.2, "nose": 0.22}
SHAPE_SKIP = ("mouth/mouth-lowerlip-middle", "mouth/mouth-upperlip-middle", "chin/chin-cleft")
OPPOSITES = [("decr", "incr"), ("down", "up"), ("in", "out"), ("backward", "forward"),
             ("compress", "uncompress"), ("concave", "convex")]


def smoothstep(edge0, edge1, x):
    t = np.clip((x - edge0) / (edge1 - edge0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


# ---------------------------------------------------------------- cameras

class LensCamera:
    """One mouth camera: Meta's Fisheye62 lens model and its pose in the headset.

    Its image is the stream's view after the calibration's flips, so the
    strip's left view comes out mirrored and the right one upright, as the
    headset sends them.
    """

    def __init__(self, entry):
        m = np.array(entry["DeviceFromCamera"], float).reshape(4, 4)
        u, _, vt = np.linalg.svd(m[:3, :3])
        self.R = u @ vt  # re-orthonormalised after rounding
        self.t = m[:3, 3].copy()
        self.f, self.cx, self.cy = entry["Projection"]["Coefficients"]
        coefficients = entry["Distortion"]["Coefficients"]
        self.k, self.p = np.array(coefficients[:6], float), np.array(coefficients[6:8], float)
        self.hflip, self.vflip = entry["HorizontalFlip"], entry["VerticalFlip"]

    def perturbed(self, rng):
        """This camera on another headset: small assembly differences."""
        other = LensCamera.__new__(LensCamera)
        other.__dict__.update({k: (v.copy() if isinstance(v, np.ndarray) else v) for k, v in self.__dict__.items()})
        axis = rng.normal(size=3)
        tilt = Matrix.Rotation(math.radians(rng.normal(0, 0.8)), 3, Vector(axis / np.linalg.norm(axis)))
        other.R = np.array(tilt) @ self.R
        other.t = self.t + rng.normal(0, 0.0008, 3)
        other.f = self.f * rng.normal(1.0, 0.01)
        other.cx, other.cy = self.cx + rng.normal(0, 2), self.cy + rng.normal(0, 2)
        return other

    def theta_d(self, theta):
        t2 = theta * theta
        return theta * (1.0 + sum(k * t2 ** (i + 1) for i, k in enumerate(self.k)))

    def pinhole_coords(self, u, v):
        """Stream pixels -> undistorted normalized coordinates (x/z, y/z)."""
        if self.hflip:
            u = SIZE - 1 - u
        if self.vflip:
            v = SIZE - 1 - v
        du, dv = (u - self.cx) / self.f, (v - self.cy) / self.f
        x, y = du.copy(), dv.copy()
        for _ in range(3):  # the tangential terms are tiny: fixed point
            r2 = x * x + y * y
            tmp = 2.0 * (x * self.p[0] + y * self.p[1])
            x = (du - r2 * self.p[0]) / (1 + tmp)
            y = (dv - r2 * self.p[1]) / (1 + tmp)
        theta_d = np.hypot(x, y)
        theta = theta_d.copy()
        for _ in range(8):  # Newton on the radial polynomial
            h = 1e-6
            slope = (self.theta_d(theta + h) - self.theta_d(theta - h)) / (2 * h)
            theta = theta - (self.theta_d(theta) - theta_d) / slope
        scale = np.where(theta_d > 1e-9, np.tan(theta) / np.maximum(theta_d, 1e-9), 1.0)
        return x * scale, y * scale

    def blender_matrix(self):
        """World matrix of a Blender camera looking down this camera's axis."""
        r = self.R @ np.diag([1.0, -1.0, -1.0])
        return Matrix([[*r[0], self.t[0]], [*r[1], self.t[1]], [*r[2], self.t[2]], [0, 0, 0, 1]])

    def warp(self):
        """For each stream pixel, where to sample the pinhole render, and its angle off axis."""
        v, u = np.mgrid[0:SIZE, 0:SIZE].astype(float)
        a, b = self.pinhole_coords(u.ravel(), v.ravel())
        focal = PINHOLE / 2 / PINHOLE_TAN
        x = PINHOLE / 2 + focal * a - 0.5
        y = PINHOLE / 2 + focal * b - 0.5
        return x, y, np.arctan(np.hypot(a, b)).reshape(SIZE, SIZE)


def load_cameras(path):
    entries = NOMINAL_CALIBRATION
    source = "nominal"
    if path and Path(path).is_file():
        entries = json.loads(Path(path).read_text())["CameraCalibration"]
        source = "headset"
    by_id = {e["Id"]: e for e in entries}
    return [LensCamera(by_id[name]) for name in MOUTH_CAMERAS], source


def sample_pinhole(image, warp):
    x, y, _ = warp
    x0 = np.clip(np.floor(x).astype(int), 0, PINHOLE - 2)
    y0 = np.clip(np.floor(y).astype(int), 0, PINHOLE - 2)
    fx, fy = np.clip(x - x0, 0, 1), np.clip(y - y0, 0, 1)
    top = image[y0, x0] * (1 - fx) + image[y0, x0 + 1] * fx
    bottom = image[y0 + 1, x0] * (1 - fx) + image[y0 + 1, x0 + 1] * fx
    return (top * (1 - fy) + bottom * fy).reshape(SIZE, SIZE)


# ---------------------------------------------------------------- sampling

def shape_axes(targets_dir):
    """Face-shape targets grouped into opposite pairs: {axis: (negative, positive, sigma)}."""
    axes = {}
    for folder, sigma in SHAPE_FOLDERS.items():
        names = {p.name[:-len(".target.gz")] for p in (targets_dir / folder).glob("*.target.gz")}
        for name in sorted(names):
            for neg, pos in OPPOSITES:
                if name.endswith("-" + neg) and name[:-len(neg)] + pos in names:
                    base = name[:-len(neg) - 1]
                    if f"{folder}/{base}" not in SHAPE_SKIP:
                        axes[f"{folder}/{base}"] = (f"{folder}/{name}", f"{folder}/{base}-{pos}", sigma)
    return axes


def sample_identity(rng, axes):
    """What stays fixed for one synthetic person, and where the headset sits on them."""
    race = rng.dirichlet([0.6, 0.6, 0.6])
    shape, shared = {}, {}
    for axis, (neg, pos, sigma) in axes.items():
        # Left and right cheeks move together, with a little asymmetry.
        key = re.sub(r"^cheek/[lr]-", "cheek/", axis)
        if key not in shared:
            shared[key] = rng.normal(0, sigma)
        value = shared[key] + (rng.normal(0, 0.05) if key != axis else 0.0)
        value = float(np.clip(value, -0.5, 0.5))
        if abs(value) > 0.03:
            shape[pos if value > 0 else neg] = abs(value)
    skin = float(rng.uniform(0.4, 0.75))
    return {
        "macros": {
            "gender": float(rng.uniform(0, 1)), "age": float(rng.uniform(0.42, 0.85)),
            "muscle": float(rng.uniform(0.25, 0.8)), "weight": float(rng.uniform(0.2, 0.85)),
            "proportions": float(rng.uniform(0.3, 0.8)), "height": float(rng.uniform(0.35, 0.7)),
            "cupsize": 0.5, "firmness": 0.5,
            "race": {"african": float(race[0]), "asian": float(race[1]), "caucasian": float(race[2])},
        },
        "shape": shape,
        "skin": skin,
        "lip_tint": float(rng.uniform(0.8, 1.0)),
        "stubble": float(rng.uniform(0.25, 1.0)) if rng.random() < 0.5 else 0.0,
        "mottling": float(rng.uniform(0.05, 0.2)),
        # Enamel is only a little brighter than lips in near infrared.
        "teeth": float(rng.uniform(0.45, 0.9)),
        "teeth_gaps": float(rng.uniform(0.35, 0.6)),
        "stubble_scale": float(rng.uniform(1800.0, 3500.0)),
        # Most fabric dyes reflect near infrared, whatever their colour.
        "cloth": float(rng.uniform(0.1, 0.8)),
        "cloth_folds": float(rng.uniform(0.0, 1.0)),
        "skin_roughness": float(rng.uniform(0.5, 0.7)),
        "lip_roughness": float(rng.uniform(0.3, 0.55)),
        # How wet the tongue is: many small glints rather than one sheen.
        "wetness": float(rng.uniform(0.3, 1.0)),
        "papillae": float(rng.uniform(0.15, 0.4)),
        # Sometimes an even gloss over the whole tongue instead of patches.
        "even_coat": bool(rng.random() < 0.4),
        # Out of the mouth, 40-60% of the lips' corner-to-corner width.
        "tongue_width": float(rng.uniform(0.036, 0.056)),
        "tongue_length": float(rng.uniform(0.8, 1.2)),
        "tongue_droop": float(rng.uniform(0.0, 0.9)),
        # How far past the lips a fully out tongue reaches (metres), and its
        # thickness for its width.
        "tongue_reach": float(rng.uniform(0.022, 0.034)),
        "tongue_bulk": float(rng.uniform(0.24, 0.3)),
        "tongue_thickness": float(rng.uniform(0.8, 1.25)),
        "tongue_tip": float(rng.uniform(0.7, 1.4)),
        "tongue_groove": float(rng.uniform(0.04, 0.2)),
        # In near infrared the tongue is about as bright as the lips, a
        # little darker than the skin around them.
        "tongue_albedo": skin * float(rng.uniform(0.6, 1.05)),
        # The headset on this face: the head's tilt in it and where the eyes sit.
        "pitch": float(rng.normal(-4.0, 6.0)), "yaw": float(rng.normal(0.0, 2.5)),
        "roll": float(rng.normal(0.0, 3.0)),
        "eyes": [float(EYES[0] + rng.normal(0, 0.002)), float(EYES[1] + rng.normal(0, 0.004)),
                 float(EYES[2] + rng.normal(0, 0.004))],
    }


def sample_sensor(rng):
    """Illuminators and the camera's own look. The ranges bracket a real
    recording's brightness percentiles, shadow levels and fine contrast, and
    reach past them: training on the harsher look alone transferred as well."""
    return {
        # Offsets of the illuminators from the cameras in the headset frame
        # (outward, up, forward): a fit of the default head to a real
        # recording's brightness put them about 1.2 cm below the cameras.
        "light_offsets": [[float(rng.normal(0, 0.004)), float(rng.normal(-0.012, 0.003)),
                           float(rng.normal(0, 0.003))] for _ in range(2)],
        "light_cone": float(rng.uniform(130.0, 160.0)),
        "light_blend": float(rng.uniform(0.7, 1.0)),
        "light_balance": float(rng.uniform(0.8, 1.2)),
        # Stray infrared from the room and the headset's own light bouncing
        # around: real frames never fall to black below the chin.
        "ambient": float(rng.uniform(0.0, 0.12)),
        "brightness": float(rng.uniform(150.0, 250.0)),  # auto-exposure's 99th percentile
        # The camera's response lifts shadows.
        "gamma": float(rng.uniform(0.6, 1.1)),
        "vignette": float(rng.uniform(0.5, 1.5)),
        "blur": float(rng.uniform(0.3, 1.8)),
        # Light scattered inside the lens, as a share of the local average.
        "glare": float(rng.uniform(0.0, 0.12)),
        "dark": float(rng.uniform(3.0, 8.0)),
        "read_noise": float(rng.uniform(0.6, 3.0)),
        "shot_noise": float(rng.uniform(0.05, 0.8)),
    }


def speech(rng):
    return {u: float(rng.uniform(0, 0.5)) for u in rng.choice(
        ["mouth-parling", "mouth-pursing", "mouth-retraction", "mouth-depression",
         "mouth-elevation", "mouth-compression", "mouth-eversion", "mouth-protusion"], 3, replace=False)}


def vowel(rng):
    """ee, ah or oh: the jaw and the mouth's units."""
    which = rng.choice(["ee", "ah", "oh"])
    if which == "ee":
        return rng.uniform(0.05, 0.2), {"mouth-retraction": rng.uniform(0.4, 1.0),
                                        "mouth-corner-puller": rng.uniform(0.0, 0.3)}
    if which == "ah":
        return rng.uniform(0.5, 0.9), {"mouth-parling": rng.uniform(0.2, 0.6)}
    return rng.uniform(0.3, 0.55), {"mouth-pursing": rng.uniform(0.4, 0.9), "mouth-protusion": rng.uniform(0.2, 0.7)}


def direction(rng, straight=0.2):
    """A tongue direction spread evenly over the disc, sometimes straight out."""
    if rng.random() < straight:
        return 0.0, 0.0
    r, a = math.sqrt(rng.random()), rng.uniform(0, 2 * math.pi)
    return r * math.cos(a), r * math.sin(a)


def hidden(name, jaw, units=None, **extra):
    return dict(kind="hidden", pose=name, jaw=jaw, units=units or {}, **extra)


def shown(name, jaw, ext, h, v, units=None):
    return dict(kind="visible", pose=name, jaw=jaw, ext=ext, h=h, v=v, units=units or {})


def tongue_out(rng):
    h, v = direction(rng)
    # Anywhere from just as far as the tongue needs, the lips closing round
    # it as they mostly do in real recordings, to wide open: training on
    # wide-open mouths alone transferred better than on closed ones alone.
    jaw = max(0.08, rng.uniform(0.12, 0.75) - 0.1 * max(0.0, v) + 0.1 * max(0.0, -v))
    return shown("Tongue out", jaw, rng.uniform(0.15, 1.0), h, v,
                 {"mouth-parling": rng.uniform(0.0, 0.7), "mouth-corner-puller": rng.uniform(0.0, 0.2),
                  "mouth-depression": rng.uniform(0.0, 0.3)})


def to_corner(rng):
    side = rng.choice([-1.0, 1.0])
    return shown("Tongue to a corner", rng.uniform(0.2, 0.45), rng.uniform(0.4, 0.8),
                 side * rng.uniform(0.7, 1.0), rng.uniform(-0.3, 0.3), {"mouth-parling": rng.uniform(0.0, 0.4)})


def puffed(name, left, right, rng):
    return hidden(name, 0.0, {"mouth-compression": rng.uniform(0.2, 0.7), "mouth-pursing": rng.uniform(0.0, 0.3)},
                  puff_left=left, puff_right=right)


# What a frame can show, as in the app's capture poses and more, with how
# often each is drawn. Training balances hidden-tongue and cheek frames by
# pose name and tongue-out frames by direction, so these weights set how
# many distinct examples each gets rather than its share of training.
SCENARIOS = [
    # Tongue hidden: the hard negatives.
    (0.03, lambda r: hidden("Neutral", r.uniform(0.0, 0.08), {"mouth-compression": r.uniform(0.0, 0.2)})),
    (0.04, lambda r: hidden("Natural speech", r.uniform(0.05, 0.5), speech(r))),
    (0.03, lambda r: hidden("Vowels", *vowel(r))),
    (0.03, lambda r: hidden("Smile", r.uniform(0.0, 0.3), {"mouth-corner-puller": r.uniform(0.5, 1.0),
                                                           "mouth-upward-retraction": r.uniform(0.0, 0.5)})),
    (0.02, lambda r: hidden("Slight smile", r.uniform(0.0, 0.05), {"mouth-corner-puller": r.uniform(0.15, 0.45)})),
    (0.03, lambda r: hidden("Teeth bared", r.uniform(0.0, 0.15), {"mouth-part-later": r.uniform(0.5, 1.0),
                                                                  "mouth-corner-puller": r.uniform(0.0, 0.4)})),
    (0.02, lambda r: hidden("Upper teeth showing", r.uniform(0.0, 0.15), {"mouth-elevation": r.uniform(0.5, 1.0)})),
    (0.02, lambda r: hidden("Lower teeth showing", r.uniform(0.05, 0.2),
                            {"mouth-depression-retraction": r.uniform(0.5, 1.0),
                             "mouth-depression": r.uniform(0.0, 0.5)})),
    (0.03, lambda r: hidden("Jaw open", r.uniform(0.5, 1.0), {"mouth-parling": r.uniform(0.0, 0.5)})),
    (0.03, lambda r: hidden("Pucker", r.uniform(0.0, 0.08), {"mouth-pursing": r.uniform(0.5, 1.0),
                                                             "mouth-protusion": r.uniform(0.0, 0.6)})),
    (0.02, lambda r: hidden("Lips pressed", 0.0, {"mouth-compression": r.uniform(0.6, 1.0)})),
    (0.02, lambda r: hidden("Pout or frown", r.uniform(0.0, 0.1),
                            {str(r.choice(["mouth-eversion", "mouth-depression"])): r.uniform(0.4, 1.0)})),
    (0.03, lambda r: hidden("Tongue in left cheek", 0.0, {"mouth-compression": r.uniform(0.1, 0.4)},
                            bulge_left=r.uniform(0.5, 1.0))),
    (0.03, lambda r: hidden("Tongue in right cheek", 0.0, {"mouth-compression": r.uniform(0.1, 0.4)},
                            bulge_right=r.uniform(0.5, 1.0))),
    (0.02, lambda r: hidden("Cheeks sucked in", r.uniform(0.0, 0.1), {"mouth-pursing": r.uniform(0.2, 0.6)},
                            suck=r.uniform(0.5, 1.0))),
    # Tongue out.
    (0.24, tongue_out),
    (0.05, lambda r: shown("Tongue tip", r.uniform(0.1, 0.3), r.uniform(0.15, 0.35), r.normal(0, 0.1),
                           r.normal(0, 0.1), {"mouth-parling": r.uniform(0.0, 0.3)})),
    (0.04, lambda r: shown("Tongue tip, jaw wide", r.uniform(0.75, 1.0), r.uniform(0.15, 0.35), 0.0, 0.0,
                           {"mouth-parling": r.uniform(0.2, 0.6)})),
    (0.04, lambda r: shown("Tongue out, smiling", r.uniform(0.2, 0.7), r.uniform(0.5, 1.0), *direction(r, 0.6),
                           {"mouth-corner-puller": r.uniform(0.5, 1.0),
                            "mouth-upward-retraction": r.uniform(0.0, 0.4)})),
    (0.04, to_corner),
    # Cheeks puffed, tongue in.
    (0.06, lambda r: puffed("Left cheek puffed", r.uniform(0.45, 1.0), 0.0, r)),
    (0.06, lambda r: puffed("Right cheek puffed", 0.0, r.uniform(0.45, 1.0), r)),
    (0.06, lambda r: puffed("Both cheeks puffed", r.uniform(0.45, 1.0), r.uniform(0.45, 1.0), r)),
]


def sample_frame(rng):
    """One frame's expression, tongue and label."""
    weights = np.array([w for w, _ in SCENARIOS])
    index = int(rng.choice(len(SCENARIOS), p=weights / weights.sum()))
    frame = {"jaw": 0.0, "units": {}, "ext": 0.0, "h": 0.0, "v": 0.0, "curl": 0.0, "puff_left": 0.0,
             "puff_right": 0.0, "bulge_left": 0.0, "bulge_right": 0.0, "suck": 0.0}
    frame.update(SCENARIOS[index][1](rng))
    frame = {k: (float(v) if isinstance(v, (float, np.floating)) else v) for k, v in frame.items()}
    frame["pose"] = "Synthetic: " + frame["pose"]
    frame["step"] = index
    frame["units"] = {k: round(float(v), 4) for k, v in frame["units"].items()}
    visible = frame["kind"] == "visible"
    targets = [0.0] * len(TARGETS)
    targets[0] = 1.0 if visible else 0.0
    targets[1] = frame["ext"] if visible else 0.0
    targets[2] = frame["h"] if visible else 0.0
    targets[3] = frame["v"] if visible else 0.0
    targets[10] = frame["puff_left"]
    targets[11] = frame["puff_right"]
    frame["targets"] = [round(float(t), 5) for t in targets]
    return frame


# ---------------------------------------------------------------- materials

def material(name, color, roughness):
    mat = bpy.data.materials.new(name)
    if bpy.app.version < (5, 0, 0):
        mat.use_nodes = True
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    bsdf = next(n for n in nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = (color, color, color, 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    return mat, bsdf, nodes, links


def skin_material(identity, metres_per_unit):
    """Skin with lips, pores and stubble, read from the `lip` and `beard` attributes."""
    mat, bsdf, nodes, links = material("Skin", identity["skin"], identity["skin_roughness"])
    bsdf.inputs["Specular IOR Level"].default_value = 0.3
    lip = nodes.new("ShaderNodeAttribute")
    lip.attribute_name = "lip"
    beard = nodes.new("ShaderNodeAttribute")
    beard.attribute_name = "beard"
    coords = nodes.new("ShaderNodeTexCoord")
    metres = nodes.new("ShaderNodeVectorMath")
    metres.operation = "SCALE"
    metres.inputs["Scale"].default_value = metres_per_unit
    links.new(coords.outputs["Object"], metres.inputs[0])
    # Uneven skin: a few millimetres of mottling.
    blotches = nodes.new("ShaderNodeTexNoise")
    blotches.inputs["Scale"].default_value = 250.0
    links.new(metres.outputs["Vector"], blotches.inputs["Vector"])
    s = identity["skin"]
    uneven = nodes.new("ShaderNodeMapRange")
    uneven.inputs["To Min"].default_value = s * (1 - identity["mottling"])
    uneven.inputs["To Max"].default_value = s * (1 + identity["mottling"])
    links.new(blotches.outputs["Fac"], uneven.inputs["Value"])
    tint = nodes.new("ShaderNodeMix")
    tint.data_type = "RGBA"
    links.new(uneven.outputs["Result"], tint.inputs["A"])
    t = s * identity["lip_tint"]
    tint.inputs["B"].default_value = (t, t, t, 1.0)
    links.new(lip.outputs["Fac"], tint.inputs["Factor"])
    links.new(tint.outputs["Result"], bsdf.inputs["Base Color"])
    rough = nodes.new("ShaderNodeMapRange")
    rough.inputs["To Min"].default_value = identity["skin_roughness"]
    rough.inputs["To Max"].default_value = identity["lip_roughness"]
    links.new(lip.outputs["Fac"], rough.inputs["Value"])
    links.new(rough.outputs["Result"], bsdf.inputs["Roughness"])
    # Pores as a fine bump.
    noise = nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 900.0
    links.new(metres.outputs["Vector"], noise.inputs["Vector"])
    bump = nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.3
    bump.inputs["Distance"].default_value = 0.0003
    links.new(noise.outputs["Fac"], bump.inputs["Height"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    if identity["stubble"] > 0:
        # Stubble: dark dots where the beard grows.
        dots = nodes.new("ShaderNodeTexVoronoi")
        dots.inputs["Scale"].default_value = identity["stubble_scale"]
        links.new(metres.outputs["Vector"], dots.inputs["Vector"])
        ramp = nodes.new("ShaderNodeMapRange")
        ramp.inputs["From Min"].default_value = 0.0
        ramp.inputs["From Max"].default_value = 0.25
        ramp.inputs["To Min"].default_value = 1.0 - 0.6 * identity["stubble"]
        ramp.inputs["To Max"].default_value = 1.0
        links.new(dots.outputs["Distance"], ramp.inputs["Value"])
        where = nodes.new("ShaderNodeMix")
        where.data_type = "FLOAT"
        where.inputs["A"].default_value = 1.0
        links.new(beard.outputs["Fac"], where.inputs["Factor"])
        links.new(ramp.outputs["Result"], where.inputs["B"])
        darken = nodes.new("ShaderNodeMix")
        darken.data_type = "RGBA"
        darken.blend_type = "MULTIPLY"
        darken.inputs["Factor"].default_value = 1.0
        links.new(tint.outputs["Result"], darken.inputs["A"])
        links.new(where.outputs["Result"], darken.inputs["B"])
        links.new(darken.outputs["Result"], bsdf.inputs["Base Color"])
    try:
        bsdf.inputs["Subsurface Weight"].default_value = 0.3
        bsdf.inputs["Subsurface Radius"].default_value = (0.004, 0.004, 0.004)
    except KeyError:
        pass
    return mat


def cloth_material(albedo, folds_strength, metres_per_unit):
    """A shirt with folds a few centimetres across."""
    mat, bsdf, nodes, links = material("Cloth", albedo, 0.9)
    coords = nodes.new("ShaderNodeTexCoord")
    metres = nodes.new("ShaderNodeVectorMath")
    metres.operation = "SCALE"
    metres.inputs["Scale"].default_value = metres_per_unit
    links.new(coords.outputs["Object"], metres.inputs[0])
    folds = nodes.new("ShaderNodeTexNoise")
    folds.inputs["Scale"].default_value = 25.0
    folds.inputs["Detail"].default_value = 3.0
    links.new(metres.outputs["Vector"], folds.inputs["Vector"])
    bump = nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = folds_strength
    bump.inputs["Distance"].default_value = 0.01
    links.new(folds.outputs["Fac"], bump.inputs["Height"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    return mat


def teeth_material(albedo, gaps_shade):
    """Enamel with a dark gap every 8 mm or so across the arc."""
    mat, bsdf, nodes, links = material("Teeth", albedo, 0.25)
    coords = nodes.new("ShaderNodeTexCoord")
    bands = nodes.new("ShaderNodeTexWave")
    bands.wave_type = "BANDS"
    bands.bands_direction = "X"
    bands.inputs["Scale"].default_value = 39.0  # 20 / (2 pi) cycles per unit: an 8 mm period
    links.new(coords.outputs["Object"], bands.inputs["Vector"])
    gaps = nodes.new("ShaderNodeMapRange")
    gaps.inputs["From Min"].default_value = 0.85
    gaps.inputs["From Max"].default_value = 1.0
    gaps.inputs["To Min"].default_value = albedo
    gaps.inputs["To Max"].default_value = albedo * gaps_shade
    links.new(bands.outputs["Fac"], gaps.inputs["Value"])
    links.new(gaps.outputs["Result"], bsdf.inputs["Base Color"])
    return mat


def wet_material(name, albedo, bump_scale, wetness, papillae, even_coat):
    """Wet tissue: papillae-like bumps (object space, metres) under patches of
    saliva, whose highlights break into small glints."""
    mat, bsdf, nodes, links = material(name, albedo, 0.6)
    try:
        bsdf.inputs["Coat Roughness"].default_value = 0.06
        bsdf.inputs["Subsurface Weight"].default_value = 0.3
        bsdf.inputs["Subsurface Radius"].default_value = (0.003, 0.003, 0.003)
    except KeyError:
        pass
    coords = nodes.new("ShaderNodeTexCoord")
    dots = nodes.new("ShaderNodeTexVoronoi")
    dots.inputs["Scale"].default_value = bump_scale
    links.new(coords.outputs["Object"], dots.inputs["Vector"])
    noise = nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = bump_scale / 8
    links.new(coords.outputs["Object"], noise.inputs["Vector"])
    height = nodes.new("ShaderNodeMath")
    height.operation = "ADD"
    links.new(dots.outputs["Distance"], height.inputs[0])
    links.new(noise.outputs["Fac"], height.inputs[1])
    bump = nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = papillae
    bump.inputs["Distance"].default_value = 0.0003
    links.new(height.outputs["Value"], bump.inputs["Height"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    # Saliva: glossy patches a millimetre or two across, following the
    # papillae, so the coat's highlights scatter into glints.
    film = nodes.new("ShaderNodeTexNoise")
    film.inputs["Scale"].default_value = bump_scale / 4
    film.inputs["Detail"].default_value = 6.0
    links.new(coords.outputs["Object"], film.inputs["Vector"])
    patches = nodes.new("ShaderNodeMapRange")
    patches.inputs["From Min"].default_value = 0.5
    patches.inputs["From Max"].default_value = 0.62
    patches.inputs["To Min"].default_value = 0.0
    patches.inputs["To Max"].default_value = wetness
    links.new(film.outputs["Fac"], patches.inputs["Value"])
    if "Coat Weight" in bsdf.inputs:
        if even_coat:
            bsdf.inputs["Coat Weight"].default_value = 0.3
            bsdf.inputs["Coat Roughness"].default_value = 0.15
        else:
            links.new(patches.outputs["Result"], bsdf.inputs["Coat Weight"])
            links.new(bump.outputs["Normal"], bsdf.inputs["Coat Normal"])
    # Patchy albedo, as the tongue's coating is.
    patches = nodes.new("ShaderNodeTexNoise")
    patches.inputs["Scale"].default_value = 180.0
    links.new(coords.outputs["Object"], patches.inputs["Vector"])
    shade = nodes.new("ShaderNodeMapRange")
    shade.inputs["To Min"].default_value = albedo * 0.8
    shade.inputs["To Max"].default_value = albedo * 1.2
    links.new(patches.outputs["Fac"], shade.inputs["Value"])
    links.new(shade.outputs["Result"], bsdf.inputs["Base Color"])
    return mat


# ---------------------------------------------------------------- geometry

def mesh_object(name, verts, faces, mat, parent, smooth=True, subdivide=1):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([tuple(v) for v in verts], [], faces)
    mesh.polygons.foreach_set("use_smooth", [smooth] * len(mesh.polygons))
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    obj.data.materials.append(mat)
    obj.parent = parent
    if subdivide:
        mod = obj.modifiers.new("Subdivision", "SUBSURF")
        mod.levels = mod.render_levels = subdivide
    return obj


def grid_faces(rows, cols, wrap):
    """Quads over a rows x cols grid, wrapping the columns if `wrap`."""
    faces = []
    for i in range(rows - 1):
        for j in range(cols if wrap else cols - 1):
            k = (j + 1) % cols
            faces.append((i * cols + j, i * cols + k, (i + 1) * cols + k, (i + 1) * cols + j))
    return faces


def ellipsoid(name, center, radii, mat, parent, flip=False, segments=32, rings=16):
    verts = []
    for i in range(rings + 1):
        phi = math.pi * i / rings
        for j in range(segments):
            th = 2 * math.pi * j / segments
            verts.append((
                center[0] + radii[0] * math.sin(phi) * math.cos(th),
                center[1] + radii[1] * math.sin(phi) * math.sin(th),
                center[2] + radii[2] * math.cos(phi),
            ))
    faces = grid_faces(rings + 1, segments, True)
    if flip:
        faces = [tuple(reversed(f)) for f in faces]
    return mesh_object(name, verts, faces, mat, parent)


class Tongue:
    """A tongue swept along a centreline that leaves the mouth and bends.

    Built in a mouth frame (origin just behind the lips' front at the lip
    line, MPFB's axes) and carried by the jaw.
    """

    def __init__(self, identity, mat, mouth, mouth_width, lip_top, jaw, parent):
        self.id = identity
        self.mouth = np.array(mouth)
        self.mouth_width = mouth_width
        # Out of the mouth it rests on the lower lip: its centre crosses the
        # lips' front half a tongue above the lip's top edge.
        self.root_z = lip_top + 0.009 + 0.064 * math.tan(0.12)
        self.jaw = jaw
        faces = grid_faces(T_ALONG, T_AROUND, True)
        self.points = self.positions(0, 0, 0, 0, 0)
        self.obj = mesh_object("Tongue", self.points, faces, mat, parent)

    def positions(self, ext, h, v, curl, jaw, hidden=True):
        """The tongue's vertices in MPFB's frame. Also sets `outside`: which
        of them belong to the part that should be past the lips."""
        i = self.id
        root = np.array([0.0, 0.052, self.root_z])
        if hidden:
            # Lying low in the mouth, its tip behind the lower teeth.
            root[2] -= 0.02
            length = 0.045 * i["tongue_length"]
        else:
            # Fully out, 2-3 cm past the lips' front, "just the tip" (0.25)
            # a few millimetres, as in the capture poses; a tongue turned
            # sideways or up reaches less far past the lips.
            reach = 1 - 0.45 * abs(h) - 0.35 * max(v, 0.0) + 0.2 * max(-v, 0.0)
            length = 0.062 + i["tongue_reach"] * ext * i["tongue_length"] * reach
        s = np.linspace(0.0, 1.0, T_ALONG)
        # Where the centreline passes the lips' front.
        inside = 0.065 / length
        past = np.zeros(T_ALONG, bool) if hidden else s > inside + 0.003 / length
        self.outside = np.repeat(past, T_AROUND)
        # Horizontal +1 is toward the person's right, -X. Sideways, the whole
        # tongue swings toward that corner from just inside the lips; up, it
        # curls up in front of the upper lip once it is out; down, it tips
        # over the lower lip and hangs.
        yaw = h * 1.3 * smoothstep(inside - 0.3, inside + 0.15, s)
        rise = v * 1.7 * smoothstep(inside - 0.12, inside + 0.1, s)
        drop = v * 1.5 * smoothstep(inside - 0.1, inside + 0.12, s)
        # Straight out, it droops over the lower lip rather than pointing at
        # the cameras.
        droop = i["tongue_droop"] * (1 - abs(v)) * smoothstep(inside - 0.05, inside + 0.35, s)
        pitch = (np.where(v > 0, rise, drop) + curl * 1.2 * smoothstep(0.7, 1.0, s) - 0.12 * (1 - abs(v))
                 - droop)
        tangent = np.stack([
            -np.sin(yaw) * np.cos(pitch),
            -np.cos(yaw) * np.cos(pitch),
            np.sin(pitch),
        ], axis=1)
        step = length / (T_ALONG - 1)
        centre = root + np.vstack([[0, 0, 0], np.cumsum(tangent[:-1] * step, axis=0)])
        # It leaves the mouth toward the corner it points at.
        centre[:, 0] -= h * 0.45 * self.mouth_width * smoothstep(inside - 0.4, inside, s)
        # Parallel-transported side vector.
        side = np.zeros_like(tangent)
        prev = np.array([1.0, 0.0, 0.0])
        for k in range(T_ALONG):
            prev = prev - prev.dot(tangent[k]) * tangent[k]
            prev /= np.linalg.norm(prev)
            side[k] = prev
        up = np.cross(tangent, side)
        # Pushed out, and more so pointed down, it narrows and lengthens.
        width = i["tongue_width"] * (1 - 0.15 * s) * (1 - 0.1 * ext) * (1 - 0.2 * max(-v, 0.0))
        from_tip = (1 - s) * length
        tip = 0.02 * i["tongue_tip"]
        rounding = np.sqrt(np.clip(1 - ((tip - np.minimum(from_tip, tip)) / tip) ** 2, 0.0, 1.0))
        width = width * rounding
        thick = width * i["tongue_bulk"] * (1 + ext / 3) * i["tongue_thickness"]
        phi = np.linspace(0, 2 * np.pi, T_AROUND, endpoint=False)
        cos, sin = np.cos(phi), np.sin(phi)
        top = np.where(sin > 0, 0.85, 1.0)
        # A shallow groove down the middle of the top.
        groove = i["tongue_groove"] * np.exp(-(cos / 0.25) ** 2) * (sin > 0)
        lateral = (width[:, None] / 2) * cos[None, :]
        vertical = (thick[:, None] / 2) * (sin * top - groove)[None, :]
        points = (centre[:, None, :] + side[:, None, :] * lateral[..., None]
                  + up[:, None, :] * vertical[..., None]).reshape(-1, 3) + self.mouth
        # It rides the lower jaw as the lower lip does.
        pivot, axis, follow = self.jaw
        turn = np.array(Matrix.Rotation(math.radians(jaw * JAW_MAX_DEGREES) * follow, 3, Vector(axis)))
        return (points - pivot) @ turn.T + pivot

    def update(self, frame):
        self.points = self.positions(frame["ext"], frame["h"], frame["v"], frame["curl"], frame["jaw"],
                                     hidden=frame["kind"] != "visible")
        self.obj.data.vertices.foreach_set("co", self.points.astype(np.float32).ravel())
        self.obj.data.update()


class Teeth:
    """Upper and lower front teeth as arcs behind the lips (MPFB's helper
    teeth sit too low and too far forward for this). The lower arc turns
    with the jaw."""

    def __init__(self, mat, mouth, jaw, parent):
        self.mouth = np.array(mouth)
        self.jaw = jaw
        # Mouth frame: the lips' front is 1.2 cm ahead of its origin.
        self.upper = self.arc("Upper teeth", 0.006, -0.002, -0.001, mat, parent)
        self.lower = self.arc("Lower teeth", -0.004, -0.012, 0.002, mat, parent)
        self.lower_base = np.array([v.co[:] for v in self.lower.data.vertices])

    def arc(self, name, top, bottom, front, mat, parent):
        verts = []
        steps = 24
        for k in range(steps):
            a = math.radians(-70 + 140 * k / (steps - 1))
            for z in (top, bottom):
                verts.append(self.mouth + (0.022 * math.sin(a), front + 0.022 * (1 - math.cos(a)), z))
        faces = [(2 * k, 2 * k + 2, 2 * k + 3, 2 * k + 1) for k in range(steps - 1)]
        obj = mesh_object(name, verts, faces, mat, parent, subdivide=0)
        solid = obj.modifiers.new("Thickness", "SOLIDIFY")
        solid.thickness = 0.004
        return obj

    def update(self, frame):
        pivot, axis, _ = self.jaw
        turn = np.array(Matrix.Rotation(math.radians(frame["jaw"] * JAW_MAX_DEGREES), 3, Vector(axis)))
        p = (self.lower_base - pivot) @ turn.T + pivot
        self.lower.data.vertices.foreach_set("co", p.astype(np.float32).ravel())
        self.lower.data.update()


# ---------------------------------------------------------------- scene

def enable_mpfb():
    for module in addon_utils.modules():
        if module.__name__.split(".")[-1] == "mpfb":
            addon_utils.enable(module.__name__, default_set=True)
            return module.__name__, Path(module.__file__).parent
    sys.exit("tongue-synth: MPFB isn't installed. Add it in Blender under Get Extensions (search MPFB).")


def clear_scene():
    for obj in list(bpy.data.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    for collection in (bpy.data.meshes, bpy.data.armatures, bpy.data.materials, bpy.data.lights,
                       bpy.data.cameras, bpy.data.images, bpy.data.node_groups, bpy.data.worlds):
        for block in list(collection):
            collection.remove(block)


class Head:
    """One MPFB person, posed per frame, placed in the headset frame."""

    def __init__(self, identity, mpfb, targets_dir):
        services = importlib.import_module(mpfb + ".services")
        self.targets = services.TargetService
        human = services.HumanService.create_human(
            mask_helpers=False, feet_on_ground=True, scale=0.1, macro_detail_dict=identity["macros"])
        for name, weight in identity["shape"].items():
            self.targets.load_target(human, str(targets_dir / f"{name}.target.gz"), weight=weight,
                                     name=name.split("/")[-1])
        rig = services.HumanService.add_builtin_rig(human, "default")
        self.human, self.rig = human, rig
        race = max(identity["macros"]["race"], key=identity["macros"]["race"].get)
        self.units = {}
        for unit in UNITS:
            path = targets_dir / "expression" / "units" / race / f"{unit}.target.gz"
            if path.is_file():
                self.units[unit] = self.targets.load_target(human, str(path), weight=0.0, name="ex-" + unit)
        bpy.context.view_layer.update()

        # Reference points in MPFB's frame, before anything is posed.
        to_world = np.array(human.matrix_world)
        self.to_world = to_world
        co, normals = self.evaluated(to_world)
        self.jaw_bone = rig.pose.bones["jaw"]
        self.jaw_bone.rotation_mode = "XYZ"
        self.jaw_bone.rotation_euler = (math.radians(JAW_MAX_DEGREES), 0.0, 0.0)
        opened, _ = self.evaluated(to_world)
        self.jaw_bone.rotation_euler = (0.0, 0.0, 0.0)
        groups = {g.name: g.index for g in human.vertex_groups}
        weights = {name: np.zeros(len(co)) for name in ("body", "lips")}
        for v in human.data.vertices:
            for g in v.groups:
                for name in weights:
                    if g.group == groups.get(name):
                        weights[name][v.index] = g.weight
        rig_world = rig.matrix_world.copy()
        bones = rig.data.bones
        self.eyes = (rig_world @ bones["eye.L"].head_local + rig_world @ bones["eye.R"].head_local) / 2
        lips = np.nonzero(weights["lips"] > 0.5)[0]
        corners = co[lips[np.argsort(np.abs(co[lips, 0]))[-6:]]]
        mid = lips[np.abs(co[lips, 0]) < 0.004]
        front = co[mid, 1].min()
        stomion = corners[:, 2].mean()
        self.mouth = np.array([0.0, front + 0.012, stomion])
        self.mouth_width = float(np.abs(corners[:, 0]).mean())
        # The lower lip's top edge at the middle (the lip that drops when the
        # jaw opens), and how far it follows the jaw.
        drop = opened[mid, 2] - co[mid, 2]
        lower = mid[drop < 0.5 * drop.min()]
        lower = lower[co[lower, 1] < front + 0.01] if (co[lower, 1] < front + 0.01).any() else lower
        lip_top = lower[np.argmax(co[lower, 2])]
        jaw = bones["jaw"]
        pivot = np.array(rig_world @ jaw.head_local)
        axis = np.array((rig_world.to_3x3() @ jaw.matrix_local.to_3x3()).col[0])
        before, after = co[lip_top] - pivot, opened[lip_top] - pivot
        before, after = before - axis * (before @ axis), after - axis * (after @ axis)
        turned = math.atan2(np.cross(before, after) @ axis, before @ after)
        self.jaw = (pivot, axis.tolist(), float(np.clip(turned / math.radians(JAW_MAX_DEGREES), 0.2, 1.0)))
        self.lip_top = float(co[lip_top, 2] - stomion)

        # Cheek puffs as shape keys: the cheeks swell out along their normals
        # (and, at negative values, suck in). A tongue pushed into a cheek
        # makes a smaller bulge beside the mouth's corner.
        puff = np.zeros(len(co))
        swellings = [("puff-left", 1.0, (0.014, 0.004, 0.004), (0.022, 0.03, 0.026), 0.012),
                     ("puff-right", -1.0, (0.014, 0.004, 0.004), (0.022, 0.03, 0.026), 0.012),
                     ("bulge-left", 1.0, (0.008, 0.01, -0.004), (0.012, 0.016, 0.014), 0.009),
                     ("bulge-right", -1.0, (0.008, 0.01, -0.004), (0.012, 0.016, 0.014), 0.009)]
        to_local = np.linalg.inv(to_world[:3, :3]).T
        self.swellings = {}
        for name, side, (out, back, up), radii, size in swellings:
            centre = np.array([side * (self.mouth_width + out), self.mouth[1] + back, self.mouth[2] + up])
            d2 = ((co - centre) ** 2 / np.array(radii) ** 2).sum(1)
            amount = size * np.exp(-d2) * (weights["body"] > 0.5)
            puff = np.maximum(puff, amount)
            key = human.shape_key_add(name=name, from_mix=False)
            base = np.array([v.co[:] for v in key.data])
            key.data.foreach_set("co", (base + (normals * amount[:, None]) @ to_local).astype(np.float32).ravel())
            key.slider_min = -1.0
            key.value = 0.0
            self.swellings[name] = key
        # The face near the headset, as it is and fully puffed, for clearance checks.
        near = (weights["body"] > 0.5) & (co[:, 2] > self.mouth[2] - 0.06) & (co[:, 1] < self.eyes.y)
        puffed = co + normals * puff[:, None]
        self.surface = np.vstack([co[near], puffed[near]])

        # Only the head, neck and upper chest are ever in view.
        head_part = (co[:, 2] > self.eyes.z - 0.45) & (np.abs(co[:, 0]) < 0.3) & (weights["body"] > 0.5)
        group = human.vertex_groups.new(name="Rendered")
        group.add(np.nonzero(head_part)[0].tolist(), 1.0, "REPLACE")
        mask = human.modifiers.new("Rendered only", "MASK")
        mask.vertex_group = "Rendered"
        # Lips and beard for the skin shader.
        lip = smoothstep(0.2, 0.8, weights["lips"])
        attr = human.data.attributes.new("lip", "FLOAT", "POINT")
        attr.data.foreach_set("value", lip.astype(np.float32))
        rel = co - self.mouth
        beard = ((1 - smoothstep(0.012, 0.02, rel[:, 2])) * smoothstep(-0.13, -0.09, rel[:, 2])
                 * (1 - lip) * (1 - smoothstep(0.06, 0.075, np.abs(rel[:, 0]))))
        attr = human.data.attributes.new("beard", "FLOAT", "POINT")
        attr.data.foreach_set("value", beard.astype(np.float32))

        metres_per_unit = float(np.linalg.norm(to_world[:3, 0]))
        mats = [skin_material(identity, metres_per_unit), cloth_material(identity["cloth"], identity["cloth_folds"], metres_per_unit)]
        human.data.materials.clear()
        for m in mats:
            human.data.materials.append(m)
        chest = co[:, 2] < self.mouth[2] - 0.13
        index = [1 if chest[list(poly.vertices)].all() else 0 for poly in human.data.polygons]
        human.data.polygons.foreach_set("material_index", index)
        human.data.polygons.foreach_set("use_smooth", [True] * len(human.data.polygons))
        sub = human.modifiers.new("Subdivision", "SUBSURF")
        sub.levels = sub.render_levels = 1

        # The whole head hangs off one empty that places it in the headset.
        self.anchor = bpy.data.objects.new("Head", None)
        bpy.context.scene.collection.objects.link(self.anchor)
        rig.parent = self.anchor
        # A dark mouth behind the teeth.
        ellipsoid("Mouth cavity", self.mouth + np.array([0.0, 0.048, -0.008]), (0.026, 0.032, 0.022),
                  material("Cavity", 0.03, 0.6)[0], self.anchor, flip=True)
        self.tongue = Tongue(identity, wet_material("Tongue", identity["tongue_albedo"], 2500.0,
                                                    identity["wetness"], identity["papillae"],
                                                    identity["even_coat"]),
                             self.mouth, self.mouth_width, self.lip_top, self.jaw, self.anchor)
        self.teeth = Teeth(teeth_material(identity["teeth"], identity["teeth_gaps"]), self.mouth, self.jaw, self.anchor)

    def evaluated(self, to_world):
        """Vertex positions and normals of the posed mesh in MPFB's frame."""
        bpy.context.view_layer.update()
        dg = bpy.context.evaluated_depsgraph_get()
        evaluated = self.human.evaluated_get(dg)
        mesh = evaluated.to_mesh()
        co = np.empty(len(mesh.vertices) * 3, np.float32)
        mesh.vertices.foreach_get("co", co)
        normals = np.empty(len(mesh.vertices) * 3, np.float32)
        mesh.vertices.foreach_get("normal", normals)
        evaluated.to_mesh_clear()
        co = co.reshape(-1, 3) @ to_world[:3, :3].T + to_world[:3, 3]
        normals = normals.reshape(-1, 3) @ to_world[:3, :3].T
        return co, normals / (np.linalg.norm(normals, axis=1, keepdims=True) + 1e-9)

    def placement(self, pitch, yaw, roll, eyes):
        """The head's matrix in the headset frame for its tilt and where its eyes sit."""
        rotation = (Matrix.Rotation(math.radians(roll), 3, "Z") @ Matrix.Rotation(math.radians(yaw), 3, "Y")
                    @ Matrix.Rotation(math.radians(pitch), 3, "X") @ MPFB_TO_HEADSET)
        return Matrix.Translation(Vector(eyes) - rotation @ self.eyes) @ rotation.to_4x4()

    def place(self, pitch, yaw, roll, eyes):
        self.anchor.matrix_world = self.placement(pitch, yaw, roll, eyes)

    def clearance(self, pitch, yaw, roll, eyes, lenses):
        """The face's closest approach to any camera, cheeks puffed or not (metres)."""
        m = np.array(self.placement(pitch, yaw, roll, eyes))
        points = self.surface @ m[:3, :3].T + m[:3, 3]
        return min(float(np.linalg.norm(points - lens.t, axis=1).min()) for lens in lenses)

    def fit(self, identity, lenses):
        """Moves the head back from the headset until the face clears the cameras,
        as a headset resting on a face with a big nose or full cheeks would sit."""
        eyes = list(identity["eyes"])
        for _ in range(50):
            gap = self.clearance(identity["pitch"], identity["yaw"], identity["roll"], eyes, lenses)
            if gap >= CLEARANCE:
                break
            eyes[2] -= CLEARANCE - gap + 0.0005
        identity["eyes"] = eyes

    def tongue_depth(self):
        """How far the posed tongue's part past the lips sinks under the skin
        (metres), by each point's nearest skin vertex and its normal: 0 while
        it stays outside the face."""
        points = self.tongue.points[self.tongue.outside]
        if not len(points):
            return 0.0
        co, normals = self.evaluated(self.to_world)
        near = np.linalg.norm(co - self.mouth, axis=1) < 0.09
        co, normals = co[near], normals[near]
        tree = KDTree(len(co))
        for index, point in enumerate(co):
            tree.insert(point, index)
        tree.balance()
        depth = 0.0
        for point in points:
            _, index, _ = tree.find(point)
            depth = max(depth, -float((point - co[index]) @ normals[index]))
        return depth

    def pose(self, frame):
        for unit, key in self.units.items():
            key.value = frame["units"].get(unit, 0.0)
        self.swellings["puff-left"].value = frame["puff_left"] - 0.6 * frame["suck"]
        self.swellings["puff-right"].value = frame["puff_right"] - 0.6 * frame["suck"]
        self.swellings["bulge-left"].value = frame["bulge_left"]
        self.swellings["bulge-right"].value = frame["bulge_right"]
        self.jaw_bone.rotation_euler = (math.radians(frame["jaw"] * JAW_MAX_DEGREES), 0.0, 0.0)
        self.tongue.update(frame)
        self.teeth.update(frame)


def build_scene(identity, sensor, cameras, mpfb, targets_dir):
    clear_scene()
    scene = bpy.context.scene
    try:
        scene.render.engine = "BLENDER_EEVEE"
    except TypeError:
        scene.render.engine = "BLENDER_EEVEE_NEXT"
    if hasattr(scene, "eevee"):
        scene.eevee.taa_render_samples = SAMPLES
    scene.render.resolution_x = scene.render.resolution_y = PINHOLE
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "OPEN_EXR"
    scene.render.image_settings.color_depth = "16"
    scene.view_settings.view_transform = "Standard"
    world = bpy.data.worlds.new("Dark")
    if bpy.app.version < (5, 0, 0):
        world.use_nodes = True
    nodes, links = world.node_tree.nodes, world.node_tree.links
    background = next(n for n in nodes if n.type == "BACKGROUND")
    a = sensor["ambient"]
    background.inputs["Color"].default_value = (a, a, a, 1)
    # The room behind the person, where the camera sees past them, stays dim.
    seen = nodes.new("ShaderNodeLightPath")
    dim = nodes.new("ShaderNodeMix")
    dim.data_type = "RGBA"
    dim.inputs["A"].default_value = (a, a, a, 1)
    dim.inputs["B"].default_value = (0.1 * a, 0.1 * a, 0.1 * a, 1)
    links.new(seen.outputs["Is Camera Ray"], dim.inputs["Factor"])
    links.new(dim.outputs["Result"], background.inputs["Color"])
    scene.world = world

    head = Head(identity, mpfb, targets_dir)
    head.fit(identity, cameras)
    views = []
    for n, lens in enumerate(cameras):
        cam = bpy.data.objects.new(f"Camera {n}", bpy.data.cameras.new(f"Camera {n}"))
        cam.data.lens_unit = "FOV"
        cam.data.angle = 2 * math.atan(PINHOLE_TAN)
        cam.data.clip_start = 0.003
        cam.matrix_world = lens.blender_matrix()
        scene.collection.objects.link(cam)
        # An IR illuminator by each camera, lighting what it sees.
        light = bpy.data.objects.new(f"IR {n}", bpy.data.lights.new(f"IR {n}", "SPOT"))
        light.data.energy = 0.1 * (sensor["light_balance"] if n else 1.0)
        light.data.spot_size = math.radians(sensor["light_cone"])
        light.data.spot_blend = sensor["light_blend"]
        light.data.shadow_soft_size = 0.003
        out, up, forward = sensor["light_offsets"][n]
        offset = np.array([math.copysign(out, lens.t[0]), up, forward])
        light.matrix_world = Matrix.Translation(Vector(offset)) @ lens.blender_matrix()
        scene.collection.objects.link(light)
        views.append((cam, lens.warp()))
    return scene, head, views


# ---------------------------------------------------------------- output

def render_view(scene, cam, warp, path):
    scene.camera = cam
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    image = bpy.data.images.load(path)
    pixels = np.empty(PINHOLE * PINHOLE * 4, dtype=np.float32)
    image.pixels.foreach_get(pixels)
    bpy.data.images.remove(image)
    return sample_pinhole(pixels.reshape(PINHOLE, PINHOLE, 4)[::-1, :, 0], warp)


def blur(image, sigma):
    x = np.arange(-3, 4)
    k = np.exp(-x * x / (2 * sigma * sigma))
    k /= k.sum()
    padded = np.pad(image, 3, mode="edge")
    rows = sum(k[i] * padded[:, i:i + image.shape[1]] for i in range(7))
    return sum(k[i] * rows[i:i + image.shape[0], :] for i in range(7))


def box_blur(image, radius):
    """Mean over a (2 radius + 1) square, from summed-area tables."""
    padded = np.pad(image, radius + 1, mode="edge").cumsum(0).cumsum(1)
    size = 2 * radius + 1
    h, w = image.shape
    total = (padded[size:size + h, size:size + w] - padded[:h, size:size + w]
             - padded[size:size + h, :w] + padded[:h, :w])
    return total / size ** 2


def sensor_image(radiance, off_axis, sensor, gain, rng):
    """The camera's own look: exposure, lens falloff, glare, blur and noise, as 8-bit."""
    dn = gain * np.maximum(radiance, 0) * np.cos(off_axis) ** sensor["vignette"]
    dn = dn + sensor["glare"] * box_blur(dn, 40)
    dn = 255.0 * (np.minimum(dn, 255.0) / 255.0) ** sensor["gamma"]
    dn = blur(dn, sensor["blur"])
    dn = dn + rng.normal(0, 1, dn.shape) * np.sqrt(sensor["shot_noise"] * dn + sensor["read_noise"] ** 2)
    return np.clip(dn + sensor["dark"] + 0.5, 0, 255).astype(np.uint8)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(prog="render_tongue.py")
    parser.add_argument("--count", type=int, default=200, help="frames to render")
    parser.add_argument("--seed", type=int, default=int(time.time()))
    parser.add_argument("--identities", type=int, default=0,
                        help="synthetic people; 0 picks one per 50 frames")
    parser.add_argument("--out", type=Path, default=repo / ".local" / "tongue-captures")
    parser.add_argument("--fast", action="store_true",
                        help="half-size renders with fewer samples, for quick checks")
    parser.add_argument("--calibration", type=Path,
                        default=repo / ".local" / "headset-calibration" / "ft_calib.scio.json",
                        help="the headset's ft_calib.scio.json; nominal values when it's missing")
    args = parser.parse_args(argv)
    if args.fast:
        global PINHOLE, SAMPLES
        PINHOLE, SAMPLES = 400, 8

    mpfb, mpfb_dir = enable_mpfb()
    targets_dir = mpfb_dir / "data" / "targets"
    axes = shape_axes(targets_dir)
    cameras, calibration = load_cameras(args.calibration)
    rng = np.random.default_rng(args.seed)
    people = args.identities or max(1, math.ceil(args.count / 50))
    per_person = math.ceil(args.count / people)

    millis = int(time.time() * 1000)
    directory = args.out / f"{millis}-synthetic-{os.getpid()}"
    directory.mkdir(parents=True)
    metadata = {
        "format": "vrft-tongue-capture-v1", "mode": "synthetic", "width": 800, "height": 400,
        "bytesPerFrame": 800 * 400, "targets": TARGETS,
        "synthetic": {
            "generator": "tools/tongue-synth/render_tongue.py", "version": 5,
            "seed": args.seed, "identities": people, "blender": bpy.app.version_string,
            "faces": "MPFB", "calibration": calibration, "pinhole": PINHOLE, "samples": SAMPLES,
        },
    }
    (directory / "metadata.json").write_text(json.dumps(metadata, indent=2))
    print(f"tongue-synth: cameras from {calibration} calibration", flush=True)
    scratch = Path(tempfile.mkdtemp(prefix="vrft-synth-"))
    started = time.time()
    index = 0
    # Tongue poses drawn again because they went through the face, by pose.
    rejected = {}
    with open(directory / "frames.gray8", "wb") as frames, \
            open(directory / "samples.jsonl", "w") as labels:
        while index < args.count:
            identity = sample_identity(rng, axes)
            sensor = sample_sensor(rng)
            lenses = [lens.perturbed(rng) for lens in cameras]
            scene, head, views = build_scene(identity, sensor, lenses, mpfb, targets_dir)
            gain = None
            for _ in range(min(per_person, args.count - index)):
                # A pose that sends the tongue through the face is drawn again.
                attempts = 0
                while True:
                    frame = sample_frame(rng)
                    attempts += 1
                    if attempts > TONGUE_ATTEMPTS and frame["kind"] == "visible":
                        continue
                    head.pose(frame)
                    if frame["kind"] != "visible" or head.tongue_depth() <= TONGUE_DEPTH:
                        break
                    rejected[frame["pose"]] = rejected.get(frame["pose"], 0) + 1
                # The headset shifts a little on the face while it's worn.
                head.place(identity["pitch"] + rng.normal(0, 1.0), identity["yaw"] + rng.normal(0, 0.7),
                           identity["roll"] + rng.normal(0, 0.7),
                           list(np.add(identity["eyes"], rng.normal(0, 0.001, 3))))
                radiance = [render_view(scene, cam, warp, str(scratch / f"{n}.exr"))
                            for n, (cam, warp) in enumerate(views)]
                # Auto-exposure: part way from the last frame's gain toward
                # this frame's, as the headset's lags behind a changing face.
                settled = sensor["brightness"] / max(np.percentile(np.concatenate(radiance), 99), 1e-6)
                gain = settled if gain is None else math.exp(0.4 * math.log(gain) + 0.6 * math.log(settled))
                gain *= float(np.exp(rng.normal(0, 0.1)))
                strip = [sensor_image(r, warp[2], sensor, gain, rng) for r, (_, warp) in zip(radiance, views)]
                frames.write(np.concatenate(strip, axis=1).tobytes())
                labels.write(json.dumps({
                    "index": index, "sequence": index, "pose": frame["pose"], "step": frame["step"],
                    "round": 1, "targets": frame["targets"], "native_tongue_out": None,
                    "captured_unix_ms": int(time.time() * 1000),
                    # Marks every frame as distinct, so training keeps them all.
                    "dot": [frame["h"], frame["v"]],
                    "synthetic": {"jaw": round(float(frame["jaw"]), 4), "units": frame["units"],
                                  "ext": frame["ext"], "curl": frame["curl"],
                                  **{k: round(frame[k], 4) for k in ("bulge_left", "bulge_right", "suck")
                                     if frame[k]}},
                }) + "\n")
                index += 1
                if index % 25 == 0:
                    rate = index / (time.time() - started)
                    print(f"tongue-synth: {index}/{args.count} frames ({rate:.1f}/s)", flush=True)
    shutil.rmtree(scratch, ignore_errors=True)
    metadata["synthetic"]["rejected_poses"] = rejected
    (directory / "metadata.json").write_text(json.dumps(metadata, indent=2))
    print(f"tongue-synth: drew {sum(rejected.values())} tongue poses again: {rejected}", flush=True)
    print(f"tongue-synth: wrote {directory}", flush=True)


if __name__ == "__main__":
    main()
