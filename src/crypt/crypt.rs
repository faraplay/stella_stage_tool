use std::{io::SeekFrom, path::Path};

use aes::{
    Aes192,
    cipher::{Array, BlockModeDecrypt, BlockModeEncrypt, KeyIvInit},
};
use cbc::{Decryptor, Encryptor};
use crc::{CRC_32_ISO_HDLC, Crc};
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use tokio::{
    fs::{File, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    task::JoinSet,
};

use super::prng::MyPrng;
use crate::{dir::try_create_dir, semaphore::PERMITS};

/// Decrypt all files in a directory. Searches the directory recursively.
pub async fn decrypt_directory(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut set = JoinSet::new();
    decrypt_directory_inner(in_path, out_path, &mut set).await?;
    set.join_all().await;
    Ok(())
}

/// Encrypt all files in a directory. Searches the directory recursively.
pub async fn encrypt_directory(
    in_path: &Path,
    out_path: &Path,
    small: bool,
) -> std::io::Result<()> {
    let mut set = JoinSet::new();
    encrypt_directory_inner(in_path, out_path, small, &mut set).await?;
    set.join_all().await;
    Ok(())
}

async fn decrypt_directory_inner(
    in_path: &Path,
    out_path: &Path,
    join_set: &mut JoinSet<()>,
) -> std::io::Result<()> {
    try_create_dir(out_path).await?;
    // recurse over entries
    let mut in_dir = read_dir(in_path).await?;
    while let Some(entry) = in_dir.next_entry().await? {
        let new_in_path = entry.path();
        let new_out_path = out_path.join(new_in_path.file_name().unwrap());
        let entry_metadata = metadata(&new_in_path).await?;
        if entry_metadata.is_dir() {
            Box::pin(decrypt_directory_inner(
                &new_in_path,
                &new_out_path,
                join_set,
            ))
            .await?;
        } else if entry_metadata.is_file() {
            join_set.spawn(async move {
                let _permit = PERMITS.acquire().await.unwrap();
                match decrypt_file(&new_in_path, &new_out_path).await {
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("Failed to decrypt {}: {error:?}", new_in_path.display());
                    }
                }
            });
        }
    }
    Ok(())
}

async fn encrypt_directory_inner(
    in_path: &Path,
    out_path: &Path,
    small: bool,
    join_set: &mut JoinSet<()>,
) -> std::io::Result<()> {
    try_create_dir(out_path).await?;
    // recurse over entries
    let mut in_dir = read_dir(in_path).await?;
    while let Some(entry) = in_dir.next_entry().await? {
        let new_in_path = entry.path();
        let new_out_path = out_path.join(new_in_path.file_name().unwrap());
        let entry_metadata = metadata(&new_in_path).await?;
        if entry_metadata.is_dir() {
            Box::pin(encrypt_directory_inner(
                &new_in_path,
                &new_out_path,
                small,
                join_set,
            ))
            .await?;
        } else if entry_metadata.is_file() {
            join_set.spawn(async move {
                let _permit = PERMITS.acquire().await.unwrap();
                match encrypt_file(&new_in_path, &new_out_path, small).await {
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("Failed to encrypt {}: {error:?}", new_in_path.display());
                    }
                }
            });
        }
    }
    Ok(())
}

/// Decrypts a file.
pub async fn decrypt_file(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut reader = File::open(in_path).await?;
    let mut writer = File::create(out_path).await?;
    decrypt_stream(&mut reader, &mut writer).await?;
    Ok(())
}

/// Encrypts a file.
pub async fn encrypt_file(in_path: &Path, out_path: &Path, small: bool) -> std::io::Result<()> {
    let mut reader = File::open(in_path).await?;
    let mut writer = File::create(out_path).await?;
    encrypt_stream(&mut reader, &mut writer, small).await?;
    Ok(())
}

async fn decrypt_stream(
    reader: &mut (impl AsyncReadExt + AsyncSeekExt + Unpin),
    writer: &mut (impl AsyncWriteExt + Unpin),
) -> std::io::Result<()> {
    let size = reader.seek(SeekFrom::End(0)).await?;
    if size % 16 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File size is not a multiple of 16!",
        ));
    }
    if size < 0x30 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File to decrypt is too small!",
        ));
    }
    reader.seek(SeekFrom::Start(0)).await?;
    let mut in_data = Vec::new();
    reader.read_to_end(&mut in_data).await?;

    decrypt_first_kb(&mut in_data[..std::cmp::min(size, 0x400) as usize], size)?;

    let crc32 = u32::from_le_bytes(in_data[0x24..0x28].try_into().unwrap());
    let decompressed_file_size: u64 =
        u32::from_le_bytes(in_data[0x28..0x2c].try_into().unwrap()).into();

    let aes_key = get_aes_key(size);
    let aes_iv = get_iv(crc32.into());
    decrypt(&aes_key, &aes_iv, &mut in_data[0x30..])?;

    in_data[0x24..0x28].copy_from_slice(&[0, 0, 0, 0]);
    let calculated_crc32 = get_crc32(&in_data);
    assert_eq!(crc32, calculated_crc32, "Incorrect CRC32 checksum!");

    let out_file_size = decompress(&in_data[0x30..], writer).await?;
    assert_eq!(
        decompressed_file_size, out_file_size,
        "Incorrect decompressed file size!"
    );
    Ok(())
}

