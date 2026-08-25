use std::{io::Cursor, path::Path};

use binrw::{BinRead, BinResult, BinWrite};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
};

use crate::{csv::parse_csv, jxb::Jxb, jxk::Jxk};

pub async fn inject_text_jxk_file(csv_path: &Path, edit_path: &Path) -> BinResult<()> {
    let inject_rows = read_inject_rows(csv_path).await?;

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

pub async fn inject_text_jxb_file(csv_path: &Path, edit_path: &Path) -> BinResult<()> {
    let inject_rows = read_inject_rows(csv_path).await?;

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
