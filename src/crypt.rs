use std::{io::SeekFrom, path::Path};

use aes::{
    Aes192,
    cipher::{Array, BlockModeDecrypt, KeyIvInit},
};
use cbc::Decryptor;
use flate2::{Decompress, FlushDecompress, Status};
use tokio::{
    fs::{File, create_dir, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    task::JoinSet,
};

mod prng;

/// Decrypt all files in a directory. Searches the directory recursively.
pub async fn decrypt_directory(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut set = JoinSet::new();
    decrypt_directory_inner(in_path, out_path, &mut set).await?;
    set.join_all().await;
    Ok(())
}

async fn decrypt_directory_inner(
    in_path: &Path,
    out_path: &Path,
    join_set: &mut JoinSet<()>,
) -> std::io::Result<()> {
    // try to create output directory
    let create_result = create_dir(out_path).await;
    match create_result {
        Ok(_) => {}
        Err(error) => {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                Err(error)?;
            }
        }
    }

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

/// Decrypts a file.
pub async fn decrypt_file(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut reader = File::open(in_path).await?;
    let mut writer = File::create(out_path).await?;
    decrypt_stream(&mut reader, &mut writer).await?;
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

    let iv_seed: u64 = u32::from_le_bytes(in_data[0x24..0x28].try_into().unwrap()).into();
    let decompressed_file_size: u64 =
        u32::from_le_bytes(in_data[0x28..0x2c].try_into().unwrap()).into();

    let aes_key = get_aes_key(size);
    let aes_iv = get_iv(iv_seed.into());
    decrypt(&aes_key, &aes_iv, &mut in_data[0x30..])?;
    let out_file_size = decompress(&in_data[0x30..], writer).await?;
    assert_eq!(decompressed_file_size, out_file_size);
    Ok(())
}

fn decrypt_first_kb(kb_buffer: &mut [u8], seed: u64) -> std::io::Result<()> {
    let mut my_prng = prng::MyPrng::new(seed);
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
        if status == Status::StreamEnd {
            return Ok(decompress.total_out());
        }
    }
}
