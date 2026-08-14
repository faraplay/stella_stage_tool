use std::{
    fs::{File, create_dir},
    io::{BufRead, Read, Seek, SeekFrom, Write},
    path::Path,
};

use aes::{
    Aes192,
    cipher::{Array, BlockModeDecrypt, KeyIvInit},
};
use cbc::Decryptor;
use flate2::bufread::ZlibDecoder;

use crate::crypt::error::CryptError;

mod error;
mod prng;

/// Decrypt all files in a directory. Searches the directory recursively.
pub fn decrypt_directory(in_path: &Path, out_path: &Path) -> Result<(), CryptError> {
    // try to create output directory
    let create_result = create_dir(out_path);
    match create_result {
        Ok(_) => {}
        Err(error) => {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                Err(error)?;
            }
        }
    }

    // recurse over entries
    let in_dir = std::fs::read_dir(in_path)?;
    for entry in in_dir {
        let entry = entry?;
        let new_in_path = entry.path();
        let new_out_path = out_path.join(new_in_path.file_name().unwrap());
        if new_in_path.is_dir() {
            decrypt_directory(&new_in_path, &new_out_path)?;
        } else if new_in_path.is_file() {
            match decrypt_file(&new_in_path, &new_out_path) {
                Ok(_) => {}
                Err(error) => {
                    eprintln!("Failed to decrypt {}: {error:?}", new_in_path.display());
                }
            }
        }
    }
    Ok(())
}

/// Decrypts a file.
pub fn decrypt_file(in_path: &Path, out_path: &Path) -> Result<(), CryptError> {
    let mut reader = File::open(in_path)?;
    let mut writer = File::create(out_path)?;
    decrypt_stream(&mut reader, &mut writer)?;
    Ok(())
}

fn decrypt_stream(
    reader: &mut (impl Read + Seek),
    writer: &mut impl Write,
) -> Result<(), CryptError> {
    let size = reader.seek(SeekFrom::End(0))?;
    if size % 16 != 0 {
        return Err(CryptError::file_size_error(
            "File size is not a multiple of 16!",
        ));
    }
    if size < 0x30 {
        return Err(CryptError::file_size_error("File to decrypt is too small!"));
    }
    reader.seek(SeekFrom::Start(0))?;
    let mut kb_buffer = vec![0u8; std::cmp::min(size, 0x400) as usize];
    reader.read_exact(&mut kb_buffer)?;
    decrypt_first_kb(&mut kb_buffer, size)?;
    // let aes_key = get_aes_key(size);
    let iv_seed: u64 = u32::from_le_bytes(kb_buffer[0x24..0x28].try_into().unwrap()).into();
    let decompressed_file_size: u64 =
        u32::from_le_bytes(kb_buffer[0x28..0x2c].try_into().unwrap()).into();
    // let aes_iv = get_iv(iv_seed.into());

    let mut decompressed_reader = ZlibDecoder::new(AesDecryptor::new(
        (&kb_buffer[0x30..]).chain(reader),
        size,
        iv_seed,
    ));
    let out_file_size = std::io::copy(&mut decompressed_reader, writer)?;
    assert_eq!(decompressed_file_size, out_file_size);
    Ok(())
}

fn decrypt_first_kb(kb_buffer: &mut [u8], seed: u64) -> Result<(), CryptError> {
    let mut my_prng = prng::MyPrng::new(seed);
    for _ in 0..5 {
        my_prng.next_u64();
    }
    let (chunks, []) = kb_buffer.as_chunks_mut::<4>() else {
        return Err(CryptError::file_size_error(
            "File size is not a multiple of 4!",
        ));
    };
    for chunk in chunks.into_iter().skip(1) {
        let value = my_prng.next_interweaved().to_le_bytes();
        for i in 0..4 {
            chunk[i] ^= value[i];
        }
    }
    Ok(())
}

const KEY_MASKS: [u32; 6] = [
    0x875F5F41, 0x73FD2031, 0x75B704FD, 0xC0FBE1D2, 0xD238F9C5, 0x4B01C826,
];

const KEY_SHIFTS: [u32; 6] = [
    0x971B98FE, 0x4F640E1F, 0x391F4EA7, 0xB8B4A217, 0xC5EEEA3C, 0xA2232CE9,
];

fn get_aes_key(seed: u64) -> [u8; 24] {
    let mut my_prng = prng::MyPrng::new(seed);
    let mut aes_key = [0u8; 24];
    for i in 0..6 {
        let value = (my_prng.next_u64() as u32).wrapping_add(KEY_SHIFTS[i]) ^ KEY_MASKS[i];
        aes_key[4 * i..4 * (i + 1)].copy_from_slice(&value.to_le_bytes());
    }
    aes_key
}

fn get_iv(seed: u64) -> [u8; 16] {
    let mut my_prng = prng::MyPrng::new(seed);
    let mut aes_iv = [0u8; 16];
    for i in 0..4 {
        let value = my_prng.next_u64() as u32;
        aes_iv[4 * i..4 * (i + 1)].copy_from_slice(&value.to_le_bytes());
    }
    aes_iv
}

const BUF_SIZE: usize = 0x8000;
struct AesDecryptor<R>
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
    pub fn new(reader: R, key_seed: u64, iv_seed: u64) -> Self {
        let aes = Decryptor::<Aes192>::new(&get_aes_key(key_seed).into(), &get_iv(iv_seed).into());
        AesDecryptor {
            reader,
            aes,
            buffer: [0u8; BUF_SIZE],
            fill_count: 0,
            used_count: 0,
        }
    }

    fn buf(&self) -> &[u8] {
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
