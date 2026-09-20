import json
import math
from pathlib import Path

import bpy
import bmesh
from mathutils import Vector, Matrix


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / 'assets/models/common-sky'
PREFIX = 'CS / '
PARTS = {}
MATERIALS = {}
CURRENT = None


def material(key, color, metallic=0.0, roughness=0.35):
    mat = bpy.data.materials.get(PREFIX + key) or bpy.data.materials.new(PREFIX + key)
    mat.use_nodes = True
    mat.diffuse_color = (*color, 1.0)
    shader = next(node for node in mat.node_tree.nodes if node.type == 'BSDF_PRINCIPLED')
    shader.inputs['Base Color'].default_value = mat.diffuse_color
    shader.inputs['Metallic'].default_value = metallic
    shader.inputs['Roughness'].default_value = roughness
    MATERIALS[key] = mat
    return mat


def start():
    global SCENE
    SCENE = bpy.data.scenes.get('Common Sky — Assembly')
    if SCENE is None:
        SCENE = bpy.data.scenes.new('Common Sky — Assembly')
    bpy.context.window.scene = SCENE
    for name in ('Common Sky — Parts inventory', 'Common Sky — Exploded assembly'):
        old_scene = bpy.data.scenes.get(name)
        if old_scene:
            for obj in list(old_scene.objects):
                bpy.data.objects.remove(obj, do_unlink=True)
            bpy.data.scenes.remove(old_scene)
    for collection in list(SCENE.collection.children):
        if collection.name.startswith(PREFIX):
            for obj in list(collection.objects):
                bpy.data.objects.remove(obj, do_unlink=True)
            bpy.data.collections.remove(collection)
    SCENE.unit_settings.system = 'METRIC'
    SCENE.unit_settings.scale_length = 1.0
    cube = bpy.data.objects.get('Cube')
    if cube is not None:
        bpy.data.objects.remove(cube, do_unlink=True)
    material('Ceramic white', (0.88, 0.91, 0.93), 0.03, 0.46)
    material('Sky blue', (0.025, 0.37, 0.80), 0.06, 0.43)
    material('Solar gold', (1.0, 0.62, 0.055), 0.14, 0.41)
    material('Glass', (0.012, 0.038, 0.07), 0.48, 0.19)
    material('Seal', (0.085, 0.12, 0.16), 0.2, 0.45)
    material('Titanium', (0.29, 0.36, 0.43), 0.8, 0.27)
    material('Refractory', (0.055, 0.07, 0.085), 0.45, 0.50)
    material('Copper', (0.49, 0.19, 0.065), 0.8, 0.31)


def recover():
    global SCENE
    SCENE = bpy.data.scenes['Common Sky — Assembly']
    for mat in bpy.data.materials:
        if mat.name.startswith(PREFIX):
            MATERIALS[mat.name[len(PREFIX):]] = mat
    for collection in SCENE.collection.children:
        for root in collection.objects:
            identifier = root.get('part_id')
            if identifier:
                PARTS[identifier] = {
                    'id': identifier, 'title': root.name[len(PREFIX):],
                    'origin': root.location.copy(), 'collection': collection,
                    'root': root, 'category': root['category'], 'nodes': []}


def part(identifier, title, origin, category):
    global CURRENT
    collection = bpy.data.collections.new(PREFIX + title)
    SCENE.collection.children.link(collection)
    root = bpy.data.objects.new(PREFIX + title, None)
    collection.objects.link(root)
    root.location = origin
    root['part_id'] = identifier
    root['category'] = category
    root['livery'] = 'Blue upper / fine gold'
    CURRENT = {'id': identifier, 'title': title, 'origin': Vector(origin),
               'collection': collection, 'root': root, 'category': category,
               'nodes': []}
    PARTS[identifier] = CURRENT
    return CURRENT


def mesh(name, vertices, faces, materials, indices=None, smooth=True):
    data = bpy.data.meshes.new(PREFIX + name)
    data.from_pydata([Vector(p) - CURRENT['origin'] for p in vertices], [], faces)
    data.update()
    for key in materials:
        data.materials.append(MATERIALS[key])
    for index, polygon in enumerate(data.polygons):
        polygon.use_smooth = smooth
        if indices is not None:
            polygon.material_index = indices[index]
    bm = bmesh.new()
    bm.from_mesh(data)
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.to_mesh(data)
    bm.free()
    obj = bpy.data.objects.new(PREFIX + name, data)
    CURRENT['collection'].objects.link(obj)
    obj.parent = CURRENT['root']
    return obj


def clip(vertices, field, threshold, above):
    result = []
    for a, b in zip(vertices, vertices[1:] + vertices[:1]):
        fa = field(a) - threshold
        fb = field(b) - threshold
        inside_a = fa >= 0 if above else fa <= 0
        inside_b = fb >= 0 if above else fb <= 0
        if inside_a:
            result.append(a)
        if inside_a != inside_b:
            result.append(a + (b - a) * (fa / (fa - fb)))
    return result


def painted_mesh(name, vertices, faces):
    output_vertices = []
    output_faces = []
    indices = []
    height = lambda p: p.y
    for face in faces:
        polygon = [Vector(vertices[i]) for i in face]
        bottom = clip(polygon, height, 0.0, False)
        top = clip(polygon, height, 0.0, True)
        stripe = clip(clip(polygon, height, 0.0, True), height, 0.055, False)
        top = clip(top, height, 0.055, True)
        regions = [(bottom, 0), (stripe, 2), (top, 1)]
        for points, index in regions:
            if len(points) < 3:
                continue
            first = len(output_vertices)
            output_vertices.extend(points)
            output_faces.append(tuple(range(first, first + len(points))))
            indices.append(index)
    obj = mesh(name, output_vertices, output_faces,
               ['Ceramic white', 'Sky blue', 'Solar gold'], indices)
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bmesh.ops.remove_doubles(bm, verts=list(bm.verts), dist=0.00001)
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.to_mesh(obj.data)
    bm.free()
    return obj


