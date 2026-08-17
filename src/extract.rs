use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    io::Cursor,
    path::Path,
};

use binrw::{
    BinRead, BinResult, binread,
    helpers::{args_iter, until_exclusive, until_exclusive_with, until_with},
};
use indexmap::IndexMap;
use quick_xml::{Writer, events::BytesText};
use tokio::{
    fs::{File, create_dir, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite},
    task::JoinSet,
};

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
        let node = jxk.jxb.get_node(metadata.node_index)?;
        if node.node_type != "file" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "jxk file metadata links to non-file node!",
            )
            .into());
        }
        let JxbValue::Text(file_name) = node.tags.get("name").ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "File node is missing name tag!",
        ))?
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "File node has name tag but it is not a string!",
            )
            .into());
        };
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

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct Jxk {
    #[br(magic = b"JXK\0")]
    #[br(temp)]
    file_count: i32,
    #[br(magic = b"\0\0\0\0")]
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
#[br(little)]
#[derive(Debug)]
struct FileMetadata {
    node_index: i32,
    data_offset: i32,
    data_size: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct Jxb {
    #[br(align_before = 0x10)]
    #[br(temp)]
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    start_pos: i32,
    #[br(magic = b"JXB\0\x01\x00\x01")]
    #[br(assert(uses_utf16 == 1 || uses_utf16 == 2))]
    uses_utf16: u8,
    #[br(temp)]
    #[br(assert(node_count != 0))]
    node_count: u32,
    #[br(temp)]
    key_string_count: u32,
    #[br(temp)]
    #[br(map(|offset: i32| start_pos + offset))]
    b_region_pos: i32,
    #[br(temp)]
    #[br(map(|relative_offset: i32| start_pos + relative_offset))]
    key_string_offset_region_pos: i32,
    #[br(magic = b"\0\0\0\0")]
    #[br(temp)]
    #[br(map(|relative_offset: i32| start_pos + relative_offset))]
    string_region_pos: i32,

    #[br(magic = b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")] // 0x10 zero bytes
    #[br(temp)]
    #[br(args { count: node_count as usize })]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == b_region_pos as u64),
        "incorrect stream position for b_region_pos, expected {:X}",
        b_region_pos,
    ))]
    node_data_as: Vec<JxbNodeDataA>,

    #[br(parse_with = args_iter(
        node_data_as.iter().map(
            |a| (b_region_pos + a.b_offset, a.tags_type_id, a.tag_count)
        )
    ))]
    node_data_bs: Vec<JxbNodeDataB>,

    #[br(temp)]
    #[br(align_after = 0x10)]
    #[br(try_calc(
        {
            let mut child_index = 1;
            node_data_bs.iter().enumerate().map(|(parent_index, b)| {
                if b.child_count == 0 {
                    if b.children_start_index != -1 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("node {:X} has no children but children_start_index is not -1", parent_index),
                        ));
                    } else {
                        return Ok(());
                    }
                }
                if b.children_start_index != child_index {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!(
                            "node {:X} has children_start_index {:X}, expected {:X}",
                            parent_index,
                            b.children_start_index,
                            child_index,
                        ),
                    ));
                }
                child_index = b.children_start_index + b.child_count;
                for index in b.children_start_index..b.children_start_index + b.child_count {
                    if node_data_as[index as usize].parent_index as usize != parent_index {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("node {:X} has incorrect parent index", index),
                        ));
                    }
                }
                Ok(())
            }).collect::<std::io::Result<()>>()
        }
    ))]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == key_string_offset_region_pos as u64),
        "incorrect stream position for key_string_offset_region_pos, expected {:X}",
        key_string_offset_region_pos,
    ))]
    assertion1: (),

    #[br(args { count: key_string_count as usize })]
    #[br(align_after = 0x10)]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == string_region_pos as u64),
        "incorrect stream position for string_region_pos, expected {:X}",
        string_region_pos,
    ))]
    #[br(assert(
        key_string_offsets.windows(2).all(
            |window| window[0] < window[1]
        ),
        "key string offsets are not in ascending order",
    ))]
    key_string_offsets: Vec<i32>,

    #[br(temp)]
    #[br(calc = node_data_bs.iter().map(|b| b.text_offset).min().unwrap())]
    #[br(assert(
        node_data_bs.iter().all(
            |b| b.child_count == 0 || b.text_offset == node_text_offset_min
        ),
        "Some node has both text content and child nodes!"
    ))]
    node_text_offset_min: i32,
    #[br(temp)]
    #[br(calc = node_data_bs.iter().map(|b| b.text_offset).max().unwrap())]
    #[br(assert(
        if uses_utf16 == 1 { node_text_offset_min == node_text_offset_max } else { true }
    ))]
    node_text_offset_max: i32,

    #[br(parse_with = until_exclusive_with(
        |(_, text): &(i32, String)| text.is_empty(),
        |reader, options, _: ()| {
            let string = JxbUtf8String::read_options(reader, options, ())?;
            Ok((string.pos - string_region_pos, string.text))
        }
    ))]
    #[br(pad_after(if uses_utf16 == 1 { 0 } else { -1 }))]
    utf8_strings: BTreeMap<i32, String>,

    #[br(if(
        uses_utf16 == 2,
        BTreeMap::from_iter(
            std::iter::once(
                (node_text_offset_max, String::new())
            )
        ),
    ))]
    #[br(parse_with = until_with(
        |(offset, _): &(i32, String)| *offset >= node_text_offset_max,
        |reader, options, _: ()| {
            let string = JxbUtf16String::read_options(reader, options, ())?;
            Ok((string.pos - string_region_pos, string.text))
        }
    ))]
    #[br(assert(
        utf16_strings.first_entry().is_none_or(|entry|entry.get().is_empty()),
        "the first utf16 string is not the empty string, it is {}",
        utf16_strings.first_key_value().unwrap().1,
    ))]
    utf16_strings: BTreeMap<i32, String>,
}