fn get_crc32(data: &[u8]) -> u32 {
    const CRC: crc::Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
    CRC.checksum(data)
}

async fn encrypt_stream(
    reader: &mut (impl AsyncReadExt + AsyncSeekExt + Unpin),
    writer: &mut (impl AsyncWriteExt + Unpin),
    small: bool,
) -> std::io::Result<()> {
    let mut in_data = Vec::new();
    reader.read_to_end(&mut in_data).await?;

    let mut buffer = vec![0u8; 0x30];
    buffer[0..0x4].copy_from_slice(b"MZNC");
    let decompressed_file_size = in_data.len();
    buffer[0x28..0x2c].copy_from_slice(&(decompressed_file_size as u32).to_le_bytes());

    compress(&in_data, &mut buffer, small)?;
    drop(in_data);

    let compressed_data_size = buffer.len().next_multiple_of(16);
    buffer.resize(compressed_data_size, 0);
    let out_file_size = buffer.len() as u64;
    let crc32: u32 = get_crc32(&buffer);
    buffer[0x24..0x28].copy_from_slice(&crc32.to_le_bytes());
    let aes_key = get_aes_key(out_file_size);
    let aes_iv = get_iv(crc32.into());
    encrypt(&aes_key, &aes_iv, &mut buffer[0x30..])?;

    let kb_buffer_size = std::cmp::min(out_file_size, 0x400) as usize;
    decrypt_first_kb(&mut buffer[..kb_buffer_size], out_file_size)?;

    writer.write_all(&buffer).await?;
    Ok(())
}

fn decrypt_first_kb(kb_buffer: &mut [u8], seed: u64) -> std::io::Result<()> {
    let mut my_prng = MyPrng::new(seed);
    for _ in 0..5 {
        my_prng.next_u64();
    }
    let (chunks, []) = kb_buffer.as_chunks_mut::<4>() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
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
    let mut my_prng = MyPrng::new(seed);
    let mut aes_key = [0u8; 24];
    for i in 0..6 {
        let value = (my_prng.next_u64() as u32).wrapping_add(KEY_SHIFTS[i]) ^ KEY_MASKS[i];
        aes_key[4 * i..4 * (i + 1)].copy_from_slice(&value.to_le_bytes());
    }
    aes_key
}

fn get_iv(seed: u64) -> [u8; 16] {
    let mut my_prng = MyPrng::new(seed);
    let mut aes_iv = [0u8; 16];
    for i in 0..4 {
        let value = my_prng.next_u64() as u32;
        aes_iv[4 * i..4 * (i + 1)].copy_from_slice(&value.to_le_bytes());
    }
    aes_iv
}

fn decrypt(key: &[u8; 24], iv: &[u8; 16], data: &mut [u8]) -> std::io::Result<()> {
    let mut decryptor = Decryptor::<Aes192>::new(key.into(), iv.into());
    let (chunks, []) = Array::slice_as_chunks_mut(data) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Buffer fill count is not a multiple of 16!",
        ));
    };
    decryptor.decrypt_blocks(chunks);
    Ok(())
}

fn encrypt(key: &[u8; 24], iv: &[u8; 16], data: &mut [u8]) -> std::io::Result<()> {
    let mut encryptor = Encryptor::<Aes192>::new(key.into(), iv.into());
    let (chunks, []) = Array::slice_as_chunks_mut(data) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Buffer fill count is not a multiple of 16!",
        ));
    };
    encryptor.encrypt_blocks(chunks);
    Ok(())
}

async fn decompress(
    compressed_data: &[u8],
    writer: &mut (impl AsyncWriteExt + Unpin),
) -> std::io::Result<u64> {
    const BUF_SIZE: usize = 0x8000;
    let mut buffer = Vec::with_capacity(BUF_SIZE);
    let mut decompress = Decompress::new(true);
    loop {
        buffer.clear();
        let status = decompress.decompress_vec(
            &compressed_data[decompress.total_in() as usize..],
            &mut buffer,
            FlushDecompress::None,
        )?;
        writer.write_all(&buffer).await?;
        match status {
            Status::BufError => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Error decompressing zlib stream! Not enough data?",
                ));
            }
            Status::StreamEnd => {
                return Ok(decompress.total_out());
            }
            _ => {}
        }
    }
}

fn compress(decompressed_data: &[u8], out_buf: &mut Vec<u8>, small: bool) -> std::io::Result<()> {
    const BUF_SIZE: usize = 0x8000;
    let mut buffer = Vec::with_capacity(BUF_SIZE);
    let mut compress = Compress::new(
        if small {
            Compression::best()
        } else {
            Compression::fast()
        },
        true,
    );
    loop {
        compress.compress_vec(
            &decompressed_data[compress.total_in() as usize..],
            &mut buffer,
            FlushCompress::None,
        )?;
        out_buf.append(&mut buffer);
        if compress.total_in() == decompressed_data.len() as u64 {
            break;
        }
    }
    loop {
        let status = compress.compress_vec(&[], &mut buffer, FlushCompress::Finish)?;
        out_buf.append(&mut buffer);
        match status {
            Status::StreamEnd => return Ok(()),
            _ => {}
        }
    }
}
