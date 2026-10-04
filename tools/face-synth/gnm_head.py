"""Google's GNM head (GNM v3, Apache-2.0) as plain NumPy, for rendering
labelled synthetic faces.

GNM is a linear 3D morphable model: a template mesh of 17,821 vertices plus
a weighted sum of 253 identity and 383 expression offsets (all in metres,
x the person's left, y up, z forward, the same axes as the headset frame
the renderer works in). Its coefficients are whitened, so each is about
N(0, 1) over real people. The expression basis is unlabelled PCA in blocks:
left eye region 0-99, right eye region 100-199, lower face 200-349,
`tongue_mean` 350, tongue 351-381 and pupil 382. There is no jaw joint: the
jaw moves inside the lower-face block.

This module needs NumPy alone, so it runs inside Blender's Python and in a
plain one (for tests), never GNM's own package or TensorFlow. The weights
are downloaded at run time from Hugging Face (`google/gnm-v3`,
`v3_0/gnm_head.npz`, about 53 MB) and never committed.

Named expressions come from ridge fits of a basis block to a displacement
field on the template ("prototypes"): the jaw turning open, the lips
pursing, a brow rising. Cheek puffs and sucks, which the PCA barely reaches,
and the tongue's sideways bend, which it reaches weakly, are deformers on
top, as in tools/tongue-synth. Every label is then measured from the posed
geometry, never taken from what was asked for.
"""

import json
import math
import os
import urllib.request
from pathlib import Path

import numpy as np

GNM_URL = "https://huggingface.co/google/gnm-v3/resolve/main/v3_0/gnm_head.npz"
# Every array the renderer reads; others in the file are left on disk.
FIELDS = ("template_vertex_positions", "vertex_identity_basis", "expression_basis",
          "template_joint_positions", "joint_identity_basis", "joint_names", "triangles",
          "quads", "vertex_group_names", "vertex_groups", "expression_names", "version")

LEFT_EYE = np.arange(0, 100)
RIGHT_EYE = np.arange(100, 200)
EYES = np.arange(0, 200)
LOWER_FACE = np.arange(200, 350)
TONGUE = np.arange(350, 382)

# A prototype is at most this long in the whitened coefficient space: that
# many standard deviations along its own direction.
MAX_SIGMA = 4.0

# The brow prototypes' movements, in metres (geometry, not labels).
BROW_RAISE_FULL = 0.006
BROW_LOWER_FULL = 0.004
PINCH_FULL = 0.003
# Tongue extension: protrusion past the lips' front (mm) to label, as
# the capture poses grade it: just the tip 0.25, half out 0.5 (about 8 mm),
# fully out 1 (about 2 cm).
EXTENSION_MM = (0.0, 2.5, 8.0, 20.0)
EXTENSION_LABEL = (0.0, 0.25, 0.5, 1.0)
# The tongue counts as out once its tip is this far past the lips' front.
VISIBLE_PAST = 0.001
# Its direction is the way its tip points from inside the mouth, as yaw and
# pitch (degrees) against the person's own straight-out tongue of the same
# length: turned this far is full left or right, up, or down. The face
# setup's held poses turn about this far (40 and 51 degrees). Down has less
# room: held straight out, a GNM tongue already points about 40 degrees
# below level from inside the mouth, and only a long one can turn to point
# straight down, so 35 degrees down is full. Under
# DIRECTION_PAST of tongue past the lips gives no direction, as for "just
# the tip".
SIDEWAYS_FULL_DEG = 40.0
UP_FULL_DEG = 50.0
DOWN_FULL_DEG = 35.0
DIRECTION_PAST = 0.003
# A shown tongue's part past the lips may sink this far under the skin
# (lips pressing on it); deeper, it goes through a lip or the chin.
TONGUE_DEPTH = 0.004


def fetch(path, url=GNM_URL):
    """The GNM head file at `path`, downloaded first if it isn't there."""
    path = Path(path)
    if path.is_file():
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    partial = path.with_suffix(".part")
    print(f"face-synth: downloading GNM head from {url}", flush=True)
    with urllib.request.urlopen(url) as response, open(partial, "wb") as out:
        while True:
            chunk = response.read(1 << 20)
            if not chunk:
                break
            out.write(chunk)
    os.replace(partial, path)
    return path


