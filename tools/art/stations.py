import importlib.util
import json
import math
import runpy
from pathlib import Path

import bpy
from mathutils import Matrix, Vector

ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / 'assets/models/stations'
SPEC = importlib.util.spec_from_file_location('station_geometry', Path(__file__).with_name('micropulse_engine.py'))
geo = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(geo)


def cylinder(name, radius, low, high, material):
    return geo.lathe(name, [(0, low), (radius, low), (radius, high), (0, high)], material)


def collar(radius, z):
    geo.ring('Structural docking flange', radius, z, 0.8, 0.8, steel)


def core(length=64):
    geo.lathe('Station pressure hull', [(0, -length / 2), (15.4, -length / 2),
              (15.4, length / 2), (0, length / 2)], armor)
    for z in (-length / 2 + 0.4, length / 2 - 0.4):
        collar(15.6, z)
    for z in range(-int(length / 2) + 8, int(length / 2), 8):
        geo.ring('Whipple panel joint', 15.43, z, 0.08, 0.12, dark)
    for z in (-length / 4, length / 4):
        for angle in (0, math.pi / 2, math.pi, 3 * math.pi / 2):
            obj = cylinder('8m equipment hardpoint', 4, -0.35, 0.35, steel)
            obj.rotation_euler = Vector((math.cos(angle), math.sin(angle), 0)).to_track_quat('Z', 'Y').to_euler()
            obj.location = geo.radial(15.65, angle, z)


def habitat():
    core(48)
    for side in (-1, 1):
        z = side * 13
        root = bpy.data.objects.new('Habitat rotor forward' if side > 0 else 'Habitat rotor reverse', None)
        geo.collection.objects.link(root)
        root['angular_velocity_rad_s'] = side * math.sqrt(9.80665 / 94)
        before = set(geo.collection.objects)
        geo.ring('Rotating pressure habitat', 94, z, 12, 10, armor)
        geo.ring('External utility channel', 99.6, z, 0.3, 1.8, steel)
        geo.ring('Habitat inner observation band', 88.1, z, 0.3, 1.0, glass)
        geo.ring('Rotor bearing', 17.0, z, 2, 4, steel)
        for index in range(8):
            angle = index * math.tau / 8 + side * 0.08
            geo.beam('Pressurized spoke', geo.radial(17, angle, z), geo.radial(89, angle, z), 1.8, steel, 16)
            for offset in (-1.7, 1.7):
                geo.beam('Spoke tension strut', geo.radial(18, angle, z + offset), geo.radial(87, angle + side * 0.075, z + offset), 0.35, steel)
        for obj in set(geo.collection.objects) - before:
            obj.parent = root


def endcap():
    geo.lathe('Station terminal pressure bulkhead', [(0, -4), (12, -3), (15.6, 1), (15.6, 4), (0, 4)], armor)
    collar(15.6, 3.6)
    cylinder('Axial service hatch', 3.5, -4.1, -3.8, steel)


def hangar():
    geo.lathe('Open hangar Whipple envelope', [(31, -48), (31, 40), (15.6, 48),
              (14.7, 47), (29, 39), (29, -48)], armor)
    cylinder('Hangar rear bulkhead', 29, 38, 39, dark)
    collar(30.6, -47.5)
    collar(15.6, 47.6)
    for z in (-42, -24, -6, 12, 30):
        geo.ring('Interior frame', 28.5, z, 0.7, 1.0, steel)
    for index in range(8):
        angle = index * math.tau / 8
        geo.beam('Dock approach light', geo.radial(28.3, angle, -43), geo.radial(28.3, angle, 34), 0.16, light)
    cylinder('Service hub', 8, 34, 38, steel)


def beacon():
    core(32)
    for z in (-26, 26):
        geo.ring('Subspace resonator', 24, z, 1.4, 1.4, steel)
        geo.ring('Energized resonator gap', 23.8, z, 0.3, 0.3, light)
        for index in range(6):
            angle = index * math.tau / 6
            geo.beam('Resonator mount', geo.radial(13, angle, z / 2), geo.radial(24, angle, z), 0.65, steel)
    cylinder('Station beacon exciter', 7, -32, 32, dark)


def gate():
    for plane in range(3):
        rotation = [Matrix.Identity(4), Matrix.Rotation(math.pi / 2, 4, 'X'), Matrix.Rotation(math.pi / 2, 4, 'Y')][plane]
        for side in (-1, 1):
            for index in range(24):
                a = side * math.pi / 2 + (index / 24 - 0.5) * 0.8
                b = side * math.pi / 2 + ((index + 1) / 24 - 0.5) * 0.8
                start = rotation @ Vector(geo.radial(254, a, 0))
                end = rotation @ Vector(geo.radial(254, b, 0))
                geo.beam('Gate containment arc', start, end, 2, steel, 8)
                geo.beam('Field control strip', start * 0.988, end * 0.988, 0.45, light, 8)


