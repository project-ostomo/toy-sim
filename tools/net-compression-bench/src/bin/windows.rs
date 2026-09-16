use std::{
    hint::black_box,
    io::{Read, Write},
    time::Instant,
};
use zstd::zstd_safe::{CCtx, CParameter, InBuffer, OutBuffer, zstd_sys::ZSTD_EndDirective};

#[allow(dead_code)]
mod baseline {
    include!("../main.rs");

    pub fn prepare(count: usize, combat: bool, ticks: usize) -> (Vec<Vec<u8>>, Vec<f64>) {
        let source = corpus(count, combat, ticks);
        let mut packets = Vec::new();
        let mut timings = Vec::new();
        for frame in source {
            let start = Instant::now();
            let bytes = postcard::to_allocvec(&frame).unwrap();
            timings.push(start.elapsed().as_secs_f64() * 1e6);
            let restored: Frame = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(frame, restored);
            packets.push(bytes);
        }
        (packets, timings)
    }
}

#[derive(Clone, Copy, Debug)]
struct Method {
    level: i32,
    window: u32,
    ldm: bool,
}

impl Method {
    fn label(self) -> String {
        if self.level == 99 {
            "lz4-linked".into()
        } else {
            format!(
                "zstd{}-w{}{}",
                self.level,
                self.window,
                if self.ldm { "-ldm" } else { "" }
            )
        }
    }
}

struct Sink {
    bytes: std::cell::RefCell<Vec<u8>>,
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes.get_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

enum Compressor {
    Zstd {
        context: CCtx<'static>,
        output: Vec<u8>,
    },
    Lz4 {
        encoder: lz4::Encoder<Sink>,
        output: Vec<u8>,
    },
}

impl Compressor {
    fn new(m: Method) -> Self {
        if m.level == 99 {
            Self::Lz4 {
                encoder: lz4::EncoderBuilder::new()
                    .block_mode(lz4::BlockMode::Linked)
                    .block_size(lz4::BlockSize::Max64KB)
                    .checksum(lz4::ContentChecksum::NoChecksum)
                    .block_checksum(lz4::liblz4::BlockChecksum::NoBlockChecksum)
                    .build(Sink {
                        bytes: std::cell::RefCell::new(Vec::with_capacity(128 << 10)),
                    })
                    .unwrap(),
                output: Vec::with_capacity(128 << 10),
            }
        } else {
            let mut context = CCtx::create();
            context
                .set_parameter(CParameter::CompressionLevel(m.level))
                .unwrap();
            context
                .set_parameter(CParameter::WindowLog(m.window))
                .unwrap();
            context
                .set_parameter(CParameter::EnableLongDistanceMatching(m.ldm))
                .unwrap();
            context.set_parameter(CParameter::NbWorkers(0)).unwrap();
            Self::Zstd {
                context,
                output: Vec::with_capacity(128 << 10),
            }
        }
    }

    fn packet(&mut self, bytes: &[u8], finish: bool) -> &[u8] {
        match self {
            Self::Zstd { context, output } => {
                output.clear();
                let mut input = InBuffer { src: bytes, pos: 0 };
                let mut chunk = [0u8; 131072];
                loop {
                    let mut out = OutBuffer::around(&mut chunk[..]);
                    let remaining = context
                        .compress_stream2(
                            &mut out,
                            &mut input,
                            if finish {
                                ZSTD_EndDirective::ZSTD_e_end
                            } else {
                                ZSTD_EndDirective::ZSTD_e_flush
                            },
                        )
                        .unwrap();
                    let written = out.pos();
                    output.extend_from_slice(&chunk[..written]);
                    if remaining == 0 && input.pos == input.src.len() {
                        break;
                    }
                }
                output
            }
            Self::Lz4 { encoder, output } => {
                assert!(!finish);
                encoder.write_all(bytes).unwrap();
                encoder.flush().unwrap();
                output.clear();
                std::mem::swap(output, &mut *encoder.writer().bytes.borrow_mut());
                output
            }
        }
    }

    fn memory(&self) -> usize {
        match self {
            Self::Zstd { context, output } => context.sizeof() + output.capacity(),
            Self::Lz4 { .. } => 0,
        }
    }
}

fn thread_cpu() -> f64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) },
        0
    );
    time.tv_sec as f64 + time.tv_nsec as f64 * 1e-9
}

fn allocated() -> usize {
    let m = unsafe { libc::mallinfo2() };
    m.uordblks + m.hblkhd
}

fn percentile(v: &[f64], q: f64) -> f64 {
    let mut v = v.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * q).round() as usize]
}