# GNM's semantic expression sampler, the decoder half of a conditional VAE
# (gnm/shape/semantic_sampler.py): a 64-d latent and a one-hot class in, 383
# expression coefficients out. Its classes, in the decoder's order.
SEMANTIC_CLASSES = (
    "surprise", "disgust", "suck", "compress_face", "stretch_face", "happy", "squint", "platysma",
    "blow", "funneler", "smile_wide", "corners_down", "pucker", "wink_left", "wink_right",
    "mouth_left", "mouth_right", "lips_roll_in", "snarl", "tongue_center",
)


class SemanticSampler:
    """GNM's semantic expression sampler in NumPy, from the decoder weights
    `semantic_decoder.py` converts (no TensorFlow): Dense layers, ReLU but
    for the last."""

    def __init__(self, path):
        with np.load(path) as data:
            self.layers = [(data[f"kernel_{i}"].astype(np.float32), data[f"bias_{i}"].astype(np.float32))
                           for i in range(int(data["layers"]))]
            self.classes = tuple(str(name) for name in data["classes"])
        self.latent = self.layers[0][0].shape[0] - len(self.classes)

    def decode(self, z, weights):
        """Expressions for latents `z` (n, 64) and class weights (n, 20)."""
        x = np.concatenate([np.asarray(z, np.float32), np.asarray(weights, np.float32)], axis=1)
        for i, (kernel, bias) in enumerate(self.layers):
            x = x @ kernel + bias
            if i < len(self.layers) - 1:
                x = np.maximum(x, 0.0)
        return x

    def sample(self, name, rng, count=1):
        """`count` expressions of class `name`, as GNM's sample_expression."""
        weights = np.zeros((count, len(self.classes)), np.float32)
        weights[:, self.classes.index(name)] = 1.0
        return self.decode(rng.normal(size=(count, self.latent)), weights)