PROFILE = [(-38, 0.15, 0.15), (-36, 2.4, 1.7), (-32, 4.9, 3.0),
           (-28, 6.8, 3.7), (-24, 8.1, 4.2), (-16, 10.1, 4.7),
           (-8, 11.5, 5.0), (-4, 12, 5.0), (6, 12, 4.8),
           (14, 12, 4.4), (22, 12, 3.9), (28, 12, 3.5)]


def profile(z):
    for index, (a, b) in enumerate(zip(PROFILE, PROFILE[1:])):
        if a[0] <= z <= b[0]:
            t = (z - a[0]) / (b[0] - a[0])
            result = []
            for axis in (1, 2):
                slope = (b[axis] - a[axis]) / (b[0] - a[0])
                before = PROFILE[max(0, index - 1)]
                after = PROFILE[min(len(PROFILE) - 1, index + 2)]
                m0 = (b[axis] - before[axis]) / (b[0] - before[0])
                m1 = (after[axis] - a[axis]) / (after[0] - a[0])
                if slope == 0:
                    m0 = m1 = 0
                else:
                    m0 = math.copysign(min(abs(m0), 2 * abs(slope)), slope)
                    m1 = math.copysign(min(abs(m1), 2 * abs(slope)), slope)
                span = b[0] - a[0]
                result.append((2 * t ** 3 - 3 * t ** 2 + 1) * a[axis]
                              + (t ** 3 - 2 * t ** 2 + t) * span * m0
                              + (-2 * t ** 3 + 3 * t ** 2) * b[axis]
                              + (t ** 3 - t ** 2) * span * m1)
            return tuple(result)
    return PROFILE[-1][1:]


def hull_point(z, angle, offset=0):
    width, height = profile(z)
    c, s = math.cos(angle), math.sin(angle)
    x = math.copysign(abs(c) ** 0.8, c) * (width + offset)
    y = math.copysign(abs(s) ** 0.8, s) * (height + offset)
    return Vector((x, y, z))


def hull(identifier, title, low, high, half, category):
    part(identifier, title, (0, 0, (low + high) / 2), category)
    angles = 64 if half == 'full' else 32
    begin, end = (0, math.tau) if half == 'full' else ((0, math.pi) if half == 'top' else (math.pi, math.tau))
    count = max(8, round((high - low) * 2))
    vertices = []
    for i in range(count + 1):
        z = low + (high - low) * i / count
        vertices.extend(hull_point(z, begin + (end - begin) * j / angles)
                        for j in range(angles + 1))
    faces = []
    stride = angles + 1
    for i in range(count):
        for j in range(angles):
            a = i * stride + j
            faces.append((a, a + 1, a + 1 + stride, a + stride))
        if half != 'full':
            faces.append((i * stride, (i + 1) * stride,
                          (i + 1) * stride + angles, i * stride + angles))
    surface = painted_mesh(title + ' shell', vertices, faces)
    normals = []
    for polygon in surface.data.polygons:
        flat = all(abs((surface.data.vertices[i].co + CURRENT['origin']).y) < 0.0001
                   for i in polygon.vertices)
        for loop in polygon.loop_indices:
            point = surface.data.vertices[surface.data.loops[loop].vertex_index].co + CURRENT['origin']
            if flat:
                normals.append(tuple(polygon.normal))
                continue
            width, height = profile(point.z)
            a = math.atan2(math.copysign((abs(point.y) / height) ** 1.25, point.y),
                           math.copysign((abs(point.x) / width) ** 1.25, point.x))
            tangent = hull_point(point.z, a + 0.001) - hull_point(point.z, a - 0.001)
            z0, z1 = max(-38, point.z - 0.01), min(28, point.z + 0.01)
            axial = hull_point(z1, a) - hull_point(z0, a)
            normals.append(tuple(tangent.cross(axial).normalized()))
    surface.data.normals_split_custom_set(normals)
    caps = [tuple(reversed(range(stride))),
            tuple(count * stride + j for j in range(stride))]
    mesh(title + ' interface bulkheads', vertices, caps,
         ['Ceramic white'], smooth=False)


def wing(side):
    name = 'Port' if side < 0 else 'Starboard'
    part('wing-' + name.lower(), name + ' lifting wing', (side * 21, 0, 14), 'Structure')
    stations = [(11.8, -4, 28, 1.6), (15, 0, 29, 1.35),
                (20, 6, 29, 0.9), (25, 12, 28, 0.5), (30, 19, 26, 0.16)]
    vertices = []
    sections = 40
    for x, front, back, thick in stations:
        for j in range(sections):
            a = math.tau * j / sections
            u = (1 - math.cos(a)) / 2
            y = thick * math.sin(a) * (1 - 0.25 * u)
            vertices.append((side * x, y, front + (back - front) * u))
    faces = []
    for i in range(len(stations) - 1):
        for j in range(sections):
            faces.append((i * sections + j, i * sections + (j + 1) % sections,
                          (i + 1) * sections + (j + 1) % sections,
                          (i + 1) * sections + j))
    faces.extend([tuple(reversed(range(sections))),
                  tuple((len(stations) - 1) * sections + j for j in range(sections))])
    painted_mesh(name + ' wing skin', vertices, faces)


def use_part(identifier):
    global CURRENT
    CURRENT = PARTS[identifier]


def lathe(name, profile_points, center, surface, segments=64):
    cx, cy, cz = center
    vertices = [(cx + radius * math.cos(math.tau * i / segments),
                 cy + radius * math.sin(math.tau * i / segments), cz + z)
                for radius, z in profile_points for i in range(segments)]
    faces = []
    for row in range(len(profile_points) - 1):
        for i in range(segments):
            j = (i + 1) % segments
            faces.append((row * segments + i, row * segments + j,
                          (row + 1) * segments + j, (row + 1) * segments + i))
    obj = mesh(name, vertices, faces, [surface])
    bm = bmesh.new()
    bm.from_mesh(obj.data)
    bmesh.ops.remove_doubles(bm, verts=list(bm.verts), dist=0.00001)
    bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces))
    bm.to_mesh(obj.data)
    bm.free()
    return obj


