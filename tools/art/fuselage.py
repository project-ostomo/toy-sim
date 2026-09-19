import math
from pathlib import Path

import bpy
from mathutils import Vector


ROOT = Path(__file__).resolve().parents[2]
MODELS = ROOT / "assets/models/parts"
SECTION = "Fuselage 8m x 16m"
END = "Fuselage end 8m"


def shield_material():
    material = bpy.data.materials.get("Whipple outer sheet")
    if material is None:
        material = bpy.data.materials.new("Whipple outer sheet")
    material.use_nodes = True
    material.diffuse_color = (0.38, 0.42, 0.45, 1)
    nodes = material.node_tree.nodes
    shader = next(node for node in nodes if node.type == "BSDF_PRINCIPLED")
    shader.inputs["Base Color"].default_value = material.diffuse_color
    shader.inputs["Metallic"].default_value = 0.4
    shader.inputs["Roughness"].default_value = 0.58
    for node in list(nodes):
        if node.type in ("TEX_IMAGE", "NORMAL_MAP"):
            nodes.remove(node)
    return material


def surface_uv(mesh):
    uv = mesh.uv_layers.active or mesh.uv_layers.new(name="Surface")
    for face in mesh.polygons:
        normal = face.normal
        tangent = Vector((-normal.y, normal.x, 0))
        if tangent.length < 0.01:
            tangent = Vector((1, 0, 0))
        tangent.normalize()
        vertical = normal.cross(tangent).normalized()
        for index in face.loop_indices:
            position = mesh.vertices[mesh.loops[index].vertex_index].co
            uv.data[index].uv = (position.dot(tangent) * 2, position.dot(vertical) * 2)


def mesh_object(collection, name, vertices, faces, material):
    mesh = bpy.data.meshes.new(name)
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    surface_uv(mesh)
    mesh.materials.append(material)
    obj = bpy.data.objects.new(name, mesh)
    collection.objects.link(obj)
    return obj


def octagon(apothem, z):
    radius = apothem / math.cos(math.pi / 8)
    return [Vector((radius * math.cos(i * math.pi / 4 - math.pi / 8),
                    radius * math.sin(i * math.pi / 4 - math.pi / 8), z))
            for i in range(8)]


def make_end(material):
    collection = bpy.data.collections.get(END)
    if collection is None:
        collection = bpy.data.collections.new(END)
    for obj in list(collection.objects):
        bpy.data.objects.remove(obj, do_unlink=True)
    scene = bpy.data.scenes.get("Fuselage - end") or bpy.data.scenes.new("Fuselage - end")
    if collection.name not in scene.collection.children:
        scene.collection.children.link(collection)

    levels = [octagon(4, 0.988), octagon(4, 0.68), octagon(3.2, -0.985)]
    for i in range(8):
        vertices = []
        for level in levels:
            a, b = level[i], level[(i + 1) % 8]
            vertices.extend((a.lerp(b, 0.004), b.lerp(a, 0.004)))
        obj = mesh_object(collection, f"End shield panel {i:02}", vertices,
                          [(0, 2, 3, 1), (2, 4, 5, 3)], material)
        thickness = obj.modifiers.new("Bumper thickness", "SOLIDIFY")
        thickness.thickness = 0.035
        thickness.offset = -1
        bevel = obj.modifiers.new("Panel edges", "BEVEL")
        bevel.width = 0.004
        bevel.segments = 2
        obj.modifiers.new("Face normals", "WEIGHTED_NORMAL")

    front = mesh_object(collection, "End shield face", octagon(3.188, -1),
                        [tuple(reversed(range(8)))], material)
    thickness = front.modifiers.new("Bumper thickness", "SOLIDIFY")
    thickness.thickness = 0.035
    thickness.offset = -1

    profile = [(3.62, 1), (3.62, 0.68), (2.96, -0.74), (2.96, -0.66),
               (3.54, 0.68), (3.54, 1)]
    vertices = [point for a, z in profile for point in octagon(a, z)]
    faces = []
    for j in (0, 1, 3, 4, 5):
        k = (j + 1) % len(profile)
        faces.extend((j * 8 + i, k * 8 + i, k * 8 + (i + 1) % 8, j * 8 + (i + 1) % 8)
                     for i in range(8))
    faces.extend([tuple(reversed(range(16, 24))), tuple(range(24, 32))])
    mesh_object(collection, "End inner closure", vertices, faces,
                bpy.data.materials["Structure - graphite"])

    for i in range(8):
        theta = i * math.pi / 4
        radial = Vector((math.cos(theta), math.sin(theta), 0))
        tangent = Vector((-math.sin(theta), math.cos(theta), 0))
        for j, (r, z, depth) in enumerate([(3.96, 0.78, 0.35), (3.53, -0.22, 0.32)]):
            vertices = [radial * radius + tangent * u + Vector((0, 0, height))
                        for radius in (r - depth, r)
                        for u, height in [(-0.05, z - 0.05), (0.05, z - 0.05),
                                          (0.05, z + 0.05), (-0.05, z + 0.05)]]
            mesh_object(collection, f"End shield support {i:02}-{j}", vertices,
                        [(3, 2, 1, 0), (4, 5, 6, 7), (0, 1, 5, 4),
                         (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)],
                        bpy.data.materials["Edges - titanium"])
    return collection