def smoothstep(edge0, edge1, x):
    t = np.clip((x - edge0) / (edge1 - edge0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def rotation(axis, angle):
    """3x3 rotation by `angle` radians about unit `axis`."""
    x, y, z = np.asarray(axis, float) / np.linalg.norm(axis)
    c, s = math.cos(angle), math.sin(angle)
    return np.array([
        [c + x * x * (1 - c), x * y * (1 - c) - z * s, x * z * (1 - c) + y * s],
        [y * x * (1 - c) + z * s, c + y * y * (1 - c), y * z * (1 - c) - x * s],
        [z * x * (1 - c) - y * s, z * y * (1 - c) + x * s, c + z * z * (1 - c)],
    ])


class Gnm:
    """The GNM head's arrays in float32, and the regions the renderer uses."""

    def __init__(self, path):
        with np.load(path, allow_pickle=False) as data:
            arrays = {name: data[name] for name in FIELDS}
        f32 = lambda name: np.ascontiguousarray(arrays[name], dtype=np.float32)
        self.template = f32("template_vertex_positions")
        self.identity_basis = f32("vertex_identity_basis")
        self.expression_basis = f32("expression_basis")
        self.joint_template = f32("template_joint_positions")
        self.joint_identity_basis = f32("joint_identity_basis")
        self.joint_names = [str(name) for name in arrays["joint_names"]]
        self.triangles = arrays["triangles"].astype(np.int64)
        self.quads = arrays["quads"].astype(np.int64)
        self.expression_names = [str(name) for name in arrays["expression_names"]]
        self.version = str(arrays["version"])
        weights = arrays["vertex_groups"]
        self.groups = {str(name): np.nonzero(weights[i] > 0.5)[0]
                       for i, name in enumerate(arrays["vertex_group_names"])}
        self.vertex_count = len(self.template)
        if self.expression_basis.shape[0] != 383 or not self.expression_names[350] == "tongue_mean":
            raise ValueError("face-synth: unsupported GNM expression layout")

    @property
    def identity_dim(self):
        return self.identity_basis.shape[0]

    @property
    def expression_dim(self):
        return self.expression_basis.shape[0]

    def group(self, name):
        return self.groups[name]

    def neutral(self, identity):
        """The person's face with every expression at zero."""
        return self.template + np.tensordot(identity.astype(np.float32), self.identity_basis, 1)

    def posed(self, neutral, expression):
        """`neutral` with `expression`; only its non-zero coefficients are summed."""
        used = np.nonzero(expression)[0]
        if not len(used):
            return neutral.copy()
        return neutral + np.tensordot(expression[used].astype(np.float32),
                                      self.expression_basis[used], 1)

    def eye_centres(self, identity):
        """The eyeballs' centres (left, right), from the eye joints."""
        joints = self.joint_template + np.tensordot(identity.astype(np.float32),
                                                    self.joint_identity_basis, 1)
        return np.stack([joints[self.joint_names.index("left_eye")],
                         joints[self.joint_names.index("right_eye")]])

    def normals(self, vertices):
        """Area-weighted vertex normals."""
        a, b, c = (vertices[self.triangles[:, k]] for k in range(3))
        face = np.cross(b - a, c - a)
        normals = np.zeros_like(vertices)
        for k in range(3):
            np.add.at(normals, self.triangles[:, k], face)
        return normals / (np.linalg.norm(normals, axis=1, keepdims=True) + 1e-12)


class Landmarks:
    """Points of the person's neutral face the poses and labels refer to."""

    def __init__(self, gnm, neutral):
        g = gnm.group
        self.lips = np.concatenate([g("upper_lip"), g("lower_lip")])
        lips = neutral[self.lips]
        middle = self.lips[np.abs(lips[:, 0]) < 0.008]
        self.lip_front = float(neutral[middle, 2].max())
        upper, lower = g("upper_lip"), g("lower_lip")
        self.stomion = float((neutral[upper, 1].min() + neutral[lower, 1].max()) / 2)
        corners = self.lips[np.argsort(np.abs(lips[:, 0]))[-8:]]
        self.mouth_width = float(np.abs(neutral[corners, 0]).mean())
        self.mouth = np.array([0.0, self.stomion, self.lip_front - 0.012])
        # The front teeth, for the jaw's opening.
        upper_teeth, lower_teeth = g("upper_teeth_and_gums"), g("lower_teeth_and_gums")
        self.upper_front = upper_teeth[np.argsort(-neutral[upper_teeth, 2])[:24]]
        self.lower_front = lower_teeth[np.argsort(-neutral[lower_teeth, 2])[:24]]
        self.tongue = g("tongue")
        # Its root: the back of the tongue at rest.
        self.tongue_root = neutral[self.tongue][np.argmin(neutral[self.tongue, 2])].copy()
        # Its tip: the front 2 mm of its middle at rest, top and underside
        # together. The mesh never changes, so these stay the tip however it
        # moves.
        rest = neutral[self.tongue]
        middle = self.tongue[np.abs(rest[:, 0]) < 0.004]
        front = neutral[middle, 2].max()
        self.tongue_tip = middle[neutral[middle, 2] > front - 0.002]
        self.skin = g("skin_exterior")
        near = np.linalg.norm(neutral[self.skin] - self.mouth, axis=1) < 0.09
        self.mouth_skin = self.skin[near]
        self.brows = {}
        for side, sign in (("left", 1.0), ("right", -1.0)):
            region = np.union1d(g(f"{side}_brow_region"),
                                g("middle_brow_region")[sign * neutral[g("middle_brow_region"), 0] > 0.002])
            offset = sign * neutral[region, 0]
            cut = np.median(offset)
            self.brows[side] = (sign, region[offset <= cut], region[offset > cut])
        self.cheeks = {side: g(f"{side}_cheek_region") for side in ("left", "right")}
        # Which way this person's tongue points held straight out, by how
        # far it's out; see `straight_out`. Directions are measured from it.
        self.straight = None


def ridge_fit(basis, block, target, weight, ridge, sigma=MAX_SIGMA):
    """Coefficients of `basis[block]` best matching the displacement field
    `target` (V x 3), weighted per vertex, with a ridge penalty, at most
    `sigma` long."""
    used = np.nonzero(weight > 0)[0]
    b = basis[block][:, used, :].astype(np.float64)
    w = np.sqrt(weight[used])[None, :, None]
    a = (b * w).reshape(len(block), -1).T
    y = (target[used] * w[0]).ravel()
    coefficients = np.linalg.solve(a.T @ a + ridge * np.eye(len(block)), a.T @ y)
    length = np.linalg.norm(coefficients)
    if length > sigma:
        coefficients *= sigma / length
    full = np.zeros(basis.shape[0], np.float32)
    full[block] = coefficients
    return full


def steepest(basis, block, points, direction, sigma, still=None):
    """The coefficients of `basis[block]`, `sigma` long, that move `points`
    furthest along `direction` on average, without moving them at all along
    `still`."""
    gradient = (basis[block][:, points, :] @ np.asarray(direction, np.float32)).mean(1)
    if still is not None:
        across = (basis[block][:, points, :] @ np.asarray(still, np.float32)).mean(1)
        gradient = gradient - (gradient @ across) / (across @ across) * across
    full = np.zeros(basis.shape[0], np.float32)
    full[block] = sigma * gradient / np.linalg.norm(gradient)
    return full


class Prototypes:
    """Named expressions, fitted once on the template: each an expression
    vector whose multiple poses that much of the movement."""

    def __init__(self, gnm):
        t = gnm.template
        g = gnm.group
        marks = Landmarks(gnm, t)
        skin = np.zeros(len(t))
        skin[g("skin_exterior")] = 1.0
        teeth = np.zeros(len(t))
        teeth[g("teeth")] = 1.0
        basis = gnm.expression_basis
        self.vectors = {}

        def keep(name, block, field, weight, ridge, sigma=MAX_SIGMA):
            self.vectors[name] = ridge_fit(basis, block, field, weight, ridge, sigma)

        # The jaw turning 20 degrees open about a hinge behind the teeth; the
        # upper face stays.
        hinge = np.array([0.0, marks.stomion + 0.014, marks.lip_front - 0.10])
        turn = rotation((1, 0, 0), math.radians(20))
        follows = smoothstep(marks.stomion + 0.002, marks.stomion - 0.010, t[:, 1]) * (t[:, 2] > 0.05)
        field = ((t - hinge) @ turn.T + hinge - t) * follows[:, None]
        # Opening the jaw is the lower face's largest movement, so it may go
        # further than the other prototypes.
        keep("jaw_open", LOWER_FACE, field, skin + 3 * teeth, 1e-4, sigma=12.0)

        lips = np.zeros(len(t))
        lips[marks.lips] = 1.0
        around = np.exp(-np.sum(((t - marks.mouth) / (0.03, 0.02, 0.03)) ** 2, axis=1)) * skin
        mouth_field = lambda f: f * np.maximum(lips, around)[:, None]
        rel = t - np.array([0.0, marks.stomion, marks.lip_front])
        # Lips pursed forward and in.
        keep("pucker", LOWER_FACE, mouth_field(np.stack([-0.35 * rel[:, 0], 0 * rel[:, 1],
                                                         np.full(len(t), 0.007)], 1)),
             skin + teeth, 1e-5)
        # The mouth's corners out and up.
        corner = smoothstep(0.008, marks.mouth_width, np.abs(rel[:, 0]))
        keep("smile", LOWER_FACE, mouth_field(np.stack([0.006 * np.sign(rel[:, 0]) * corner,
                                                        0.005 * corner, -0.002 * corner], 1)),
             skin + teeth, 1e-5)
        # Lips apart: the upper up a little, the lower down.
        part = np.where(rel[:, 1] > 0, 0.0025, -0.0045) * lips
        keep("lips_part", LOWER_FACE, np.stack([0 * part, part, 0 * part], 1), skin + teeth, 1e-5)
        # The whole mouth to one side: positive is the person's right, -x.
        keep("mouth_right", LOWER_FACE, mouth_field(np.stack([np.full(len(t), -0.006), 0 * rel[:, 1],
                                                              0 * rel[:, 2]], 1)), skin + teeth, 1e-5)

        # Brows, each side and part on its own; the rest of the face stays.
        for side, (sign, medial, lateral) in marks.brows.items():
            region = np.zeros(len(t))
            region[medial] = 1.0
            middle = np.zeros(len(t))
            middle[medial] = 1.0
            outer = np.zeros(len(t))
            outer[lateral] = 1.0
            soft = lambda points: np.exp(-np.min(np.linalg.norm(t[:, None, :] - t[None, points[::4], :],
                                                                axis=2), axis=1) ** 2 / 0.006 ** 2)
            near_middle, near_outer = soft(medial), soft(lateral)
            up = np.array([0.0, 1.0, 0.0])
            keep(f"brow_inner_up_{side}", EYES, near_middle[:, None] * up * BROW_RAISE_FULL, skin, 1e-5)
            keep(f"brow_outer_up_{side}", EYES, near_outer[:, None] * up * BROW_RAISE_FULL, skin, 1e-5)
            both = np.maximum(near_middle, near_outer)
            keep(f"brow_lowerer_{side}", EYES, both[:, None] * -up * BROW_LOWER_FULL, skin, 1e-5)
            inward = np.array([-sign, 0.0, 0.0])
            keep(f"brow_pinch_{side}", EYES, near_middle[:, None] * inward * PINCH_FULL, skin, 1e-5)

        # The tongue: the tongue block's steepest way to move its tip out, or
        # out and up or down, at 3 standard deviations.
        tip = marks.tongue[np.argsort(-t[marks.tongue, 2])[:12]]
        sideways = (1.0, 0.0, 0.0)
        self.vectors["tongue_out"] = steepest(basis, TONGUE, tip, (0.0, -0.15, 1.0), 3.0, sideways)
        self.vectors["tongue_up"] = steepest(basis, TONGUE, tip, (0.0, 1.0, 0.6), 3.0, sideways)
        self.vectors["tongue_down"] = steepest(basis, TONGUE, tip, (0.0, -1.0, 0.6), 3.0, sideways)

    def __getitem__(self, name):
        return self.vectors[name]

    def mix(self, weights):
        """The sum of `weights[name] * prototype`."""
        out = np.zeros(next(iter(self.vectors.values())).shape, np.float32)
        for name, weight in weights.items():
            out += weight * self.vectors[name]
        return out


class Deformers:
    """What the PCA barely reaches, as displacements on the posed mesh: each
    cheek swelling out along its normals (or in, sucked), the tongue
    pushed into a cheek, and the tongue bending sideways or stretching out."""

    def __init__(self, gnm, neutral, marks):
        self.marks = marks
        normals = gnm.normals(neutral)
        skin = np.zeros(len(neutral))
        skin[marks.skin] = 1.0
        self.fields = {}
        for name, side, (out, back, up), radii, size in (
                ("puff_left", 1.0, (0.012, -0.012, 0.004), (0.02, 0.026, 0.03), 0.0093),
                ("puff_right", -1.0, (0.012, -0.012, 0.004), (0.02, 0.026, 0.03), 0.0093),
                ("bulge_left", 1.0, (0.006, -0.004, -0.004), (0.012, 0.014, 0.016), 0.009),
                ("bulge_right", -1.0, (0.006, -0.004, -0.004), (0.012, 0.014, 0.016), 0.009)):
            centre = np.array([side * (marks.mouth_width + out), marks.stomion + up, marks.lip_front + back])
            d2 = (((neutral - centre) / np.array(radii)) ** 2).sum(1)
            self.fields[name] = normals * (size * np.exp(-d2) * skin)[:, None]

    def apply(self, vertices, frame):
        """`vertices` with the frame's cheeks and tongue deformations."""
        out = vertices.copy()
        # A full puff or suck moves the cheek about 7 mm, as GNM's semantic
        # sampler's BLOW and SUCK do (README.md, Labels).
        out += (frame.get("puff_left", 0.0) - frame.get("suck_left", 0.0)) * self.fields["puff_left"]
        out += (frame.get("puff_right", 0.0) - frame.get("suck_right", 0.0)) * self.fields["puff_right"]
        out += frame.get("bulge_left", 0.0) * self.fields["bulge_left"]
        out += frame.get("bulge_right", 0.0) * self.fields["bulge_right"]
        bend, lift, stretch = frame.get("bend", 0.0), frame.get("lift", 0.0), frame.get("stretch", 0.0)
        if bend or lift or stretch:
            m = self.marks
            tongue = out[m.tongue]
            # How far along the tongue each point is: its distance from the
            # root, against the distance where the tongue crosses the lips.
            # A tongue that hangs over the lower lip keeps its order this
            # way, where depth alone wouldn't.
            along = np.linalg.norm(tongue - m.tongue_root, axis=1)
            crossing = np.abs(tongue[:, 2] - m.lip_front) < 0.002
            at_lips = float(np.median(along[crossing])) if crossing.any() else m.lip_front - m.tongue_root[2]
            # Stretching pushes the part from inside the lips forward.
            reach = smoothstep(at_lips - 0.025, at_lips, along)
            tongue = tongue + np.array([0.0, 0.0, stretch]) * reach[:, None]
            # Sideways the whole front swings toward a corner from inside
            # the mouth (positive is the person's right, -x, a turn about
            # +y); up or down only the front tips over. Both turn fully at
            # the tip, however far out it is, so a short tongue points too.
            tip = float(along.max())
            for amount, axis, pivot, span in (
                    (-bend, (0, 1, 0), m.lip_front - 0.014, 0.032),
                    (-lift, (1, 0, 0), m.lip_front - 0.002, 0.018)):
                if not amount:
                    continue
                centre = np.array([0.0, m.stomion, pivot])
                weight = smoothstep(tip - span, tip, along)
                for k in np.nonzero(weight > 0)[0]:
                    turn = rotation(axis, amount * weight[k])
                    tongue[k] = (tongue[k] - centre) @ turn.T + centre
            out[m.tongue] = tongue
        return out


def beyond_lips(points, lips, left, forward):
    """How far each point is ahead of the lips' front at its own sideways
    position (metres): the mouth curves back toward its corners, so a tongue
    pushed into a corner is out there before it is ahead of the middle."""
    across = points @ left
    lips_across, lips_ahead = lips @ left, lips @ forward
    # The lips' front profile in 2 mm columns; past the corners, the corner's.
    corner = np.abs(lips_across).max()
    across = np.clip(across, -corner, corner)
    near = np.abs(across[:, None] - lips_across[None, :]) < 0.002
    front = np.where(near, lips_ahead[None, :], -np.inf).max(1)
    missing = ~np.isfinite(front)
    if missing.any():
        nearest = np.argmin(np.abs(across[missing, None] - lips_across[None, :]), axis=1)
        front[missing] = lips_ahead[nearest]
    return points @ forward - front


STRAIGHT_OUT = {"tongue_out": 1.0, "jaw_open": 0.4, "lips_part": 1.0}
STRAIGHT_STRETCH = {"stretch": 0.008}


def tongue_angles(vertices, marks):
    """Which way the tongue's tip points from inside the mouth (12 mm behind
    the lips' front), in the head's frame: (yaw, pitch) in degrees, yaw
    positive toward the person's right, pitch positive up. Measured from a
    point inside the mouth, so even a short tongue has a lever to point
    with; the part past the lips alone is too small and lopsided."""
    way = vertices[marks.tongue_tip].mean(0) - marks.mouth
    way = way / max(float(np.linalg.norm(way)), 1e-9)
    yaw = math.degrees(math.asin(float(np.clip(-way[0], -1.0, 1.0))))
    pitch = math.degrees(math.asin(float(np.clip(way[1], -1.0, 1.0))))
    return yaw, pitch


def tongue_past(vertices, marks):
    """How far the tongue reaches past the lips (metres), in the head's frame."""
    return float(beyond_lips(vertices[marks.tongue], vertices[marks.lips],
                             np.array([1.0, 0.0, 0.0]), np.array([0.0, 0.0, 1.0])).max())


def straight_out(gnm, neutral, marks, prototypes, deformers):
    """Sets the person's straight-out tongue directions, which their
    directions are measured from, at every length from just out to fully
    out: a tongue held straight out droops over the lower lip, more the
    shorter it is, and the capture poses label that 0, not down."""
    table = []
    for amount in np.linspace(0.5, 1.25, 16):
        for stretch in (0.0, 0.004, 0.008):
            pose = deformers.apply(gnm.posed(neutral, prototypes.mix({**STRAIGHT_OUT, "tongue_out": float(amount)})),
                                   {"stretch": stretch})
            table.append((tongue_past(pose, marks), *tongue_angles(pose, marks)))
    table.sort()
    marks.straight = np.array(table)


def straight_at(marks, past):
    """The person's straight-out (yaw, pitch) for a tongue `past` metres out."""
    table = marks.straight
    return (float(np.interp(past, table[:, 0], table[:, 1])), float(np.interp(past, table[:, 0], table[:, 2])))


def placement(eye_centres, pitch, yaw, roll, eyes):
    """Rotation and translation that put the head in the headset frame, its
    eyes' midpoint at `eyes`, tilted by `pitch`, `yaw` and `roll` degrees."""
    r = (rotation((0, 0, 1), math.radians(roll)) @ rotation((0, 1, 0), math.radians(yaw))
         @ rotation((1, 0, 0), math.radians(pitch)))
    centre = eye_centres.mean(0)
    return r, np.asarray(eyes, float) - r @ centre


# The label scales, by name: what each label's 1 (and 0) means. They're a
# judgment, not a measurement of real people, so they can be overridden
# (`render_face.py --label-scales`, `relabel.py`) and tuned against a real
# face setup (README.md, Tuning the labels).
SCALES = {
    "visible_past_mm": VISIBLE_PAST * 1000,
    "extension_mm": list(EXTENSION_MM),
    "extension_label": list(EXTENSION_LABEL),
    "sideways_full_deg": SIDEWAYS_FULL_DEG,
    "up_full_deg": UP_FULL_DEG,
    "down_full_deg": DOWN_FULL_DEG,
    # Movement against the person's own neutral face, in mm, that labels 1,
    # and the movement that still labels 0: an open jaw stretches the
    # cheeks in about a millimetre, and a frown lifts nothing. Set from GNM's
    # semantic sampler, which is trained on scans of real faces (median over
    # people): BLOW pushes the cheeks out 5.9 mm, SUCK pulls them in 6.8 mm,
    # STRETCH_FACE opens the front teeth 21.6 mm and raises the inner brows
    # 3.7 mm, COMPRESS_FACE lowers them 3.6 mm.
    "puff_full_mm": 6.0,
    "suck_full_mm": 6.0,
    "cheek_dead_mm": 1.0,
    "brow_raise_full_mm": 4.0,
    "brow_lower_full_mm": 4.0,
    "pinch_full_mm": 3.0,
    "brow_dead_mm": 0.5,
    "jaw_full_mm": 22.0,
}


def scales(overrides=None):
    """SCALES with `overrides` (a mapping, or a JSON file's path) applied;
    an unknown name is an error, so a typo can't pass silently."""
    if overrides is None:
        return dict(SCALES)
    if not isinstance(overrides, dict):
        overrides = json.loads(Path(overrides).read_text())
    unknown = sorted(set(overrides) - set(SCALES))
    if unknown:
        raise ValueError(f"unknown label scales {unknown}; known: {sorted(SCALES)}")
    return {**SCALES, **overrides}


def measure(vertices, neutral, rotation_matrix, marks, normals):
    """What the labels are graded from, measured from geometry, in mm (and
    the tongue's direction as sines). `vertices` and `neutral` are in the
    head's frame; everything is turned into the headset frame by
    `rotation_matrix` before measuring, as the cameras see it."""
    r = rotation_matrix
    v = vertices @ r.T
    n0 = neutral @ r.T
    up, left = r @ np.array([0.0, 1.0, 0.0]), r @ np.array([1.0, 0.0, 0.0])
    forward = r @ np.array([0.0, 0.0, 1.0])
    out = {}

    # Tongue: how far past the lips it reaches: its farthest point ahead of
    # them, which on a tongue hanging over the lower lip isn't the tip.
    past = float(beyond_lips(v[marks.tongue], v[marks.lips], left, forward).max())
    out["tongue_past_lips_mm"] = round(past * 1000, 3)
    # Its direction, in the head's own frame against the person's
    # straight-out tongue of the same length, which is as they hold it
    # whichever way the headset sits. None under DIRECTION_PAST past the
    # lips.
    if past > DIRECTION_PAST:
        (yaw, pitch), (yaw0, pitch0) = tongue_angles(vertices, marks), straight_at(marks, tongue_past(vertices, marks))
        out["tongue_yaw_deg"], out["tongue_pitch_deg"] = round(yaw - yaw0, 3), round(pitch - pitch0, 3)
    else:
        out["tongue_yaw_deg"] = out["tongue_pitch_deg"] = None

    # Cheeks, along the neutral face's normals: the core of the cheek, the
    # part that moved most either way.
    nn = normals @ r.T
    for side in ("left", "right"):
        region = marks.cheeks[side]
        moved = ((v[region] - n0[region]) * nn[region]).sum(1)
        core = moved[np.argsort(-np.abs(moved))[: max(8, len(moved) // 3)]]
        out[f"cheek_{side}_mm"] = round(float(core.mean()) * 1000, 4)

    # Brows: the medial and lateral halves' rise, and the medial half
    # moving toward the middle.
    for side, (sign, medial, lateral) in marks.brows.items():
        out[f"brow_medial_rise_{side}_mm"] = round(float(((v[medial] - n0[medial]) @ up).mean()) * 1000, 4)
        out[f"brow_lateral_rise_{side}_mm"] = round(float(((v[lateral] - n0[lateral]) @ up).mean()) * 1000, 4)
        out[f"brow_inward_{side}_mm"] = round(float(((v[medial] - n0[medial]) @ (-sign * left)).mean()) * 1000, 4)

    # The jaw: the gap between the front teeth, against the neutral one.
    def gap(points):
        return float((points[marks.upper_front] @ up).mean() - (points[marks.lower_front] @ up).mean())
    out["jaw_gap_mm"] = round((gap(v) - gap(n0)) * 1000, 3)
    return out


def grade(measured, scale=None):
    """Labels from `measure`'s measurements and the label scales."""
    k = scale or SCALES
    out = {}
    past = measured["tongue_past_lips_mm"]
    visible = past > k["visible_past_mm"]
    out["visibility"] = 1.0 if visible else 0.0
    out["extension"] = float(np.interp(past, k["extension_mm"], k["extension_label"])) if visible else 0.0
    out["horizontal"] = out["vertical"] = 0.0
    if visible and measured["tongue_yaw_deg"] is not None:
        pitch = measured["tongue_pitch_deg"]
        out["horizontal"] = float(np.clip(measured["tongue_yaw_deg"] / k["sideways_full_deg"], -1, 1))
        out["vertical"] = float(np.clip(pitch / (k["up_full_deg"] if pitch > 0 else k["down_full_deg"]), -1, 1))
    out["tongue_past_lips_mm"] = round(past, 2)

    def graded(moved, full, dead):
        return float(np.clip((moved - dead) / (full - dead), 0, 1))
    for side in ("left", "right"):
        cheek = measured[f"cheek_{side}_mm"]
        out[f"cheek_puff_{side}"] = graded(cheek, k["puff_full_mm"], k["cheek_dead_mm"])
        out[f"cheek_suck_{side}"] = graded(-cheek, k["suck_full_mm"], k["cheek_dead_mm"])
    for side in ("left", "right"):
        medial, lateral = measured[f"brow_medial_rise_{side}_mm"], measured[f"brow_lateral_rise_{side}_mm"]
        dead = k["brow_dead_mm"]
        out[f"brow_inner_up_{side}"] = graded(medial, k["brow_raise_full_mm"], dead)
        out[f"brow_outer_up_{side}"] = graded(lateral, k["brow_raise_full_mm"], dead)
        out[f"brow_lowerer_{side}"] = graded(-(medial + lateral) / 2, k["brow_lower_full_mm"], dead)
        out[f"brow_pinch_{side}"] = graded(measured[f"brow_inward_{side}_mm"], k["pinch_full_mm"], dead)
    out["jaw_open"] = float(np.clip(measured["jaw_gap_mm"] / k["jaw_full_mm"], 0, 1))
    return out


def labels(vertices, neutral, rotation_matrix, marks, normals, scale=None):
    """Labels measured from geometry: `grade(measure(...))`."""
    return grade(measure(vertices, neutral, rotation_matrix, marks, normals), scale)


def tongue_depth(vertices, normals, marks):
    """How far the tongue's part past the lips sinks under the skin around
    the mouth (metres), by each point's nearest skin vertex and its normal: 0
    while it stays outside the face."""
    tongue = vertices[marks.tongue]
    past = tongue[tongue[:, 2] > marks.lip_front - 0.004]
    if not len(past):
        return 0.0
    skin = vertices[marks.mouth_skin]
    skin_normals = normals[marks.mouth_skin]
    depth = 0.0
    for chunk in np.array_split(past, max(1, len(past) // 256)):
        nearest = np.argmin(((chunk[:, None, :] - skin[None, :, :]) ** 2).sum(2), axis=1)
        sunk = -((chunk - skin[nearest]) * skin_normals[nearest]).sum(1)
        depth = max(depth, float(sunk.max()))
    return depth
