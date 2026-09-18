import math
import runpy
from pathlib import Path

import bpy
from mathutils import Vector

ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / 'assets/models/parts'
NAME = 'Micropulse drive 8m'


def material(name, color, metallic, roughness):
    result = bpy.data.materials.get(name) or bpy.data.materials.new(name)
    result.use_nodes = True
    result.diffuse_color = (*color, 1)
    shader = next(n for n in result.node_tree.nodes if n.type == 'BSDF_PRINCIPLED')
    shader.inputs['Base Color'].default_value = result.diffuse_color
    shader.inputs['Metallic'].default_value = metallic
    shader.inputs['Roughness'].default_value = roughness
    return result


def mesh(name, vertices, faces, surface, smooth=False):
    data = bpy.data.meshes.new(name)
    data.from_pydata(vertices, [], faces)
    data.update()
    data.materials.append(surface)
    for polygon in data.polygons:
        polygon.use_smooth = smooth
    obj = bpy.data.objects.new(name, data)
    collection.objects.link(obj)
    return obj


def lathe(name, profile, surface, segments=96, phase=0, smooth=True):
    vertices = []
    rings = []
    for radius, z in profile:
        if radius == 0:
            rings.append([len(vertices)] * segments)
            vertices.append((0, 0, z))
        else:
            rings.append(list(range(len(vertices), len(vertices) + segments)))
            vertices.extend((radius * math.cos(math.tau * i / segments + phase),
                             radius * math.sin(math.tau * i / segments + phase), z)
                            for i in range(segments))
    faces = []
    shading = []
    for j in range(len(profile)):
        k = (j + 1) % len(profile)
        for i in range(segments):
            n = (i + 1) % segments
            face = tuple(dict.fromkeys((rings[j][i], rings[j][n], rings[k][n], rings[k][i])))
            if len(face) >= 3:
                faces.append(face)
                shading.append(smooth and profile[j][1] != profile[k][1])
    obj = mesh(name, vertices, faces, surface)
    for polygon, use_smooth in zip(obj.data.polygons, shading):
        polygon.use_smooth = use_smooth
    return obj


def ring(name, radius, z, width, depth, surface):
    bevel = min(width, depth) * 0.2
    profile = [(radius - width / 2 + bevel, z - depth / 2),
               (radius + width / 2 - bevel, z - depth / 2),
               (radius + width / 2, z - depth / 2 + bevel),
               (radius + width / 2, z + depth / 2 - bevel),
               (radius + width / 2 - bevel, z + depth / 2),
               (radius - width / 2 + bevel, z + depth / 2),
               (radius - width / 2, z + depth / 2 - bevel),
               (radius - width / 2, z - depth / 2 + bevel)]
    return lathe(name, profile, surface)


def beam(name, start, end, radius, surface, sides=12):
    start, end = Vector(start), Vector(end)
    direction = (end - start).normalized()
    axis = direction.cross(Vector((0, 1, 0))).normalized()
    other = direction.cross(axis)
    vertices = [tuple(point + radius * (axis * math.cos(i * math.tau / sides)
                                      + other * math.sin(i * math.tau / sides)))
                for point in [start, end] for i in range(sides)]
    faces = [tuple(reversed(range(sides))), tuple(range(sides, 2 * sides))]
    faces.extend((i, (i + 1) % sides, (i + 1) % sides + sides, i + sides)
                 for i in range(sides))
    return mesh(name, vertices, faces, surface, True)


def radial(radius, angle, z):
    return (radius * math.cos(angle), radius * math.sin(angle), z)