def ring(name, radius, z, width, depth, center, surface):
    inner, outer = radius - width / 2, radius + width / 2
    profile_points = [(inner, z - depth / 2), (outer, z - depth / 2),
                      (outer, z + depth / 2), (inner, z + depth / 2),
                      (inner, z - depth / 2)]
    return lathe(name, profile_points, center, surface)


def tube(name, points, radius, surface, sides=6, closed=False):
    points = [Vector(p) for p in points]
    vertices = []
    for index, point in enumerate(points):
        before = points[(index - 1) % len(points)] if index or closed else point
        after = points[(index + 1) % len(points)] if index < len(points) - 1 or closed else point
        tangent = (after - before).normalized()
        reference = Vector((0, 1, 0)) if abs(tangent.y) < 0.95 else Vector((1, 0, 0))
        a = tangent.cross(reference).normalized()
        b = tangent.cross(a)
        for i in range(sides):
            angle = math.tau * i / sides
            vertices.append(point + radius * (a * math.cos(angle) + b * math.sin(angle)))
    faces = []
    for j in range(len(points) if closed else len(points) - 1):
        k = (j + 1) % len(points)
        for i in range(sides):
            faces.append((j * sides + i, j * sides + (i + 1) % sides,
                          k * sides + (i + 1) % sides, k * sides + i))
    return mesh(name, vertices, faces, [surface])


def box(name, center, size, surface, bevel=0.0):
    c = Vector(center)
    h = Vector(size) / 2
    vertices = [c + Vector((x * h.x, y * h.y, z * h.z))
                for x, y, z in [(-1, -1, -1), (1, -1, -1), (1, 1, -1), (-1, 1, -1),
                                (-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]]
    obj = mesh(name, vertices, [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4),
                                (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)],
               [surface], smooth=False)
    if bevel:
        modifier = obj.modifiers.new('Manufactured edge', 'BEVEL')
        modifier.width = bevel
        modifier.segments = 3
    return obj


def copy_part(identifier, instance_name, offset):
    original = PARTS[identifier]
    collection = bpy.data.collections.new(PREFIX + instance_name)
    SCENE.collection.children.link(collection)
    root = bpy.data.objects.new(PREFIX + instance_name, None)
    collection.objects.link(root)
    root.location = original['origin'] + Vector(offset)
    root['instance_of'] = identifier
    for obj in original['collection'].objects:
        if obj.type != 'MESH':
            continue
        copied = obj.copy()
        copied.data = obj.data
        collection.objects.link(copied)
        copied.parent = root
    return root


def build_engines():
    for identifier, title, x, radius, length, kind in [
        ('jet', '09 Atmospheric jet', -8.3, 1.95, 10.0, 'jet'),
        ('chemical-rocket', '10 Chemical rocket', -4.1, 1.45, 8.0, 'rocket'),
        ('micropulse-drive', '11 Nuclear micropulse drive', 0, 2.55, 10.0, 'nuclear'),
    ]:
        center = (x, 0, 33)
        part(identifier, title, center, 'Propulsion')
        front = -length / 2
        back = length / 2
        lathe(title + ' fairing', [(radius * 0.86, front), (radius, front + 0.7),
              (radius, back - 1.5), (radius * 0.92, back - 0.3),
              (radius * 0.88, back), (radius * 0.82, back)], center, 'Ceramic white')
        for z in (front + 0.7, back - 1.5):
            ring('Service joint', radius + 0.015, z, 0.045, 0.055, center, 'Titanium')
        ring('Engine gold identity band', radius + 0.01, front + 1.5, 0.035, 0.48,
             center, 'Solar gold')
        ring('Exhaust lip', radius * 0.86, back, radius * 0.10, 0.16, center, 'Titanium')
        throat = 0.28 if kind == 'rocket' else 0.36
        interior = 'Copper' if kind == 'rocket' else 'Refractory'
        lathe('Open expansion nozzle', [(radius * 0.81, back),
              (radius * 0.77, back - 0.35), (radius * 0.61, back - 1.35),
              (radius * throat, back - 2.65), (radius * throat, back - 3.7)], center, interior)
        lathe('Dark nozzle throat', [(radius * throat, back - 3.7), (0, back - 3.8)],
              center, 'Refractory')
        if kind == 'rocket':
            for j in range(28):
                a = j * math.tau / 28
                points = [(x + r * math.cos(a), r * math.sin(a), 33 + z)
                          for r, z in [(radius * 0.80, back - 0.02),
                                       (radius * 0.75, back - 0.4),
                                       (radius * 0.59, back - 1.35),
                                       (radius * 0.30, back - 2.6)]]
                tube('Regenerative cooling channel', points, 0.018, 'Titanium')
        elif kind == 'nuclear':
            for z, r in [(back - 0.15, radius * 0.96),
                         (back - 0.8, radius * 1.02), (back - 1.5, radius * 1.04)]:
                ring('Magnetic nozzle coil', r, z, 0.17, 0.22, center, 'Titanium')
            for j in range(12):
                a = j * math.tau / 12
                p = Vector((x + radius * math.cos(a), radius * math.sin(a), 33 + back - 1.4))
                q = Vector((x + radius * math.cos(a), radius * math.sin(a), 33 + back))
                tube('Coil restraint', [p, q], 0.065, 'Solar gold', sides=8)
        else:
            for j in range(20):
                a = j * math.tau / 20
                vertices = []
                for z, r, angle in [(back - 1.9, radius * 0.26, a),
                                     (back - 2.1, radius * 0.66, a + 0.16),
                                     (back - 2.15, radius * 0.66, a + 0.26),
                                     (back - 1.95, radius * 0.26, a + 0.15)]:
                    vertices.append((x + r * math.cos(angle), r * math.sin(angle), 33 + z))
                mesh('Turbine vane', vertices, [(0, 1, 2, 3)], ['Titanium'])
            lathe('Jet exhaust cone', [(radius * 0.26, back - 2.0),
                  (radius * 0.14, back - 0.9), (0, back - 0.35)], center, 'Titanium')
        CURRENT['root']['engine_role'] = kind
        CURRENT['root']['thrust_axis'] = '-Z'
        CURRENT['root']['exhaust_axis'] = '+Z'
    copy_part('jet', 'Atmospheric jet · starboard', (16.6, 0, 0))
    copy_part('chemical-rocket', 'Chemical rocket · starboard', (8.2, 0, 0))
    bpy.context.view_layer.update()
    print('Three reusable engine assets; five installed engines share one thrust plane.')


