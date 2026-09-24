//! cargo run -p osg-client --example wallet_watch -- ADDRESS SERVER_KEY ACCOUNT SECRET_KEY
use anyhow::{Context, Result, ensure};
use osg_client::{NetEvent, OsgNetClient};
use osg_model::{Id, ownership::Principal};

fn key(text: &str) -> Result<[u8; 32]> {
    ensure!(
        text.len() == 64 && text.is_ascii(),
        "keys must be 64 hexadecimal characters"
    );
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)?;
    }
    Ok(bytes)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4,
        "expected ADDRESS SERVER_KEY ACCOUNT SECRET_KEY"
    );
    let account: Id = args[2].parse().context("account UUID")?;
    let server = ed25519_dalek::VerifyingKey::from_bytes(&key(&args[1])?)?;
    let secret = ed25519_dalek::SigningKey::from_bytes(&key(&args[3])?);
    let client = OsgNetClient::connect(&args[0], server, account, &secret).await?;
    let mut events = client.subscribe_events();
    while !matches!(events.recv().await?, NetEvent::Session { .. }) {}
    drop(events);
    loop {
        let (world, _) = client.session().context("session unavailable")?;
        let balance = client
            .wallet_balance(world, Principal::Player(account))
            .await??;
        let history = client
            .wallet_history(world, Principal::Player(account), None, 25)
            .await??;
        println!("{balance:#?}\n{history:#?}");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