def build():
    global collection
    scene = bpy.data.scenes.get(NAME) or bpy.data.scenes.new(NAME)
    collection = bpy.data.collections.get(NAME)
    if collection is None:
        collection = bpy.data.collections.new(NAME)
        scene.collection.children.link(collection)
    for obj in list(collection.objects):
        data = obj.data
        bpy.data.objects.remove(obj, do_unlink=True)
        if data.users == 0:
            bpy.data.meshes.remove(data)
    scene.unit_settings.system = 'METRIC'
    bpy.context.window.scene = scene
    armor = material('Drive - Whipple armor', (0.38, 0.42, 0.45), 0.4, 0.58)
    steel = material('Drive - titanium structure', (0.22, 0.26, 0.30), 0.75, 0.34)
    ceramic = material('Drive - refractory shield', (0.075, 0.085, 0.095), 0.15, 0.72)
    coil = material('Drive - coil jackets', (0.46, 0.49, 0.52), 0.7, 0.3)
    feed = material('Drive - insulated coolant lines', (0.13, 0.19, 0.23), 0.45, 0.45)

    radius = 4 / math.cos(math.pi / 8)
    lathe('Octagonal mounting bulkhead', [(0, -6), (radius, -6),
          (radius, -5.65), (0, -5.65)], steel, 8, math.pi / 8, False)
    lathe('Shielded machinery housing', [(3.92 / math.cos(math.pi / 8), -5.65),
          (3.92 / math.cos(math.pi / 8), -3.7), (2.8, -2.9),
          (2.65, -2.9), (3.75 / math.cos(math.pi / 8), -3.8),
          (3.75 / math.cos(math.pi / 8), -5.65)], armor, 8, math.pi / 8, False)
    lathe('Aft radiation shadow shield', [(0, -3), (3.9, -3),
          (3.9, -2.65), (3.65, -2.4), (0, -2.4)], ceramic)
    lathe('Reaction chamber jacket', [(0, -2.45), (1.7, -2.45),
          (1.7, -0.9), (1.2, -0.3), (0.8, -0.3),
          (1.25, -1), (1.25, -2.2), (0, -2.2)], steel)
    ring('Throat confinement coil', 1.8, -1.15, 0.5, 0.7, coil)
    stages = [(2.05, 0.1), (2.55, 1.9), (3.08, 3.7), (3.65, 5.675)]
    for index, (r, z) in enumerate(stages):
        ring(f'Magnetic nozzle coil {index + 1}', r, z, 0.42, 0.65, coil)
        ring(f'Plasma-facing coil liner {index + 1}', r - 0.23, z, 0.08, 0.55, ceramic)
    for index in range(8):
        angle = (index + 0.5) * math.tau / 8
        points = [(3.35, -2.65), (2.32, 0.1), (2.82, 1.9), (3.35, 3.7), (3.88, 5.675)]
        for segment, ((r0, z0), (r1, z1)) in enumerate(zip(points, points[1:])):
            beam(f'Nozzle support {index + 1}.{segment + 1}', radial(r0, angle, z0),
                 radial(r1, angle, z1), 0.105, steel)
        if index % 2 == 0:
            for segment, ((r0, z0), (r1, z1)) in enumerate(zip(points, points[1:])):
                beam(f'Coil coolant feed {index + 1}.{segment + 1}',
                     radial(r0 - 0.13, angle + 0.035, z0),
                     radial(r1 - 0.13, angle + 0.035, z1), 0.045, feed, 8)
        beam(f'Reaction mass feed {index + 1}', radial(2.25, angle, -2.45),
             radial(1.45, angle, -0.8), 0.09, feed)
    scene.world = bpy.data.worlds.new('Drive studio')
    scene.world.use_nodes = True
    background = next(n for n in scene.world.node_tree.nodes if n.type == 'BACKGROUND')
    background.inputs['Color'].default_value = (0.15, 0.18, 0.23, 1)
    background.inputs['Strength'].default_value = 0.7
    bpy.context.view_layer.update()
    for area in bpy.context.screen.areas:
        if area.type == 'VIEW_3D':
            area.spaces.active.shading.type = 'MATERIAL'
            area.spaces.active.region_3d.view_rotation = Vector((1.4, -1.8, 1.1)).to_track_quat('Z', 'Y')
            area.spaces.active.region_3d.view_distance = 23
            area.spaces.active.region_3d.view_location = (0, 0, 0)
    return scene, collection


if __name__ == '__main__':
    scene, collection = build()
    exporter = runpy.run_path(str(Path(__file__).with_name('fuselage.py')))['export']
    exporter(collection, 'micropulse-engine-8m.glb')
    bpy.ops.wm.save_as_mainfile(filepath=str(MODELS / 'micropulse-engine-8m.blend'))
    print('Created', len(collection.objects), 'objects in', scene.name)
