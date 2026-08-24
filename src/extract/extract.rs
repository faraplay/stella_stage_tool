use std::{io::Cursor, path::Path};

use binrw::{BinRead, BinResult};
use quick_xml::Writer;
use tokio::{
    fs::{File, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt},
    task::JoinSet,
};

use crate::{dir::try_create_dir, semaphore::PERMITS};

use crate::jxb::Jxb;
use crate::jxk::Jxk;

/// Extract all files in a directory. Searches the directory recursively.
pub async fn extract_directory(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut set = JoinSet::new();
    extract_directory_inner(in_path, out_path, &mut set).await?;
    set.join_all().await;
    Ok(())
}

async fn extract_directory_inner(
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
            Box::pin(extract_directory_inner(
                &new_in_path,
                &new_out_path,
                join_set,
            ))
            .await?;
        } else if entry_metadata.is_file() {
            let Some(extension) = new_in_path.extension() else {
                continue;
            };
            if extension.to_ascii_lowercase() == "jxb" {
                join_set.spawn(async move {
                    let _permit = PERMITS.acquire().await.unwrap();
                    match extract_jxb_file(&new_in_path, &new_out_path.with_added_extension("xml"))
                        .await
                    {
                        Ok(_) => {
                            eprintln!("Extracted {}", new_in_path.display());
                        }
                        Err(error) => {
                            eprintln!("Failed to extract {}: {error:?}", new_in_path.display());
                        }
                    }
                });
            } else if extension.to_ascii_lowercase() == "jxk" {
                join_set.spawn(async move {
                    let _permit = PERMITS.acquire().await.unwrap();
                    match extract_jxk_file(&new_in_path, &&new_out_path.with_extension("")).await {
                        Ok(_) => {
                            eprintln!("Extracted {}", new_in_path.display());
                        }
                        Err(error) => {
                            eprintln!("Failed to extract {}: {error:?}", new_in_path.display());
                        }
                    }
                });
            }
        }
    }
    Ok(())
}

pub async fn extract_jxk_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    try_create_dir(out_path).await?;
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;

    let mut cursor = Cursor::new(buffer);
    let jxk = Jxk::read(&mut cursor)?;
    drop(cursor);

    let root_node = jxk.root_node()?;
    let info_writer = File::create(out_path.join("info.xml")).await?;
    let mut xml_writer = Writer::new_with_indent(info_writer, b' ', 2);
    root_node
        .write_xml(&mut xml_writer)
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    drop(root_node);

    for (node_data, metadata) in jxk.get_metadatas()? {
        if node_data.get_type() != "file" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jxk file metadata links to non-file node!",
            )
            .into());
        }
        let file_name = node_data.get_text_tag("name")?;
        let trimmed_name_start_index = file_name.rfind('\\').map_or(0, |i| i + 1);
        let trimmed_name = &file_name[trimmed_name_start_index..];
        let mut file_writer = File::create(out_path.join(trimmed_name)).await?;
        reader
            .seek(std::io::SeekFrom::Start(metadata.data_offset as u64))
            .await?;
        let mut short_reader = reader.take(metadata.data_size as u64);
        tokio::io::copy(&mut short_reader, &mut file_writer).await?;
        drop(file_writer);
        reader = short_reader.into_inner();
    }
    Ok(())
}

pub async fn extract_jxb_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);
    let jxb = Jxb::read(&mut cursor)?;
    let root_node = jxb.root_node()?;
    let writer = File::create(out_path).await?;
    let mut xml_writer = Writer::new_with_indent(writer, b' ', 2);
    root_node
        .write_xml(&mut xml_writer)
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(())
}
