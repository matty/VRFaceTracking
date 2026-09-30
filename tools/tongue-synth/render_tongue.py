"""Renders synthetic Quest Pro mouth-camera recordings with Blender.

Run headless (see README.md):

    blender -b --factory-startup -P tools/tongue-synth/render_tongue.py -- --count 200

Writes a recording in the same `vrft-tongue-capture-v1` format the daemon
saves, under `.local/tongue-captures/<unixms>-synthetic-<pid>/`, so it shows
up in the Train tab beside real recordings. Every frame is a random face,
mouth and tongue with its exact label.

The scene is procedural (no assets): a lower-face sheet with a mouth, jaw,
lips, cheeks and chin; a nose; teeth; a dark mouth cavity; and a tongue
built along a bending centreline. Two spot lights beside the two cameras
stand in for the headset's IR illuminators. Blender units are metres; the
mouth sits at the origin, the face looks down -Y, +Z is up and the person's
left is +X.
"""

import argparse
import json
import math
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

import bpy
import numpy as np
from mathutils import Matrix, Vector

SIZE = 400
TARGETS = [
    "visibility", "extension", "horizontal", "vertical", "curl_up", "bend_down",
    "roll", "flat", "squish", "twist", "cheek_puff_left", "cheek_puff_right",
]
# The real strips show the person's left on the image's left, the mirror of
# a camera facing the face.
MIRROR = True

# Face sheet: rings out from the lip line, angles around the mouth.
RINGS, ANGLES = 30, 96
# Tongue: rings along the centreline, points around each ring.
T_ALONG, T_AROUND = 44, 28
JAW_PIVOT = np.array([0.0, 0.075, 0.025])


