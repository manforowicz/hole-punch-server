#![allow(dead_code)]
use chacha20poly1305::{aead::stream::DecryptorLE31, ChaCha20Poly1305};
use pin_project::pin_project;
use std::{
    io::ErrorKind,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, ReadBuf};

use crate::{HelperBuf, MAX_CHUNK_SIZE};

pub trait AsyncReadable: AsyncRead + Send + Unpin {}
impl<T: AsyncRead + Send + Unpin> AsyncReadable for T {}

fn peek_cipher_chunk(buf: &mut HelperBuf) -> Option<&[u8]> {
    if let Some(len) = buf.data().get(0..4) {
        let len = u32::from_be_bytes(len.try_into().unwrap()) as usize;
        buf.data().get(4..4 + len)
    } else {
        None
    }
}

#[pin_project]
pub struct EncryptedReader<T: AsyncReadable> {
    #[pin]
    reader: T,
    decryptor: DecryptorLE31<ChaCha20Poly1305>,
    cleartext: HelperBuf,
    ciphertext: HelperBuf,
}

impl<T: AsyncReadable> EncryptedReader<T> {
    pub async fn new(mut reader: T, shared_key: [u8; 32]) -> std::io::Result<Self> {
        let mut nonce = [0; 8];
        reader.read_exact(&mut nonce).await?;

        let decryptor = DecryptorLE31::new(&shared_key.into(), &nonce.into());
        Ok(Self {
            reader,
            decryptor,
            cleartext: HelperBuf::with_capacity(MAX_CHUNK_SIZE),
            ciphertext: HelperBuf::with_capacity(MAX_CHUNK_SIZE * 2),
        })
    }

    fn inner_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.as_mut().project();

        let old_cipherbuf_len = this.ciphertext.buf.len();
        let spare = this.ciphertext.buf.spare_capacity_mut();
        debug_assert!(!spare.is_empty());
        let mut read_buf = ReadBuf::uninit(spare);
        ready!(this.reader.poll_read(cx, &mut read_buf))?;

        let new_len = old_cipherbuf_len + read_buf.filled().len();
        unsafe { this.ciphertext.buf.set_len(new_len) }

        self.decrypt_all_full_chunks()?;

        Poll::Ready(Ok(()))
    }

    fn decrypt_all_full_chunks(self: Pin<&mut Self>) -> std::io::Result<()> {
        let this = self.project();
        while let Some(msg) = peek_cipher_chunk(this.ciphertext) {
            let msg_len = msg.len();
            if this.cleartext.spare_capacity_len() < msg_len {
                break;
            }

            let cleartext_len = this.cleartext.buf.len();
            let mut decryption_space = this.cleartext.buf.split_off(cleartext_len);

            decryption_space.extend_from_slice(msg);

            this.ciphertext.advance_cursor(msg_len + 4);

            this.decryptor
                .decrypt_next_in_place(&[], &mut decryption_space)
                .map_err(|_| std::io::Error::new(ErrorKind::InvalidData, "Decryption error"))?;

            this.cleartext.buf.unsplit(decryption_space);
        }

        if peek_cipher_chunk(this.ciphertext).is_none() && this.ciphertext.spare_capacity_len() == 0
        {
            this.ciphertext.wrap();
        }

        Ok(())
    }

    /// True if eof, false if not. Stops reading when cleartext has length at least max_bytes
    fn read_if_necessary(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        max_bytes: Option<usize>,
    ) -> Poll<std::io::Result<bool>> {
        debug_assert!(self.cleartext.buf.capacity() == MAX_CHUNK_SIZE);
        debug_assert!(self.ciphertext.buf.capacity() == 2 * MAX_CHUNK_SIZE);

        let mut bytes_amount = self.cleartext.data().len() + self.cleartext.spare_capacity_len();

        if let Some(max_bytes) = max_bytes {
            bytes_amount = std::cmp::min(bytes_amount, max_bytes);
        }

        self.as_mut().decrypt_all_full_chunks()?;

        while bytes_amount > self.cleartext.data().len() && self.ciphertext.spare_capacity_len() != 0 {
            let poll = self.as_mut().inner_read(cx)?;
            if matches!(poll, Poll::Ready(_))
                && self.ciphertext.data().is_empty()
                && self.cleartext.data().is_empty()
            {
                return Poll::Ready(Ok(true));
            } else if poll == Poll::Pending {
                if self.cleartext.data().is_empty() {
                    return Poll::Pending;
                } else {
                    break;
                }
            }
        }

        Poll::Ready(Ok(false))
    }
}

impl<T: AsyncReadable> AsyncRead for EncryptedReader<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {

        let is_eof = ready!(self.as_mut().read_if_necessary(cx, Some(buf.remaining()))?);
        if is_eof {
            println!("hao");
            return Poll::Ready(Ok(()));
        }

        let chunk = self.cleartext.data();
        let num_bytes = std::cmp::min(buf.remaining(), chunk.len());

        buf.put_slice(&chunk[0..num_bytes]);

        self.cleartext.advance_cursor(num_bytes);

        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncReadable> AsyncBufRead for EncryptedReader<T> {
    fn poll_fill_buf(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<&[u8]>> {
        let is_eof = ready!(self.as_mut().read_if_necessary(cx, None)?);
        if is_eof {
            Poll::Ready(Ok(&[]))
        } else {
            Poll::Ready(Ok(self.project().cleartext.data()))
        }
    }

    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        self.cleartext.advance_cursor(amt);
    }
}
