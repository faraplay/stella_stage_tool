use std::{collections::BTreeMap, io::Cursor, path::Path};

use binrw::{BinRead, BinResult, BinWrite};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{csv::parse_csv, jxb::Jxb, jxk::Jxk};

pub async fn inject_text_dir_files(csv_path: &Path, dir_path: &Path) -> BinResult<()> {
    let inject_rows = read_inject_rows(csv_path).await?;
    let mut rows_by_file: BTreeMap<String, Vec<InjectRow>> = BTreeMap::new();
    for row in inject_rows.into_iter() {
        if let Some(entry) = rows_by_file.get_mut(&row.file_name) {
            entry.push(row);
        } else {
            rows_by_file.insert(row.file_name.clone(), vec![row]);
        }
    }
    for (file_name, rows) in rows_by_file {
        let file_path = dir_path.join(&file_name);
        let Some(extension) = file_path.extension() else {
            eprintln!("File name {file_name} in csv does not have an extension!");
            continue;
        };
        if extension.to_ascii_lowercase() == "jxb" {
            match inject_rows_into_jxb_file(&file_path, rows).await {
                Ok(_) => {
                    eprintln!("Injected text into {}", file_path.display());
                }
                Err(error) => {
                    eprintln!(
                        "Failed to inject text into {}: {error:?}",
                        file_path.display()
                    );
                }
            }
        } else if extension.to_ascii_lowercase() == "jxk" {
            match inject_rows_into_jxk_file(&file_path, rows).await {
                Ok(_) => {
                    eprintln!("Injected text into {}", file_path.display());
                }
                Err(error) => {
                    eprintln!(
                        "Failed to inject text into {}: {error:?}",
                        file_path.display()
                    );
                }
            }
        }
    }
    Ok(())
}

pub async fn inject_text_jxk_file(csv_path: &Path, edit_path: &Path) -> BinResult<()> {
    let inject_rows = read_inject_rows(csv_path).await?;
    inject_rows_into_jxk_file(edit_path, inject_rows).await
}

pub async fn inject_text_jxb_file(csv_path: &Path, edit_path: &Path) -> BinResult<()> {
    let inject_rows = read_inject_rows(csv_path).await?;
    inject_rows_into_jxb_file(edit_path, inject_rows).await
}

async fn inject_rows_into_jxk_file(
    edit_path: &Path,
    inject_rows: Vec<InjectRow>,
) -> Result<(), binrw::Error> {
    let mut reader = File::open(edit_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);
    let jxk = Jxk::read(&mut cursor)?;
    buffer = cursor.into_inner();

    let new_jxb = jxk.jxb().inject_text(&inject_rows)?;
    let mut new_jxk = Jxk::new(new_jxb)?;
    let file_datas = jxk.get_file_datas(&buffer)?;

    let mut writer = File::create(edit_path).await?;
    new_jxk
        .add_file_datas_and_write(&file_datas, &mut writer)
        .await?;
    Ok(())
}

async fn inject_rows_into_jxb_file(
    edit_path: &Path,
    inject_rows: Vec<InjectRow>,
) -> Result<(), binrw::Error> {
    let mut reader = File::open(edit_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);
    let jxb = Jxb::read(&mut cursor)?;

    let new_jxb = jxb.inject_text(&inject_rows)?;

    let buffer = Vec::new();
    let mut cursor = Cursor::new(buffer);
    new_jxb.write_le(&mut cursor)?;

    let mut writer = File::create(edit_path).await?;
    writer.write_all(&cursor.into_inner()).await?;
    Ok(())
}

async fn read_inject_rows(csv_path: &Path) -> Result<Vec<InjectRow>, binrw::Error> {
    let mut csv_reader = File::open(csv_path).await?;
    let mut csv_text = String::new();
    csv_reader.read_to_string(&mut csv_text).await?;
    drop(csv_reader);
    let Ok((_, rows)) = parse_csv(&csv_text) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Could not parse csv file!",
        )
        .into());
    };
    let inject_rows = rows
        .into_iter()
        .skip(1)
        .map(InjectRow::from_row)
        .collect::<std::io::Result<Vec<_>>>()?;
    Ok(inject_rows)
}

#[derive(Debug)]
pub struct InjectRow {
    pub file_name: String,
    pub index: i32,
    pub node_type: String,
    pub original_text: String,
    pub inject_text: String,
}

impl InjectRow {
    fn from_row(row: Vec<String>) -> std::io::Result<InjectRow> {
        if row.len() < 5 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Csv row does not have enough cells!",
            ));
        }
        let mut iter = row.into_iter();
        let file_name = iter.next().unwrap();
        let index = str::parse(&iter.next().unwrap()).or(Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Could not parse csv data as integer!",
        )))?;
        let node_type = iter.next().unwrap();
        let original_text = iter.next().unwrap();
        let inject_text = iter.next().unwrap();
        Ok(InjectRow {
            file_name,
            index,
            node_type,
            original_text,
            inject_text,
        })
    }
}