def prepare():
    material = shield_material()
    section = bpy.data.collections[SECTION]
    for obj in section.objects:
        if obj.type == "MESH":
            surface_uv(obj.data)
            if obj.name.startswith("Whipple bumper"):
                obj.data.materials.clear()
                obj.data.materials.append(material)
    return section, make_end(material)


def export(collection, filename):
    original_scene = bpy.context.window.scene
    scratch = bpy.data.scenes.new("Fuselage export scratch")
    bpy.context.window.scene = scratch
    copies = []
    try:
        for original in collection.objects:
            if original.type != "MESH":
                continue
            obj = original.copy()
            obj.data = original.data.copy()
            obj.parent = None
            obj.matrix_world = original.matrix_world.copy()
            for modifier in list(obj.modifiers):
                if modifier.type == "BEVEL":
                    if modifier.width <= 0.003:
                        obj.modifiers.remove(modifier)
                    else:
                        modifier.segments = 1
            obj.modifiers.new("Export triangles", "TRIANGULATE")
            scratch.collection.objects.link(obj)
            copies.append(obj)
        bpy.context.view_layer.update()
        graph = bpy.context.evaluated_depsgraph_get()
        for obj in copies:
            evaluated = obj.evaluated_get(graph)
            mesh = bpy.data.meshes.new_from_object(evaluated, preserve_all_data_layers=True, depsgraph=graph)
            obj.modifiers.clear()
            old = obj.data
            obj.data = mesh
            bpy.data.meshes.remove(old)
            obj.select_set(True)
        bpy.context.view_layer.objects.active = copies[0]
        bpy.ops.object.join()
        joined = bpy.context.object
        joined.name = filename.removesuffix(".glb")
        bpy.ops.export_scene.gltf(filepath=str(MODELS / filename), export_format="GLB",
                                  use_selection=True, use_active_scene=True,
                                  export_yup=False, export_apply=True,
                                  export_normals=True, export_tangents=True,
                                  export_cameras=False, export_lights=False)
    finally:
        bpy.context.window.scene = original_scene
        for obj in list(scratch.objects):
            mesh = obj.data
            bpy.data.objects.remove(obj, do_unlink=True)
            if mesh.users == 0:
                bpy.data.meshes.remove(mesh)
        bpy.data.scenes.remove(scratch)


if __name__ == "__main__":
    section, end = prepare()
    export(section, "fuselage-8m.glb")
    export(end, "fuselage-end-8m.glb")
    bpy.ops.wm.save_as_mainfile(filepath=str(MODELS / "fuselage-08m-16m.blend"))
