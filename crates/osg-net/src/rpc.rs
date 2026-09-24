use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_REQUEST: usize = 64 * 1024;
pub const MAX_RESPONSE: usize = 8 * 1024 * 1024;
pub const MAX_CALLS: usize = 16;
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug)]
pub enum RpcError {
    Closed,
    Timeout,
    Protocol(String),
    Io(std::io::Error),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Closed => f.write_str("connection closed"),
            Self::Timeout => f.write_str("RPC deadline exceeded; execution outcome is unknown"),
            Self::Protocol(reason) => write!(f, "RPC protocol error: {reason}"),
            Self::Io(error) => write!(f, "RPC stream: {error}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<std::io::Error> for RpcError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

pub(crate) fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, RpcError> {
    let bytes =
        postcard::to_allocvec(value).map_err(|error| RpcError::Protocol(error.to_string()))?;
    if bytes.len() > limit {
        return Err(RpcError::Protocol("message exceeds byte limit".into()));
    }
    Ok(bytes)
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, RpcError> {
    let (value, remaining) =
        postcard::take_from_bytes(bytes).map_err(|error| RpcError::Protocol(error.to_string()))?;
    if !remaining.is_empty() {
        return Err(RpcError::Protocol("trailing postcard bytes".into()));
    }
    Ok(value)
}

pub(crate) async fn read<R: AsyncRead + Unpin>(
    stream: &mut R,
    limit: usize,
) -> Result<Vec<u8>, RpcError> {
    let length = stream.read_u32().await? as usize;
    if length > limit {
        return Err(RpcError::Protocol("message exceeds byte limit".into()));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    if stream.read(&mut [0]).await? != 0 {
        return Err(RpcError::Protocol("trailing stream bytes".into()));
    }
    Ok(bytes)
}

pub(crate) async fn write<W: AsyncWrite + Unpin>(
    stream: &mut W,
    bytes: &[u8],
) -> Result<(), RpcError> {
    stream
        .write_u32(
            bytes
                .len()
                .try_into()
                .map_err(|_| RpcError::Protocol("message too large".into()))?,
        )
        .await?;
    stream.write_all(bytes).await?;
    stream.shutdown().await?;
    Ok(())
}
