use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    io::Cursor,
    iter::once,
    path::Path,
};

use binrw::{
    BinRead, BinResult, binread, binwrite,
    helpers::{args_iter, until, until_exclusive},
};
use indexmap::IndexMap;
use quick_xml::{Writer, events::BytesText};
use tokio::{
    fs::{File, create_dir, metadata, read_dir},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite},
    task::JoinSet,
};

use crate::size::get_size;

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

#[binread]
#[binwrite]
#[brw(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct Jxb {
    // some fields have an alignment of 0x10, so we align the whole struct
    #[brw(align_before = 0x10)]
    // record stream position at start of read
    #[br(temp, try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    start_pos: i32,

    #[brw(magic = b"JXB\0\x01\x00\x01")]
    #[br(temp)]
    #[br(assert(uses_utf16 == 1 || uses_utf16 == 2))]
    #[bw(calc(match strings {
        JxbStrings::Utf8Only{..} => 1,
        JxbStrings::Utf8AndUtf16{..} => 2,
    }))]
    uses_utf16: u8,
    #[br(temp)]
    #[br(assert(node_count != 0))]
    #[bw(calc(node_data_bs.len() as u32))]
    node_count: u32,
    #[br(temp)]
    #[bw(calc(key_string_offsets.len() as u32))]
    key_string_count: u32,

    // offsets
    #[br(temp)]
    #[bw(calc(
        (0x30 + get_size(node_data_as))
        .next_multiple_of(0x10) as i32
    ))]
    b_region_offset: i32,
    #[br(temp, calc(start_pos + b_region_offset))]
    #[bw(ignore)]
    b_region_pos: i32,

    #[br(temp)]
    #[bw(calc(
        (b_region_offset as usize + get_size(node_data_bs))
        .next_multiple_of(0x10) as i32
    ))]
    key_string_offset_region_offset: i32,
    #[br(temp, calc(start_pos + key_string_offset_region_offset))]
    #[bw(ignore)]
    key_string_offset_region_pos: i32,

    #[brw(magic = b"\0\0\0\0")]
    #[br(temp)]
    #[bw(calc(
        (key_string_offset_region_offset as usize + get_size(key_string_offsets))
        .next_multiple_of(0x10) as i32
    ))]
    string_region_offset: i32,
    #[br(temp, calc(start_pos + string_region_offset))]
    #[bw(ignore)]
    string_region_pos: i32,

    #[brw(magic = b"\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0")] // 0x10 zero bytes
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

    #[brw(align_after = 0x10)]
    #[br(temp, try_calc(
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
    #[bw(ignore)]
    assertion1: (),

    #[br(args { count: key_string_count as usize })]
    #[brw(align_after = 0x10)]
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

    #[br(temp, calc(node_data_bs.iter().map(|b| b.text_offset).min().unwrap()))]
    #[br(assert(
        node_data_bs.iter().all(
            |b| b.child_count == 0 || b.text_offset == node_text_offset_min
        ),
        "Some node has both text content and child nodes!"
    ))]
    #[bw(ignore)]
    node_text_offset_min: i32,
    #[br(temp, calc(node_data_bs.iter().map(|b| b.text_offset).max().unwrap()))]
    #[br(assert(
        if uses_utf16 == 1 { node_text_offset_min == node_text_offset_max } else { true }
    ))]
    #[bw(ignore)]
    node_text_offset_max: i32,

    #[brw(align_before = 0x10)]
    #[br(args(uses_utf16, string_region_pos, node_text_offset_max))]
    strings: JxbStrings,
}

#[binread]
#[binwrite]
#[brw(little)]
#[derive(Debug)]
struct JxbNodeDataA {
    #[brw(magic = b"\x03\0")]
    tags_type_id: u16,
    tag_count: u32,
    b_offset: i32,
    parent_index: i32,
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(stream = reader)]
#[br(import(expected_offset: i32, tags_type_id: u16, tag_count: u32))]
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

    #[br(args { count: tag_count as usize, inner: (tags_type_id,) })]
    #[bw(args (
        if tags.is_empty() {
            0
        } else if tags.windows(2).all(|window| window[0].type_id == window[1].type_id) {
            tags[0].type_id as u16
        } else {
            1
        }
    ))]
    tags: Vec<JxbTag>,

    #[br(temp, try_calc(
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
    #[bw(ignore)]
    assertion1: (),
}