fn trial(
    name: &str,
    packets: &[Vec<u8>],
    serial: &[f64],
    assets: bool,
    method: Method,
    repeat: usize,
) {
    let warm = packets.len() - 200;
    let mut compressor = Compressor::new(method);
    let mut bytes = 0usize;
    let mut asset_bytes = 0usize;
    let mut asset_cpu = 0.0;
    let mut cpu = 0.0;
    let mut walls = Vec::new();
    let mut seed = 829345u64;
    let mut asset = vec![0u8; 256 << 10];
    // Validation is outside timed calls. Keep the compressed stream to decode it fully.
    let mut wire = Vec::new();
    let mut expected = Vec::new();
    for (tick, packet) in packets.iter().enumerate() {
        let start = Instant::now();
        let cstart = thread_cpu();
        let output = compressor.packet(packet, false);
        let elapsed = thread_cpu() - cstart;
        let wall = start.elapsed().as_secs_f64();
        if tick >= warm {
            bytes += output.len();
            cpu += elapsed;
            walls.push(wall * 1e6);
        }
        wire.extend_from_slice(output);
        expected.extend_from_slice(packet);
        if assets && tick % 10 == 9 {
            for byte in &mut asset {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                *byte = seed as u8;
            }
            let cstart = thread_cpu();
            let output = compressor.packet(&asset, false);
            let elapsed = thread_cpu() - cstart;
            if tick >= warm {
                asset_bytes += output.len();
                asset_cpu += elapsed;
            }
            wire.extend_from_slice(output);
            expected.extend_from_slice(&asset);
        }
    }
    let memory = compressor.memory();
    // Both decoders must deliver all flushed bytes even before the stream ends.
    let mut reader: Box<dyn Read> = if method.level == 99 {
        Box::new(lz4::Decoder::new(wire.as_slice()).unwrap())
    } else {
        Box::new(zstd::stream::read::Decoder::new(wire.as_slice()).unwrap())
    };
    let mut restored = vec![0u8; expected.len()];
    reader.read_exact(&mut restored).unwrap();
    assert_eq!(restored, expected);
    let raw = packets[warm..].iter().map(Vec::len).sum::<usize>() as f64 / 200.;
    let serialize = serial[warm..].iter().sum::<f64>() / 200.;
    println!(
        "{name},{},{repeat},{raw:.1},{:.1},{:.1},{:.3},{:.3},{:.3},{:.3},{memory}",
        method.label(),
        bytes as f64 / 200.,
        asset_bytes as f64 / 200.,
        serialize,
        cpu * 1e6 / 200.,
        asset_cpu * 1e6 / 200.,
        percentile(&walls, 0.95)
    );
}

fn methods() -> Vec<Method> {
    let mut result = vec![Method {
        level: 99,
        window: 16,
        ldm: false,
    }];
    for level in [-1, 1, 3] {
        for window in [16, 18, 20, 21, 22, 23, 24] {
            result.push(Method {
                level,
                window,
                ldm: false,
            });
        }
    }
    for window in [23, 24] {
        result.push(Method {
            level: 3,
            window,
            ldm: true,
        });
    }
    result
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "memory") {
        memory_study(args.get(2).map(|s| s.parse().unwrap()).unwrap_or(1024));
        return;
    }
    if args.get(1).is_some_and(|s| s == "parallel") {
        parallel_study();
        return;
    }
    let repeats: usize = std::env::var("WINDOW_REPEATS")
        .ok()
        .map(|s| s.parse().unwrap())
        .unwrap_or(3);
    println!(
        "scenario,codec,repeat,raw_bytes,wire_bytes,asset_wire_bytes_per_tick,serialize_us,compress_cpu_us,asset_cpu_us_per_tick,compress_wall_p95_us,encoder_bytes"
    );
    for (name, count, combat, assets) in [
        ("coast128", 128, false, false),
        ("coast1024", 1024, false, false),
        ("battle1024", 1024, true, false),
        ("battle8192", 8192, true, false),
        ("battle1024-assets", 1024, true, true),
    ] {
        if args.get(1).is_some_and(|filter| filter != name) {
            continue;
        }
        // At least two maximum windows of unique warmup data, plus 200 measured ticks.
        let ticks = 200 + ((32 << 20) / (count * 130)).max(100);
        let (packets, serial) = baseline::prepare(count, combat, ticks);
        let filter = std::env::var("WINDOW_CODEC_FILTER").ok();
        let methods: Vec<_> = methods()
            .into_iter()
            .filter(|m| {
                filter
                    .as_ref()
                    .is_none_or(|f| f.split(',').any(|part| m.label().contains(part)))
            })
            .collect();
        for repeat in 0..repeats {
            for i in 0..methods.len() {
                let method = methods[(i + repeat * 7) % methods.len()];
                eprintln!("{name} {} repeat {repeat}", method.label());
                trial(name, &packets, &serial, assets, method, repeat);
            }
        }
    }
}

