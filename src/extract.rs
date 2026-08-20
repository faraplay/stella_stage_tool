use std::{io::Cursor, path::Path};

use binrw::{BinRead, BinResult, BinWrite, binread, binwrite};
use quick_xml::Writer;
use tokio::{
    fs::{File, create_dir, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    task::JoinSet,
};

use self::jxb::Jxb;

mod jxb;

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
                    match extract_jxb_file(&new_in_path, &new_out_path.with_added_extension("xml"))
                        .await
                    {
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("Failed to decrypt {}: {error:?}", new_in_path.display());
                        }
                    }
                });
            } else if extension.to_ascii_lowercase() == "jxk" {
                join_set.spawn(async move {
                    match extract_jxk_file(&new_in_path, &&new_out_path.with_extension("")).await {
                        Ok(_) => {}
                        Err(error) => {
                            eprintln!("Failed to decrypt {}: {error:?}", new_in_path.display());
                        }
                    }
                });
            }
        }
    }
    Ok(())
}

pub async fn extract_jxk_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
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

    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;

    let mut cursor = Cursor::new(buffer);
    let jxk = Jxk::read(&mut cursor)?;
    drop(cursor);

    let root_node = jxk.jxb.root_node()?;
    let info_writer = File::create(out_path.join("info.xml")).await?;
    let mut xml_writer = Writer::new_with_indent(info_writer, b' ', 2);
    root_node
        .write_xml(&mut xml_writer)
        .await
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    drop(root_node);

    for metadata in jxk.file_metadatas {
        let node_data = jxk.jxb.get_node_data(metadata.node_index)?;
        if node_data.get_type() != "file" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jxk file metadata links to non-file node!",
            )
            .into());
        }
        let file_name = node_data.get_text_tag("name")?;
        let mut file_writer = File::create(out_path.join(file_name)).await?;
        reader
            .seek(std::io::SeekFrom::Start(metadata.data_offset as u64))
            .await?;
        let mut short_reader = reader.take(metadata.data_size as u64);
        tokio::io::copy(&mut short_reader, &mut file_writer).await?;
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

pub async fn build_jxb_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    let reader = BufReader::new(File::open(in_path).await?);
    let mut xml_reader = quick_xml::Reader::from_reader(reader);
    let jxb = Jxb::from_xml(&mut xml_reader).await?;

    let buffer = Vec::new();
    let mut cursor = Cursor::new(buffer);
    jxb.write_le(&mut cursor)?;
    let mut writer = File::create(out_path).await?;
    writer.write_all(&cursor.into_inner()).await?;
    Ok(())
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct Jxk {
    #[brw(magic = b"JXK\0")]
    #[br(temp)]
    file_count: i32,
    #[brw(magic = b"\0\0\0\0")]
    #[br(args { count: file_count as usize })]
    #[br(assert(
        file_metadatas.windows(2).all(
            |window|
            (window[0].data_offset as u32 + window[0].data_size as u32).next_multiple_of(0x10)
            == window[1].data_offset as u32
        ),
        "File metadata table has an unexpected offset!"
    ))]
    file_metadatas: Vec<FileMetadata>,

    #[br(align_before = 0x10)]
    jxb: Jxb,

    // check jxb end position
    #[br(temp)]
    #[br(if (file_count == 0))]
    #[br(try)]
    #[br(assert(
        other_bytes.is_none(),
        "Stream is not at end of file after reading jxb!",
    ))]
    other_bytes: Option<u8>,

    #[br(align_before = 0x10)]
    #[br(temp)]
    #[br(if (file_count > 0))]
    #[br(try_calc(reader.stream_position()))]
    #[br(assert(
        file_count == 0 || end_pos == file_metadatas[0].data_offset as u64,
        "Unexpected stream position {:#X}",
        end_pos,
    ))]
    end_pos: u64,
}

#[binread]
#[binwrite]
#[brw(little)]
#[derive(Debug)]
struct FileMetadata {
    node_index: i32,
    data_offset: i32,
    data_size: i32,
}
