use std::{
    io::{Cursor, SeekFrom::Start},
    path::Path,
};

use binrw::{BinResult, BinWrite, binread, binwrite};
use tokio::{
    fs::File,
    io::{AsyncSeekExt, AsyncWriteExt, copy},
};

use super::jxb::{Jxb, Node, NodeData};

#[binread]
#[binwrite]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
pub struct Jxk {
    #[brw(magic = b"JXK\0")]
    #[br(temp)]
    #[bw(calc(file_metadatas.len() as i32))]
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
    #[bw(ignore)]
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
    #[bw(ignore)]
    end_pos: u64,
}

impl Jxk {
    pub fn jxb(&self) -> &Jxb {
        &self.jxb
    }
    pub fn root_node<'a>(&'a self) -> std::io::Result<Node<'a>> {
        self.jxb.root_node()
    }

    pub fn get_metadatas<'a>(&'a self) -> std::io::Result<Vec<(NodeData<'a>, &'a FileMetadata)>> {
        self.file_metadatas
            .iter()
            .map(|metadata| {
                self.jxb
                    .get_node_data(metadata.node_index)
                    .and_then(|node| Ok((node, metadata)))
            })
            .collect()
    }

    /// Create a new Jxk from a Jxb.
    /// Note that the file offsets and sizes are set to zero and need to be filled in.
    pub fn new(jxb: Jxb) -> std::io::Result<Jxk> {
        let node_list = jxb.node_list()?;
        let file_metadatas: Vec<FileMetadata> = node_list
            .iter()
            .enumerate()
            .filter(|(_, node_data)| node_data.get_type() == "file")
            .map(|(index, _)| FileMetadata {
                node_index: index as i32,
                data_offset: 0,
                data_size: 0,
            })
            .collect();
        Ok(Jxk {
            file_metadatas,
            jxb,
        })
    }
    pub async fn add_files_and_write(
        &mut self,
        dir_path: &Path,
        writer: &mut (impl AsyncWriteExt + AsyncSeekExt + Unpin),
    ) -> BinResult<()> {
        writer
            .write_all(&vec![0u8; crate::size::get_size(self)])
            .await?;
        for file_metadata in self.file_metadatas.iter_mut() {
            let old_offset = writer.stream_position().await?;
            let offset = old_offset.next_multiple_of(0x10);
            writer
                .write_all(&vec![0u8; (offset - old_offset) as usize])
                .await?;

            let node_data = self.jxb.get_node_data(file_metadata.node_index)?;
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
            let mut reader = File::open(dir_path.join(trimmed_name)).await?;
            let size = copy(&mut reader, writer).await?;
            file_metadata.data_offset = offset as i32;
            file_metadata.data_size = size as i32;
        }
        writer.seek(Start(0)).await?;
        let buffer = Vec::new();
        let mut cursor = Cursor::new(buffer);
        self.write_le(&mut cursor)?;
        writer.write_all(&cursor.into_inner()).await?;

        Ok(())
    }
}

#[binread]
#[binwrite]
#[brw(little)]
#[derive(Debug)]
pub struct FileMetadata {
    pub node_index: i32,
    pub data_offset: i32,
    pub data_size: i32,
}
