use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    address: String,
    server_public_key: String,
    account: String,
    account_secret: String,
}

fn key(value: &str) -> Result<[u8; 32]> {
    ensure!(
        value.is_ascii() && value.len() == 64,
        "key must be 64 hexadecimal characters"
    );
    let mut bytes = [0; 32];
    for (byte, chunk) in bytes.iter_mut().zip(value.as_bytes().chunks_exact(2)) {
        *byte = u8::from_str_radix(std::str::from_utf8(chunk)?, 16)?;
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: toy-sim-client <config.toml>")?;
    let config: Config = toml::from_str(&std::fs::read_to_string(path)?)?;
    let endpoint = toy_sim_client::connect(
        &config.address,
        ed25519_dalek::VerifyingKey::from_bytes(&key(&config.server_public_key)?)?,
        config.account.parse()?,
        &ed25519_dalek::SigningKey::from_bytes(&key(&config.account_secret)?),
    )
    .await?;
    toy_sim_client::ui::run(endpoint, false);
    Ok(())
}