fn memory_study(count: usize) {
    // Retain many independent, warmed contexts; allocator delta excludes corpus/output history.
    let (packets, _) = baseline::prepare(count, true, if count > 1024 { 100 } else { 300 });
    println!("codec,contexts,allocator_bytes_per_context,reported_bytes_per_context");
    for method in methods() {
        if std::env::var("WINDOW_CODEC_FILTER").is_ok_and(|f| !method.label().contains(&f)) {
            continue;
        }
        if count > 1024
            && !(method.level == 99
                || (!method.ldm
                    && [1, 3].contains(&method.level)
                    && [18, 20, 21, 22, 24].contains(&method.window)))
        {
            continue;
        }
        let before = allocated();
        let mut encoders = Vec::new();
        for _ in 0..16 {
            let mut encoder = Compressor::new(method);
            for packet in &packets {
                black_box(encoder.packet(packet, false));
            }
            encoders.push(encoder);
        }
        let delta = allocated().saturating_sub(before) / encoders.len();
        let reported = encoders.iter().map(Compressor::memory).sum::<usize>() / encoders.len();
        println!("{},{},{delta},{reported}", method.label(), encoders.len());
        black_box(encoders);
    }
}

fn parallel_study() {
    let (packets, _) = baseline::prepare(1024, true, 200);
    // Separate allocations model private per-connection snapshot buffers.
    let datasets: Vec<_> = (0..16).map(|_| packets.clone()).collect();
    println!("codec,workers,repeat,frames,wall_s,frames_per_s,cpu_s");
    for method in [
        Method {
            level: 99,
            window: 16,
            ldm: false,
        },
        Method {
            level: 1,
            window: 20,
            ldm: false,
        },
        Method {
            level: 3,
            window: 20,
            ldm: false,
        },
        Method {
            level: 3,
            window: 23,
            ldm: false,
        },
    ] {
        for workers in [1, 4, 8, 16] {
            for repeat in 0..3 {
                let barrier = std::sync::Barrier::new(workers + 1);
                std::thread::scope(|scope| {
                    let mut handles = Vec::new();
                    for worker in 0..workers {
                        let packets = &datasets[worker];
                        let barrier = &barrier;
                        handles.push(scope.spawn(move || {
                            unsafe {
                                let mut set: libc::cpu_set_t = std::mem::zeroed();
                                libc::CPU_SET(worker, &mut set);
                                assert_eq!(
                                    libc::sched_setaffinity(0, std::mem::size_of_val(&set), &set),
                                    0
                                );
                            }
                            let mut encoder = Compressor::new(method);
                            for p in packets {
                                black_box(encoder.packet(p, false));
                            }
                            barrier.wait();
                            let start = thread_cpu();
                            for _ in 0..12 {
                                for p in packets {
                                    black_box(encoder.packet(p, false));
                                }
                            }
                            thread_cpu() - start
                        }));
                    }
                    barrier.wait();
                    let timer = Instant::now();
                    let cpu = handles.into_iter().map(|h| h.join().unwrap()).sum::<f64>();
                    let elapsed = timer.elapsed().as_secs_f64();
                    let frames = workers * packets.len() * 12;
                    println!(
                        "{},{workers},{repeat},{frames},{elapsed:.6},{:.1},{cpu:.6}",
                        method.label(),
                        frames as f64 / elapsed
                    );
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_flush_is_decodable_without_ending_the_stream() {
        for method in [
            Method {
                level: 99,
                window: 16,
                ldm: false,
            },
            Method {
                level: 1,
                window: 20,
                ldm: false,
            },
            Method {
                level: 3,
                window: 24,
                ldm: true,
            },
        ] {
            let mut encoder = Compressor::new(method);
            let mut wire = Vec::new();
            let mut expected = Vec::new();
            for tick in 0..4 {
                let bytes = postcard::to_allocvec(&(tick, "snapshot", [1.0, 2.0, 3.0])).unwrap();
                wire.extend_from_slice(encoder.packet(&bytes, false));
                expected.extend_from_slice(&bytes);
                let mut reader: Box<dyn Read + '_> = if method.level == 99 {
                    Box::new(lz4::Decoder::new(wire.as_slice()).unwrap())
                } else {
                    Box::new(zstd::stream::read::Decoder::new(wire.as_slice()).unwrap())
                };
                let mut actual = vec![0; expected.len()];
                reader.read_exact(&mut actual).unwrap();
                assert_eq!(actual, expected);
            }
        }
    }
}