#[binread]
#[br(little)]
#[derive(Debug)]
struct JxbNodeDataA {
    #[br(magic = b"\x03\0")]
    tags_type_id: u16,
    tag_count: u32,
    b_offset: i32,
    parent_index: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[br(import(expected_offset: i32, tags_type_id: u16, extra_count: u32))]
#[br(pre_assert(
    reader.stream_position().map_or(false, |pos| pos == expected_offset as u64),
    "incorrect stream position for NodeDataB item, expected {:X}",
    expected_offset,
))]
#[derive(Debug)]
struct JxbNodeDataB {
    node_type_offset: i32,
    children_start_index: i32,
    child_count: i32,
    text_offset: i32,

    #[br(args { count: extra_count as usize, inner: (tags_type_id,) })]
    tags: Vec<JxbTag>,

    #[br(temp)]
    #[br(try_calc(
        (||{
            match tags_type_id {
                0 => if !tags.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tags_type_id is 0 but there are tags present",
                    ));
                },
                1 => if tags.iter().map(|tag| tag.type_id).collect::<HashSet<_>>().len() <= 1 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tags_type_id is 1 but all tags present have the same type_id",
                    ));
                },
                _ => if tags.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "tags_type_id is not 0 but there are no tags present",
                    ));
                },
            };
            let mut key_offsets = HashSet::new();
            for tag in &tags {
                if !key_offsets.insert(tag.key_offset) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Duplicate tag key offset {}", tag.key_offset),
                    ));
                }
            }
            return Ok(());
        })()
    ))]
    assertion1: (),
}

