use std::{io::Cursor, path::Path};

use binrw::{BinRead, BinResult};
use tokio::{
    fs::{File, metadata, read_dir},
    io::{AsyncReadExt, AsyncWriteExt},
    task::JoinSet,
};

use crate::semaphore::PERMITS;

use crate::jxb::Jxb;
use crate::jxk::Jxk;

/// Extract all files in a directory. Searches the directory recursively.
pub async fn extract_text_directory(in_path: &Path, out_path: &Path) -> std::io::Result<()> {
    let mut set = JoinSet::new();
    extract_text_directory_inner(in_path, in_path, &mut set).await?;
    let mut rows: Vec<_> = set.join_all().await.into_iter().flatten().collect();

    rows.sort();
    let mut writer = File::create(out_path).await?;
    write_header(&mut writer).await?;
    write_csv_rows(&mut writer, rows).await?;

    Ok(())
}

async fn extract_text_directory_inner(
    in_path: &Path,
    base_in_path: &Path,
    join_set: &mut JoinSet<Vec<Row>>,
) -> std::io::Result<()> {
    // recurse over entries
    let mut in_dir = read_dir(in_path).await?;
    while let Some(entry) = in_dir.next_entry().await? {
        let new_in_path = entry.path();
        let entry_metadata = metadata(&new_in_path).await?;
        if entry_metadata.is_dir() {
            Box::pin(extract_text_directory_inner(
                &new_in_path,
                base_in_path,
                join_set,
            ))
            .await?;
        } else if entry_metadata.is_file() {
            let Some(extension) = new_in_path.extension() else {
                continue;
            };
            let Some(relative_file_path) = in_path
                .strip_prefix(base_in_path)
                .ok()
                .and_then(|path| path.to_str())
                .and_then(|path| Some(path.to_string()))
            else {
                eprintln!(
                    "Could not get relative file path for {}",
                    new_in_path.display()
                );
                continue;
            };
            if extension.to_ascii_lowercase() == "jxb" {
                join_set.spawn(async move {
                    let _permit = PERMITS.acquire().await.unwrap();
                    match extract_rows_from_jxb_file(&new_in_path, &relative_file_path).await {
                        Ok(rows) => {
                            eprintln!("Extracted text from {}", new_in_path.display());
                            rows
                        }
                        Err(error) => {
                            eprintln!(
                                "Failed to extract text from {}: {error:?}",
                                new_in_path.display()
                            );
                            Vec::new()
                        }
                    }
                });
            } else if extension.to_ascii_lowercase() == "jxk" {
                join_set.spawn(async move {
                    let _permit = PERMITS.acquire().await.unwrap();
                    match extract_rows_from_jxk_file(&new_in_path, &relative_file_path).await {
                        Ok(rows) => {
                            eprintln!("Extracted text from {}", new_in_path.display());
                            rows
                        }
                        Err(error) => {
                            eprintln!(
                                "Failed to extract text from {}: {error:?}",
                                new_in_path.display()
                            );
                            Vec::new()
                        }
                    }
                });
            }
        }
    }
    Ok(())
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Row {
    file_name: String,
    index: i32,
    text: String,
}

pub async fn extract_text_jxk_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    let file_name = if let Some(file_name) = in_path.file_name() {
        file_name.to_str().unwrap_or("")
    } else {
        ""
    };
    let rows = extract_rows_from_jxk_file(in_path, file_name).await?;
    let mut writer = File::create(out_path).await?;
    write_header(&mut writer).await?;
    write_csv_rows(&mut writer, rows).await?;
    Ok(())
}

pub async fn extract_text_jxb_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    let file_name = if let Some(file_name) = in_path.file_name() {
        file_name.to_str().unwrap_or("")
    } else {
        ""
    };
    let rows = extract_rows_from_jxb_file(in_path, file_name).await?;
    let mut writer = File::create(out_path).await?;
    write_header(&mut writer).await?;
    write_csv_rows(&mut writer, rows).await?;
    Ok(())
}

async fn extract_rows_from_jxk_file(in_path: &Path, file_name: &str) -> BinResult<Vec<Row>> {
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;

    let mut cursor = Cursor::new(buffer);
    let jxk = Jxk::read(&mut cursor)?;
    drop(cursor);

    Ok(extract_text_jxb(&jxk.jxb(), file_name)?)
}

async fn extract_rows_from_jxb_file(in_path: &Path, file_name: &str) -> BinResult<Vec<Row>> {
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);
    let jxb = Jxb::read(&mut cursor)?;
    Ok(extract_text_jxb(&jxb, file_name)?)
}

async fn write_header(writer: &mut (impl AsyncWriteExt + Unpin)) -> std::io::Result<()> {
    writer
        .write_all("Filename,Index,Original Text,Translated Text\n".as_bytes())
        .await
}

async fn write_csv_rows<'a>(
    writer: &mut (impl AsyncWriteExt + Unpin),
    rows: impl IntoIterator<Item = Row>,
) -> std::io::Result<()> {
    for row in rows {
        writer
            .write_all(
                format!(
                    "{},{},\"{}\",\n",
                    row.file_name,
                    row.index,
                    row.text.replace('"', "\"\"")
                )
                .as_bytes(),
            )
            .await?;
    }
    Ok(())
}

fn extract_text_jxb<'a>(jxb: &'a Jxb, file_name: &str) -> std::io::Result<Vec<Row>> {
    Ok(jxb
        .node_list()?
        .iter()
        .map(|data| data.get_inner_text())
        .enumerate()
        .filter(|(_, text)| !text.is_empty())
        .map(|(index, text)| Row {
            file_name: file_name.to_string(),
            index: index as i32,
            text: text.to_string(),
        })
        .collect())
}
