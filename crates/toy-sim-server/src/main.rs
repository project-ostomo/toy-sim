use anyhow::{Context, Result, bail};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut logging = bevy::prelude::App::new();
    logging.add_plugins(bevy::log::LogPlugin::default());
    let mut args = std::env::args_os().skip(1);
    let path = args.next().context(
        "usage: toy-sim-server <config.toml> [--ready-file PATH] [--shutdown-on-stdin-close]",
    )?;
    if path == "--init" {
        let directory = args.next().context("--init requires a directory")?;
        anyhow::ensure!(args.next().is_none(), "unexpected argument");
        return toy_sim_server::provision::demo(std::path::Path::new(&directory));
    }
    let mut options = toy_sim_server::launch::Options::default();
    while let Some(arg) = args.next() {
        if arg == "--ready-file" {
            options.ready_file = Some(PathBuf::from(
                args.next().context("--ready-file requires a path")?,
            ));
        } else if arg == "--shutdown-on-stdin-close" {
            options.shutdown_on_stdin_close = true;
        } else {
            bail!("unknown argument: {}", arg.to_string_lossy());
        }
    }
    toy_sim_server::launch::run(std::path::Path::new(&path), options).await
}
