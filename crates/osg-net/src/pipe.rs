use crate::crypto::{self, Keys};
use anyhow::{Result, ensure};
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll, ready},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use zstd::stream::raw::{DParameter, Operation};

type Reply = oneshot::Receiver<Result<(), String>>;
type Work = (Option<Vec<u8>>, oneshot::Sender<Result<(), String>>);

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
                    let permit = workers().acquire_owned().await?;
                    let (next, output) = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let mut output = Vec::new();
                        let mut offset = 0;
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
                        Ok::<_, anyhow::Error>((decoder, output))
                    })
                    .await??;
                    decoder = next;
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
                        let permit = workers().acquire_owned().await?;
                        let (next, bytes) = tokio::task::spawn_blocking(move || {
                            use std::io::Write;
                            let _permit = permit;
                            encoder.write_all(&data)?;
                            encoder.flush()?;
                            let bytes = encoder.get_ref().clone();
                            encoder.get_mut().clear();
                            Ok::<_, anyhow::Error>((encoder, bytes))
                        })
                        .await??;
                        encoder = next;
                        for chunk in bytes.chunks(crypto::MAX_RECORD - 1) {
                            crypto::write_record(&mut write, &keys.write, sequence, chunk, false)
                                .await?;
                            sequence = sequence
                                .checked_add(1)
                                .ok_or_else(|| anyhow::anyhow!("record counter exhausted"))?;
                        }
                    } else {
                        crypto::write_record(&mut write, &keys.write, sequence, &[], true).await?;
                        write.shutdown().await?;
                    }
                    Ok(encoder)
                }
                .await;
                match result {
                    Ok(next) => {
                        encoder = next;
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

fn workers() -> Arc<Semaphore> {
    static WORKERS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    WORKERS
        .get_or_init(|| {
            Arc::new(Semaphore::new(
                std::thread::available_parallelism().map_or(1, usize::from),
            ))
        })
        .clone()
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
        let n = bytes.len().min(32768);
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