def build():
    global armor, steel, dark, glass, light
    for scene in list(bpy.data.scenes):
        if scene.name == 'Station catalogue' or scene.name.startswith(('station-core-', 'station-end-', 'station-habitat-', 'station-hangar-', 'station-beacon-', 'wormhole-frame-')):
            for obj in list(scene.objects):
                bpy.data.objects.remove(obj, do_unlink=True)
            bpy.data.scenes.remove(scene)
    uv = runpy.run_path(str(Path(__file__).with_name('fuselage.py')))['surface_uv']
    MODELS.mkdir(parents=True, exist_ok=True)
    armor = bpy.data.materials.get('Whipple outer sheet') or geo.material('Station Whipple sheet', (0.38, 0.42, 0.45), 0.4, 0.58)
    steel = geo.material('Station structural titanium', (0.22, 0.26, 0.30), 0.75, 0.34)
    dark = geo.material('Station shadowed surfaces', (0.07, 0.085, 0.1), 0.2, 0.7)
    glass = geo.material('Station observation windows', (0.04, 0.11, 0.16), 0.7, 0.16)
    light = geo.material('Station service illumination', (0.23, 0.6, 0.8), 0.15, 0.35)
    shader = next(n for n in light.node_tree.nodes if n.type == 'BSDF_PRINCIPLED')
    shader.inputs['Emission Color'].default_value = (0.15, 0.6, 1.0, 1.0)
    shader.inputs['Emission Strength'].default_value = 4
    definitions = [
        ('station-core-32m', core, [32, 32, 64]),
        ('station-end-32m', endcap, [32, 32, 8]),
        ('station-habitat-200m', habitat, [200, 200, 48]),
        ('station-hangar-64m', hangar, [64, 64, 96]),
        ('station-beacon-48m', beacon, [48, 48, 64]),
        ('wormhole-frame-512m', gate, [512, 512, 512]),
    ]
    scenes = []
    manifest = []
    for name, builder, dimensions in definitions:
        scene = bpy.data.scenes.new(name)
        scenes.append(scene)
        bpy.context.window.scene = scene
        scene.unit_settings.system = 'METRIC'
        collection = bpy.data.collections.new(name)
        scene.collection.children.link(collection)
        geo.collection = collection
        builder()
        for obj in collection.objects:
            if obj.type == 'MESH':
                uv(obj.data)
        groups = {}
        for obj in collection.objects:
            if obj.type == 'MESH':
                groups.setdefault(obj.parent, []).append(obj)
        for parent, objects in groups.items():
            for obj in collection.objects:
                obj.select_set(False)
            for obj in objects:
                obj.select_set(True)
            bpy.context.view_layer.objects.active = objects[0]
            bpy.ops.object.join()
            bpy.context.object.name = 'Rotating habitat assembly' if parent else 'Fixed station assembly'
        for obj in collection.objects:
            obj.select_set(True)
        bpy.context.view_layer.update()
        bpy.ops.export_scene.gltf(filepath=str(MODELS / (name + '.glb')), export_format='GLB',
            use_selection=True, use_active_scene=True, export_yup=False, export_apply=True,
            export_extras=True, export_cameras=False, export_lights=False)
        manifest.append({'model': 'models/stations/' + name + '.glb', 'dimensions_m': dimensions,
                         'objects': len(collection.objects)})
    overview = bpy.data.scenes.new('Station catalogue')
    overview.unit_settings.system = 'METRIC'
    for index, scene in enumerate(scenes):
        instance = bpy.data.objects.new(scene.name, None)
        instance.instance_type = 'COLLECTION'
        instance.instance_collection = scene.collection.children[0]
        overview.collection.objects.link(instance)
        instance.location = ((index % 3) * 320, (index // 3) * 420, 0)
    scenes.append(overview)
    bpy.context.window.scene = overview
    for area in bpy.context.screen.areas:
        if area.type == 'VIEW_3D':
            area.spaces.active.shading.type = 'MATERIAL'
            area.spaces.active.region_3d.view_rotation = Vector((0.8, -1.4, 1.6)).to_track_quat('Z', 'Y')
            area.spaces.active.region_3d.view_location = (270, 150, 0)
            area.spaces.active.region_3d.view_distance = 1150
    (MODELS / 'catalogue.json').write_text(json.dumps(manifest, indent=2) + '\n')
    path = MODELS / 'station-catalogue.blend'
    bpy.data.libraries.write(str(path), set(scenes), path_remap='RELATIVE', fake_user=True, compress=True)
    print('Station catalogue saved:', path)
    return str(path)


if __name__ == '__main__':
    build()
