import json
import math
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / 'assets/models/common-sky/catalogue.json'
OUTPUT = ROOT / 'crates/toy-sim-ships/data/common-sky.toml'
PROFILE = [(-38, .15, .15), (-36, 2.4, 1.7), (-32, 4.9, 3),
           (-28, 6.8, 3.7), (-24, 8.1, 4.2), (-16, 10.1, 4.7),
           (-8, 11.5, 5), (-4, 12, 5), (6, 12, 4.8),
           (14, 12, 4.4), (22, 12, 3.9), (28, 12, 3.5)]


def profile(z):
    for a, b in zip(PROFILE, PROFILE[1:]):
        if a[0] <= z <= b[0]:
            t = (z - a[0]) / (b[0] - a[0])
            return [a[i] * (1 - t) + b[i] * t for i in (1, 2)]
    raise ValueError(z)


def subtract(box, hole):
    low, high = box
    lo = [max(low[i], hole[0][i]) for i in range(3)]
    hi = [min(high[i], hole[1][i]) for i in range(3)]
    if any(lo[i] >= hi[i] for i in range(3)):
        return [box]
    result = []
    low, high = low.copy(), high.copy()
    for i in range(3):
        if low[i] < lo[i]:
            end = high.copy()
            end[i] = lo[i]
            result.append((low.copy(), end))
            low[i] = lo[i]
        if high[i] > hi[i]:
            start = low.copy()
            start[i] = hi[i]
            result.append((start, high.copy()))
            high[i] = hi[i]
    return result


def collision(record):
    identifier = record['id']
    center = record['assembly_position_m']
    boxes = []
    hulls = {'flight-deck': (-38, -24, -1, 1),
             'passenger-forward': (-24, -4, 0, 1),
             'passenger-aft': (-4, 14, 0, 1),
             'cargo-keel': (-24, 6, -1, 0),
             'service-keel': (6, 14, -1, 0),
             'propulsion-frame': (14, 28, -1, 1)}
    if identifier in hulls:
        front, back, bottom, top = hulls[identifier]
        for z in range(front, back):
            width, height = profile(z + .5)
            low_y, high_y = height * bottom, height * top
            count = math.ceil((high_y - low_y) / .5)
            for row in range(count):
                y0 = low_y + (high_y - low_y) * row / count
                y1 = low_y + (high_y - low_y) * (row + 1) / count
                y = (y0 + y1) / 2
                x = width * max(0, 1 - abs(y / height) ** 2.5) ** .4
                boxes.append(([-x, y0, z], [x, y1, z + 1]))
        if identifier == 'propulsion-frame':
            for x, z in [(-3, 19), (3, 19), (0, 23)]:
                hole = ([x - 1.1, -1.1, z - 1.1], [x + 1.1, 1.1, z + 1.1])
                boxes = [piece for box in boxes for piece in subtract(box, hole)]
    elif identifier.startswith('wing-'):
        side = -1 if identifier.endswith('port') and not identifier.endswith('starboard') else 1
        stations = [(12, -4, 28, 1.6), (15, 0, 29, 1.35),
                    (20, 6, 29, .9), (25, 12, 28, .5), (30, 19, 26, .16)]
        for x in range(12, 30):
            for a, b in zip(stations, stations[1:]):
                if a[0] <= x + .5 <= b[0]:
                    t = (x + .5 - a[0]) / (b[0] - a[0])
                    front, back, thick = [a[i] * (1 - t) + b[i] * t for i in (1, 2, 3)]
                    for z in range(math.ceil(front), math.floor(back)):
                        u = (z + .5 - front) / (back - front)
                        h = thick * math.sqrt(1 - (1 - 2 * u) ** 2) * (1 - .25 * u)
                        xs = sorted((side * (x + .02), side * (x + 1)))
                        boxes.append(([xs[0], -h, z], [xs[1], h, z + 1]))
                    break
    else:
        low, high = [[p[i] + center[i] for i in range(3)] for p in record['bounds_m']]
        if identifier.startswith('fin-'):
            low[1] = 1.05
        elif identifier == 'nose-gear':
            high[1] = -4.0
        elif identifier == 'main-gear':
            high[1] = -5.1
        elif identifier == 'docking-collar':
            low[1] = 5.01
        elif identifier in ('jet', 'micropulse-drive'):
            low[2] = 28.01
        boxes.append((low, high))
    return {'shape': 'compound', 'boxes': [
        {'min': [round(low[i] - center[i], 6) for i in range(3)],
         'max': [round(high[i] - center[i], 6) for i in range(3)]}
        for low, high in boxes]}


def value(item):
    if isinstance(item, dict):
        return '{ ' + ', '.join(f'{key} = {value(v)}' for key, v in item.items()) + ' }'
    if isinstance(item, list):
        return '[' + ', '.join(value(v) for v in item) + ']'
    return json.dumps(item)


