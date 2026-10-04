"""Renders synthetic five-camera Quest Pro recordings of GNM heads with
Blender, labelled from geometry, for the universal face model and the
tongue pair.

Run headless (see README.md):

    blender -b --factory-startup -P tools/face-synth/render_face.py -- --count 200

Writes a recording in the `vrft-tongue-capture-v1` format with all five
cameras (`"cameras": [0, 1, 2, 3, 4]`, 2000 x 400 strips) under
`.local/tongue-captures/<unixms>-synthetic-<pid>/`.

- **Heads**: Google's GNM head (v3, Apache-2.0), a NumPy linear blend
  (`gnm_head.py`) with no GNM, TensorFlow or Blender add-on needed. The
  weights download on first use to `.local/gnm/gnm_head.npz`.
- **Expressions**: named prototypes fitted to GNM's expression blocks (jaw,
  lips, brows, tongue), plus tools/tongue-synth's cheek swellings and a
  sideways tongue bend, with small random expression noise, and a share
  of expressions from GNM's semantic sampler (BLOW, SUCK, PUCKER,
  MOUTH_LEFT, TONGUE_CENTER and the rest), run in NumPy from the weights
  `semantic_decoder.py` converts.
- **Labels**: measured from the posed mesh (`gnm_head.measure`, then graded
  by `gnm_head.grade` with the label scales), never copied from what was
  asked for: the tongue's visibility, extension and
  direction from its tip and centreline against the lips, cheek puff and
  suck from the cheeks' movement along the neutral face's normals, brows
  from the brow regions, and jaw open from the front teeth's gap.
- **Cameras**: the mouth pair from the headset's Fisheye62 calibration as in
  tools/tongue-synth. The eye and brow cameras from the same calibration
  file when it has them (`--camera-ids`), else nominal poses documented in
  README.md, which haven't been checked against a headset.
- **Look**: tools/tongue-synth's IR skin, wet tongue and teeth materials,
  IR spot lights by each camera and sensor model.
- **Enrollment**: each person also gets the face setup's poses (neutral,
  jaw open, kiss, puffs, the tongue's five held directions, suck), marked
  with their slot (`anchor`) and the person (`identity`), so the universal
  model can learn to read frames against them.
"""

import argparse
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent / "tongue-synth"))
import gnm_head as gh  # noqa: E402
import render_tongue as rt  # noqa: E402

TARGETS = rt.TARGETS
SIZE = rt.SIZE
CAMERAS = 5
# The strip's views, in camera order: the eyes, the mouth, the brow.
VIEWS = ["eye_left", "eye_right", "mouth_left", "mouth_right", "brow"]
# What the universal face model reads beyond the twelve tongue targets.
FACE_LABELS = ["cheek_suck_left", "cheek_suck_right", "jaw_open",
               "brow_inner_up_left", "brow_inner_up_right", "brow_outer_up_left", "brow_outer_up_right",
               "brow_lowerer_left", "brow_lowerer_right", "brow_pinch_left", "brow_pinch_right"]
# Where the eyeballs' centres sit in the headset frame, as in tongue-synth.
EYES = rt.EYES
# How close the face may come to each camera: the mouth pair's housings sit
# out in front (as in tongue-synth); the eye and brow cameras sit in the
# lens rims and the nose bridge, near the face by design.
CLEARANCE = {"mouth": 0.02, "eye": 0.008, "brow": 0.008}
TONGUE_ATTEMPTS = 8


# ---------------------------------------------------------------- cameras

def look_at(position, target, up=(0.0, 1.0, 0.0)):
    """A DeviceFromCamera matrix (row-major 4x4 list) for a camera at
    `position` looking at `target`: OpenCV axes, x right, y down, z ahead."""
    position, target, up = (np.asarray(v, float) for v in (position, target, up))
    z = target - position
    z /= np.linalg.norm(z)
    down = -(up - (up @ z) * z)
    y = down / np.linalg.norm(down)
    x = np.cross(y, z)
    m = np.eye(4)
    m[:3, :3] = np.stack([x, y, z], 1)
    m[:3, 3] = position
    return m.ravel().tolist()


def nominal_entry(name, position, target, like, horizontal_flip, vertical_flip, focal):
    """A calibration entry for a camera the factory calibration doesn't give
    here: the mouth cameras' lens model with focal length `focal` (pixels),
    at a nominal pose."""
    entry = dict(next(e for e in rt.NOMINAL_CALIBRATION if e["Id"] == like))
    entry.update({"Id": name, "DeviceFromCamera": look_at(position, target),
                  "Projection": {"Model": "PinholeSymmetric", "Coefficients": [focal, 200.0, 200.0]},
                  "HorizontalFlip": horizontal_flip, "VerticalFlip": vertical_flip})
    return entry


