import bpy, sys, os, struct

source, output = sys.argv[sys.argv.index('--') + 1:]
bpy.ops.object.select_all(action='SELECT')
bpy.ops.object.delete(use_global=False)
ext = os.path.splitext(source)[1].lower()
if ext == '.obj':
    bpy.ops.wm.obj_import(filepath=source)
elif ext in ('.gltf', '.glb'):
    bpy.ops.import_scene.gltf(filepath=source)
elif ext == '.fbx':
    bpy.ops.import_scene.fbx(filepath=source)
elif ext in ('.usd', '.usda', '.usdc', '.usdz'):
    bpy.ops.wm.usd_import(filepath=source)
else:
    raise RuntimeError('Unsupported 3D format: ' + ext)

depsgraph = bpy.context.evaluated_depsgraph_get()
total = 0
with open(output, 'wb') as stream:
    stream.write(b'AV3D0001' + struct.pack('<I', 0))
    for instance in depsgraph.object_instances:
        obj = instance.object
        if obj.type != 'MESH':
            continue
        mesh = obj.to_mesh()
        try:
            mesh.calc_loop_triangles()
            matrix = instance.matrix_world
            normal_matrix = matrix.to_3x3().inverted_safe().transposed()
            for triangle in mesh.loop_triangles:
                total += 1
                if total > 2000000:
                    raise RuntimeError('Model exceeds the preview limit of 2 million triangles')
                color = (0.65, 0.72, 0.82)
                if triangle.material_index < len(mesh.materials):
                    material = mesh.materials[triangle.material_index]
                    if material:
                        color = tuple(material.diffuse_color[:3])
                        if material.use_nodes:
                            principled = next((n for n in material.node_tree.nodes if n.type == 'BSDF_PRINCIPLED'), None)
                            if principled:
                                color = tuple(principled.inputs['Base Color'].default_value[:3])
                for index in triangle.vertices:
                    vertex = mesh.vertices[index]
                    position = matrix @ vertex.co
                    normal = (normal_matrix @ (vertex.normal if triangle.use_smooth else triangle.normal)).normalized()
                    # Blender's Z-up world becomes a Y-up preview.
                    stream.write(struct.pack('<9f', position.x, position.z, -position.y,
                        normal.x, normal.z, -normal.y, *color))
        finally:
            obj.to_mesh_clear()
    if total == 0:
        raise RuntimeError('No mesh geometry found in this file')
    stream.seek(8)
    stream.write(struct.pack('<I', total))
print('ARCHVIEW_TRIANGLES', total)
