import importlib.util
import json
import math
import runpy
from pathlib import Path

import bpy
from mathutils import Matrix, Vector

ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / 'assets/models/parts'
BLEND = MODELS / 'parts-catalogue.blend'

spec = importlib.util.spec_from_file_location('drive_geometry', Path(__file__).with_name('micropulse_engine.py'))
geo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(geo)
fuselage = runpy.run_path(str(Path(__file__).with_name('fuselage.py')))


def box(name, center, size, material):
    x, y, z = center
    a, b, c = (v / 2 for v in size)
    vertices = [(x + i * a, y + j * b, z + k * c)
                for i, j, k in [(-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
                                (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]]
    obj = geo.mesh(name, vertices, [(3, 2, 1, 0), (4, 5, 6, 7), (0, 1, 5, 4),
                                   (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)], material)
    bevel = obj.modifiers.new('Manufactured edge', 'BEVEL')
    bevel.width = min(size) * 0.025
    bevel.segments = 2
    obj.modifiers.new('Panel normals', 'WEIGHTED_NORMAL')
    return obj


def cylinder(name, radius, low, high, material, sides=64):
    return geo.lathe(name, [(0, low), (radius, low), (radius, high), (0, high)], material, sides)


def shell(name, width, low, high, material=None):
    r = width / (2 * math.cos(math.pi / 8))
    return geo.lathe(name, [(r, low), (r, high), (r - 0.055, high), (r - 0.055, low)],
                     material or armor, 8, math.pi / 8, False)


def flange(width, z):
    radius = width / (2 * math.cos(math.pi / 8))
    geo.lathe('Octagonal attachment frame', [(radius, z - 0.06), (radius, z + 0.06),
              (radius * 0.84, z + 0.06), (radius * 0.84, z - 0.06)], steel, 8, math.pi / 8, False)


def enclosure(width, length, closed=True):
    shell('Stand-off Whipple shield', width * 0.98, -length / 2 + 0.08, length / 2 - 0.08)
    flange(width, -length / 2 + 0.06)
    flange(width, length / 2 - 0.06)
    if closed:
        cylinder('Forward bulkhead', width * 0.45, -length / 2, -length / 2 + 0.07, dark, 8)
        cylinder('Aft bulkhead', width * 0.45, length / 2 - 0.07, length / 2, dark, 8)


def service_panel(width, z=0):
    box('Removable service hatch', (0, -width * 0.477, z), (width * 0.45, 0.025, width * 0.4), steel)
    for x in (-width * 0.19, width * 0.19):
        geo.beam('Hatch release handle', (x, -width * 0.487, z - 0.08),
                 (x, -width * 0.487, z + 0.08), min(0.015, width * 0.01), bright)


def vessel(width, length):
    profile = [(0, -length / 2), (width * 0.28, -length / 2),
               (width * 0.46, -length / 2 + width * 0.22),
               (width * 0.46, length / 2 - width * 0.22),
               (width * 0.28, length / 2), (0, length / 2)]
    return geo.lathe('Pressure vessel', profile, steel)


def reactor(width, length, breeder=False, hot=False):
    enclosure(width, length)
    cylinder('Shadow shield', width * 0.44, -length / 2 + 0.1, -length / 2 + 0.35, dark)
    service_panel(width)
    for i in range(4):
        theta = math.tau * (i + 0.5) / 4
        obj = vessel(width * 0.18, length * 0.65)
        obj.name = 'Power conversion unit'
        obj.location = geo.radial(width * 0.39, theta, 0)
    for z in (-length * 0.3, length * 0.3):
        flange(width, z)
    if breeder:
        shell('Breeding blanket housing', width * 0.84, -length * 0.3, length * 0.3, steel)
        for z in (-length * 0.3, length * 0.3):
            geo.ring('Fuel service manifold', width * 0.39, z, 0.12, 0.12, feed)
    if hot:
        for z in (-length * 0.2, length * 0.2):
            geo.ring('Insulated heat exchanger', width * 0.43, z, 0.12, 0.22, dark)
    box('Fuel access cover', (0, 0, -length / 2 + 0.03), (width * 0.28, width * 0.28, 0.06), bright)


def thermal_engine():
    enclosure(4, 1.5)
    for obj in list(geo.collection.objects):
        obj.location.z -= 2.25
    geo.lathe('Cooled expansion nozzle', [(0.8, -1.5), (0.52, -0.95), (0.58, -0.25),
              (0.83, 0.5), (1.22, 1.4), (1.85, 3), (1.77, 3), (1.14, 1.4),
              (0.75, 0.5), (0.50, -0.25), (0.44, -0.95), (0.72, -1.5)], dark)
    geo.ring('Nozzle coolant manifold', 1.81, 2.88, 0.1, 0.16, steel)
    for i in range(8):
        angle = math.tau * i / 8
        geo.beam('Regenerative cooling feed', geo.radial(1.2, angle, -1.5),
                 geo.radial(0.63, angle, 0), 0.045, feed)
    geo.ring('Throat jacket', 0.61, -0.6, 0.14, 0.6, steel)


def electric_engine():
    enclosure(2, 0.8)
    for obj in list(geo.collection.objects):
        obj.location.z -= 0.6
    cylinder('Discharge chamber', 0.65, -0.2, 0.45, dark)
    geo.ring('Accelerator assembly', 0.8, 0.6, 0.24, 0.6, steel)
    geo.ring('Exit electrode', 0.66, 0.92, 0.06, 0.1, bright)
    for i in range(4):
        angle = math.tau * (i + 0.5) / 4
        geo.beam('Accelerator support', geo.radial(0.78, angle, -0.2),
                 geo.radial(0.87, angle, 0.8), 0.06, steel)
    box('Neutralizer', (0.85, 0, 0.5), (0.18, 0.22, 0.7), dark)


def resistojet():
    box('Insulated heater block', (0, 0, -0.2), (0.85, 0.85, 0.6), armor)
    geo.lathe('Expansion bell', [(0.12, 0.02), (0.1, 0.14), (0.3, 0.5),
              (0.26, 0.5), (0.07, 0.14), (0.09, 0.02)], dark)
    for x in (-0.38, 0.38):
        box('Mounting foot', (x, 0, -0.44), (0.24, 1, 0.12), steel)


def truss(width=4, length=8):
    for z in (-length / 2 + 0.06, length / 2 - 0.06):
        flange(width, z)
    radius = width * 0.46
    for i in range(8):
        angle = math.tau * (i + 0.5) / 8
        next_angle = angle + math.tau / 8
        geo.beam('Longitudinal spar', geo.radial(radius, angle, -length / 2 + 0.12),
                 geo.radial(radius, angle, length / 2 - 0.12), 0.08, steel)
        for z0, z1 in [(-length / 2 + 0.12, 0), (0, length / 2 - 0.12)]:
            geo.beam('Diagonal brace', geo.radial(radius, angle, z0),
                     geo.radial(radius, next_angle, z1), 0.045, steel)


def adapter():
    geo.lathe('Tapered Whipple panels', [(2 / math.cos(math.pi / 8), -1),
              (1 / math.cos(math.pi / 8), 1), (0.97 / math.cos(math.pi / 8), 1),
              (1.97 / math.cos(math.pi / 8), -1)], armor, 8, math.pi / 8, False)
    flange(4, -0.94)
    flange(2, 0.94)


def radiator():
    box('Coolant distribution spine', (0, 0, 0), (0.4, 0.5, 8), steel)
    for side in (-1, 1):
        for i in range(4):
            x = side * 2.1
            z = -3 + i * 2
            box('Radiating panel', (x, 0, z), (3.7, 0.07, 1.88), dark)
            for dz in (-0.91, 0.91):
                geo.beam('Panel coolant header', (side * 0.25, 0, z + dz),
                         (side * 3.95, 0, z + dz), 0.04, feed)
            for dx in (0.7, 1.5, 2.3, 3.1):
                box('Heat pipe', (side * dx, -0.045, z), (0.018, 0.025, 1.8), steel)
    box('Root attachment', (0, 0, -3.8), (1, 1, 0.4), armor)


def module(kind, width=2, length=2):
    enclosure(width, length)
    service_panel(width)
    if kind in ('battery', 'heat-sink'):
        for x in (-0.24, 0, 0.24):
            box('Removable cartridge cover', (x * width, 0, -length / 2 + 0.03),
                (width * 0.18, width * 0.68, 0.06), dark if kind == 'heat-sink' else steel)
    elif kind == 'shield-emitter':
        for i in range(8):
            theta = math.tau * (i + 0.5) / 8
            obj = geo.lathe('Shield medium nozzle', [(0.08, -0.05), (0.12, 0.15),
                            (0.09, 0.15), (0.05, -0.05)], bright, 32)
            obj.rotation_euler = Vector(geo.radial(1, theta, 0)).to_track_quat('Z', 'Y').to_euler()
            obj.location = geo.radial(width * 0.40, theta, 0)
        geo.ring('Shield medium manifold', width * 0.39, 0, width * 0.06, 0.2, feed)
    elif kind in ('life-support', 'emergency-cooler'):
        for x in (-width * 0.22, width * 0.22):
            obj = vessel(width * 0.3, length * 0.7)
            obj.location.x = x
        if kind == 'emergency-cooler':
            geo.ring('Coolant discharge outlet', width * 0.2, length / 2 - 0.1, 0.12, 0.2, dark)
    elif kind == 'command':
        box('Avionics access', (0, 0, -length / 2 + 0.03), (width * 0.6, width * 0.6, 0.06), steel)
        for x in (-0.25, 0.25):
            box('Data connection', (x * width, -width * 0.3, -length / 2 + 0.0315), (0.15, 0.08, 0.04), dark)


def accommodation(cargo=False):
    enclosure(4, 8)
    for x in (-0.75, 0.75):
        box('Cargo access door' if cargo else 'Pressure access hatch',
            (x, -1.98, 0), (1.42, 0.035, 4.8 if cargo else 2), armor if cargo else steel)
    if not cargo:
        for z in (-2.5, 2.5):
            box('External life support connection', (0, -1.97, z), (0.65, 0.045, 0.4), feed)


def power_coupler():
    enclosure(2, 1)
    for x in (-0.35, 0.35):
        obj = cylinder('Recessed electrical contact', 0.18, 0.32, 0.47, bright)
        obj.location.x = x
        ring = geo.ring('Contact insulation', 0.23, 0.4, 0.07, 0.18, dark)
        ring.location.x = x


def docking(width=2):
    shell('Docking sleeve', width, -0.5, 0.25)
    geo.ring('Docking seal', width * 0.38, 0.35, width * 0.10, 0.25, dark)
    geo.ring('Capture ring', width * 0.45, 0.35, width * 0.08, 0.3, steel)
    cylinder('Pressure hatch', width * 0.34, 0.16, 0.22, armor)
    for i in range(4):
        angle = math.tau * i / 4
        obj = box('Capture latch', geo.radial(width * 0.42, angle, 0.35), (0.14, 0.14, 0.3), bright)


def sensor(active=False):
    if active:
        box('Array backplane', (0, 0, 0.25), (4, 4, 0.5), armor)
        for x in range(4):
            for y in range(4):
                box('Phased array tile', (-1.48 + x * 0.985, -1.48 + y * 0.985, 0.55),
                    (0.95, 0.95, 0.08), dark)
        cylinder('Elevation pedestal', 0.65, -1, 0, steel)
    else:
        box('Optical bench', (0, 0, -0.35), (1.8, 1.8, 1.3), armor)
        for x, y in [(-0.43, -0.43), (0.43, 0.43)]:
            obj = geo.lathe('Optical baffle', [(0.31, 0), (0.31, 0.9), (0.25, 0.9), (0.25, 0)], dark)
            obj.location = (x, y, 0)
            lens = cylinder('Recessed optical aperture', 0.24, 0.14, 0.16, glass)
            lens.location = (x, y, 0)
        cylinder('Azimuth mounting', 0.6, -1, -0.85, steel)


def beacon():
    enclosure(2, 2)
    for obj in list(geo.collection.objects):
        obj.location.z -= 3
    for z, radius in [(-1, 1.8), (1, 1.4), (3, 1)]:
        geo.ring('Subspace resonator', radius, z, 0.15, 0.3, bright)
    for i in range(4):
        angle = math.tau * (i + 0.5) / 4
        geo.beam('Antenna mast', geo.radial(0.7, angle, -2), geo.radial(0.7, angle, 4), 0.065, steel)


def hangar():
    enclosure(8, 16, closed=False)
    cylinder('Hangar rear bulkhead', 3.7, -8, -7.8, dark, 8)
    for x in (-2.8, 2.8):
        box('Docking guide rail', (x, -2.7, 0), (0.15, 0.15, 15), steel)
    for x in (-3.55, 3.55):
        box('Retracted door leaf', (x, 0, 7.5), (0.5, 5, 0.7), armor)
    for y in (-2.65, 2.65):
        box('Entrance safety marking', (0, y, 7.75), (5.5, 0.09, 0.08), marking)


def cargo_handler():
    box('Manipulator base', (0, 0, -1.6), (2, 2, 0.8), armor)
    joints = [(0, 0, -1.2), (0, 0, 0.3), (1.25, 0, 1.15)]
    for a, b in zip(joints, joints[1:]):
        geo.beam('Manipulator link', a, b, 0.2, steel)
    for side in (-1, 1):
        geo.beam('Capture jaw', (1.25, side * 0.35, 1.15), (1.25, side * 0.5, 1.85), 0.08, bright)
    box('Jaw drive', (1.25, 0, 1.15), (0.4, 1, 0.4), dark)


def processor(industrial=False):
    width, length = (8, 8) if industrial else (4, 4)
    truss(width, length)
    for x in (-width * 0.23, width * 0.23):
        obj = vessel(width * 0.4, length * 0.75)
        obj.location.x = x
    box('Remote handling enclosure', (0, -width * 0.3, 0), (width * 0.72, width * 0.2, length * 0.5), armor)
    for z in (-length * 0.25, length * 0.25):
        geo.beam('Process manifold', (-width * 0.23, 0, z), (width * 0.23, 0, z), 0.09, feed)


def railgun():
    box('Armoured breech', (0, 0, -2), (2, 2, 2), armor)
    for x in (-0.38, 0.38):
        box('Rail support', (x, 0, 0.8), (0.35, 0.7, 4.4), steel)
        box('Rail electrode', (x * 0.6, 0, 0.8), (0.07, 0.3, 4.4), dark)
    for z in (-0.8, 0.4, 1.6, 2.8):
        box('Barrel brace', (0, 0.4, z), (1.2, 0.12, 0.18), steel)


def laser(width=2):
    enclosure(width, width, closed=False)
    cylinder("Optical assembly rear closure", width * 0.44, -width / 2, -width / 2 + 0.08, steel)
    geo.ring('Optical aperture baffle', width * 0.34, width * 0.35, width * 0.1, width * 0.3, dark)
    cylinder('Recessed output optic', width * 0.28, width * 0.23, width * 0.25, glass)
    for x in (-width * 0.42, width * 0.42):
        box('Elevation bearing', (x, 0, 0), (width * 0.12, width * 0.4, width * 0.4), steel)


def build_asset(identifier, title, dimensions, builder):
    name = 'Part - ' + title
    scene = bpy.data.scenes.get(name) or bpy.data.scenes.new(name)
    collection = bpy.data.collections.get(name) or bpy.data.collections.new(name)
    if collection.name not in scene.collection.children:
        scene.collection.children.link(collection)
    for obj in list(collection.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    geo.collection = collection
    bpy.context.window.scene = scene
    scene.unit_settings.system = 'METRIC'
    builder()
    if identifier.startswith(('railgun-', 'laser-')):
        rotation = Matrix.Rotation(math.pi, 4, 'Y')
        for obj in collection.objects:
            obj.matrix_world = rotation @ obj.matrix_world
    for obj in collection.objects:
        if obj.type == 'MESH':
            fuselage['surface_uv'](obj.data)
    bpy.context.view_layer.update()
    points = [obj.matrix_world @ Vector(v) for obj in collection.objects for v in obj.bound_box]
    bounds = [[min(p[i] for p in points), max(p[i] for p in points)] for i in range(3)]
    fuselage['export'](collection, identifier + '.glb')
    return {'id': identifier, 'title': title, 'dimensions_m': dimensions,
            'bounds_m': bounds, 'model': 'models/parts/' + identifier + '.glb',
            'objects': len(collection.objects), 'scene': name}


def build():
    global armor, steel, bright, dark, feed, glass, marking
    armor = bpy.data.materials.get('Whipple outer sheet')
    if armor is None:
        armor = geo.material('Whipple outer sheet', (0.38, 0.42, 0.45), 0.4, 0.58)
    steel = geo.material('Catalogue - titanium', (0.22, 0.26, 0.30), 0.75, 0.34)
    bright = geo.material('Catalogue - machined edges', (0.46, 0.49, 0.52), 0.7, 0.3)
    dark = geo.material('Catalogue - refractory', (0.075, 0.085, 0.095), 0.15, 0.72)
    feed = geo.material('Catalogue - coolant lines', (0.13, 0.19, 0.23), 0.45, 0.45)
    glass = geo.material('Catalogue - optical surface', (0.045, 0.10, 0.13), 0.8, 0.13)
    marking = geo.material('Catalogue - safety ochre', (0.5, 0.31, 0.08), 0.2, 0.6)
    definitions = [
        ('auxiliary-generator-1m', 'Auxiliary generator', [1, 1, 1], lambda: module('generator', 1, 1)),
        ('torquer-1m', 'Attitude torque actuator', [1, 1, 1], lambda: module('torquer', 1, 1)),
        ('storage-1m', 'Small cargo hold', [1, 1, 1], lambda: module('cargo', 1, 1)),
        ('coolant-tank-1m', 'Small coolant tank', [1, 1, 1], lambda: vessel(1, 1)),
        ('adapter-4m', '4m to 2m adapter', [4, 4, 2], adapter),
        ('truss-4m', '4m open truss', [4, 4, 8], truss),
        ('mount-frame-2m', '2m mounting frame', [2, 2, 0.5], lambda: truss(2, 0.5)),
        ('reactor-compact-2m', 'Compact power reactor', [2, 2, 3], lambda: reactor(2, 3)),
        ('reactor-hot-4m', 'High temperature reactor', [4, 4, 6], lambda: reactor(4, 6, hot=True)),
        ('reactor-breeder-4m', 'Breeder power reactor', [4, 4, 8], lambda: reactor(4, 8, breeder=True)),
        ('thermal-engine-4m', 'Nuclear thermal engine', [4, 4, 6], thermal_engine),
        ('electric-engine-2m', 'Electric engine', [2, 2, 2], electric_engine),
        ('resistojet-1m', 'Resistojet', [1, 1, 1], resistojet),
        ('radiator-8m', 'Radiator panel', [8, 1, 8], radiator),
        ('battery-2m', 'Battery module', [2, 2, 2], lambda: module('battery')),
        ('heat-sink-2m', 'Heat sink', [2, 2, 2], lambda: module('heat-sink')),
        ('shield-emitter-2m', 'Shield emitter', [2, 2, 2], lambda: module('shield-emitter')),
        ('shield-reservoir-2m', 'Shield medium reservoir', [2, 2, 4], lambda: enclosure(2, 4)),
        ('emergency-cooler-2m', 'Emergency cooler', [2, 2, 2], lambda: module('emergency-cooler')),
        ('command-2m', 'Command module', [2, 2, 2], lambda: module('command')),
        ('sensor-passive-2m', 'Passive sensor', [2, 2, 2], sensor),
        ('sensor-active-4m', 'Active sensor array', [4, 4, 2], lambda: sensor(True)),
        ('beacon-4m', 'Subspace beacon', [4, 4, 8], beacon),
        ('docking-port-2m', 'Docking port', [2, 2, 1], docking),
        ('power-coupler-2m', 'Power coupler', [2, 2, 1], power_coupler),
        ('hangar-8m', 'Hangar bay', [8, 8, 16], hangar),
        ('crew-4m', 'Crew compartment', [4, 4, 8], accommodation),
        ('cargo-hold-4m', 'Cargo hold', [4, 4, 8], lambda: accommodation(True)),
        ('life-support-2m', 'Life support', [2, 2, 2], lambda: module('life-support')),
        ('workshop-4m', 'Repair workshop', [4, 4, 4], lambda: module('workshop', 4, 4)),
        ('cargo-handler-4m', 'Cargo manipulator', [4, 4, 4], cargo_handler),
        ('fuel-processor-4m', 'Fuel processor', [4, 4, 4], processor),
        ('fuel-plant-8m', 'Fuel fabrication plant', [8, 8, 8], lambda: processor(True)),
        ('railgun-2m', 'Railgun', [2, 2, 6], railgun),
        ('laser-2m', 'Point defence laser', [2, 2, 2], laser),
        ('laser-4m', 'Heavy laser', [4, 4, 4], lambda: laser(4)),
    ]
    manifest = []
    for args in definitions:
        manifest.append(build_asset(*args))
    overview = bpy.data.scenes.get('Parts catalogue') or bpy.data.scenes.new('Parts catalogue')
    for obj in list(overview.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    for index, entry in enumerate(manifest):
        collection = bpy.data.collections[entry['scene']]
        origin = Vector(((index % 6) * 12, (index // 6) * 14, 0))
        for original in collection.objects:
            obj = original.copy()
            overview.collection.objects.link(obj)
            obj.location += origin
    bpy.context.window.scene = overview
    overview.unit_settings.system = 'METRIC'
    for area in bpy.context.screen.areas:
        if area.type == 'VIEW_3D':
            area.spaces.active.shading.type = 'MATERIAL'
            area.spaces.active.region_3d.view_rotation = Vector((0.8, -1.4, 1.6)).to_track_quat('Z', 'Y')
            area.spaces.active.region_3d.view_location = (30, 35, 0)
            area.spaces.active.region_3d.view_distance = 100
    (MODELS / 'catalogue-models.json').write_text(json.dumps(manifest, indent=2) + '\n')
    bpy.ops.wm.save_as_mainfile(filepath=str(BLEND))
    print('Built', len(manifest), 'catalogue models')


if __name__ == '__main__':
    build()