# Nominal eye and brow cameras, in the headset frame (x the person's left,
# y up, z away from the face, metres). Estimates, not measured: each eye
# camera below and inside its lens, 2-3 cm from the cornea, looking up at
# it through a narrower lens than the mouth cameras' (about 60 degrees
# across); the brow camera above the nose between the lenses, looking at the
# glabella, forehead up. The eye views follow the mouth pair's flips (the
# left one mirrored). Replace them with a headset's own calibration where it
# has them (--camera-ids), and check the flips against a real five-camera
# recording.
NOMINAL_UPPER = {
    "eye_left": nominal_entry("nominal_eye_left", (0.024, -0.017, 0.008), (0.031, -0.002, -0.008),
                              "cam07_left_mouth", False, True, 330.0),
    "eye_right": nominal_entry("nominal_eye_right", (-0.024, -0.017, 0.008), (-0.031, -0.002, -0.008),
                               "cam08_right_mouth", True, True, 330.0),
    "brow": nominal_entry("nominal_brow", (0.0, 0.0, 0.03), (0.0, 0.016, 0.0),
                          "cam07_left_mouth", False, False, 218.4),
}


def load_cameras(path, camera_ids):
    """The five cameras, in strip order, and where each came from."""
    entries, source = rt.NOMINAL_CALIBRATION, "nominal"
    if path and Path(path).is_file():
        entries, source = json.loads(Path(path).read_text())["CameraCalibration"], "headset"
    by_id = {entry["Id"]: entry for entry in entries}
    cameras, sources = [], []
    for view in VIEWS:
        if view.startswith("mouth"):
            name = rt.MOUTH_CAMERAS[0 if view.endswith("left") else 1]
            cameras.append(rt.LensCamera(by_id[name]))
            sources.append(f"{source}:{name}")
        elif camera_ids.get(view) in by_id:
            cameras.append(rt.LensCamera(by_id[camera_ids[view]]))
            sources.append(f"{source}:{camera_ids[view]}")
        else:
            cameras.append(rt.LensCamera(NOMINAL_UPPER[view]))
            sources.append("nominal")
    return cameras, sources


# ---------------------------------------------------------------- sampling

def sample_identity(rng, gnm):
    """GNM's identity coefficients, and tongue-synth's look for the person."""
    look = rt.sample_identity(rng, {})
    # Eyebrows read as hair, darker dots, even without a beard.
    look["stubble"] = max(look["stubble"], float(rng.uniform(0.5, 0.9)))
    look["beard"] = bool(rng.random() < 0.4)
    look["sclera"] = float(rng.uniform(0.45, 0.7))
    look["iris"] = float(rng.uniform(0.2, 0.45))
    coefficients = np.clip(rng.normal(0, 1, gnm.identity_dim), -2.5, 2.5)
    look["identity"] = coefficients.astype(np.float32)
    return look


def noise(rng, lower=0.25, eyes=0.25):
    """Small expression movement everywhere: the lower face and around the eyes."""
    e = np.zeros(383, np.float32)
    e[gh.LOWER_FACE] = rng.normal(0, lower, len(gh.LOWER_FACE))
    e[gh.EYES] = rng.normal(0, eyes, len(gh.EYES))
    return e


def frame(name, weights=None, deform=None, lower=0.25, eyes=0.25, kind="face"):
    return {"pose": name, "weights": weights or {}, "deform": deform or {}, "lower": lower,
            "eyes": eyes, "kind": kind}


def brows(r):
    side = r.choice(["left", "right", "both"])
    sides = ["left", "right"] if side == "both" else [side]
    which = r.choice(["raise", "inner", "outer", "frown", "pinch"])
    weights = {}
    for s in sides:
        if which in ("raise", "inner"):
            weights[f"brow_inner_up_{s}"] = r.uniform(0.4, 1.6)
        if which in ("raise", "outer"):
            weights[f"brow_outer_up_{s}"] = r.uniform(0.4, 1.6)
        if which == "frown":
            weights[f"brow_lowerer_{s}"] = r.uniform(0.4, 1.6)
            weights[f"brow_pinch_{s}"] = r.uniform(0.0, 1.5)
        if which == "pinch":
            weights[f"brow_pinch_{s}"] = r.uniform(0.6, 1.8)
    return frame(f"Brows: {which} {side}", weights)


