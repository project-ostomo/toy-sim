pub mod crypto;
pub mod pipe;

use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
pub use picomux;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use toy_sim_model::AccountId;

pub async fn connect(
    address: &str,
    server_key: VerifyingKey,
    account: AccountId,
    account_key: &SigningKey,
) -> Result<picomux::PicoMux> {
    let mut stream = TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;
    let keys = tokio::time::timeout(
        Duration::from_secs(10),
        crypto::client(&mut stream, server_key, account, account_key),
    )
    .await??;
    multiplex(stream, keys)
}

pub async fn accept(
    mut stream: TcpStream,
    server_key: &SigningKey,
    accounts: &BTreeMap<AccountId, VerifyingKey>,
) -> Result<(AccountId, picomux::PicoMux)> {
    stream.set_nodelay(true)?;
    let keys = tokio::time::timeout(
        Duration::from_secs(10),
        crypto::server(&mut stream, server_key, accounts),
    )
    .await??;
    Ok((keys.account, multiplex(stream, keys)?))
}

fn multiplex(stream: TcpStream, keys: crypto::Keys) -> Result<picomux::PicoMux> {
    let (read, write) = tokio::io::split(pipe::Pipe::new(stream, keys)?);
    let mux = picomux::PicoMux::new(read, write);
    mux.set_debloat(true);
    Ok(mux)
}

pub async fn read_message<R: AsyncRead + Unpin>(read: &mut R) -> Result<toy_sim_protocol::Message> {
    let mut header = [0; toy_sim_protocol::HEADER_SIZE];
    read.read_exact(&mut header).await?;
    let length = toy_sim_protocol::payload_length(&header)?;
    let mut message = Vec::with_capacity(header.len() + length);
    message.extend_from_slice(&header);
    message.resize(header.len() + length, 0);
    read.read_exact(&mut message[header.len()..]).await?;
    toy_sim_protocol::decode(&message)
}

pub async fn write_message<W: AsyncWrite + Unpin>(
    write: &mut W,
    message: &toy_sim_protocol::Message,
) -> Result<()> {
    write.write_all(&toy_sim_protocol::encode(message)?).await?;
    write.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toy_sim_model::Id;

    #[tokio::test]
    async fn authenticated_stream_flushes_and_preserves_history() {
        let server = SigningKey::from_bytes(&[3; 32]);
        let account = Id([9; 16]);
        let account_key = SigningKey::from_bytes(&[7; 32]);
        let accounts = BTreeMap::from([(account, account_key.verifying_key())]);
        let (mut a, mut b) = tokio::io::duplex(65536);
        let (client_keys, server_keys) = tokio::join!(
            crypto::client(&mut a, server.verifying_key(), account, &account_key),
            crypto::server(&mut b, &server, &accounts)
        );
        let mut a = pipe::Pipe::new(a, client_keys.unwrap()).unwrap();
        let mut b = pipe::Pipe::new(b, server_keys.unwrap()).unwrap();
        let writer = tokio::spawn(async move {
            for i in 0..32_u8 {
                a.write_all(&vec![i; 16000]).await.unwrap();
                a.flush().await.unwrap();
            }
            a.shutdown().await.unwrap();
        });
        for i in 0..32_u8 {
            let mut bytes = vec![0; 16000];
            tokio::time::timeout(Duration::from_secs(5), b.read_exact(&mut bytes))
                .await
                .unwrap()
                .unwrap();
            assert!(bytes.iter().all(|b| *b == i));
        }
        assert_eq!(b.read(&mut [0; 1]).await.unwrap(), 0);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn handshake_rejects_wrong_server_pin_and_account_signature() {
        for wrong_pin in [true, false] {
            let server = SigningKey::from_bytes(&[3; 32]);
            let account = Id([9; 16]);
            let account_key = SigningKey::from_bytes(&[7; 32]);
            let bad_key = SigningKey::from_bytes(&[8; 32]);
            let accounts = BTreeMap::from([(account, account_key.verifying_key())]);
            let (mut a, mut b) = tokio::io::duplex(65536);
            let pin = if wrong_pin {
                bad_key.verifying_key()
            } else {
                server.verifying_key()
            };
            let client = async move {
                let result = crypto::client(&mut a, pin, account, &bad_key).await;
                drop(a);
                result
            };
            let server = async move {
                let result = crypto::server(&mut b, &server, &accounts).await;
                drop(b);
                result
            };
            let (a, b) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(client, server)
            })
            .await
            .unwrap();
            assert!(a.is_err());
            assert!(b.is_err());
        }
    }

    #[tokio::test]
    async fn replayed_record_and_wrong_key_fail() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        crypto::write_record(&mut a, &[1; 32], 0, b"payload", false)
            .await
            .unwrap();
        assert!(crypto::read_record(&mut b, &[1; 32], 1).await.is_err());
        crypto::write_record(&mut a, &[1; 32], 1 << 20, b"next epoch", false)
            .await
            .unwrap();
        assert_eq!(
            crypto::read_record(&mut b, &[1; 32], 1 << 20)
                .await
                .unwrap()
                .unwrap(),
            b"next epoch"
        );
    }
}
