use std::{io::Cursor, path::Path};

use binrw::{BinResult, BinWrite};
use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufReader},
};

use crate::jxb::Jxb;
use crate::jxk::Jxk;

pub async fn build_jxk_file(in_path: &Path, out_path: &Path) -> BinResult<()> {
    let info_path = in_path.join("info.xml");
    let info_reader = BufReader::new(File::open(info_path).await?);
    let mut xml_reader = quick_xml::Reader::from_reader(info_reader);
    let jxb = Jxb::from_xml(&mut xml_reader).await?;
    let mut jxk = Jxk::new(jxb)?;
    let mut writer = File::create(out_path).await?;
    jxk.add_files_and_write(in_path, &mut writer).await?;

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