def toward_ends(r):
    """-1..1, more often near the ends: a direction is usually held."""
    return float(r.choice([-1.0, 1.0]) * r.uniform(0, 1) ** 0.6)


def tongue_out(r, h=None, v=None, length=None):
    """The tongue out some way: GNM's own tongue for out, up and down, a
    bend for sideways, a stretch for the last few millimetres. `length`
    0..1 goes from the tip just past the lips to about 2 cm out, drawn
    evenly, so short tongues are as common as long ones."""
    h = toward_ends(r) if h is None else h
    v = toward_ends(r) if v is None else v
    length = r.uniform(0, 1) if length is None else length
    weights = {"tongue_out": 0.55 + 0.45 * length, "jaw_open": r.uniform(0.3, 0.55) + 0.35 * max(0.0, -v),
               "lips_part": r.uniform(0.6, 1.2)}
    if v > 0:
        weights["tongue_up"] = v * r.uniform(0.5, 0.9)
    else:
        weights["tongue_down"] = -v * r.uniform(0.5, 0.8)
    deform = {"bend": 0.95 * h, "lift": 0.8 * v, "stretch": r.uniform(0.0, 0.005) * length}
    return frame("Tongue out", weights, deform, lower=0.15)


# What a frame can show, with how often each is drawn. Labels come from the
# geometry, so these only steer what gets rendered.
SCENARIOS = [
    (0.05, lambda r: frame("Neutral", {}, {}, lower=0.15, eyes=0.15)),
    (0.05, lambda r: frame("Speech", {"jaw_open": r.uniform(0.0, 0.5), "lips_part": r.uniform(0, 1)}, {}, lower=0.5)),
    (0.03, lambda r: frame("Smile", {"smile": r.uniform(0.5, 1.5), "jaw_open": r.uniform(0, 0.3)})),
    (0.03, lambda r: frame("Pucker", {"pucker": r.uniform(0.5, 1.3)})),
    (0.03, lambda r: frame("Jaw open", {"jaw_open": r.uniform(0.4, 1.2), "lips_part": r.uniform(0, 1)})),
    (0.03, lambda r: frame("Mouth to one side", {"mouth_right": r.choice([-1.0, 1.0]) * r.uniform(0.5, 1.2)})),
    (0.04, lambda r: frame("Left cheek puffed", {"pucker": r.uniform(0, 0.4)}, {"puff_left": r.uniform(0.4, 1.0)})),
    (0.04, lambda r: frame("Right cheek puffed", {"pucker": r.uniform(0, 0.4)}, {"puff_right": r.uniform(0.4, 1.0)})),
    (0.04, lambda r: frame("Both cheeks puffed", {"pucker": r.uniform(0, 0.4)},
                           {"puff_left": r.uniform(0.4, 1.0), "puff_right": r.uniform(0.4, 1.0)})),
    (0.04, lambda r: frame("Cheeks sucked in", {"pucker": r.uniform(0, 0.6)},
                           {"suck_left": r.uniform(0.4, 1.0), "suck_right": r.uniform(0.4, 1.0)})),
    (0.03, lambda r: frame("Tongue in a cheek", {}, {str(r.choice(["bulge_left", "bulge_right"])): r.uniform(0.5, 1.0)})),
    (0.14, brows),
    (0.25, tongue_out),
    # Left and right on their own, at any length.
    (0.10, lambda r: tongue_out(r, h=r.choice([-1.0, 1.0]) * r.uniform(0.6, 1.0), v=r.uniform(-0.3, 0.3))),
    (0.04, lambda r: frame("Tongue tip", {"tongue_out": r.uniform(0.2, 0.5), "lips_part": 1.0,
                                          "jaw_open": r.uniform(0.1, 0.3)}, lower=0.15)),
]