#[binread]
#[br(little)]
#[br(import(tags_type_id: u16))]
#[derive(Debug)]
struct JxbTag {
    key_offset: i32,
    #[br(if(tags_type_id == 1, tags_type_id as u32))]
    type_id: u32,
    value: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbUtf8String {
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    pos: i32,
    #[br(temp)]
    #[br(parse_with = until_exclusive(|&value| value == 0))]
    utf8_values: Vec<u8>,
    #[br(try_calc(String::from_utf8(utf8_values)))]
    text: String,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbUtf16String {
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    pos: i32,
    #[br(temp)]
    #[br(parse_with = until_exclusive(|&value| value == 0))]
    utf16_values: Vec<u16>,
    #[br(try_calc(String::from_utf16(&utf16_values)))]
    text: String,
}

impl<'a> Jxb {
    fn get_node(&'a self, index: i32) -> std::io::Result<JxbNode<'a>> {
        JxbNode::new(
            index,
            &self.node_data_bs,
            &self.utf8_strings,
            &self.utf16_strings,
        )
    }
    fn root_node(&'a self) -> std::io::Result<JxbNode<'a>> {
        self.get_node(0)
    }
}

#[derive(Debug)]
struct JxbNode<'a> {
    node_type: &'a str,
    tags: IndexMap<&'a str, JxbValue<'a>>,
    text: &'a str,
    children: Vec<JxbNode<'a>>,
}

impl<'a> JxbNode<'a> {
    fn new(
        index: i32,
        node_data_bs: &'a [JxbNodeDataB],
        utf8_strings: &'a BTreeMap<i32, String>,
        utf16_strings: &'a BTreeMap<i32, String>,
    ) -> std::io::Result<JxbNode<'a>> {
        let b = &node_data_bs[index as usize];
        Ok(JxbNode {
            node_type: get_string(b.node_type_offset, utf8_strings)?,
            tags: b
                .tags
                .iter()
                .map(|b_tag| {
                    Ok((
                        get_string(b_tag.key_offset, utf8_strings)?,
                        JxbValue::new(b_tag, utf8_strings)?,
                    ))
                })
                .collect::<std::io::Result<_>>()?,
            text: get_string(b.text_offset, utf16_strings)?,
            children: (b.children_start_index..b.children_start_index + b.child_count)
                .map(|child_index| {
                    JxbNode::new(child_index, node_data_bs, utf8_strings, utf16_strings)
                })
                .collect::<std::io::Result<_>>()?,
        })
    }

    async fn write_xml<W>(&self, writer: &mut Writer<W>) -> quick_xml::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let element_writer = writer.create_element(self.node_type).with_attributes(
            self.tags
                .iter()
                .map(|(key, value)| (*key, value.to_string())),
        );
        if !self.text.is_empty() {
            element_writer
                .write_text_content_async(BytesText::new(self.text))
                .await?;
            return Ok(());
        }
        if !self.children.is_empty() {
            Box::pin(
                element_writer.write_inner_content_async::<_, _, quick_xml::Error>(
                    |writer| async {
                        for child_node in &self.children {
                            child_node.write_xml(writer).await?;
                        }
                        Ok(writer)
                    },
                ),
            )
            .await?;
            return Ok(());
        }
        element_writer.write_empty_async().await?;
        Ok(())
    }
}

#[derive(Debug)]
enum JxbValue<'a> {
    Text(&'a str),
    Float(f32),
    Int(i32),
    Bool(bool),
}

fn get_string(offset: i32, strings: &BTreeMap<i32, String>) -> std::io::Result<&str> {
    match strings.get(&offset) {
        Some(value) => Ok(value),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Could not find string at {:#X}", offset),
        )),
    }
}

impl<'a> JxbValue<'a> {
    fn new(b_tag: &JxbTag, strings: &'a BTreeMap<i32, String>) -> std::io::Result<JxbValue<'a>> {
        Ok(match b_tag.type_id {
            3 => JxbValue::Text(get_string(b_tag.value, strings)?),
            4 => JxbValue::Float(f32::from_le_bytes(b_tag.value.to_le_bytes())),
            5 => JxbValue::Int(b_tag.value),
            6 => JxbValue::Bool(match b_tag.value {
                0 => false,
                1 => true,
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Invalid boolean value {:#X}!", b_tag.value),
                    ));
                }
            }),
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "Invalid tag type! type_id: {:#X}, value: {:#X}",
                        b_tag.type_id, b_tag.value
                    ),
                ));
            }
        })
    }

    fn to_string(&'a self) -> Cow<'a, str> {
        match self {
            JxbValue::Text(text) => Cow::Borrowed(*text),
            JxbValue::Float(value) => Cow::Owned(value.to_string()),
            JxbValue::Int(value) => Cow::Owned(value.to_string()),
            JxbValue::Bool(value) => Cow::Owned(value.to_string()),
        }
    }
}
