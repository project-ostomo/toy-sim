use crate::crypto::{self, Keys};
use anyhow::{Result, ensure};
use std::{
    future::Future,
    io::{self, Write},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use zstd::stream::raw::{DParameter, Operation};

type Reply = oneshot::Receiver<Result<(), String>>;
type Work = (Option<Vec<u8>>, oneshot::Sender<Result<(), String>>);

const WRITE_CHUNK: usize = 32 * 1024;

pub struct Pipe {
    reader: DuplexStream,
    sender: mpsc::UnboundedSender<Work>,
    pending: Option<Reply>,
    closing: bool,
    error: Arc<Mutex<Option<String>>>,
    read_task: JoinHandle<()>,
    write_task: JoinHandle<()>,
}

impl Pipe {
    pub fn new<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
        stream: S,
        keys: Keys,
    ) -> Result<Self> {
        let (mut read, mut write) = tokio::io::split(stream);
        let (reader, mut delivered) = tokio::io::duplex(65536);
        let (sender, mut receiver) = mpsc::unbounded_channel::<Work>();
        let error = Arc::new(Mutex::new(None));
        let read_error = error.clone();
        let write_error = error.clone();
        let mut decoder = zstd::stream::raw::Decoder::new()?;
        decoder.set_parameter(DParameter::WindowLogMax(21))?;
        let mut encoder = zstd::stream::write::Encoder::new(Vec::new(), 3)?;
        encoder.window_log(21)?;
        let read_task = tokio::spawn(async move {
            let result: Result<()> = async {
                let mut sequence = 1_u64;
                loop {
                    let Some(data) = crypto::read_record(&mut read, &keys.read, sequence).await?
                    else {
                        return Ok(());
                    };
                    sequence = sequence
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("record counter exhausted"))?;
                    let mut offset = 0;
                    let mut output = Vec::new();
                    loop {
                        let mut buffer = [0; 32768];
                        let status = decoder.run_on_buffers(&data[offset..], &mut buffer)?;
                        offset += status.bytes_read;
                        ensure!(
                            output.len() + status.bytes_written <= 256 * 1024,
                            "decoded record limit"
                        );
                        output.extend_from_slice(&buffer[..status.bytes_written]);

                        if offset == data.len() && status.bytes_written < buffer.len() {
                            break;
                        }
                        ensure!(
                            status.bytes_read > 0 || status.bytes_written > 0,
                            "stalled decompressor"
                        );
                    }
                    delivered.write_all(&output).await?;
                }
            }
            .await;
            if let Err(err) = result {
                *read_error.lock().unwrap() = Some(format!("{err:#}"));
            }
        });
        let write_task = tokio::spawn(async move {
            let mut sequence = 1_u64;
            while let Some((data, reply)) = receiver.recv().await {
                let close = data.is_none();
                let result: Result<_> = async {
                    if let Some(data) = data {
                        encoder.write_all(&data)?;
                        encoder.flush()?;
                        for chunk in encoder.get_ref().chunks(crypto::MAX_RECORD - 1) {
                            crypto::write_record(&mut write, &keys.write, sequence, chunk, false)
                                .await?;
                            sequence = sequence
                                .checked_add(1)
                                .ok_or_else(|| anyhow::anyhow!("record counter exhausted"))?;
                        }
                        encoder.get_mut().clear();
                    } else {
                        crypto::write_record(&mut write, &keys.write, sequence, &[], true).await?;
                        write.shutdown().await?;
                    }
                    Ok(())
                }
                .await;
                match result {
                    Ok(()) => {
                        let _ = reply.send(Ok(()));
                    }
                    Err(err) => {
                        let message = format!("{err:#}");
                        *write_error.lock().unwrap() = Some(message.clone());
                        let _ = reply.send(Err(message));
                        break;
                    }
                }
                if close {
                    break;
                }
            }
        });
        Ok(Self {
            reader,
            sender,
            pending: None,
            closing: false,
            error,
            read_task,
            write_task,
        })
    }

    fn drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(pending) = &mut self.pending {
            let result = ready!(Pin::new(pending).poll(cx));
            self.pending = None;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Poll::Ready(Err(io::Error::other(err))),
                Err(_) => return Poll::Ready(Err(io::Error::other("transport writer stopped"))),
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Pipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buffer.filled().len();
        ready!(Pin::new(&mut self.reader).poll_read(cx, buffer))?;
        if buffer.filled().len() == before {
            if let Some(error) = self.error.lock().unwrap().as_ref() {
                return Poll::Ready(Err(io::Error::other(error.clone())));
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Pipe {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        ready!(self.drain(cx))?;
        if self.closing {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "transport closed",
            )));
        }
        let n = bytes.len().min(WRITE_CHUNK);
        if n == 0 {
            return Poll::Ready(Ok(0));
        }
        let (send, recv) = oneshot::channel();
        self.sender
            .send((Some(bytes[..n].to_vec()), send))
            .map_err(|_| io::Error::other("transport writer stopped"))?;
        self.pending = Some(recv);
        Poll::Ready(Ok(n))
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.drain(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        ready!(self.drain(cx))?;
        if !self.closing {
            self.closing = true;
            let (send, recv) = oneshot::channel();
            self.sender
                .send((None, send))
                .map_err(|_| io::Error::other("transport writer stopped"))?;
            self.pending = Some(recv);
        }
        self.drain(cx)
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        self.read_task.abort();
        self.write_task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{RngCore, SeedableRng, rngs::StdRng};
    use tokio::io::AsyncReadExt;

    fn keys() -> Keys {
        Keys {
            read: [1; 32],
            write: [1; 32],
            account: osg_model::Id([0; 16]),
        }
    }

    #[tokio::test]
    async fn streams_large_mixed_input_with_backpressure() {
        let mut expected = vec![0; 1024 * 1024];
        StdRng::seed_from_u64(4).fill_bytes(&mut expected[..512 * 1024]);
        let input = expected.clone();
        let (a, b) = tokio::io::duplex(1024);
        let mut writer = Pipe::new(a, keys()).unwrap();
        let mut reader = Pipe::new(b, keys()).unwrap();

        let sending = tokio::spawn(async move {
            writer.write_all(&input).await.unwrap();
            writer.shutdown().await.unwrap();
        });
        let mut received = Vec::new();
        reader.read_to_end(&mut received).await.unwrap();
        sending.await.unwrap();
        assert_eq!(received, expected);
    }

    #[tokio::test]
    async fn rejects_excessive_decompression() {
        let compressed = zstd::encode_all(&vec![0; 1024 * 1024][..], 3).unwrap();
        let (mut sender, receiver) = tokio::io::duplex(1024);
        let mut reader = Pipe::new(receiver, keys()).unwrap();
        crypto::write_record(&mut sender, &[1; 32], 1, &compressed, false)
            .await
            .unwrap();

        let mut received = Vec::new();
        let error = reader.read_to_end(&mut received).await.unwrap_err();
        assert!(error.to_string().contains("decoded record limit"));
        assert!(received.len() <= 256 * 1024);
    }
}