# The face setup's slots, as the universal model's enrollment reads them.
ENROLLMENT = [
    ("neutral", frame("Relax and look ahead", {}, {}, lower=0.1, eyes=0.1)),
    ("jaw_open", frame("Open your mouth wide", {"jaw_open": 1.2, "lips_part": 1.0})),
    ("pucker", frame("Kiss", {"pucker": 1.2})),
    ("puff", frame("Puff both cheeks", {"pucker": 0.2}, {"puff_left": 0.9, "puff_right": 0.9})),
    ("puff_left", frame("Puff only your left cheek", {"pucker": 0.2}, {"puff_left": 0.9})),
    ("puff_right", frame("Puff only your right cheek", {"pucker": 0.2}, {"puff_right": 0.9})),
    ("tongue_out", frame("Stick your tongue out", gh.STRAIGHT_OUT, dict(gh.STRAIGHT_STRETCH), lower=0.1)),
    ("tongue_up", frame("Point your tongue up", {**gh.STRAIGHT_OUT, "tongue_up": 0.7}, {"lift": 0.6, "stretch": 0.006}, lower=0.1)),
    ("tongue_down", frame("Point your tongue down", {**gh.STRAIGHT_OUT, "tongue_down": 0.7, "jaw_open": 0.9},
                          {"lift": -0.8, "stretch": 0.005}, lower=0.1)),
    ("tongue_left", frame("Point your tongue left", gh.STRAIGHT_OUT, {"bend": -0.9, "stretch": 0.006}, lower=0.1)),
    ("tongue_right", frame("Point your tongue right", gh.STRAIGHT_OUT, {"bend": 0.9, "stretch": 0.006}, lower=0.1)),
    ("suck", frame("Suck in your cheeks", {"pucker": 0.3}, {"suck_left": 0.9, "suck_right": 0.9})),
]


# The share of random frames drawn from GNM's semantic sampler, when it's
# available.
SEMANTIC_SHARE = 0.15


def sample_frame(rng, semantic):
    weights = np.array([w for w, _ in SCENARIOS], float)
    weights /= weights.sum()
    if semantic is not None:
        weights = np.append(weights * (1 - SEMANTIC_SHARE), SEMANTIC_SHARE)
    index = int(rng.choice(len(weights), p=weights))
    if index == len(SCENARIOS):
        name = str(rng.choice(semantic.classes))
        spec = frame(f"Semantic: {name}", lower=0.0, eyes=0.0)
        spec["expression"] = semantic.sample(name, rng)[0]
        spec["semantic"] = name
    else:
        spec = SCENARIOS[index][1](rng)
    spec["step"] = index
    return spec


# ---------------------------------------------------------------- scene

def cornea_material():
    """GNM's eyeball shell: clear, but for the illuminators' glints."""
    mat = bpy.data.materials.new("Cornea")
    if bpy.app.version < (5, 0, 0):
        mat.use_nodes = True
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    nodes.clear()
    output = nodes.new("ShaderNodeOutputMaterial")
    clear = nodes.new("ShaderNodeBsdfTransparent")
    gloss = nodes.new("ShaderNodeBsdfGlossy")
    gloss.inputs["Roughness"].default_value = 0.02
    mix = nodes.new("ShaderNodeMixShader")
    mix.inputs["Fac"].default_value = 0.08
    links.new(clear.outputs["BSDF"], mix.inputs[1])
    links.new(gloss.outputs["BSDF"], mix.inputs[2])
    links.new(mix.outputs["Shader"], output.inputs["Surface"])
    if hasattr(mat, "surface_render_method"):
        mat.surface_render_method = "DITHERED"
    elif hasattr(mat, "blend_method"):
        mat.blend_method = "HASHED"
    return mat


def eye_material(name, albedo, roughness):
    """Glossy, so the illuminators make corneal glints."""
    mat, bsdf, _, _ = rt.material(name, albedo, roughness)
    try:
        bsdf.inputs["Coat Weight"].default_value = 1.0
        bsdf.inputs["Coat Roughness"].default_value = 0.02
    except KeyError:
        pass
    return mat