def hull_patch(name, z0, z1, a0, a1, surface, offset=0.045):
    vertices = []
    nz, na = 4, 6
    for i in range(nz + 1):
        for j in range(na + 1):
            vertices.append(hull_point(z0 + (z1 - z0) * i / nz,
                                       a0 + (a1 - a0) * j / na, offset))
    faces = []
    for i in range(nz):
        for j in range(na):
            k = i * (na + 1) + j
            faces.append((k, k + 1, k + na + 2, k + na + 1))
    return mesh(name, vertices, faces, [surface])


def build_hull_details():
    use_part('flight-deck')
    for side in (-1, 1):
        for index in range(4):
            angle = 0.44 + index * 0.28
            a0, a1 = (angle, angle + 0.235) if side > 0 else (math.pi - angle - 0.235, math.pi - angle)
            hull_patch('Flight deck glazing', -32.1, -29.6, a0, a1, 'Glass', 0.06)
    for identifier, start_z, end_z, seats in [('passenger-forward', -23, -5, 175),
                                              ('passenger-aft', -3, 12, 175)]:
        use_part(identifier)
        CURRENT['root']['passenger_seats'] = seats
        for side in (-1, 1):
            angle = 0.22 if side > 0 else math.pi - 0.22
            for j in range(18):
                z = start_z + (end_z - start_z) * j / 17
                hull_patch('Passenger window surround', z - 0.24, z + 0.24,
                           angle - 0.071, angle + 0.071, 'Titanium', 0.035)
                hull_patch('Passenger window', z - 0.185, z + 0.185,
                           angle - 0.053, angle + 0.053, 'Glass', 0.055)
            for z in (start_z + 1.4, end_z - 1.2):
                hull_patch('Boarding door outline', z - 0.64, z + 0.64,
                           angle - 0.21, angle + 0.38, 'Solar gold', 0.065)
                hull_patch('Boarding door', z - 0.56, z + 0.56,
                           angle - 0.19, angle + 0.36, 'Ceramic white', 0.085)
                hull_patch('Door observation window', z - 0.20, z + 0.20,
                           angle + 0.08, angle + 0.18, 'Glass', 0.10)
    use_part('cargo-keel')
    CURRENT['root']['cargo_volume_m3'] = 400.0
    CURRENT['root']['target_payload_kg'] = 50000.0
    for side in (-1, 1):
        a = -0.22 if side > 0 else math.pi + 0.22
        hull_patch('Cargo hatch perimeter', -8, -1, a - 0.19, a + 0.19, 'Solar gold', 0.06)
        hull_patch('Cargo hatch panel', -7.86, -1.14, a - 0.17, a + 0.17, 'Ceramic white', 0.085)
        for z in (-7, -2):
            hull_patch('Cargo latch', z - 0.15, z + 0.15, a - 0.04, a + 0.04,
                       'Titanium', 0.12)
    use_part('propulsion-frame')
    for side in (-1, 1):
        x = side * 8.3
        center = (x, 3.0, 16.0)
        lathe('Intake shoulder', [(1.28, -3.0), (1.65, -2.7), (1.82, -1.3),
              (1.60, 3.2), (1.15, 5.0)], center, 'Sky blue')
        ring('Intake lip', 1.29, -3.0, 0.18, 0.20, center, 'Ceramic white')
        lathe('Intake duct', [(1.19, -3.0), (1.10, -2.0), (0.9, 0)], center, 'Refractory')
        lathe('Intake shadow', [(0.9, 0), (0, 0.1)], center, 'Refractory')
    bpy.context.view_layer.update()
    print('Cockpit, 350-seat passenger sections, boarding doors, cargo hatches and intakes added.')


def axis_lathe(name, profile_points, center, surface, axis):
    obj = lathe(name, profile_points, center, surface)
    rotation = Matrix.Rotation(-math.pi / 2, 3, 'X') if axis == 'Y' else Matrix.Rotation(math.pi / 2, 3, 'Y')
    local_center = Vector(center) - CURRENT['origin']
    for vertex in obj.data.vertices:
        vertex.co = local_center + rotation @ (vertex.co - local_center)
    return obj


def label(name, body, position, size, surface, rotation=None):
    data = bpy.data.curves.new(PREFIX + name, 'FONT')
    data.body = body
    data.size = size
    data.extrude = 0.002
    data.align_x = 'CENTER'
    data.materials.append(MATERIALS[surface])
    obj = bpy.data.objects.new(PREFIX + name, data)
    CURRENT['collection'].objects.link(obj)
    obj.parent = CURRENT['root']
    obj.location = Vector(position) - CURRENT['origin']
    if rotation:
        obj.rotation_euler = rotation
    return obj


