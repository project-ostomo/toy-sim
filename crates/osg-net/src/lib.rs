mod client;
pub mod crypto;
mod game;
pub mod pipe;
pub mod rpc;

pub use client::{EventSubscription, InputError, NetEvent, OsgNetClient};
pub use game::{GameRpc, GameRpcDispatcher};
pub use rpc::RpcError;

use anyhow::Result;
use ed25519_dalek::{SigningKey, VerifyingKey};
use osg_model::AccountId;
pub use picomux;
use std::{collections::BTreeMap, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

async fn connect_mux(
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

pub async fn read_message<R: AsyncRead + Unpin>(read: &mut R) -> Result<osg_protocol::Message> {
    let mut header = [0; osg_protocol::HEADER_SIZE];
    read.read_exact(&mut header).await?;
    let length = osg_protocol::payload_length(&header)?;
    let mut message = Vec::with_capacity(header.len() + length);
    message.extend_from_slice(&header);
    message.resize(header.len() + length, 0);
    read.read_exact(&mut message[header.len()..]).await?;
    osg_protocol::decode(&message)
}

pub async fn write_message<W: AsyncWrite + Unpin>(
    write: &mut W,
    message: &osg_protocol::Message,
) -> Result<()> {
    write.write_all(&osg_protocol::encode(message)?).await?;
    write.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use osg_model::Id;

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
    async fn handshake_rejects_other_game_versions() {
        let (mut client, mut server) = tokio::io::duplex(1024);
        let mut hello = [0; 34];
        hello[..2].copy_from_slice(&(osg_model::GAME_VERSION + 1).to_be_bytes());
        client.write_all(&hello).await.unwrap();

        let error = crypto::server(
            &mut server,
            &SigningKey::from_bytes(&[3; 32]),
            &BTreeMap::new(),
        )
        .await
        .err()
        .expect("mismatched game version must fail before authentication");
        assert!(error.to_string().contains("unsupported game version"));
    }

    #[tokio::test]
    async fn replayed_record_and_wrong_key_fail() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        crypto::write_record(&mut a, &[1; 32], 0, b"payload", false)
            .await
            .unwrap();
        assert!(crypto::read_record(&mut b, &[1; 32], 1).await.is_err());
        crypto::write_record(&mut a, &[1; 32], 1, b"payload", false)
            .await
            .unwrap();
        assert!(crypto::read_record(&mut b, &[2; 32], 1).await.is_err());
    }

    #[tokio::test]
    async fn records_interoperate_with_the_directional_key_directly() {
        use chacha20poly1305::{
            ChaCha20Poly1305, KeyInit,
            aead::{Aead, Payload},
        };

        let key = [1; 32];
        let cipher = ChaCha20Poly1305::new((&key).into());
        for sequence in [0_u64, 1, (1 << 20) - 1, 1 << 20, 1 << 32, u64::MAX - 1] {
            let mut wire = Vec::new();
            crypto::write_record(&mut wire, &key, sequence, b"payload", false)
                .await
                .unwrap();

            let mut nonce = [0; 12];
            nonce[4..].copy_from_slice(&sequence.to_be_bytes());
            assert_eq!(u32::from_be_bytes(wire[..4].try_into().unwrap()), 24);
            let plaintext = cipher
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &wire[4..],
                        aad: &[],
                    },
                )
                .unwrap();
            assert_eq!(plaintext, b"\0payload");

            let ciphertext = cipher
                .encrypt(
                    (&nonce).into(),
                    Payload {
                        msg: b"\0payload",
                        aad: &[],
                    },
                )
                .unwrap();
            let mut direct_record = wire[..4].to_vec();
            direct_record.extend_from_slice(&ciphertext);
            assert_eq!(
                crypto::read_record(&mut direct_record.as_slice(), &key, sequence)
                    .await
                    .unwrap(),
                Some(b"payload".to_vec())
            );
        }
    }

    #[tokio::test]
    async fn altered_record_lengths_and_ciphertexts_fail_authentication() {
        let mut wire = Vec::new();
        crypto::write_record(&mut wire, &[1; 32], 1, b"payload", false)
            .await
            .unwrap();
        let length = u32::from_be_bytes(wire[..4].try_into().unwrap());

        let mut shortened = wire.clone();
        shortened[..4].copy_from_slice(&(length - 1).to_be_bytes());
        shortened.pop();

        let mut extended = wire.clone();
        extended[..4].copy_from_slice(&(length + 1).to_be_bytes());
        extended.push(0);

        let mut corrupted = wire;
        corrupted[4] ^= 1;

        for altered in [shortened, extended, corrupted] {
            let error = crypto::read_record(&mut altered.as_slice(), &[1; 32], 1)
                .await
                .unwrap_err();
            assert!(error.to_string().contains("record authentication failed"));
        }
    }
}