class Head:
    """One GNM person as a Blender mesh, posed per frame from NumPy."""

    def __init__(self, gnm, prototypes, look):
        self.gnm = gnm
        self.prototypes = prototypes
        self.look = look
        self.neutral = gnm.neutral(look["identity"])
        self.marks = gh.Landmarks(gnm, self.neutral)
        self.deformers = gh.Deformers(gnm, self.neutral, self.marks)
        gh.straight_out(gnm, self.neutral, self.marks, prototypes, self.deformers)
        self.normals = gnm.normals(self.neutral)
        self.eye_centres = gnm.eye_centres(look["identity"])
        g = gnm.group
        self.eyeballs = [g("left_eye"), g("right_eye")]

        mesh = bpy.data.meshes.new("GNM head")
        mesh.from_pydata([tuple(p) for p in self.neutral], [], [tuple(q) for q in gnm.quads])
        mesh.polygons.foreach_set("use_smooth", [True] * len(mesh.polygons))
        self.obj = bpy.data.objects.new("GNM head", mesh)
        bpy.context.scene.collection.objects.link(self.obj)

        # Materials by part, each polygon taking the first part all its
        # corners belong to.
        member = lambda name: np.isin(np.arange(gnm.vertex_count), g(name))
        parts = [("tongue", rt.wet_material("Tongue", look["tongue_albedo"], 2500.0, look["wetness"],
                                            look["papillae"], look["even_coat"])),
                 ("teeth", rt.teeth_material(look["teeth"], look["teeth_gaps"])),
                 ("gums", rt.wet_material("Gums", look["skin"] * 0.8, 2500.0, 0.6, 0.1, True)),
                 ("mouth_sock", rt.material("Cavity", 0.03, 0.6)[0]),
                 # The clear shell over the eyeball, then the eyeball's own
                 # surface beneath it.
                 ("eye_exteriors", cornea_material()),
                 ("pupils", eye_material("Pupil", 0.02, 0.05)),
                 ("irises", eye_material("Iris", look["iris"], 0.1)),
                 ("eyes", eye_material("Sclera", look["sclera"], 0.15))]
        skin = rt.skin_material(look, 1.0)
        mesh.materials.append(skin)
        index = np.zeros(len(gnm.quads), np.int32)
        assigned = np.zeros(len(gnm.quads), bool)
        for slot, (name, mat) in enumerate(parts, start=1):
            mesh.materials.append(mat)
            inside = member(name)[gnm.quads].all(1) & ~assigned
            index[inside] = slot
            assigned |= inside
        mesh.polygons.foreach_set("material_index", index.tolist())

        # The skin shader's lips and beard (here also the eyebrows).
        lips = np.zeros(gnm.vertex_count)
        lips[np.concatenate([g("upper_lip"), g("lower_lip")])] = 1.0
        attr = mesh.attributes.new("lip", "FLOAT", "POINT")
        attr.data.foreach_set("value", lips.astype(np.float32))
        rel = self.neutral - self.marks.mouth
        beard = ((1 - gh.smoothstep(0.012, 0.02, rel[:, 1])) * gh.smoothstep(-0.13, -0.09, rel[:, 1])
                 * (1 - lips) * (1 - gh.smoothstep(0.06, 0.075, np.abs(rel[:, 0]))))
        beard = beard * float(look["beard"])
        brow = np.zeros(gnm.vertex_count)
        for name in ("left_brow_region", "right_brow_region", "middle_brow_region"):
            region = g(name)
            # The hairs grow along the brow's lower half.
            top = self.neutral[region, 1].max()
            brow[region] = np.maximum(brow[region], 1 - gh.smoothstep(top - 0.02, top - 0.008, self.neutral[region, 1]))
        attr = mesh.attributes.new("beard", "FLOAT", "POINT")
        attr.data.foreach_set("value", np.maximum(beard, brow).astype(np.float32))
        sub = self.obj.modifiers.new("Subdivision", "SUBSURF")
        sub.levels = sub.render_levels = 1

    def pose(self, spec, rng):
        """Poses the mesh for `spec`; returns the head-frame vertices."""
        expression = spec.get("expression")
        e = np.zeros(383, np.float32) if expression is None else np.array(expression, np.float32)
        e += noise(rng, spec["lower"], spec["eyes"])
        e += self.prototypes.mix(spec["weights"])
        vertices = self.deformers.apply(self.gnm.posed(self.neutral, e), spec["deform"])
        # Gaze: each eyeball turns about its centre.
        yaw, pitch = rng.normal(0, 12), rng.normal(0, 8)
        turn = gh.rotation((0, 1, 0), math.radians(yaw)) @ gh.rotation((1, 0, 0), math.radians(-pitch))
        for eyeball, centre in zip(self.eyeballs, self.eye_centres):
            vertices[eyeball] = (vertices[eyeball] - centre) @ turn.T + centre
        spec["gaze"] = [round(float(yaw), 2), round(float(pitch), 2)]
        self.obj.data.vertices.foreach_set("co", vertices.astype(np.float32).ravel())
        self.obj.data.update()
        return vertices

    def place(self, pitch, yaw, roll, eyes):
        r, t = gh.placement(self.eye_centres, pitch, yaw, roll, eyes)
        m = Matrix.Identity(4)
        for i in range(3):
            for j in range(3):
                m[i][j] = r[i, j]
            m[i][3] = t[i]
        self.obj.matrix_world = m
        return r, t

    def fit(self, look, lenses):
        """Moves the head back until the face clears every camera."""
        eyes = list(look["eyes"])
        for _ in range(60):
            r, t = gh.placement(self.eye_centres, look["pitch"], look["yaw"], look["roll"], eyes)
            points = self.neutral[self.marks.skin] @ r.T + t
            short = max(CLEARANCE[view.split("_")[0]] - float(np.linalg.norm(points - lens.t, axis=1).min())
                        for view, lens in zip(VIEWS, lenses))
            if short <= 0:
                break
            eyes[2] -= short + 0.0005
        look["eyes"] = eyes