def build_support():
    for side, title in [(-1, 'Port'), (1, 'Starboard')]:
        part('fin-' + title.lower(), title + ' stabilizer', (side * 24, 3.5, 23), 'Structure')
        outline = [(side * 22.5, 0.4, 17), (side * 22.5, 0.4, 28),
                   (side * 25.1, 7.3, 28.2), (side * 25.1, 7.3, 25.5)]
        vertices = [Vector(p) + Vector((offset, 0, 0)) for offset in (-0.16, 0.16) for p in outline]
        mesh(title + ' fin', vertices, [(0, 1, 2, 3), (7, 6, 5, 4),
             (0, 4, 5, 1), (1, 5, 6, 2), (2, 6, 7, 3), (3, 7, 4, 0)],
             ['Sky blue'], smooth=False)
        for offset in (-0.18, 0.18):
            vertices = [(side * 22.5 + offset, 0.45, 27.45),
                        (side * 22.5 + offset, 0.45, 27.8),
                        (side * 25.1 + offset, 7.22, 28.0),
                        (side * 25.1 + offset, 7.22, 27.65)]
            mesh('Gold rudder field', vertices, [(0, 1, 2, 3)], ['Solar gold'], smooth=False)
        tube('Rudder hinge', [(side * 22.7, 0.5, 25.2), (side * 25.3, 7.15, 27.4)],
             0.023, 'Seal')
    part('docking-collar', '12 Dorsal docking collar', (0, 5, -1), 'Docking')
    axis_lathe('Docking plinth', [(1.75, -0.1), (1.9, 0), (1.75, 0.3),
               (1.3, 0.3), (1.3, 0), (1.75, -0.1)], (0, 5, -1), 'Ceramic white', 'Y')
    axis_lathe('Docking seal', [(1.28, 0.18), (1.43, 0.18), (1.43, 0.39),
               (1.28, 0.39), (1.28, 0.18)], (0, 5, -1), 'Titanium', 'Y')
    axis_lathe('Airlock cover', [(0, 0.23), (1.24, 0.23), (1.24, 0.26), (0, 0.26)],
               (0, 5, -1), 'Ceramic white', 'Y')
    for j in range(8):
        a = j * math.tau / 8
        box('Docking capture latch', (1.60 * math.cos(a), 5.37, -1 + 1.60 * math.sin(a)),
            (0.25, 0.14, 0.25), 'Solar gold', 0.025)
    part('radiator', '13 Flush radiator cassette', (-4.4, 4.2, 20), 'Thermal')
    box('Radiator frame', (-4.4, 4.2, 20), (2.3, 0.22, 5.2), 'Titanium', 0.08)
    box('Radiator surface', (-4.4, 4.33, 20), (2.0, 0.045, 4.9), 'Refractory')
    for j in range(18):
        box('Heat-pipe channel', (-4.4, 4.37, 17.65 + j * 0.275),
            (1.92, 0.035, 0.06), 'Titanium')
    copy_part('radiator', 'Flush radiator · starboard', (8.8, 0, 0))
    part('rcs', '14 Manoeuvring thruster block', (-8.1, 0.0, -24), 'Propulsion')
    box('RCS housing', (-8.1, 0, -24), (0.6, 1.0, 1.6), 'Ceramic white', 0.14)
    for z in (-24.45, -23.55):
        axis_lathe('RCS lateral nozzle', [(0.23, -0.37), (0.24, -0.28), (0.11, -0.1)],
                   (-8.1, 0, z), 'Refractory', 'X')
    for name, offset in [('forward starboard', (16.2, 0, 0)),
                         ('aft port', (-3.7, 0, 49)), ('aft starboard', (19.9, 0, 49))]:
        copy_part('rcs', 'Manoeuvring block · ' + name, offset)
    for identifier, title, x, z, upper, radius in [
        ('nose-gear', '15 Nose landing gear', 0, -29, -3.3, 0.7),
        ('main-gear', '16 Main landing gear', -7, 4, -4.7, 0.85),
    ]:
        wheel_y = -8.4 + radius
        part(identifier, title, (x, (upper + wheel_y) / 2, z), 'Structure')
        box('Gear mounting cassette', (x, upper, z), (1.5, 0.30, 3.3), 'Ceramic white', 0.10)
        tube('Main oleo strut', [(x, upper, z), (x, wheel_y + 0.25, z)], 0.16, 'Titanium', sides=12)
        tube('Drag brace', [(x, upper, z + 1.2), (x, wheel_y + 0.7, z)], 0.10, 'Titanium', sides=10)
        wheel_zs = [z] if identifier == 'nose-gear' else [z - 1.0, z + 1.0]
        for wz in wheel_zs:
            for side in (-1, 1):
                center = (x + side * 0.6, wheel_y, wz)
                axis_lathe('Landing tyre', [(radius * 0.50, -0.24), (radius * 0.9, -0.24),
                    (radius, -0.12), (radius, 0.12), (radius * 0.9, 0.24),
                    (radius * 0.50, 0.24), (radius * 0.50, -0.24)], center, 'Refractory', 'X')
                axis_lathe('Wheel hub', [(0, -0.245), (radius * 0.5, -0.245),
                           (radius * 0.5, 0.245), (0, 0.245)], center, 'Titanium', 'X')
    copy_part('main-gear', 'Main landing gear · starboard', (14, 0, 0))
    use_part('passenger-forward')
    label('Upper registration', 'USE', (0, 4.74, -16), 1.65, 'Sky blue', (-math.pi / 2, 0, 0))
    bpy.context.view_layer.update()
    frame_view()
    print('Docking, twin fins, radiators, RCS and landing gear added as separate assets.')


def look_rotation(position, target):
    z = (Vector(position) - Vector(target)).normalized()
    x = Vector((0, 1, 0)).cross(z).normalized()
    y = z.cross(x)
    return Matrix((x, y, z)).transposed().to_quaternion()