#[binread]
#[binwrite]
#[brw(little)]
#[brw(import(tags_type_id: u16))]
#[derive(Debug)]
struct JxbTag {
    key_offset: i32,
    #[br(if(tags_type_id == 1, tags_type_id as u32))]
    #[bw(if(tags_type_id == 1))]
    type_id: u32,
    value: i32,
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(import(uses_utf16: u8, string_region_pos: i32, node_text_offset_max: i32))]
#[derive(Debug)]
enum JxbStrings {
    #[br(assert(uses_utf16 == 1))]
    Utf8Only {
        #[br(temp)]
        #[br(parse_with = until(
            |string: &JxbUtf8String| string.utf8_values.len() == 1 // empty string is 1 byte
        ))]
        #[bw(calc(strings.iter().map(
            |(_, text)| JxbUtf8String{ pos: <_>::default(), utf8_values: text.bytes().chain(once(0)).collect() }
        ).collect()))]
        strings_vec: Vec<JxbUtf8String>,

        #[br(try_calc(strings_vec.into_iter().map(|string| Ok((
            string.pos - string_region_pos,
            str::from_utf8(&string.utf8_values[..string.utf8_values.len() - 1])?.to_string()
        ))).collect::<Result<_,std::str::Utf8Error>>()))]
        #[bw(ignore)]
        strings: BTreeMap<i32, String>,
    },
    #[br(assert(uses_utf16 == 2))]
    Utf8AndUtf16 {
        #[br(temp)]
        #[br(parse_with = until_exclusive(
            |string: &JxbUtf8String| string.utf8_values.len() == 1 // empty string is 1 byte
        ))]
        #[br(pad_after(-1))]
        #[bw(calc(utf8_strings.iter().map(|(_, text)|
            JxbUtf8String{
                pos: <_>::default(),
                utf8_values: text.bytes().chain(once(0)).collect()
            }
        ).collect()))]
        utf8_strings_vec: Vec<JxbUtf8String>,

        #[br(try_calc(utf8_strings_vec.into_iter().map(|string| Ok((
            string.pos - string_region_pos,
            str::from_utf8(&string.utf8_values[..string.utf8_values.len() - 1])?.to_string()
        ))).collect::<Result<_,std::str::Utf8Error>>()))]
        #[bw(ignore)]
        utf8_strings: BTreeMap<i32, String>,

        #[br(temp)]
        #[br(parse_with = until(
            |string: &JxbUtf16String| string.pos >= string_region_pos + node_text_offset_max
        ))]
        #[bw(calc(utf16_strings.iter().map(|(_, text)|
            JxbUtf16String{
                pos: <_>::default(),
                utf16_values: text.encode_utf16().chain(once(0)).collect()
            }
        ).collect()))]
        utf16_strings_vec: Vec<JxbUtf16String>,

        #[br(try_calc(utf16_strings_vec.into_iter().map(|string| Ok((
            string.pos - string_region_pos,
            String::from_utf16(&string.utf16_values[..string.utf16_values.len() - 1])?
        ))).collect::<Result<_,std::string::FromUtf16Error>>()))]
        #[br(assert(
            utf16_strings.first_entry().is_none_or(|entry|entry.get().is_empty()),
            "the first utf16 string is not the empty string, it is {}",
            utf16_strings.first_key_value().unwrap().1,
        ))]
        #[bw(ignore)]
        utf16_strings: BTreeMap<i32, String>,
    },
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbUtf8String {
    // store current stream position when reading starts
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    pos: i32,

    #[br(parse_with = until(|&value| value == 0))]
    utf8_values: Vec<u8>,
}

#[binread]
#[binwrite]
#[brw(little)]
#[brw(stream = reader)]
#[derive(Debug)]
struct JxbUtf16String {
    // store current stream position when reading starts
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    pos: i32,

    #[br(parse_with = until(|&value| value == 0))]
    utf16_values: Vec<u16>,
}

impl<'a> Jxb {
    fn get_node(&'a self, index: i32) -> std::io::Result<JxbNode<'a>> {
        JxbNode::new(index, &self.node_data_bs, &self.strings)
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
        strings: &'a JxbStrings,
    ) -> std::io::Result<JxbNode<'a>> {
        let b = &node_data_bs[index as usize];
        let (key_source, value_source, node_text_source);
        match strings {
            JxbStrings::Utf8Only { strings } => {
                key_source = strings;
                value_source = strings;
                node_text_source = strings;
            }
            JxbStrings::Utf8AndUtf16 {
                utf8_strings,
                utf16_strings,
            } => {
                key_source = utf8_strings;
                value_source = utf8_strings;
                node_text_source = utf16_strings;
            }
        }
        Ok(JxbNode {
            node_type: get_string(b.node_type_offset, key_source)?,
            tags: b
                .tags
                .iter()
                .map(|b_tag| {
                    Ok((
                        get_string(b_tag.key_offset, key_source)?,
                        JxbValue::new(b_tag, value_source)?,
                    ))
                })
                .collect::<std::io::Result<_>>()?,
            text: get_string(b.text_offset, node_text_source)?,
            children: (b.children_start_index..b.children_start_index + b.child_count)
                .map(|child_index| JxbNode::new(child_index, node_data_bs, strings))
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
