use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use std::{
    collections::BTreeMap,
    hint::black_box,
    io::{Read, Write},
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Object {
    id: u64,
    name: String,
    design: u64,
    position: [f64; 3],
    velocity: [f64; 3],
    rotation: [f64; 4],
    heat: f64,
    reserve: f64,
    throttle: f64,
    turret: [f64; 2],
    flags: u64,
}

impl Object {
    fn words(&self) -> [u64; 16] {
        let mut words = [0; 16];
        let values = self
            .position
            .into_iter()
            .chain(self.velocity)
            .chain(self.rotation)
            .chain([self.heat, self.reserve, self.throttle])
            .chain(self.turret);
        for (word, value) in words.iter_mut().zip(values) {
            *word = value.to_bits();
        }
        words[15] = self.flags;
        words
    }

    fn set_words(&mut self, words: [u64; 16]) {
        let values = words.map(f64::from_bits);
        self.position.copy_from_slice(&values[..3]);
        self.velocity.copy_from_slice(&values[3..6]);
        self.rotation.copy_from_slice(&values[6..10]);
        self.heat = values[10];
        self.reserve = values[11];
        self.throttle = values[12];
        self.turret.copy_from_slice(&values[13..15]);
        self.flags = words[15];
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Header {
    tick: u64,
    time_ns: u64,
    view_revision: u64,
    focus: u64,
    own_ship: [f64; 8],
    screen_text: Vec<String>,
    events: Vec<(u64, u64, f64)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
struct Frame {
    header: Header,
    objects: Vec<Object>,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Snapshot,
    Compact,
    Delta,
}

impl Format {
    fn label(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Compact => "compact",
            Self::Delta => "delta",
        }
    }
}

type ObjectTuple = (
    u64,
    String,
    u64,
    [f64; 3],
    [f64; 3],
    [f64; 4],
    f64,
    f64,
    f64,
    [f64; 2],
    u64,
);

fn compact_encode(frame: &Frame) -> Vec<u8> {
    let objects: Vec<_> = frame
        .objects
        .iter()
        .map(|o| {
            (
                o.id, &o.name, o.design, o.position, o.velocity, o.rotation, o.heat, o.reserve,
                o.throttle, o.turret, o.flags,
            )
        })
        .collect();
    cbor(&(&frame.header, objects))
}

fn compact_decode(bytes: &[u8]) -> Frame {
    let (header, objects): (Header, Vec<ObjectTuple>) = ciborium::from_reader(bytes).unwrap();
    let objects = objects
        .into_iter()
        .map(|o| Object {
            id: o.0,
            name: o.1,
            design: o.2,
            position: o.3,
            velocity: o.4,
            rotation: o.5,
            heat: o.6,
            reserve: o.7,
            throttle: o.8,
            turret: o.9,
            flags: o.10,
        })
        .collect();
    Frame { header, objects }
}

#[derive(Serialize, Deserialize)]
struct Delta {
    header: Header,
    removed: Vec<u64>,
    added: Vec<Object>,
    patches: ByteBuf,
}

fn cbor<T: Serialize>(value: &T) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::into_writer(value, &mut bytes).unwrap();
    bytes
}

fn varint(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8 & 127) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

fn take_varint(input: &mut &[u8]) -> u64 {
    let mut n = 0;
    for shift in (0..=63).step_by(7) {
        let b = input[0];
        *input = &input[1..];
        n |= u64::from(b & 127) << shift;
        if b < 128 {
            return n;
        }
    }
    panic!("invalid generated varint")
}

fn delta_encode(frame: &Frame, previous: &[Object]) -> Vec<u8> {
    let mut removed = Vec::new();
    let mut added = Vec::new();
    let mut patches = Vec::new();
    let mut old = previous.iter().peekable();
    let mut last_id = 0;

    for object in &frame.objects {
        while old.peek().is_some_and(|p| p.id < object.id) {
            removed.push(old.next().unwrap().id);
        }
        let Some(prior) = old.peek().filter(|p| p.id == object.id) else {
            added.push(object.clone());
            continue;
        };
        if prior.name != object.name || prior.design != object.design {
            added.push(object.clone());
        } else {
            let before = prior.words();
            let after = object.words();
            let mut mask = 0u64;
            for i in 0..16 {
                if before[i] != after[i] {
                    mask |= 1 << i;
                }
            }
            if mask != 0 {
                varint(object.id - last_id, &mut patches);
                last_id = object.id;
                varint(mask, &mut patches);
                for i in 0..16 {
                    if mask & (1 << i) != 0 {
                        varint(before[i] ^ after[i], &mut patches);
                    }
                }
            }
        }
        old.next();
    }
    removed.extend(old.map(|p| p.id));
    cbor(&Delta {
        header: frame.header.clone(),
        removed,
        added,
        patches: patches.into(),
    })
}

fn delta_decode(bytes: &[u8], state: &mut BTreeMap<u64, Object>) -> Frame {
    let delta: Delta = ciborium::from_reader(bytes).unwrap();
    for id in delta.removed {
        state.remove(&id);
    }
    for object in delta.added {
        state.insert(object.id, object);
    }
    let mut input = delta.patches.as_slice();
    let mut id = 0;
    while !input.is_empty() {
        id += take_varint(&mut input);
        let mask = take_varint(&mut input);
        let object = state.get_mut(&id).unwrap();
        let mut words = object.words();
        for (i, word) in words.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                *word ^= take_varint(&mut input);
            }
        }
        object.set_words(words);
    }
    Frame {
        header: delta.header,
        objects: state.values().cloned().collect(),
    }
}

