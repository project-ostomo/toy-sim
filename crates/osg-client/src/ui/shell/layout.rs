use super::*;
use std::path::PathBuf;

const WINDOWS: &[WindowSpec] = &[
    SELECTED, OVERVIEW, INVENTORY, HANGAR, CARGO, MAP, SOCIETY, INDUSTRY, CHAT, SETTINGS, WALLET,
    MARKET, ASSETS,
];

#[derive(Resource, Default)]
pub(super) struct Store {
    path: Option<PathBuf>,
    saved: SavedLayout,
    last_attempt: f64,
}

pub(super) fn load(mut shell: ResMut<Shell>, mut store: ResMut<Store>) {
    if cfg!(test) || cfg!(target_arch = "wasm32") {
        return;
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    let Some(path) = root.map(|root| root.join("toy-sim/workspace.toml")) else {
        return;
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<SavedLayout>(&text) {
            Ok(saved) => {
                shell.desktop.restore_layout(&saved, WINDOWS);
                store.saved = saved;
            }
            Err(error) => tracing::warn!("Cannot read workspace layout: {error}"),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!("Cannot read workspace layout: {error}"),
    }
    store.path = Some(path);
}

pub(super) fn save(
    shell: Res<Shell>,
    mut store: ResMut<Store>,
    time: Res<Time<Real>>,
    mut exit: MessageReader<AppExit>,
) {
    let exiting = exit.read().next().is_some();
    let Some(path) = store.path.clone() else {
        return;
    };
    if !exiting && time.elapsed_secs_f64() - store.last_attempt < 1. {
        return;
    }
    store.last_attempt = time.elapsed_secs_f64();
    let saved = shell.desktop.save_layout();
    if saved == store.saved {
        return;
    }
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let temporary = path.with_extension("toml.tmp");
        std::fs::write(&temporary, toml::to_string(&saved)?)?;
        std::fs::rename(temporary, &path)?;
        Ok(())
    })();
    match result {
        Ok(()) => store.saved = saved,
        Err(error) => tracing::warn!("Cannot save workspace layout: {error}"),
    }
}