def build_scene(look, sensor, cameras, gnm, prototypes, engine):
    rt.clear_scene()
    scene = bpy.context.scene
    if engine == "cycles":
        scene.render.engine = "CYCLES"
        scene.cycles.device = "CPU"
        scene.cycles.samples = rt.SAMPLES
    else:
        try:
            scene.render.engine = "BLENDER_EEVEE"
        except TypeError:
            scene.render.engine = "BLENDER_EEVEE_NEXT"
        if hasattr(scene, "eevee"):
            scene.eevee.taa_render_samples = rt.SAMPLES
    scene.render.resolution_x = scene.render.resolution_y = rt.PINHOLE
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "OPEN_EXR"
    scene.render.image_settings.color_depth = "16"
    scene.view_settings.view_transform = "Standard"
    world = bpy.data.worlds.new("Dark")
    if bpy.app.version < (5, 0, 0):
        world.use_nodes = True
    background = next(n for n in world.node_tree.nodes if n.type == "BACKGROUND")
    a = sensor["ambient"]
    background.inputs["Color"].default_value = (a, a, a, 1)
    scene.world = world

    head = Head(gnm, prototypes, look)
    head.fit(look, cameras)
    views = []
    offsets = sensor["light_offsets"]
    for n, lens in enumerate(cameras):
        cam = bpy.data.objects.new(f"Camera {n}", bpy.data.cameras.new(f"Camera {n}"))
        cam.data.lens_unit = "FOV"
        cam.data.angle = 2 * math.atan(rt.PINHOLE_TAN)
        cam.data.clip_start = 0.003
        cam.matrix_world = lens.blender_matrix()
        scene.collection.objects.link(cam)
        # An IR illuminator by each camera. The mouth pair's sit about
        # 1.2 cm below them (tongue-synth's fit); the eye and brow cameras'
        # rings sit close around them.
        light = bpy.data.objects.new(f"IR {n}", bpy.data.lights.new(f"IR {n}", "SPOT"))
        mouth = VIEWS[n].startswith("mouth")
        light.data.energy = (0.1 if mouth else 0.05) * (sensor["light_balance"] if n % 2 else 1.0)
        light.data.spot_size = math.radians(sensor["light_cone"])
        light.data.spot_blend = sensor["light_blend"]
        light.data.shadow_soft_size = 0.003
        out, up, forward = offsets[n % 2] if mouth else (0.0, -0.004, 0.0)
        offset = np.array([math.copysign(out, lens.t[0]), up, forward])
        light.matrix_world = Matrix.Translation(Vector(offset)) @ lens.blender_matrix()
        scene.collection.objects.link(light)
        views.append((cam, lens.warp()))
    return scene, head, views


# ---------------------------------------------------------------- output

def graded(measured, scale):
    """The sample's tongue `targets` and `face` labels from measurements."""
    values = gh.grade(measured, scale)
    targets = [0.0] * len(TARGETS)
    for column, name in enumerate(TARGETS):
        if name in values:
            targets[column] = round(values[name], 5)
    face = {name: round(values[name], 5) for name in FACE_LABELS}
    return targets, face


def labelled(head, vertices, r, scale):
    measured = gh.measure(vertices, head.neutral, r, head.marks, head.normals)
    return (*graded(measured, scale), measured)