def main():
    manifest = json.loads(MANIFEST.read_text())
    records = {p['id']: p for p in manifest['parts']}
    equipment = {
        'flight-deck': dict(kind='utility', utility=dict(type='command', power_w=2000.0)),
        'passenger-forward': dict(kind='utility', utility=dict(type='crew', capacity=175)),
        'passenger-aft': dict(kind='utility', utility=dict(type='crew', capacity=175)),
        'cargo-keel': dict(kind='storage', capacity_m3=400.0),
        'service-keel': dict(kind='utility', utility=dict(type='life_support', capacity=350,
                            power_w=350000.0, supplies_kg_per_person_s=.00003)),
        'propulsion-frame': dict(kind='battery', capacity_j=10000000000),
        'chemical-rocket': dict(kind='engine', thrust_n=2000000.0, propellant_kg_s=500.0,
                               power_w=100000.0, propellant_resource='cs_rocket_propellant',
                               propellant_energy_j_kg=12000000.0),
        'micropulse-drive': dict(kind='micropulse_engine', thrust_n=4000000.0,
                                specific_impulse_s=45000.0, charge_energy_j_kg=300000000000.0,
                                electric_efficiency=.99, absorbed_heat_fraction=.0001),
        'rcs': dict(kind='rcs', propellant_resource='propellant', thrust_n=1000.0,
                    propellant_kg_s=1.0, power_w=500000.0),
        'radiator': dict(kind='radiator', area_m2=24.0, emissivity=.92),
        'docking-collar': dict(kind='utility', utility=dict(type='docking', radius_m=1.9,
                                                        mass_capacity_kg=1000000.0)),
    }
    masses = [12000, 20000, 18000, 16000, 10000, 24000, 14000, 14000,
              6000, 4000, 12000, 1800, 1800, 1000, 600, 300, 1800, 3200]
    parts = {}
    for record, mass in zip(manifest['parts'], masses, strict=True):
        identifier = record['id']
        parts[identifier] = dict(id='cs_' + identifier.replace('-', '_'),
            title='USE Common Sky / ' + record['title'], dimensions=record['dimensions_dm'],
            mass_kg=float(mass), hull=float(mass * 2), color=[.025, .37, .80],
            model=record['model'], model_scale=1.0, nodes=[], collision=collision(record),
            **equipment.get(identifier, dict(kind='structure')))
    parts['propulsion-frame']['tank_volume_m3'] = 240.0
    for identifier, length, radius, color in [
        ('chemical-rocket', 45.0, 1.17, [0.5, 0.7, 1.0]),
        ('micropulse-drive', 130.0, 2.06, [0.15, 0.35, 1.0]),
    ]:
        record = records[identifier]
        parts[identifier]['plume'] = dict(
            origin_m=[0.0, 0.0, record['bounds_m'][1][2] - .08],
            length_m=length, nozzle_radius_m=radius, expansion_half_angle_rad=.025,
            color_linear_rgb=color, intensity=60.0, axial_falloff=2.0,
            radial_falloff=2.0, noise_strength=.12, noise_scale_m=.5,
            noise_speed_m_s=8.0)
    for identifier, kind, x, z in [('auxiliary-generator', 'generator', -3, 19),
                                   ('attitude-actuator', 'torquer', 3, 19),
                                   ('thermal-shield', 'shield', 0, 23)]:
        record = dict(id=identifier, assembly_position_m=[x, 0, z])
        records[identifier] = record
        part = dict(id='cs_' + identifier.replace('-', '_'),
                    title='USE Common Sky / ' + identifier.replace('-', ' ').title(),
                    dimensions=[20, 20, 20], mass_kg=2000.0, hull=4000.0,
                    color=[.025, .37, .80], nodes=[], model_scale=2.0, kind=kind)
        if kind == 'generator':
            part.update(model='models/parts/auxiliary-generator-1m.glb', power_w=5000000.0,
                        fuel_kg_s=.5, efficiency=.4)
        elif kind == 'torquer':
            part.update(model='models/parts/torquer-1m.glb', torque_nm=5000000.0,
                        power_w=2000000.0)
        else:
            part.update(model='models/parts/shield-emitter-2m.glb', model_scale=1.0,
                        deployed_mass_kg=250.0, radiator_area_m2=1000.0,
                        feed_rate_kg_s=1.0, power_w=500000.0)
        parts[identifier] = part

    assembly = []
    centers = {}

    def attach(identifier, parent=None, socket='', plug='mount', point=None,
               normal=(0, 0, 1), offset=(0, 0, 0)):
        index = len(assembly) + 1
        center = [records[identifier]['assembly_position_m'][i] + offset[i] for i in range(3)]
        centers[index] = center
        entry = dict(prototype=parts[identifier]['id'], parent=parent or 0,
                     socket=socket, plug=plug, roll=0,
                     position_m=center)
        if parent:
            parent_identifier = next(k for k, p in parts.items() if p['id'] == assembly[parent - 1]['prototype'])
            connector = 'cs_' + identifier.replace('-', '_')
            for key, name, origin, direction in [(parent_identifier, socket, centers[parent], normal),
                                               (identifier, plug, center, tuple(-n for n in normal))]:
                node = dict(name=name, connector=connector,
                            position_m=[round(point[i] - origin[i], 6) for i in range(3)],
                            normal=list(direction))
                existing = next((n for n in parts[key]['nodes'] if n['name'] == name), None)
                if existing:
                    assert existing == node, (key, existing, node)
                else:
                    parts[key]['nodes'].append(node)
        assembly.append(entry)
        return index

    nose = attach('flight-deck')
    front = attach('passenger-forward', nose, 'aft', point=(0, 0, -24))
    aft = attach('passenger-aft', front, 'aft', point=(0, 2, -4))
    cargo = attach('cargo-keel', front, 'keel', point=(0, 0, -14), normal=(0, -1, 0))
    attach('service-keel', aft, 'service', point=(0, 0, 10), normal=(0, -1, 0))
    frame = attach('propulsion-frame', aft, 'aft', point=(0, 0, 14))
    for side, label in [(-1, 'port'), (1, 'starboard')]:
        wing = attach('wing-' + label, frame, 'wing_' + label,
                      point=(side * 12, 0, 14), normal=(side, 0, 0))
        attach('fin-' + label, wing, 'fin', point=(side * 22.5, .4, 23), normal=(0, 1, 0))
        offset = (16.6, 0, 0) if side == 1 else (0, 0, 0)
        attach('jet', frame, 'jet_' + label, point=(side * 8.3, 0, 28), offset=offset)
        offset = (8.6, 0, 0) if side == 1 else (0, 0, 0)
        attach('chemical-rocket', frame, 'rocket_' + label,
               point=(side * 4.3, 0, 28), offset=offset)
        attach('radiator', frame, 'radiator_' + label, point=(side * 4.4, 4.1, 20),
               normal=(0, 1, 0), offset=(8.8 if side == 1 else 0, 0, 0))
        attach('main-gear', cargo, 'gear_' + label, point=(side * 7, -5.1, 4),
               normal=(0, -1, 0), offset=(14 if side == 1 else 0, 0, 0))
    attach('micropulse-drive', frame, 'nuclear', point=(0, 0, 28))
    attach('docking-collar', aft, 'dock', point=(0, 5, -1), normal=(0, 1, 0))
    attach('nose-gear', nose, 'gear', point=(0, -4, -29), normal=(0, -1, 0))
    for index, offset in enumerate([(0, 0, 0), (17.47, 0, 0), (-3.1, 2, 49), (20.57, 2, 49)]):
        center = [records['rcs']['assembly_position_m'][i] + offset[i] for i in range(3)]
        attach('rcs', frame if index >= 2 else nose, 'rcs_' + str(index),
               point=(center[0], center[1], center[2] - .8), offset=offset)
    attach('auxiliary-generator', frame, 'generator', point=(-3, 0, 18))
    attach('attitude-actuator', frame, 'torquer', point=(3, 0, 18))
    attach('thermal-shield', frame, 'thermal_shield', point=(0, 0, 22))

    lines = ['revision = 3', '', '[[resources]]', 'id = "cs_rocket_propellant"',
             'title = "Common Sky chemical propellant mixture (kg)"',
             'mass_kg = 1.0', 'volume_m3 = 0.001', '']
    for part in parts.values():
        lines.append('[[parts]]')
        for key, item in part.items():
            lines.append(f'{key} = {value(item)}')
        lines.append('')
    for instance in assembly:
        lines.append('[[assembly]]')
        lines.extend(f'{key} = {value(item)}' for key, item in instance.items())
        lines.append('')
    OUTPUT.write_text('\n'.join(lines))
    manifest['runtime_catalogue_registered'] = True
    manifest['runtime_blueprint'] = 'ships/common-sky.ship'
    for record in manifest['parts']:
        definition = parts[record['id']]
        record['runtime_prototype'] = definition['id']
        record['nodes'] = definition['nodes']
    exported = {record['runtime_prototype']: record['id'] for record in manifest['parts']}
    manifest['instances'] = [
        dict(part=exported[instance['prototype']], position_m=instance['position_m'])
        for instance in assembly if instance['prototype'] in exported]
    manifest['runtime_instances'] = assembly
    MANIFEST.write_text(json.dumps(manifest, indent=2) + '\n')
    print(f'Generated {len(parts)} catalogue parts and {len(assembly)} assembled instances.')


if __name__ == '__main__':
    main()
