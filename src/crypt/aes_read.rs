use std::io::{BufRead, Read};

use aes::{
    Aes192,
    cipher::{Array, BlockModeDecrypt, KeyIvInit},
};
use cbc::Decryptor;

const BUF_SIZE: usize = 0x8000;

pub struct AesDecryptor<R>
where
    R: Read,
{
    reader: R,
    aes: Decryptor<Aes192>,
    buffer: [u8; BUF_SIZE],
    fill_count: usize,
    used_count: usize,
}

impl<R: Read> AesDecryptor<R> {
    pub fn new(reader: R, key: &[u8; 24], iv: &[u8; 16]) -> Self {
        let aes = Decryptor::<Aes192>::new(key.into(), iv.into());
        AesDecryptor {
            reader,
            aes,
            buffer: [0u8; BUF_SIZE],
            fill_count: 0,
            used_count: 0,
        }
    }

    pub(crate) fn buf(&self) -> &[u8] {
        &self.buffer[self.used_count..self.fill_count]
    }
}

impl<R: Read> Read for AesDecryptor<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let my_buf = self.fill_buf()?;
        let out_count = std::cmp::min(buf.len(), my_buf.len());
        buf[..out_count].copy_from_slice(&my_buf[..out_count]);
        self.used_count += out_count;
        Ok(out_count)
    }
}

impl<R: Read> BufRead for AesDecryptor<R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.fill_count > self.used_count {
            return Ok(self.buf());
        }

        self.used_count = 0;
        self.fill_count = 0;
        while self.fill_count < self.buffer.len() {
            let read_count = self.reader.read(&mut self.buffer[self.fill_count..])?;
            if read_count == 0 {
                break;
            }
            self.fill_count += read_count;
        }
        let (chunks, []) = Array::slice_as_chunks_mut(&mut self.buffer[..self.fill_count]) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Buffer fill count is not a multiple of 16!",
            ));
        };
        self.aes.decrypt_blocks(chunks);
        Ok(self.buf())
    }

    fn consume(&mut self, amount: usize) {
        self.used_count += amount;
    }
}
