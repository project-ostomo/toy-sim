use anyhow::{Context, Result, ensure};
use ed25519_dalek::SigningKey;
use std::{
    io::Write,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use toy_sim_model::*;

struct Options {
    ships: usize,
    sessions: usize,
    frames: usize,
    warmup_ticks: u64,
    server: PathBuf,
}

impl Options {
    fn parse() -> Result<Self> {
        let mut result = Self {
            ships: 16,
            sessions: 4,
            frames: 20,
            warmup_ticks: 70,
            server: std::env::current_exe()?
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join(if cfg!(windows) {
                    "toy-sim-server.exe"
                } else {
                    "toy-sim-server"
                }),
        };
        let mut args = std::env::args().skip(1);
        while let Some(name) = args.next() {
            let value = args.next().context("benchmark options require a value")?;
            match name.as_str() {
                "--ships" => result.ships = value.parse()?,
                "--sessions" => result.sessions = value.parse()?,
                "--frames" => result.frames = value.parse()?,
                "--warmup-ticks" => result.warmup_ticks = value.parse()?,
                "--server" => result.server = value.into(),
                _ => anyhow::bail!("unknown option {name}"),
            }
        }
        ensure!(
            result.ships > 0 && result.ships <= 1024,
            "ships must be 1..=1024"
        );
        ensure!(
            result.sessions > 0 && result.sessions <= result.ships,
            "sessions must be 1..=ships"
        );
        ensure!(result.frames >= 2, "at least two frames are required");
        ensure!(
            result.server.is_file(),
            "build toy-sim-server or supply --server PATH"
        );
        Ok(result)
    }
}

struct Server {
    child: Option<Child>,
    directory: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            drop(child.stdin.take());
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    _ if Instant::now() >= deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[derive(Default)]
struct Measurements {
    frames: usize,
    tracks: usize,
    bytes: usize,
    compressed: usize,
    encode_s: f64,
    compress_s: f64,
    elapsed_s: f64,
    tick_ms: f64,
    tick_samples: usize,
    context_bytes: usize,
    skipped_ticks: u64,
}

async fn receive(endpoint: &mut toy_sim_client::Endpoint) -> Result<Arc<Frame>> {
    tokio::time::timeout(Duration::from_secs(30), endpoint.state.recv())
        .await?
        .context("server disconnected")
}

async fn acknowledge(
    endpoint: &toy_sim_client::Endpoint,
    frame: &Frame,
    sequence: &mut u64,
    actions: Vec<Action>,
) -> Result<()> {
    *sequence += 1;
    endpoint
        .input
        .send(InputFrame {
            world: frame.world,
            sequence: *sequence,
            actions: actions
                .into_iter()
                .map(|action| (Id::new(), action))
                .collect(),
        })
        .await?;
    Ok(())
}

async fn measure(
    address: String,
    server_key: ed25519_dalek::VerifyingKey,
    account: Id,
    key: SigningKey,
    debug: bool,
    options: Arc<Options>,
    barrier: Arc<tokio::sync::Barrier>,
) -> Result<Measurements> {
    let mut endpoint = toy_sim_client::connect(&address, server_key, account, &key).await?;
    let first = receive(&mut endpoint).await?;
    let group = first
        .tracks
        .keys()
        .copied()
        .find(|group| *group != PUBLIC_GROUP)
        .context("missing account group")?;
    let mut sequence = 0;
    let mut actions = vec![Action::Subscribe(ViewSubscription {
        id: 1,
        revision: 1,
        group,
        focused_ship: first.ships.first().map(|ship| ship.ship),
        query: TrackQuery {
            limit: 256,
            work: 1_000_000,
            ..Default::default()
        },
    })];
    if debug {
        actions.push(Action::Debug(DebugCommand::Inspect(true)));
    }
    acknowledge(&endpoint, &first, &mut sequence, actions).await?;
    let warm_until = first.tick + options.warmup_ticks;
    loop {
        let frame = receive(&mut endpoint).await?;
        acknowledge(&endpoint, &frame, &mut sequence, Vec::new()).await?;
        if frame.tick >= warm_until {
            break;
        }
    }
    barrier.wait().await;
    let mut context = zstd::zstd_safe::CCtx::create();
    context
        .set_parameter(zstd::zstd_safe::CParameter::CompressionLevel(3))
        .map_err(|code| anyhow::anyhow!("zstd {code}"))?;
    context
        .set_parameter(zstd::zstd_safe::CParameter::WindowLog(21))
        .map_err(|code| anyhow::anyhow!("zstd {code}"))?;
    let mut encoder = zstd::stream::write::Encoder::with_context(Vec::new(), &mut context);
    let mut result = Measurements::default();
    let start = Instant::now();
    let mut last_tick = None;
    for _ in 0..options.frames {
        let frame = receive(&mut endpoint).await?;
        if let Some(previous) = last_tick {
            result.skipped_ticks += frame.tick.saturating_sub(previous).saturating_sub(1);
        }
        last_tick = Some(frame.tick);
        acknowledge(&endpoint, &frame, &mut sequence, Vec::new()).await?;
        result.frames += 1;
        result.tracks += frame.tracks.values().map(Vec::len).sum::<usize>();
        if let Some(diagnostics) = &frame.presentation.diagnostics {
            result.tick_ms += diagnostics.tick_duration_ms;
            result.tick_samples += 1;
        }
        let encoding = Instant::now();
        let bytes = toy_sim_protocol::encode(&toy_sim_protocol::Message::State((*frame).clone()))?;
        result.encode_s += encoding.elapsed().as_secs_f64();
        result.bytes += bytes.len();
        let compression = Instant::now();
        encoder.write_all(&bytes)?;
        encoder.flush()?;
        result.compress_s += compression.elapsed().as_secs_f64();
        result.compressed += encoder.get_ref().len();
        encoder.get_mut().clear();
    }
    result.elapsed_s = start.elapsed().as_secs_f64();
    drop(encoder);
    result.context_bytes = context.sizeof();
    Ok(result)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn cpu_seconds(pid: u32, ticks_per_second: f64) -> Result<f64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<_> = stat
        .rsplit_once(')')
        .context("invalid process stat")?
        .1
        .split_whitespace()
        .collect();
    Ok((fields[11].parse::<f64>()? + fields[12].parse::<f64>()?) / ticks_per_second)
}

fn memory_kib(pid: u32, field: &str) -> Result<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    status
        .lines()
        .find_map(|line| line.strip_prefix(field))
        .context("missing process memory field")?
        .split_whitespace()
        .next()
        .context("missing process memory amount")?
        .parse()
        .map_err(Into::into)
}

#[tokio::main]
async fn main() -> Result<()> {
    let options = Arc::new(Options::parse()?);
    let directory = std::env::temp_dir().join(format!("toy-sim-benchmark-{}", Id::new()));
    let mut directory_options = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directory_options.mode(0o700);
    }
    directory_options.create(&directory)?;
    let mut server = Server {
        child: None,
        directory,
    };
    let server_key = SigningKey::from_bytes(&rand::random());
    let accounts: Vec<_> = (0..options.ships)
        .map(|_| (Id::new(), SigningKey::from_bytes(&rand::random())))
        .collect();
    let mut config = format!(
        "listen = \"127.0.0.1:0\"\nserver_secret = \"{}\"\ndebug_account = \"{}\"\n",
        hex(&server_key.to_bytes()),
        accounts[0].0
    );
    for (id, key) in &accounts {
        config.push_str(&format!(
            "\n[[accounts]]\nid = \"{id}\"\npublic_key = \"{}\"\n",
            hex(&key.verifying_key().to_bytes())
        ));
    }
    let config_path = server.directory.join("server.toml");
    std::fs::write(&config_path, config)?;
    let ready = server.directory.join("ready");
    server.child = Some(
        Command::new(&options.server)
            .arg(config_path)
            .arg("--ready-file")
            .arg(&ready)
            .arg("--shutdown-on-stdin-close")
            .stdin(Stdio::piped())
            .spawn()?,
    );
    let pid = server.child.as_ref().unwrap().id();
    let deadline = Instant::now() + Duration::from_secs(120);
    let address = loop {
        ensure!(
            server.child.as_mut().unwrap().try_wait()?.is_none(),
            "server exited before readiness"
        );
        if let Ok(address) = std::fs::read_to_string(&ready) {
            if address.parse::<std::net::SocketAddr>().is_ok() {
                break address;
            }
        }
        ensure!(Instant::now() < deadline, "server initialization timed out");
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    let ticks_per_second: f64 =
        String::from_utf8(Command::new("getconf").arg("CLK_TCK").output()?.stdout)?
            .trim()
            .parse()?;
    let barrier = Arc::new(tokio::sync::Barrier::new(options.sessions + 1));
    let mut clients = tokio::task::JoinSet::new();
    for (index, (account, key)) in accounts.into_iter().take(options.sessions).enumerate() {
        clients.spawn(measure(
            address.clone(),
            server_key.verifying_key(),
            account,
            key,
            index == 0,
            options.clone(),
            barrier.clone(),
        ));
    }
    tokio::select! {
        _ = barrier.wait() => {},
        result = clients.join_next() => {
            result.context("no benchmark clients")???;
            anyhow::bail!("client stopped before warmup completed");
        }
    }
    let cpu_start = cpu_seconds(pid, ticks_per_second)?;
    let wall_start = Instant::now();
    let mut results = Vec::new();
    while let Some(result) = clients.join_next().await {
        results.push(result??);
    }
    let wall_s = wall_start.elapsed().as_secs_f64();
    let cpu_s = cpu_seconds(pid, ticks_per_second)? - cpu_start;
    let frames = results.iter().map(|result| result.frames).sum::<usize>() as f64;
    let tick_samples = results
        .iter()
        .map(|result| result.tick_samples)
        .sum::<usize>();
    println!(
        "ships,sessions,frames_per_session,tracks_per_frame,frame_bytes,recompressed_zstd_bytes,reencode_ms,recompress_ms,encoder_bytes_per_session,server_tick_ms,server_cpu_percent,server_rss_kib,server_peak_rss_kib,received_hz,skipped_ticks"
    );
    println!(
        "{},{},{},{:.1},{:.1},{:.1},{:.3},{:.3},{},{:.3},{:.1},{},{},{:.2},{}",
        options.ships,
        options.sessions,
        options.frames,
        results.iter().map(|result| result.tracks).sum::<usize>() as f64 / frames,
        results.iter().map(|result| result.bytes).sum::<usize>() as f64 / frames,
        results
            .iter()
            .map(|result| result.compressed)
            .sum::<usize>() as f64
            / frames,
        results.iter().map(|result| result.encode_s).sum::<f64>() * 1000.0 / frames,
        results.iter().map(|result| result.compress_s).sum::<f64>() * 1000.0 / frames,
        results
            .iter()
            .map(|result| result.context_bytes)
            .sum::<usize>()
            / options.sessions,
        results.iter().map(|result| result.tick_ms).sum::<f64>() / tick_samples.max(1) as f64,
        cpu_s / wall_s * 100.0,
        memory_kib(pid, "VmRSS:")?,
        memory_kib(pid, "VmHWM:")?,
        frames / results.iter().map(|result| result.elapsed_s).sum::<f64>(),
        results
            .iter()
            .map(|result| result.skipped_ticks)
            .sum::<u64>(),
    );
    Ok(())
}
