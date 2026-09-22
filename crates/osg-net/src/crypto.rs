use anyhow::{Context, Result, ensure};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload},
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use osg_model::{AccountId, GAME_VERSION};
use rand::{RngCore, rngs::OsRng};
use std::collections::BTreeMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use x25519_dalek::{EphemeralSecret, PublicKey};

pub const MAX_RECORD: usize = 65536;

pub struct Keys {
    pub read: [u8; 32],
    pub write: [u8; 32],
    pub account: AccountId,
}

fn hello() -> (EphemeralSecret, [u8; 66]) {
    let secret = EphemeralSecret::random_from_rng(OsRng);
    let mut bytes = [0; 66];
    bytes[..2].copy_from_slice(&GAME_VERSION.to_le_bytes());
    OsRng.fill_bytes(&mut bytes[2..34]);
    bytes[34..].copy_from_slice(PublicKey::from(&secret).as_bytes());
    (secret, bytes)
}

fn transcript(client: &[u8; 66], server: &[u8; 66]) -> [u8; 32] {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame handshake");
    hash.update(client);
    hash.update(server);
    *hash.finalize().as_bytes()
}

fn derive(
    secret: EphemeralSecret,
    peer: &[u8],
    transcript: &[u8; 32],
) -> Result<([u8; 32], [u8; 32])> {
    let public = PublicKey::from(<[u8; 32]>::try_from(peer)?);
    let shared = secret.diffie_hellman(&public);
    ensure!(shared.was_contributory(), "invalid X25519 shared secret");
    let mut material = [0; 64];
    material[..32].copy_from_slice(shared.as_bytes());
    material[32..].copy_from_slice(transcript);
    Ok((
        blake3::derive_key("OpenSpaceGame c2s", &material),
        blake3::derive_key("OpenSpaceGame s2c", &material),
    ))
}

fn account_challenge(account: AccountId, transcript: &[u8; 32]) -> [u8; 32] {
    let mut hash = blake3::Hasher::new_derive_key("OpenSpaceGame account authentication");
    hash.update(&account.0);
    hash.update(transcript);
    *hash.finalize().as_bytes()
}

pub async fn client<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    server_key: VerifyingKey,
    account: AccountId,
    account_key: &SigningKey,
) -> Result<Keys> {
    let (secret, client) = hello();
    stream.write_all(&client).await?;
    stream.flush().await?;
    let mut server = [0; 66];
    let mut signature = [0; 64];
    stream.read_exact(&mut server).await?;
    stream.read_exact(&mut signature).await?;
    ensure!(
        server[..2] == GAME_VERSION.to_le_bytes(),
        "unsupported game version"
    );
    let transcript = transcript(&client, &server);
    server_key
        .verify_strict(&transcript, &Signature::from_bytes(&signature))
        .context("server identity mismatch")?;
    let (write, read) = derive(secret, &server[34..], &transcript)?;
    let mut auth = account.0.to_vec();
    auth.extend_from_slice(
        &account_key
            .sign(&account_challenge(account, &transcript))
            .to_bytes(),
    );
    write_record(stream, &write, 0, &auth, false).await?;
    let response = read_record(stream, &read, 0)
        .await?
        .context("server closed during authentication")?;
    ensure!(
        response == b"authenticated",
        "account authentication failed"
    );
    Ok(Keys {
        read,
        write,
        account,
    })
}

pub async fn server<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    server_key: &SigningKey,
    accounts: &BTreeMap<AccountId, VerifyingKey>,
) -> Result<Keys> {
    let mut client = [0; 66];
    stream.read_exact(&mut client).await?;
    ensure!(
        client[..2] == GAME_VERSION.to_le_bytes(),
        "unsupported game version"
    );
    let (secret, server) = hello();
    let transcript = transcript(&client, &server);
    let (read, write) = derive(secret, &client[34..], &transcript)?;
    stream.write_all(&server).await?;
    stream
        .write_all(&server_key.sign(&transcript).to_bytes())
        .await?;
    stream.flush().await?;
    let auth = read_record(stream, &read, 0)
        .await?
        .context("client closed during authentication")?;
    ensure!(auth.len() == 80, "invalid account proof");
    let account = osg_model::Id(auth[..16].try_into()?);
    let key = accounts.get(&account).context("unknown account")?;
    key.verify_strict(
        &account_challenge(account, &transcript),
        &Signature::from_slice(&auth[16..])?,
    )
    .context("invalid account signature")?;
    write_record(stream, &write, 0, b"authenticated", false).await?;
    Ok(Keys {
        read,
        write,
        account,
    })
}

fn cipher(root: &[u8; 32], sequence: u64) -> ChaCha20Poly1305 {
    let epoch = sequence / (1 << 20);
    let key = blake3::keyed_hash(root, &epoch.to_le_bytes());
    ChaCha20Poly1305::new(key.as_bytes().into())
}

fn nonce(sequence: u64) -> [u8; 12] {
    let mut nonce = [0; 12];
    nonce[4..].copy_from_slice(&sequence.to_le_bytes());
    nonce
}

pub async fn write_record<W: AsyncWrite + Unpin>(
    stream: &mut W,
    key: &[u8; 32],
    sequence: u64,
    data: &[u8],
    close: bool,
) -> Result<()> {
    ensure!(
        data.len() < MAX_RECORD && (!close || data.is_empty()),
        "invalid encrypted record"
    );
    let mut plaintext = Vec::with_capacity(data.len() + 1);
    plaintext.push(u8::from(close));
    plaintext.extend_from_slice(data);
    let length = (plaintext.len() + 16) as u32;
    let mut aad = length.to_le_bytes().to_vec();
    aad.extend_from_slice(&sequence.to_le_bytes());
    let ciphertext = cipher(key, sequence)
        .encrypt(
            (&nonce(sequence)).into(),
            Payload {
                msg: &plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;
    stream.write_all(&length.to_le_bytes()).await?;
    stream.write_all(&ciphertext).await?;
    stream.flush().await?;
    Ok(())
}

pub async fn read_record<R: AsyncRead + Unpin>(
    stream: &mut R,
    key: &[u8; 32],
    sequence: u64,
) -> Result<Option<Vec<u8>>> {
    let length = stream
        .read_u32_le()
        .await
        .context("truncated encrypted stream")?;
    ensure!(
        (17..=(MAX_RECORD + 16) as u32).contains(&length),
        "encrypted record limit"
    );
    let mut bytes = vec![0; length as usize];
    stream.read_exact(&mut bytes).await?;
    let mut aad = length.to_le_bytes().to_vec();
    aad.extend_from_slice(&sequence.to_le_bytes());
    let plaintext = cipher(key, sequence)
        .decrypt(
            (&nonce(sequence)).into(),
            Payload {
                msg: &bytes,
                aad: &aad,
            },
        )
        .map_err(|_| anyhow::anyhow!("record authentication failed"))?;
    match plaintext[0] {
        0 => Ok(Some(plaintext[1..].to_vec())),
        1 if plaintext.len() == 1 => Ok(None),
        _ => anyhow::bail!("invalid encrypted record type"),
    }
}
