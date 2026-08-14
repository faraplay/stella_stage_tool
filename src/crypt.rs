use std::{
    fs::{File, create_dir},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

use aes::{
    Aes192,
    cipher::{Array, BlockModeDecrypt, KeyIvInit},
};
use cbc::Decryptor;
use flate2::write::ZlibDecoder;

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
                    eprintln!("Failed to decrypt {}: {error}", new_in_path.display());
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
    let aes_key = get_aes_key(size);
    let iv_seed = u32::from_le_bytes(kb_buffer[0x24..0x28].try_into().unwrap());
    let aes_iv = get_iv(iv_seed.into());

    let mut encrypted_reader = Cursor::new(&kb_buffer[0x30..]).chain(reader);
    let mut zlib_writer = ZlibDecoder::new(writer);
    aes_decrypt(&aes_key, &aes_iv, &mut encrypted_reader, &mut zlib_writer)?;
    zlib_writer.finish()?;
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

fn aes_decrypt(
    aes_key: &[u8; 24],
    aes_iv: &[u8; 16],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(), CryptError> {
    let mut aes = Decryptor::<Aes192>::new(aes_key.into(), aes_iv.into());
    let mut buffer = [0u8; 0x1000];
    let mut last_nonzero_index = buffer.len();
    loop {
        // write out the zeros from the previous iteration
        // note that the value of fill_count on the previous iteration must be buffer.len()
        writer.write_all(&buffer[last_nonzero_index..])?;
        let mut fill_count = 0;
        while fill_count < buffer.len() {
            let read_count = reader.read(&mut buffer[fill_count..])?;
            if read_count == 0 {
                break;
            }
            fill_count += read_count;
        }
        let (chunks, []) = Array::slice_as_chunks_mut(&mut buffer[..fill_count]) else {
            return Err(CryptError::file_size_error(
                "Buffer fill count is not a multiple of 16!",
            ));
        };
        aes.decrypt_blocks(chunks);

        // do not write trailing zeros on this iteration
        last_nonzero_index = fill_count;
        while last_nonzero_index > 0 {
            if buffer[last_nonzero_index - 1] != 0 {
                break;
            }
            last_nonzero_index -= 1;
        }
        writer.write_all(&buffer[..last_nonzero_index])?;
        if fill_count < buffer.len() {
            break;
        }
    }
    Ok(())
}