struct Random(u64);

impl Random {
    fn word(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    fn unit(&mut self) -> f64 {
        (self.word() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn object(id: u64, random: &mut Random) -> Object {
    Object {
        id,
        name: format!("Ship {id:08x}"),
        design: 1 + id % 32,
        position: std::array::from_fn(|_| (random.unit() - 0.5) * 2e7),
        velocity: std::array::from_fn(|_| (random.unit() - 0.5) * 2e4),
        rotation: [0.0, 0.0, 0.0, 1.0],
        heat: random.unit() * 1e8,
        reserve: 100.0,
        throttle: 0.0,
        turret: [0.0; 2],
        flags: 3,
    }
}

fn corpus(count: usize, combat: bool, frames: usize) -> Vec<Frame> {
    let mut random = Random(42);
    let mut objects: Vec<_> = (1..=count as u64)
        .map(|id| object(id, &mut random))
        .collect();
    let mut next_id = count as u64 + 1;
    let mut result = Vec::new();

    for tick in 0..frames {
        if combat && tick > 0 && tick % 10 == 0 {
            let churn = (count / 100).max(1);
            objects.drain(..churn);
            for _ in 0..churn {
                objects.push(object(next_id, &mut random));
                next_id += 1;
            }
        }
        for object in &mut objects {
            let maneuver = combat && object.id % 3 == 0;
            for i in 0..3 {
                if maneuver {
                    object.velocity[i] += (random.unit() - 0.5) * 4.0;
                }
                object.position[i] += object.velocity[i] * 0.1;
            }
            if maneuver {
                let angle = (tick as f64 * 0.007 + object.id as f64 * 0.13).sin();
                object.rotation = [0.0, (angle * 0.5).sin(), 0.0, (angle * 0.5).cos()];
                object.heat = (object.heat + (random.unit() - 0.4) * 1e5).max(0.0);
                object.reserve = (object.reserve - random.unit() * 0.002).max(0.0);
                object.throttle = 0.5 + 0.5 * angle;
                object.turret = [angle * 2.0, angle * 0.3];
                object.flags = if object.heat > 5e7 { 7 } else { 3 };
            }
        }
        let header = Header {
            tick: tick as u64,
            time_ns: tick as u64 * 100_000_000,
            view_revision: 1,
            focus: objects[0].id,
            own_ship: [
                1e8 - tick as f64 * 1000.0,
                3500.0,
                99.0,
                0.7,
                2500.0,
                1.0,
                0.0,
                0.0,
            ],
            screen_text: (0..16)
                .map(|i| format!("Instrument {i}: {}", tick / 10 + i))
                .collect(),
            events: if combat {
                (0..4)
                    .map(|i| {
                        (
                            tick as u64 * 4 + i,
                            objects[i as usize].id,
                            tick as f64 * 0.1 + i as f64 * 0.02,
                        )
                    })
                    .collect()
            } else {
                Vec::new()
            },
        };
        result.push(Frame {
            header,
            objects: objects.clone(),
        });
    }
    result
}

#[derive(Clone, Copy)]
enum Codec {
    Raw,
    Lz4,
    Zstd(i32, u32),
    ZstdIndependent,
}

impl Codec {
    fn label(self) -> String {
        match self {
            Self::Raw => "raw".into(),
            Self::Lz4 => "lz4-linked".into(),
            Self::Zstd(level, window) => format!("zstd{level}-w{window}"),
            Self::ZstdIndependent => "zstd1-independent".into(),
        }
    }
}

enum Encoder {
    Raw(Vec<u8>),
    Lz4(lz4::Encoder<Vec<u8>>),
    Zstd(zstd::stream::write::Encoder<'static, Vec<u8>>),
    Independent(Vec<u8>),
}

impl Encoder {
    fn new(codec: Codec) -> Self {
        match codec {
            Codec::Raw => Self::Raw(Vec::new()),
            Codec::Lz4 => Self::Lz4(
                lz4::EncoderBuilder::new()
                    .block_mode(lz4::BlockMode::Linked)
                    .build(Vec::new())
                    .unwrap(),
            ),
            Codec::Zstd(level, window) => {
                let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), level).unwrap();
                encoder.window_log(window).unwrap();
                Self::Zstd(encoder)
            }
            Codec::ZstdIndependent => Self::Independent(Vec::new()),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Raw(v) | Self::Independent(v) => v.len(),
            Self::Lz4(e) => e.writer().len(),
            Self::Zstd(e) => e.get_ref().len(),
        }
    }

    fn packet(&mut self, bytes: &[u8]) {
        match self {
            Self::Raw(v) => v.extend_from_slice(bytes),
            Self::Lz4(e) => {
                e.write_all(bytes).unwrap();
                e.flush().unwrap();
            }
            Self::Zstd(e) => {
                e.write_all(bytes).unwrap();
                e.flush().unwrap();
            }
            Self::Independent(v) => v.extend(zstd::stream::encode_all(bytes, 1).unwrap()),
        }
    }

    fn finish(self) -> Vec<u8> {
        match self {
            Self::Raw(v) | Self::Independent(v) => v,
            Self::Lz4(e) => {
                let (v, status) = e.finish();
                status.unwrap();
                v
            }
            Self::Zstd(e) => e.finish().unwrap(),
        }
    }
}

fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted[((sorted.len() - 1) as f64 * q).round() as usize]
}

fn run(name: &str, frames: &[Frame], assets: bool, format: Format, codec: Codec, repeat: usize) {
    let mut encoder = Encoder::new(codec);
    let mut expected = Vec::new();
    let mut sizes = Vec::new();
    let mut wire_sizes = Vec::new();
    let mut encode_us = Vec::new();
    let mut compress_us = Vec::new();
    let mut asset_total = 0;
    let mut previous: &[Object] = &[];
    let mut random = Random(12345);

    for frame in frames {
        let start = Instant::now();
        let bytes = match format {
            Format::Delta => delta_encode(frame, previous),
            Format::Compact => compact_encode(frame),
            Format::Snapshot => cbor(frame),
        };
        encode_us.push(start.elapsed().as_secs_f64() * 1e6);
        previous = &frame.objects;
        let before = encoder.len();
        let start = Instant::now();
        encoder.packet(&bytes);
        compress_us.push(start.elapsed().as_secs_f64() * 1e6);
        wire_sizes.push(encoder.len() - before);
        sizes.push(bytes.len());
        expected.push(bytes);
        if assets && frame.header.tick % 10 == 9 {
            let blob: Vec<u8> = (0..32768)
                .flat_map(|_| random.word().to_le_bytes())
                .collect();
            let before = encoder.len();
            encoder.packet(&blob);
            asset_total += encoder.len() - before;
            expected.push(blob);
        }
    }
    let wire = encoder.finish();
    let mut reader: Box<dyn Read + '_> = match codec {
        Codec::Raw => Box::new(wire.as_slice()),
        Codec::Lz4 => Box::new(lz4::Decoder::new(wire.as_slice()).unwrap()),
        _ => Box::new(zstd::stream::read::Decoder::new(wire.as_slice()).unwrap()),
    };
    let mut decode_us = Vec::new();
    let mut decompress_us = Vec::new();
    let mut state = BTreeMap::new();
    let mut packets = expected.iter();
    for frame in frames {
        let expected = packets.next().unwrap();
        let mut decoded = vec![0; expected.len()];
        let start = Instant::now();
        reader.read_exact(&mut decoded).unwrap();
        decompress_us.push(start.elapsed().as_secs_f64() * 1e6);
        assert_eq!(&decoded, expected);
        let start = Instant::now();
        let restored: Frame = match format {
            Format::Delta => delta_decode(&decoded, &mut state),
            Format::Compact => compact_decode(&decoded),
            Format::Snapshot => ciborium::from_reader(decoded.as_slice()).unwrap(),
        };
        decode_us.push(start.elapsed().as_secs_f64() * 1e6);
        assert_eq!(&restored, frame);
        black_box(restored);
        if assets && frame.header.tick % 10 == 9 {
            let expected = packets.next().unwrap();
            let mut decoded = vec![0; expected.len()];
            reader.read_exact(&mut decoded).unwrap();
            assert_eq!(&decoded, expected);
        }
    }
    let mut end = Vec::new();
    reader.read_to_end(&mut end).unwrap();
    assert!(end.is_empty());
    let warm = 20.min(frames.len() / 4);
    let count = (frames.len() - warm) as f64;
    let mean = |v: &[f64]| v[warm..].iter().sum::<f64>() / count;
    let total_encode: Vec<_> = encode_us
        .iter()
        .zip(&compress_us)
        .map(|(a, b)| a + b)
        .collect();
    let total_decode: Vec<_> = decode_us
        .iter()
        .zip(&decompress_us)
        .map(|(a, b)| a + b)
        .collect();
    println!(
        "{name},{},{},{repeat},{},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{:.1},{asset_total}",
        format.label(),
        codec.label(),
        wire_sizes[0],
        sizes[warm..].iter().sum::<usize>() as f64 / count,
        wire_sizes[warm..].iter().sum::<usize>() as f64 / count,
        mean(&encode_us),
        mean(&compress_us),
        mean(&decompress_us),
        mean(&decode_us),
        mean(&total_encode),
        percentile(&total_encode[warm..], 0.95),
        mean(&total_decode)
    );
}

fn main() {
    let frames = std::env::var("BENCH_FRAMES")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(160);
    let repeats = std::env::var("BENCH_REPEATS")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(3);
    let methods = [
        (Format::Snapshot, Codec::Raw),
        (Format::Snapshot, Codec::Lz4),
        (Format::Snapshot, Codec::Zstd(1, 20)),
        (Format::Snapshot, Codec::Zstd(1, 23)),
        (Format::Snapshot, Codec::Zstd(3, 20)),
        (Format::Snapshot, Codec::Zstd(3, 23)),
        (Format::Compact, Codec::Zstd(1, 23)),
        (Format::Compact, Codec::Zstd(3, 23)),
        (Format::Snapshot, Codec::ZstdIndependent),
        (Format::Delta, Codec::Raw),
        (Format::Delta, Codec::Lz4),
        (Format::Delta, Codec::Zstd(1, 23)),
    ];
    println!(
        "scenario,format,codec,repeat,first_wire_bytes,raw_bytes,wire_bytes,serialize_us,compress_us,decompress_us,parse_us,server_us,server_p95_us,client_us,asset_wire_bytes"
    );
    for (name, count, combat, assets) in [
        ("coast128", 128, false, false),
        ("coast1024", 1024, false, false),
        ("battle1024", 1024, true, false),
        ("battle8192", 8192, true, false),
        ("battle1024-assets", 1024, true, true),
    ] {
        let corpus = corpus(count, combat, frames);
        for repeat in 0..repeats {
            for offset in 0..methods.len() {
                let (format, codec) = methods[(offset + repeat * 4) % methods.len()];
                eprintln!(
                    "{name} {} {} repeat {repeat}",
                    format.label(),
                    codec.label()
                );
                run(name, &corpus, assets, format, codec, repeat);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flushed_ticks_decode_before_stream_is_finished() {
        for codec in [Codec::Lz4, Codec::Zstd(1, 20), Codec::Zstd(3, 23)] {
            let mut encoder = Encoder::new(codec);
            let mut expected = Vec::new();
            for tick in 0..4 {
                let packet = cbor(&(
                    tick,
                    "small message must arrive without waiting for the next tick",
                ));
                expected.extend_from_slice(&packet);
                encoder.packet(&packet);
                let prefix = match &encoder {
                    Encoder::Lz4(e) => e.writer().as_slice(),
                    Encoder::Zstd(e) => e.get_ref().as_slice(),
                    _ => unreachable!(),
                };
                let mut reader: Box<dyn Read + '_> = match codec {
                    Codec::Lz4 => Box::new(lz4::Decoder::new(prefix).unwrap()),
                    _ => Box::new(zstd::stream::read::Decoder::new(prefix).unwrap()),
                };
                let mut actual = vec![0; expected.len()];
                reader.read_exact(&mut actual).unwrap();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn delta_handles_removal_metadata_changes_and_every_numeric_bit() {
        let mut random = Random(1);
        let mut first = corpus(4, false, 1).remove(0);
        first.objects[0].position[0] = -0.0;
        let mut second = first.clone();
        second.header.tick = 1;
        second.header.time_ns = 100_000_000;
        second.objects.remove(1);
        second.objects[0].name = "renamed".into();
        second.objects[1].design = 99;
        second.objects[2].reserve = f64::from_bits(second.objects[2].reserve.to_bits() ^ 1);
        second.objects[2].flags = u64::MAX;
        second.objects.push(object(10, &mut random));
        let mut state = BTreeMap::new();
        let restored = delta_decode(&delta_encode(&first, &[]), &mut state);
        assert_eq!(cbor(&restored), cbor(&first));
        let restored = delta_decode(&delta_encode(&second, &first.objects), &mut state);
        assert_eq!(cbor(&restored), cbor(&second));
    }
}