def smoothstep(edge0, edge1, x):
    t = np.clip((x - edge0) / (edge1 - edge0), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def rotate_x(points, pivot, angles):
    """Rotates each point about the X axis through `pivot` by its angle."""
    d = points - pivot
    c, s = np.cos(angles), np.sin(angles)
    out = points.copy()
    out[:, 1] = pivot[1] + d[:, 1] * c - d[:, 2] * s
    out[:, 2] = pivot[2] + d[:, 1] * s + d[:, 2] * c
    return out


# ---------------------------------------------------------------- sampling

def sample_identity(rng):
    """What stays fixed for one synthetic person."""
    return {
        "face_rx": rng.uniform(0.068, 0.084),
        "face_ry": rng.uniform(0.082, 0.098),
        "mouth_width": rng.uniform(0.021, 0.027),
        "lip_prominence": rng.uniform(0.004, 0.009),
        "chin_prominence": rng.uniform(0.2, 1.0),
        "chin_drop": rng.uniform(0.065, 0.085),
        "skin": rng.uniform(0.35, 0.75),
        "lip_tint": rng.uniform(0.75, 0.95),
        "stubble": rng.choice([0.0, 0.0, rng.uniform(0.2, 1.0)]),
        "stubble_scale": rng.uniform(250.0, 600.0),
        "tongue_width": rng.uniform(0.042, 0.054),
        "tongue_length": rng.uniform(0.9, 1.1),
        "tongue_albedo": rng.uniform(0.3, 0.55),
    }


def sample_frame(rng):
    """One frame's expression, tongue and label."""
    kind = rng.choice(["hidden", "visible", "cheeks"], p=[0.35, 0.45, 0.20])
    frame = {
        "kind": kind, "jaw": 0.0, "smile": 0.0, "pucker": 0.0,
        "ext": 0.0, "h": 0.0, "v": 0.0, "curl": 0.0,
        "puff_left": 0.0, "puff_right": 0.0,
    }
    if kind == "hidden":
        mouth = rng.choice(["relaxed", "open", "smile", "pucker", "speech"])
        frame["jaw"] = {"relaxed": rng.uniform(0.0, 0.08), "open": rng.uniform(0.4, 1.0),
                        "smile": rng.uniform(0.0, 0.3), "pucker": rng.uniform(0.0, 0.1),
                        "speech": rng.uniform(0.05, 0.5)}[mouth]
        frame["smile"] = rng.uniform(0.5, 1.0) if mouth == "smile" else rng.uniform(0.0, 0.15)
        frame["pucker"] = rng.uniform(0.5, 1.0) if mouth == "pucker" else 0.0
        frame["pose"] = "Synthetic: tongue hidden"
        frame["step"] = 0
    elif kind == "visible":
        # Spread evenly over the direction disc, with some straight out.
        if rng.random() < 0.2:
            h = v = 0.0
        else:
            r, a = math.sqrt(rng.random()), rng.uniform(0, 2 * math.pi)
            h, v = r * math.cos(a), r * math.sin(a)
        frame.update(h=h, v=v, ext=rng.uniform(0.15, 1.0), curl=0.0)
        frame["jaw"] = rng.uniform(0.45, 0.8) + 0.2 * max(0.0, -v)
        frame["smile"] = rng.uniform(0.0, 0.2)
        frame["pose"] = "Synthetic: tongue out"
        frame["step"] = 1
    else:
        which = rng.choice(["left", "right", "both"])
        amount = rng.uniform(0.45, 1.0)
        frame["puff_left"] = amount if which in ("left", "both") else 0.0
        frame["puff_right"] = amount if which in ("right", "both") else 0.0
        frame["pucker"] = rng.uniform(0.0, 0.3)
        frame["pose"] = f"Synthetic: {which} cheek puffed" if which != "both" else "Synthetic: both cheeks puffed"
        frame["step"] = {"left": 2, "right": 3, "both": 4}[which]
    visible = kind == "visible"
    targets = [0.0] * len(TARGETS)
    targets[0] = 1.0 if visible else 0.0
    targets[1] = frame["ext"] if visible else 0.0
    targets[2] = frame["h"] if visible else 0.0
    targets[3] = frame["v"] if visible else 0.0
    targets[10] = frame["puff_left"]
    targets[11] = frame["puff_right"]
    frame["targets"] = [round(float(t), 5) for t in targets]
    return frame


def sample_rig(rng):
    """Where the headset sits on this face: cameras, lights and exposure."""
    return {
        "cam_origin": [rng.normal(0.0, 0.008), rng.uniform(-0.088, -0.072), rng.uniform(0.028, 0.042)],
        "cam_target": [rng.normal(0.0, 0.004), rng.uniform(-0.01, 0.005), rng.uniform(-0.038, -0.024)],
        "baseline": rng.uniform(0.004, 0.009),
        "fov": rng.uniform(66.0, 78.0),
        "roll": rng.normal(-8.0, 4.0),
        "light_power": rng.uniform(0.08, 0.18),
        "gain": rng.uniform(0.85, 1.15),
        "vignette": rng.uniform(0.35, 0.65),
        "noise": rng.uniform(0.005, 0.02),
    }


# ---------------------------------------------------------------- scene

def material(name, color, roughness):
    mat = bpy.data.materials.new(name)
    if bpy.app.version < (5, 0, 0):
        mat.use_nodes = True
    nodes, links = mat.node_tree.nodes, mat.node_tree.links
    bsdf = next(n for n in nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = (color, color, color, 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    return mat, bsdf, nodes, links


def skin_material(identity):
    mat, bsdf, nodes, links = material("Skin", identity["skin"], 0.5)
    # Lips: a darker, glossier band read from the face sheet's `lip` attribute.
    attr = nodes.new("ShaderNodeAttribute")
    attr.attribute_name = "lip"
    tint = nodes.new("ShaderNodeMix")
    tint.data_type = "RGBA"
    s = identity["skin"]
    tint.inputs["A"].default_value = (s, s, s, 1.0)
    t = s * identity["lip_tint"]
    tint.inputs["B"].default_value = (t, t, t, 1.0)
    links.new(attr.outputs["Fac"], tint.inputs["Factor"])
    links.new(tint.outputs["Result"], bsdf.inputs["Base Color"])
    rough = nodes.new("ShaderNodeMapRange")
    rough.inputs["To Min"].default_value = 0.5
    rough.inputs["To Max"].default_value = 0.22
    links.new(attr.outputs["Fac"], rough.inputs["Value"])
    links.new(rough.outputs["Result"], bsdf.inputs["Roughness"])
    # Pores as a fine bump; stubble as dark dots.
    noise = nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = 900.0
    bump = nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.15
    bump.inputs["Distance"].default_value = 0.0003
    links.new(noise.outputs["Fac"], bump.inputs["Height"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    if identity["stubble"] > 0:
        dots = nodes.new("ShaderNodeTexVoronoi")
        dots.inputs["Scale"].default_value = identity["stubble_scale"]
        ramp = nodes.new("ShaderNodeMapRange")
        ramp.inputs["From Min"].default_value = 0.0
        ramp.inputs["From Max"].default_value = 0.25
        ramp.inputs["To Min"].default_value = 1.0 - 0.6 * identity["stubble"]
        ramp.inputs["To Max"].default_value = 1.0
        links.new(dots.outputs["Distance"], ramp.inputs["Value"])
        # No stubble on the lips.
        keep = nodes.new("ShaderNodeMath")
        keep.operation = "MAXIMUM"
        links.new(ramp.outputs["Result"], keep.inputs[0])
        links.new(attr.outputs["Fac"], keep.inputs[1])
        darken = nodes.new("ShaderNodeMix")
        darken.data_type = "RGBA"
        darken.blend_type = "MULTIPLY"
        darken.inputs["Factor"].default_value = 1.0
        links.new(tint.outputs["Result"], darken.inputs["A"])
        links.new(keep.outputs["Value"], darken.inputs["B"])
        links.new(darken.outputs["Result"], bsdf.inputs["Base Color"])
    try:
        bsdf.inputs["Subsurface Weight"].default_value = 0.25
        bsdf.inputs["Subsurface Radius"].default_value = (0.004, 0.004, 0.004)
    except KeyError:
        pass
    return mat


def wet_material(name, albedo, bump_scale):
    mat, bsdf, nodes, links = material(name, albedo, 0.3)
    try:
        bsdf.inputs["Coat Weight"].default_value = 0.6
        bsdf.inputs["Coat Roughness"].default_value = 0.08
    except KeyError:
        pass
    noise = nodes.new("ShaderNodeTexNoise")
    noise.inputs["Scale"].default_value = bump_scale
    bump = nodes.new("ShaderNodeBump")
    bump.inputs["Strength"].default_value = 0.35
    bump.inputs["Distance"].default_value = 0.0004
    links.new(noise.outputs["Fac"], bump.inputs["Height"])
    links.new(bump.outputs["Normal"], bsdf.inputs["Normal"])
    return mat


def mesh_object(name, verts, faces, mat, smooth=True, subdivide=1):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata([tuple(v) for v in verts], [], faces)
    mesh.polygons.foreach_set("use_smooth", [smooth] * len(mesh.polygons))
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    obj.data.materials.append(mat)
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


def ellipsoid(name, center, radii, mat, flip=False, segments=32, rings=16):
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
    return mesh_object(name, verts, faces, mat)


class Face:
    """The lower-face sheet around the mouth, deformed per frame."""

    def __init__(self, identity, mat):
        self.id = identity
        theta = np.linspace(0.0, 2 * np.pi, ANGLES, endpoint=False)
        u = (np.arange(RINGS) / (RINGS - 1)) ** 1.7
        self.theta, self.u = np.meshgrid(theta, u)
        self.theta, self.u = self.theta.ravel(), self.u.ravel()
        faces = grid_faces(RINGS, ANGLES, True)
        self.obj = mesh_object("Face", self.positions(0, 0, 0, 0, 0), faces, mat, subdivide=1)
        lip = 1.0 - smoothstep(0.08, 0.13, self.u)
        attr = self.obj.data.attributes.new("lip", "FLOAT", "POINT")
        attr.data.foreach_set("value", lip.astype(np.float32))

    def positions(self, jaw, smile, pucker, puff_left, puff_right):
        i = self.id
        th, u = self.theta, self.u
        width = i["mouth_width"] * (1 + 0.25 * smile - 0.35 * pucker)
        inner = np.stack([width * np.cos(th), 0.0012 * np.sin(th)], axis=1)
        lower = np.sin(th) < 0
        outer_z = np.where(lower, 0.1, 0.11)
        outer = np.stack([0.13 * np.cos(th), outer_z * np.sin(th)], axis=1)
        xz = inner + (outer - inner) * u[:, None]
        x, z = xz[:, 0], xz[:, 1]
        # The face as an ellipsoid, except below the mouth, where the chin
        # keeps a flat front and then turns sharply under.
        rx, ry, cy = i["face_rx"], i["face_ry"], 0.08
        rz = np.where(z < 0, i["chin_drop"], 0.12)
        zz = np.where(z < 0, (z / rz) ** 4, (z / rz) ** 2)
        depth = np.sqrt(np.clip(1 - (x / rx) ** 2 - zz, 0.0, 1.0))
        y = cy - ry * depth
        mid = np.exp(-(x / 0.03) ** 2)
        # Lips roll inward at the line and bulge out.
        y -= i["lip_prominence"] * (1 + 0.8 * pucker) * np.sin(np.pi * np.clip(u / 0.12, 0, 1))
        y += 0.003 * (u < 0.01)
        # Groove under the lower lip, then the chin.
        y += 0.004 * mid * np.exp(-((z + 0.022) / 0.007) ** 2)
        y -= 0.006 * i["chin_prominence"] * mid * np.exp(-((z + 0.047) / 0.012) ** 2)
        # Smile: the corners go out, up and back.
        corner = np.exp(-(((np.abs(x) - width) ** 2 + z ** 2) / 0.012 ** 2))
        x += smile * 0.004 * np.sign(x) * corner
        z += smile * 0.005 * corner
        y += smile * 0.003 * corner
        points = np.stack([x, y, z], axis=1)
        # Cheek puffs bulge outward from each cheek.
        normal = np.stack([x / rx ** 2, (y - cy) / ry ** 2, z / rz ** 2], axis=1)
        normal /= np.linalg.norm(normal, axis=1, keepdims=True) + 1e-9
        for side, amount in ((1.0, puff_left), (-1.0, puff_right)):
            if amount > 0:
                d2 = (x - side * 0.042) ** 2 + (z + 0.005) ** 2
                points += normal * (amount * 0.012 * np.exp(-d2 / 0.022 ** 2))[:, None]
        # The jaw swings the lower lip and chin open about the hinge.
        weight = lower * smoothstep(0.0, 0.35, -np.sin(th)) * (1 - smoothstep(0.04, 0.085, np.abs(x)))
        return rotate_x(points, JAW_PIVOT, jaw * 0.35 * weight)

    def update(self, frame):
        p = self.positions(frame["jaw"], frame["smile"], frame["pucker"],
                           frame["puff_left"], frame["puff_right"])
        self.obj.data.vertices.foreach_set("co", p.astype(np.float32).ravel())
        self.obj.data.update()


class Tongue:
    """A tongue swept along a centreline that leaves the mouth and bends."""

    def __init__(self, identity, mat):
        self.id = identity
        faces = grid_faces(T_ALONG, T_AROUND, True)
        self.obj = mesh_object("Tongue", self.positions(0, 0, 0, 0, 0), faces, mat, subdivide=1)

    def positions(self, ext, h, v, curl, jaw, hidden=True):
        i = self.id
        root = np.array([0.0, 0.052, -0.013])
        if hidden:
            length = 0.05 * i["tongue_length"]
        else:
            length = (0.07 + 0.04 * ext) * i["tongue_length"]
        s = np.linspace(0.0, 1.0, T_ALONG)
        inside = 0.065 / length
        bend = smoothstep(inside - 0.2, 1.0, s)
        # Horizontal +1 is toward the person's right, -X.
        yaw = h * 1.0 * bend
        pitch = v * 0.95 * bend + curl * 1.2 * smoothstep(0.7, 1.0, s) - 0.12
        tangent = np.stack([
            -np.sin(yaw) * np.cos(pitch),
            -np.cos(yaw) * np.cos(pitch),
            np.sin(pitch),
        ], axis=1)
        step = length / (T_ALONG - 1)
        centre = root + np.vstack([[0, 0, 0], np.cumsum(tangent[:-1] * step, axis=0)])
        # Parallel-transported side vector.
        side = np.zeros_like(tangent)
        prev = np.array([1.0, 0.0, 0.0])
        for k in range(T_ALONG):
            prev = prev - prev.dot(tangent[k]) * tangent[k]
            prev /= np.linalg.norm(prev)
            side[k] = prev
        up = np.cross(tangent, side)
        width = i["tongue_width"] * (1 - 0.3 * s) * (1 - 0.2 * ext)
        from_tip = (1 - s) * length
        tip = 0.02
        rounding = np.sqrt(np.clip(1 - ((tip - np.minimum(from_tip, tip)) / tip) ** 2, 0.0, 1.0))
        width = width * rounding
        thick = width * (0.5 + 0.12 * ext)
        phi = np.linspace(0, 2 * np.pi, T_AROUND, endpoint=False)
        cos, sin = np.cos(phi), np.sin(phi)
        top = np.where(sin > 0, 0.85, 1.0)
        # A shallow groove down the middle of the top.
        groove = 0.12 * np.exp(-(cos / 0.25) ** 2) * (sin > 0)
        lateral = (width[:, None] / 2) * cos[None, :]
        vertical = (thick[:, None] / 2) * (sin * top - groove)[None, :]
        points = (centre[:, None, :] + side[:, None, :] * lateral[..., None]
                  + up[:, None, :] * vertical[..., None]).reshape(-1, 3)
        # It rides the lower jaw.
        return rotate_x(points, JAW_PIVOT, np.full(len(points), jaw * 0.35 * 0.85))

    def update(self, frame):
        p = self.positions(frame["ext"], frame["h"], frame["v"], frame["curl"], frame["jaw"],
                           hidden=frame["kind"] != "visible")
        self.obj.data.vertices.foreach_set("co", p.astype(np.float32).ravel())
        self.obj.data.update()


class Teeth:
    def __init__(self, mat):
        self.upper = self.arc("Upper teeth", 0.006, -0.003, mat)
        self.lower = self.arc("Lower teeth", -0.003, -0.011, mat)
        self.lower_base = np.array([v.co[:] for v in self.lower.data.vertices])

    @staticmethod
    def arc(name, top, bottom, mat):
        verts = []
        steps = 24
        for k in range(steps):
            a = math.radians(-65 + 130 * k / (steps - 1))
            for z in (top, bottom):
                verts.append((0.022 * math.sin(a), 0.016 - 0.022 * math.cos(a), z))
        faces = [(2 * k, 2 * k + 2, 2 * k + 3, 2 * k + 1) for k in range(steps - 1)]
        obj = mesh_object(name, verts, faces, mat, subdivide=0)
        solid = obj.modifiers.new("Thickness", "SOLIDIFY")
        solid.thickness = 0.004
        return obj

    def update(self, frame):
        p = rotate_x(self.lower_base, JAW_PIVOT, np.full(len(self.lower_base), frame["jaw"] * 0.35))
        self.lower.data.vertices.foreach_set("co", p.astype(np.float32).ravel())
        self.lower.data.update()


def look_at(obj, target, roll_deg):
    direction = Vector(target) - obj.location
    rotation = direction.to_track_quat("-Z", "Y").to_matrix()
    obj.rotation_mode = "XYZ"
    obj.rotation_euler = (rotation @ Matrix.Rotation(math.radians(roll_deg), 3, "Z")).to_euler()


def build_scene(identity):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    try:
        scene.render.engine = "BLENDER_EEVEE"
    except TypeError:
        scene.render.engine = "BLENDER_EEVEE_NEXT"
    if hasattr(scene, "eevee"):
        scene.eevee.taa_render_samples = 16
    scene.render.resolution_x = scene.render.resolution_y = SIZE
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.render.image_settings.color_mode = "BW"
    scene.view_settings.view_transform = "Standard"
    world = bpy.data.worlds.new("Dark")
    if bpy.app.version < (5, 0, 0):
        world.use_nodes = True
    background = next(n for n in world.node_tree.nodes if n.type == "BACKGROUND")
    background.inputs["Color"].default_value = (0, 0, 0, 1)
    scene.world = world

    face = Face(identity, skin_material(identity))
    ellipsoid("Nose", (0.0, 0.002, 0.047), (0.017, 0.03, 0.02), face.obj.data.materials[0])
    ellipsoid("Mouth cavity", (0.0, 0.04, -0.01), (0.03, 0.04, 0.028),
              material("Cavity", 0.03, 0.6)[0], flip=True)
    teeth = Teeth(material("Teeth", 0.75, 0.2)[0])
    tongue = Tongue(identity, wet_material("Tongue", identity["tongue_albedo"], 1500.0))
    ellipsoid("Neck", (0.0, 0.1, -0.13), (0.06, 0.06, 0.08), face.obj.data.materials[0])
    ellipsoid("Shirt", (0.0, 0.08, -0.3), (0.22, 0.14, 0.14), material("Cloth", 0.12, 0.9)[0])

    cams, lights = [], []
    for name in ("Left", "Right"):
        cam = bpy.data.objects.new(f"Camera {name}", bpy.data.cameras.new(name))
        cam.data.lens_unit = "FOV"
        cam.data.clip_start = 0.005
        scene.collection.objects.link(cam)
        cams.append(cam)
        light = bpy.data.objects.new(f"IR {name}", bpy.data.lights.new(name, "SPOT"))
        light.data.spot_size = math.radians(120)
        light.data.spot_blend = 1.0
        light.data.shadow_soft_size = 0.004
        scene.collection.objects.link(light)
        lights.append(light)
    return scene, face, teeth, tongue, cams, lights


def place_rig(rig, cams, lights):
    origin = Vector(rig["cam_origin"])
    target = Vector(rig["cam_target"])
    # Cameras side by side along X, the strip's left half from the -X one:
    # that matches the real strips, whose right view sees the face about
    # 18 px further right (at 400 px) than the left view.
    offsets = (-rig["baseline"] / 2, rig["baseline"] / 2)
    for cam, light, dx in zip(cams, lights, offsets):
        cam.location = origin + Vector((dx, 0, 0))
        cam.data.angle = math.radians(rig["fov"])
        look_at(cam, target + Vector((dx, 0, 0)), rig["roll"])
        light.location = cam.location + Vector((0.0, 0.004, 0.003))
        light.data.energy = rig["light_power"]
        look_at(light, target, 0)


# ---------------------------------------------------------------- output

def render_view(scene, cam, path):
    scene.camera = cam
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    image = bpy.data.images.load(path)
    pixels = np.empty(SIZE * SIZE * 4, dtype=np.float32)
    image.pixels.foreach_get(pixels)
    bpy.data.images.remove(image)
    gray = pixels.reshape(SIZE, SIZE, 4)[::-1, :, 0]
    return gray[:, ::-1] if MIRROR else gray


def sensor(view, rig, rng):
    """The camera's own look: gain, falloff to the corners, blur and noise."""
    yy, xx = np.mgrid[0:SIZE, 0:SIZE] / (SIZE - 1) * 2 - 1
    falloff = np.clip(1 - rig["vignette"] * (xx ** 2 + yy ** 2) / 2, 0.0, 1.0)
    out = view * rig["gain"] * falloff
    if rng.random() < 0.5:
        padded = np.pad(out, 1, mode="edge")
        out = sum(padded[dy:dy + SIZE, dx:dx + SIZE] for dy in range(3) for dx in range(3)) / 9
    out = out + rng.normal(0, rig["noise"], out.shape)
    return (np.clip(out, 0, 1) * 255 + 0.5).astype(np.uint8)


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    repo = Path(__file__).resolve().parents[2]
    parser = argparse.ArgumentParser(prog="render_tongue.py")
    parser.add_argument("--count", type=int, default=200, help="frames to render")
    parser.add_argument("--seed", type=int, default=int(time.time()))
    parser.add_argument("--identities", type=int, default=0,
                        help="synthetic people; 0 picks one per 50 frames")
    parser.add_argument("--out", type=Path, default=repo / ".local" / "tongue-captures")
    args = parser.parse_args(argv)

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
            "generator": "tools/tongue-synth/render_tongue.py", "version": 1,
            "seed": args.seed, "identities": people, "blender": bpy.app.version_string,
        },
    }
    (directory / "metadata.json").write_text(json.dumps(metadata, indent=2))
    scratch = Path(tempfile.mkdtemp(prefix="vrft-synth-"))
    started = time.time()
    index = 0
    with open(directory / "frames.gray8", "wb") as frames, \
            open(directory / "samples.jsonl", "w") as labels:
        while index < args.count:
            identity = sample_identity(rng)
            scene, face, teeth, tongue, cams, lights = build_scene(identity)
            rig = sample_rig(rng)
            for _ in range(min(per_person, args.count - index)):
                frame = sample_frame(rng)
                # Small shifts of the headset on the face while it's worn.
                jitter = dict(rig, cam_origin=list(np.add(rig["cam_origin"], rng.normal(0, 0.0015, 3))))
                place_rig(jitter, cams, lights)
                face.update(frame)
                teeth.update(frame)
                tongue.update(frame)
                views = [sensor(render_view(scene, cam, str(scratch / f"{n}.png")), rig, rng)
                         for n, cam in enumerate(cams)]
                frames.write(np.concatenate(views, axis=1).tobytes())
                labels.write(json.dumps({
                    "index": index, "sequence": index, "pose": frame["pose"], "step": frame["step"],
                    "round": 1, "targets": frame["targets"], "native_tongue_out": None,
                    "captured_unix_ms": int(time.time() * 1000),
                    # Marks every frame as distinct, so training keeps them all.
                    "dot": [frame["h"], frame["v"]],
                    "synthetic": {k: frame[k] for k in ("jaw", "smile", "pucker", "ext", "curl")},
                }) + "\n")
                index += 1
                if index % 25 == 0:
                    rate = index / (time.time() - started)
                    print(f"tongue-synth: {index}/{args.count} frames ({rate:.1f}/s)", flush=True)
    shutil.rmtree(scratch, ignore_errors=True)
    print(f"tongue-synth: wrote {directory}", flush=True)


if __name__ == "__main__":
    main()