def semantic_sampler(path, enabled):
    """GNM's semantic sampler from `path`, converted first by
    semantic_decoder.py with a Python that has h5py when it's missing; None,
    with a warning, when that can't be done."""
    if not enabled:
        return None
    if not path.is_file():
        script = HERE / "semantic_decoder.py"
        tried = []
        for python in filter(None, [os.environ.get("VRFT_PYTHON"), "python3", "python", "py"]):
            if not shutil.which(python):
                continue
            tried.append(python)
            done = subprocess.run([python, str(script), "--out", str(path)], capture_output=True, text=True)
            if done.returncode == 0 and path.is_file():
                print(f"face-synth: converted GNM's semantic sampler with {python}", flush=True)
                break
        if not path.is_file():
            print(f"face-synth: WARNING: no semantic expressions: {path} is missing and no Python with h5py "
                  f"could make it (tried {tried or 'none on PATH'}). Run `pip install h5py numpy` and "
                  f"`python tools/face-synth/semantic_decoder.py`, or pass --no-semantic.", flush=True)
            return None
    return gh.SemanticSampler(path)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    repo = HERE.parents[1]
    parser = argparse.ArgumentParser(prog="render_face.py")
    parser.add_argument("--count", type=int, default=200, help="frames to render, besides enrollment poses")
    parser.add_argument("--seed", type=int, default=int(time.time()))
    parser.add_argument("--identities", type=int, default=0, help="people; 0 picks one per 40 frames")
    parser.add_argument("--out", type=Path, default=repo / ".local" / "tongue-captures")
    parser.add_argument("--fast", action="store_true", help="half-size renders with fewer samples")
    parser.add_argument("--engine", choices=["eevee", "cycles"], default="eevee",
                        help="cycles renders on the CPU, for machines without a GPU")
    parser.add_argument("--no-enrollment", action="store_true", help="skip each person's face setup poses")
    parser.add_argument("--gnm", type=Path, default=repo / ".local" / "gnm" / "gnm_head.npz",
                        help="the GNM head file; downloaded there when missing")
    parser.add_argument("--semantic", type=Path, default=repo / ".local" / "gnm" / "semantic_decoder.npz",
                        help="GNM's semantic sampler, from semantic_decoder.py; converted there when missing")
    parser.add_argument("--no-semantic", action="store_true", help="no semantic-sampler expressions")
    parser.add_argument("--label-scales", type=Path,
                        help="a JSON file overriding gnm_head.SCALES, what each label's 1 means")
    parser.add_argument("--calibration", type=Path,
                        default=repo / ".local" / "headset-calibration" / "ft_calib.scio.json",
                        help="the headset's ft_calib.scio.json; nominal values when it's missing")
    parser.add_argument("--camera-ids", default="",
                        help="calibration ids of the eye and brow cameras, as eye_left=<id>,eye_right=<id>,brow=<id>")
    parser.add_argument("--list-calibration", action="store_true",
                        help="print the cameras the calibration file has, then stop")
    args = parser.parse_args(argv)
    if args.list_calibration:
        entries = json.loads(args.calibration.read_text())["CameraCalibration"]
        for entry in entries:
            print(f"face-synth: {entry['Id']} {entry.get('ImageSize')} {entry['Projection']['Model']}")
        return
    if args.fast:
        rt.PINHOLE, rt.SAMPLES = 400, 8
    camera_ids = dict(part.split("=", 1) for part in args.camera_ids.split(",") if "=" in part)

    gnm = gh.Gnm(gh.fetch(args.gnm))
    prototypes = gh.Prototypes(gnm)
    semantic = semantic_sampler(args.semantic, not args.no_semantic)
    scale = gh.scales(args.label_scales)
    cameras, sources = load_cameras(args.calibration, camera_ids)
    rng = np.random.default_rng(args.seed)
    people = args.identities or max(1, math.ceil(args.count / 40))
    per_person = math.ceil(args.count / people)

    millis = int(time.time() * 1000)
    directory = args.out / f"{millis}-synthetic-{os.getpid()}"
    directory.mkdir(parents=True)
    width = CAMERAS * SIZE
    metadata = {
        "format": "vrft-tongue-capture-v1", "mode": "synthetic", "width": width, "height": SIZE,
        "bytesPerFrame": width * SIZE, "cameras": list(range(CAMERAS)), "targets": TARGETS,
        "synthetic": {
            "generator": "tools/face-synth/render_face.py", "version": 1, "seed": args.seed,
            "identities": people, "blender": bpy.app.version_string, "faces": f"GNM head v{gnm.version}",
            "cameras": dict(zip(VIEWS, sources)), "pinhole": rt.PINHOLE, "samples": rt.SAMPLES,
            "engine": args.engine, "enrollment": not args.no_enrollment,
            "semantic": None if semantic is None else {"classes": list(semantic.classes), "share": SEMANTIC_SHARE},
            "labels": "measured from geometry (tools/face-synth/gnm_head.py)", "label_scales": scale,
        },
    }
    (directory / "metadata.json").write_text(json.dumps(metadata, indent=2))
    print(f"face-synth: cameras {dict(zip(VIEWS, sources))}", flush=True)
    scratch = Path(tempfile.mkdtemp(prefix="vrft-face-synth-"))
    started = time.time()
    index = rendered = 0
    rejected = {}
    with open(directory / "frames.gray8", "wb") as frames, open(directory / "samples.jsonl", "w") as lines:
        for person in range(people):
            look = sample_identity(rng, gnm)
            sensor = rt.sample_sensor(rng)
            lenses = [lens.perturbed(rng) for lens in cameras]
            scene, head, views = build_scene(look, sensor, lenses, gnm, prototypes, args.engine)
            identity = f"gnm-{args.seed}-{person}"
            todo = [] if args.no_enrollment else [(slot, dict(spec, step=1000 + n))
                                                   for n, (slot, spec) in enumerate(ENROLLMENT)]
            todo += [(None, None)] * min(per_person, args.count - rendered)
            gains = {}
            for slot, spec in todo:
                for attempt in range(TONGUE_ATTEMPTS + 1):
                    current = dict(spec) if spec else sample_frame(rng, semantic)
                    vertices = head.pose(current, rng)
                    r, _ = head.place(look["pitch"] + rng.normal(0, 1.0), look["yaw"] + rng.normal(0, 0.7),
                                      look["roll"] + rng.normal(0, 0.7),
                                      list(np.add(look["eyes"], rng.normal(0, 0.001, 3))))
                    targets, face, measured = labelled(head, vertices, r, scale)
                    sunk = gh.tongue_depth(vertices, gnm.normals(vertices), head.marks) if targets[0] else 0.0
                    if sunk <= gh.TONGUE_DEPTH or attempt == TONGUE_ATTEMPTS:
                        break
                    rejected[current["pose"]] = rejected.get(current["pose"], 0) + 1
                radiance = [rt.render_view(scene, cam, warp, str(scratch / f"{n}.exr"))
                            for n, (cam, warp) in enumerate(views)]
                strip = []
                for n, (r_view, (_, warp)) in enumerate(zip(radiance, views)):
                    # Each camera group exposes on its own: the eyes, the
                    # mouth and the brow.
                    group = VIEWS[n].split("_")[0]
                    settled = sensor["brightness"] / max(np.percentile(r_view, 99), 1e-6)
                    gain = gains.get(group)
                    gain = settled if gain is None else math.exp(0.4 * math.log(gain) + 0.6 * math.log(settled))
                    gains[group] = gain
                    strip.append(rt.sensor_image(r_view, warp[2], sensor, gain * float(np.exp(rng.normal(0, 0.1))), rng))
                frames.write(np.concatenate(strip, axis=1).tobytes())
                line = {
                    "index": index, "sequence": index, "pose": "Synthetic: " + current["pose"],
                    "step": current["step"], "round": 1, "targets": targets, "native_tongue_out": None,
                    "captured_unix_ms": int(time.time() * 1000),
                    # Marks every frame as distinct, so training keeps them all.
                    "dot": [targets[2], targets[3]],
                    "face": face, "identity": identity,
                    "synthetic": {"weights": {k: round(float(v), 3) for k, v in current["weights"].items()},
                                  "deform": {k: round(float(v), 4) for k, v in current["deform"].items()},
                                  "gaze": current.get("gaze"),
                                  "semantic": current.get("semantic"),
                                  # What the labels are graded from: relabel.py
                                  # grades them again with other scales.
                                  "measured": measured,
                                  "tongue_depth_mm": round(sunk * 1000, 2)},
                }
                if slot:
                    line["anchor"] = slot
                lines.write(json.dumps(line) + "\n")
                index += 1
                if slot is None:
                    rendered += 1
                if index % 10 == 0:
                    print(f"face-synth: {index} frames ({index / (time.time() - started):.2f}/s)", flush=True)
    shutil.rmtree(scratch, ignore_errors=True)
    metadata["synthetic"]["rejected_poses"] = rejected
    metadata["synthetic"]["frames"] = index
    (directory / "metadata.json").write_text(json.dumps(metadata, indent=2))
    print(f"face-synth: drew {sum(rejected.values())} tongue poses again: {rejected}", flush=True)
    print(f"face-synth: wrote {directory}", flush=True)


if __name__ == "__main__":
    main()