def camera(scene, name, position, target, size):
    data = bpy.data.cameras.new(name)
    data.type = 'ORTHO'
    data.ortho_scale = size
    obj = bpy.data.objects.new(name, data)
    scene.collection.objects.link(obj)
    obj.location = position
    obj.rotation_mode = 'QUATERNION'
    obj.rotation_quaternion = look_rotation(position, target)
    scene.camera = obj
    return obj


def build_inventory():
    inventory = bpy.data.scenes.new('Common Sky — Parts inventory')
    inventory.unit_settings.system = 'METRIC'
    for index, item in enumerate(PARTS.values()):
        collection = bpy.data.collections.new('Inventory / ' + item['title'])
        inventory.collection.children.link(collection)
        position = Vector(((index % 4) * 43, 0, (index // 4) * 43))
        for obj in item['collection'].objects:
            if obj.type not in {'MESH', 'FONT'}:
                continue
            copied = obj.copy()
            copied.data = obj.data
            copied.parent = None
            copied.location = position + obj.location
            collection.objects.link(copied)
        data = bpy.data.curves.new('Inventory label', 'FONT')
        data.body = item['id'].upper().replace('-', ' ')
        data.size = 1.45
        data.align_x = 'CENTER'
        data.materials.append(MATERIALS['Seal'])
        obj = bpy.data.objects.new('Label / ' + item['id'], data)
        collection.objects.link(obj)
        obj.location = position + Vector((0, 0, 18))
        obj.rotation_euler.x = -math.pi / 2
    camera(inventory, 'Inventory camera', (64.5, 250, 75.9), (64.5, 0, 76), 185)
    print('Inventory scene contains', len(PARTS), 'separate part designs.')


def build_surface_details():
    for identifier, low, high, half in [
        ('flight-deck', -37, -24, 'full'),
        ('passenger-forward', -24, -4, 'top'),
        ('passenger-aft', -4, 14, 'top'),
        ('cargo-keel', -24, 6, 'bottom'),
        ('service-keel', 6, 14, 'bottom'),
        ('propulsion-frame', 14, 28, 'full'),
    ]:
        use_part(identifier)
        begin, end = (0, math.tau) if half == 'full' else ((0, math.pi) if half == 'top' else (math.pi, math.tau))
        for z in range(math.ceil(low / 5) * 5, math.floor(high), 5):
            points = [hull_point(z, begin + (end - begin) * j / 80, 0.018)
                      for j in range(81)]
            tube('Flush circumferential seam', points, 0.011, 'Seal', sides=4)
        if half != 'bottom':
            for angle in (0.70, 1.05, 2.09, 2.44):
                points = [hull_point(low + (high - low) * j / 50, angle, 0.019)
                          for j in range(51)]
                tube('Flush longitudinal seam', points, 0.009, 'Seal', sides=4)
    for side, name in [(-1, 'port'), (1, 'starboard')]:
        use_part('wing-' + name)
        tube('Elevon hinge', [(side * 14, 0.91, 24.2), (side * 20, 0.55, 24.9),
                             (side * 25, 0.26, 25.0), (side * 29, 0.08, 24.8)], 0.022, 'Seal')
        for x, front, back in [(16, 2, 28), (20, 6, 28.3), (24, 10.8, 27.4), (27, 14.8, 26.7)]:
            points = []
            for j in range(1, 30):
                u = j / 30
                thickness = max(0.18, 1.6 * (30 - x) / 18.2)
                y = thickness * math.sqrt(max(0, 1 - (1 - 2 * u) ** 2)) * (1 - 0.25 * u)
                points.append((side * x, y + 0.015, front + (back - front) * u))
            tube('Wing skin seam', points, 0.012, 'Seal', sides=4)
    print('Fine panel joints and control-surface boundaries added.')


def build_exploded():
    scene = bpy.data.scenes.new('Common Sky — Exploded assembly')
    offsets = {'flight-deck': (0, 0, -16), 'passenger-forward': (0, 12, -5),
               'passenger-aft': (0, 12, 5), 'cargo-keel': (0, -12, -5),
               'service-keel': (0, -12, 5), 'propulsion-frame': (0, 0, 14),
               'wing-port': (-16, 0, 0), 'wing-starboard': (16, 0, 0),
               'fin-port': (-12, 10, 8), 'fin-starboard': (12, 10, 8),
               'jet': (-4, 0, 24), 'chemical-rocket': (-2, 0, 24),
               'micropulse-drive': (0, 0, 24), 'docking-collar': (0, 25, 0),
               'radiator': (0, 15, 10), 'rcs': (-8, 0, -5),
               'nose-gear': (0, -18, -8), 'main-gear': (-6, -18, 0)}
    for source in SCENE.collection.children:
        roots = [obj for obj in source.objects if obj.get('part_id') or obj.get('instance_of')]
        if not roots:
            continue
        root = roots[0]
        identifier = root.get('part_id') or root['instance_of']
        delta = Vector(offsets.get(identifier, (0, 0, 0)))
        if root.get('instance_of') and root.location.x > 0:
            delta.x = abs(delta.x)
        collection = bpy.data.collections.new('Exploded / ' + root.name)
        scene.collection.children.link(collection)
        for obj in source.objects:
            if obj.type not in {'MESH', 'FONT'}:
                continue
            copied = obj.copy()
            copied.data = obj.data
            copied.parent = None
            copied.matrix_world = obj.matrix_world.copy()
            copied.location += delta
            collection.objects.link(copied)
    camera(scene, 'Exploded camera', (115, 125, -150), (0, 0, 5), 150)


def build_all():
    build_shell()
    build_engines()
    build_hull_details()
    build_support()
    build_surface_details()
    fit_runtime_mounts()
    build_inventory()
    build_exploded()


def fit_runtime_mounts():
    PARTS['chemical-rocket']['root'].location.x -= 0.2
    PARTS['rcs']['root'].location.x -= 0.6
    for obj in SCENE.objects:
        if obj.get('instance_of') == 'chemical-rocket':
            obj.location.x += 0.2
        elif obj.get('instance_of') == 'rcs':
            if obj.location.z < 0:
                obj.location.x += 0.67
            else:
                obj.location.y += 2.0
                if obj.location.x > 0:
                    obj.location.x += 0.07
    for identifier in ('chemical-rocket', 'rcs'):
        PARTS[identifier]['origin'] = PARTS[identifier]['root'].location.copy()
    bpy.context.view_layer.update()


def setup_presentation():
    for scene in [SCENE, bpy.data.scenes['Common Sky — Parts inventory'],
                  bpy.data.scenes['Common Sky — Exploded assembly']]:
        world = bpy.data.worlds.new(scene.name + ' world')
        world.use_nodes = True
        background = next(n for n in world.node_tree.nodes if n.type == 'BACKGROUND')
        background.inputs[0].default_value = (0.55, 0.65, 0.8, 1)
        background.inputs[1].default_value = 0.4
        scene.world = world
        scene.render.engine = 'CYCLES'
        scene.cycles.samples = 24
        scene.cycles.use_denoising = False
        scene.render.resolution_x = 1500
        scene.render.resolution_y = 1100
        scene.render.resolution_percentage = 100
        scene.render.image_settings.file_format = 'PNG'
        scene.render.film_transparent = True
        for name, position, energy, size in [
            ('Key', (20, 65, -35), 180000, 45),
            ('Fill', (-50, 35, -10), 90000, 40),
            ('Rim', (10, 30, 60), 140000, 35),
        ]:
            data = bpy.data.lights.new(scene.name + ' ' + name, 'AREA')
            data.energy = energy
            data.size = size
            obj = bpy.data.objects.new(data.name, data)
            scene.collection.objects.link(obj)
            obj.location = position
            obj.rotation_mode = 'QUATERNION'
            obj.rotation_quaternion = look_rotation(position, (0, 0, 0))
    camera(SCENE, 'Assembly camera', (75, 65, -100), (0, 0, 0), 88)
    camera(SCENE, 'Engine inspection camera', (0, 12, 115), (0, 0, 14), 70)
    SCENE.camera = bpy.data.objects['Assembly camera']


ATTACHMENTS = {
    'flight-deck': [('aft', 'cs-hull', (0, 0, -24), (0, 0, 1))],
    'passenger-forward': [('fore', 'cs-hull', (0, 0, -24), (0, 0, -1)),
                          ('aft', 'cs-deck', (0, 2, -4), (0, 0, 1)),
                          ('keel', 'cs-keel', (0, 0, -14), (0, -1, 0))],
    'passenger-aft': [('fore', 'cs-deck', (0, 2, -4), (0, 0, -1)),
                      ('aft', 'cs-hull', (0, 0, 14), (0, 0, 1)),
                      ('service', 'cs-service', (0, 0, 10), (0, -1, 0)),
                      ('dock', 'cs-dock', (0, 5, -1), (0, 1, 0))],
    'cargo-keel': [('deck', 'cs-keel', (0, 0, -14), (0, 1, 0))],
    'service-keel': [('deck', 'cs-service', (0, 0, 10), (0, 1, 0))],
    'propulsion-frame': [('fore', 'cs-hull', (0, 0, 14), (0, 0, -1)),
                         ('wing_port', 'cs-wing', (-12, 0, 14), (-1, 0, 0)),
                         ('wing_starboard', 'cs-wing', (12, 0, 14), (1, 0, 0)),
                         ('nuclear', 'cs-nuclear', (0, 0, 28), (0, 0, 1)),
                         ('rocket_port', 'cs-rocket', (-4.3, 0, 28), (0, 0, 1)),
                         ('rocket_starboard', 'cs-rocket', (4.3, 0, 28), (0, 0, 1)),
                         ('jet_port', 'cs-jet', (-8.3, 0, 28), (0, 0, 1)),
                         ('jet_starboard', 'cs-jet', (8.3, 0, 28), (0, 0, 1))],
    'wing-port': [('root', 'cs-wing', (-12, 0, 14), (1, 0, 0)),
                  ('fin', 'cs-fin', (-22.5, 0.4, 23), (0, 1, 0))],
    'wing-starboard': [('root', 'cs-wing', (12, 0, 14), (-1, 0, 0)),
                       ('fin', 'cs-fin', (22.5, 0.4, 23), (0, 1, 0))],
    'fin-port': [('root', 'cs-fin', (-22.5, 0.4, 23), (0, -1, 0))],
    'fin-starboard': [('root', 'cs-fin', (22.5, 0.4, 23), (0, -1, 0))],
    'jet': [('mount', 'cs-jet', (-8.3, 0, 28), (0, 0, -1))],
    'chemical-rocket': [('mount', 'cs-rocket', (-4.3, 0, 28), (0, 0, -1))],
    'micropulse-drive': [('mount', 'cs-nuclear', (0, 0, 28), (0, 0, -1))],
    'docking-collar': [('mount', 'cs-dock', (0, 5, -1), (0, -1, 0))],
}


def export_part(item):
    graph = bpy.context.evaluated_depsgraph_get()
    evaluated_meshes = []
    points = []
    for original in item['collection'].objects:
        if original.type not in {'MESH', 'FONT'}:
            continue
        evaluated = original.evaluated_get(graph)
        data = bpy.data.meshes.new_from_object(evaluated, preserve_all_data_layers=True,
                                              depsgraph=graph)
        data.transform(original.matrix_world)
        evaluated_meshes.append(data)
        points.extend(vertex.co.copy() for vertex in data.vertices)
    low = Vector(tuple(min(p[i] for p in points) for i in range(3)))
    high = Vector(tuple(max(p[i] for p in points) for i in range(3)))
    center = (low + high) / 2
    scratch = bpy.data.scenes.new('CS export scratch')
    previous = bpy.context.window.scene
    bpy.context.window.scene = scratch
    try:
        copies = []
        for data in evaluated_meshes:
            data.transform(Matrix.Translation(-center))
            obj = bpy.data.objects.new('Export component', data)
            scratch.collection.objects.link(obj)
            obj.select_set(True)
            copies.append(obj)
        bpy.context.view_layer.objects.active = copies[0]
        bpy.ops.object.join()
        joined = bpy.context.object
        joined.name = item['id']
        triangulate = joined.modifiers.new('Export triangles', 'TRIANGULATE')
        bpy.ops.object.modifier_apply(modifier=triangulate.name)
        local_points = [tuple(vertex.co) for vertex in joined.data.vertices]
        assert all(math.isfinite(v) for p in local_points for v in p), item['id']
        for axis in range(3):
            assert abs(min(p[axis] for p in local_points) + max(p[axis] for p in local_points)) < 0.001
        dimensions = [math.ceil((high[i] - low[i]) * 10 - 1e-6) for i in range(3)]
        nodes = [{'name': name, 'connector': connector,
                  'position_m': list(Vector(position) - center), 'normal': list(normal)}
                 for name, connector, position, normal in ATTACHMENTS.get(item['id'], [])]
        filename = item['id'] + '.glb'
        bpy.ops.export_scene.gltf(filepath=str(OUTPUT / filename), export_format='GLB',
                                  use_selection=True, use_active_scene=True,
                                  export_yup=False, export_apply=True,
                                  export_normals=True, export_animations=False,
                                  export_cameras=False, export_lights=False,
                                  export_extras=True)
        item['root']['model'] = 'models/common-sky/' + filename
        item['root']['model_origin_world_m'] = list(center)
        item['root']['dimensions_dm'] = dimensions
        return {'id': item['id'], 'title': item['title'], 'category': item['category'],
                'model': 'models/common-sky/' + filename, 'model_scale': 1.0,
                'dimensions_dm': dimensions, 'bounds_m': [list(low - center), list(high - center)],
                'assembly_position_m': list(center), 'nodes': nodes,
                'triangles': len(joined.data.polygons),
                'materials': sorted({slot.material.name for slot in joined.material_slots if slot.material}),
                'passenger_seats': item['root'].get('passenger_seats', 0),
                'cargo_volume_m3': item['root'].get('cargo_volume_m3', 0)}
    finally:
        bpy.context.window.scene = previous
        for obj in list(scratch.objects):
            data = obj.data
            bpy.data.objects.remove(obj, do_unlink=True)
            if data.users == 0:
                bpy.data.meshes.remove(data)
        bpy.data.scenes.remove(scratch)


def save_assets():
    OUTPUT.mkdir(parents=True, exist_ok=True)
    bpy.context.window.scene = SCENE
    bpy.context.view_layer.update()
    manifest = {'format_version': 1, 'units': 'metres',
                'axes': {'up': '+Y', 'forward': '-Z', 'exhaust': '+Z'},
                'livery': 'blue-upper-fine-gold-white-belly',
                'runtime_catalogue_registered': False, 'parts': [], 'instances': []}
    for item in PARTS.values():
        record = export_part(item)
        manifest['parts'].append(record)
        manifest['instances'].append({'part': item['id'], 'position_m': record['assembly_position_m']})
    by_id = {item['id']: item for item in manifest['parts']}
    for obj in SCENE.objects:
        identifier = obj.get('instance_of')
        if identifier:
            offset = obj.location - PARTS[identifier]['origin']
            center = Vector(by_id[identifier]['assembly_position_m']) + offset
            manifest['instances'].append({'part': identifier, 'position_m': list(center)})
    assert sum(p['passenger_seats'] for p in manifest['parts']) == 350
    assert sum(i['part'] in {'jet', 'chemical-rocket', 'micropulse-drive'}
               for i in manifest['instances']) == 5
    (OUTPUT / 'catalogue.json').write_text(json.dumps(manifest, indent=2) + '\n')
    bpy.context.window.scene = SCENE
    bpy.ops.wm.save_as_mainfile(filepath=str(OUTPUT / 'common-sky.blend'))
    print('Saved', len(manifest['parts']), 'GLBs and', len(manifest['instances']), 'assembly instances.')


def frame_view():
    for screen in bpy.data.screens:
        for area in screen.areas:
            if area.type == 'VIEW_3D':
                space = area.spaces.active
                space.shading.type = 'MATERIAL'
                space.clip_end = 3000
                space.overlay.show_floor = False
                space.overlay.show_axis_x = False
                space.overlay.show_axis_y = False
                view = Vector((65, 75, -85))
                z = view.normalized()
                x = Vector((0, 1, 0)).cross(z).normalized()
                y = z.cross(x)
                space.region_3d.view_rotation = Matrix((x, y, z)).transposed().to_quaternion()
                space.region_3d.view_location = (0, 0, 0)
                space.region_3d.view_distance = 94


def build_shell():
    start()
    hull('flight-deck', '01 Flight deck and nose', -38, -24, 'full', 'Command')
    hull('passenger-forward', '02 Forward passenger deck', -24, -4, 'top', 'Accommodation')
    hull('passenger-aft', '03 Aft passenger deck', -4, 14, 'top', 'Accommodation')
    hull('cargo-keel', '04 Cargo keel', -24, 6, 'bottom', 'Storage')
    hull('service-keel', '05 Service keel', 6, 14, 'bottom', 'Utilities')
    hull('propulsion-frame', '06 Propulsion frame', 14, 28, 'full', 'Structure')
    wing(-1)
    wing(1)
    frame_view()
    bpy.context.view_layer.update()
    print('Common Sky: eight primary hull and wing assets are visible.')


if __name__ == '__main__':
    build_all()
    setup_presentation()
    save_assets()
