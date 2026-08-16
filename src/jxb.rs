use std::{
    collections::{BTreeMap, HashSet},
    io::Cursor,
    path::Path,
};

use binrw::{
    BinRead, binread,
    helpers::{args_iter, until_exclusive, until_with},
};
use indexmap::IndexMap;
use tokio::{fs::File, io::AsyncReadExt};

pub async fn check_file(in_path: &Path) -> std::io::Result<()> {
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);

    let jxb = Jxb::read(&mut cursor).expect("Jxb parsing failure");
    let node = jxb.root_node()?;
    eprintln!("Parsed {}", in_path.display());
    println!("{node:#X?}");

    Ok(())
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
    #[br(magic = b"JXB\0")]
    unknown_0x4: u32,
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
    unknown_0x18: u32,
    #[br(temp)]
    #[br(map(|relative_offset: i32| start_pos + relative_offset))]
    string_region_pos: i32,
    unknown_0x20: u32,
    unknown_0x24: u32,
    unknown_0x28: u32,
    unknown_0x2c: u32,

    #[br(args { count: node_count as usize })]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == b_region_pos as u64),
        "incorrect stream position for b_region_pos, expected {:X}",
        b_region_pos
    ))]
    node_data_as: Vec<JxbNodeDataA>,

    #[br(parse_with = args_iter(
        node_data_as.iter().map(
            |a| (b_region_pos + a.b_offset, a.tag_version, a.tag_count)
        )
    ))]
    #[br(align_after = 0x10)]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == key_string_offset_region_pos as u64),
        "incorrect stream position for key_string_offset_region_pos, expected {:X}",
        key_string_offset_region_pos
    ))]
    node_data_bs: Vec<JxbNodeDataB>,

    #[br(args { count: key_string_count as usize })]
    #[br(align_after = 0x10)]
    #[br(assert(
        reader.stream_position().map_or(false, |pos| pos == string_region_pos as u64),
        "incorrect stream position for string_region_pos, expected {:X}",
        string_region_pos
    ))]
    key_string_offsets: Vec<u32>,

    #[br(temp)]
    #[br(calc = node_data_bs
        .iter()
        .flat_map(
            |b|
            std::iter::once(b.node_type_offset)
            .chain(b.tags.iter().map(|tag|tag.utf8_offset()))
        )
        .max()
        .unwrap_or(0)
        )]
    utf8_max_offset: i32,
    #[br(parse_with = until_with(
        |(offset, _): &(i32, String)| *offset >= utf8_max_offset,
        |reader, options, _: ()| {
            let string = JxbUtf8String::read_options(reader, options, ())?;
            Ok((string.pos - string_region_pos, string.text))
        }
    ))]
    utf8_strings: BTreeMap<i32, String>,

    #[br(temp)]
    #[br(calc = node_data_bs.iter().map(|jxb_b| jxb_b.text_offset).max().unwrap_or(0))]
    utf16_max_offset: i32,
    #[br(parse_with = until_with(
        |(offset, _): &(i32, String)| *offset >= utf16_max_offset,
        |reader, options, _: ()| {
            let string = JxbUtf16String::read_options(reader, options, ())?;
            Ok((string.pos - string_region_pos, string.text))
        }
    ))]
    utf16_strings: BTreeMap<i32, String>,
}

#[binread]
#[br(little)]
#[derive(Debug)]
struct JxbNodeDataA {
    #[br(temp)]
    #[br(assert(unknown_0x0 == 3))]
    unknown_0x0: u16,
    tag_version: u16,
    tag_count: u32,
    b_offset: i32,
    parent_index: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[br(import(expected_offset: i32, tag_version: u16, extra_count: u32))]
#[br(pre_assert(
    reader.stream_position().map_or(false, |pos| pos == expected_offset as u64),
    "incorrect stream position for NodeDataB item, expected {:X}",
    expected_offset
))]
#[derive(Debug)]
struct JxbNodeDataB {
    node_type_offset: i32,
    first_child_index: i32,
    child_count: i32,
    text_offset: i32,

    #[br(args { count: extra_count as usize, inner: (tag_version,) })]
    #[br(assert(match tag_version {
        0 => tags.is_empty(),
        1 => tags.iter().any(|tag| tag.type_id != 3),
        3 => !tags.is_empty(),
        _ => false,
    }))]
    #[br(assert({
        let mut keys = HashSet::new();
        tags.iter().all(|tag| keys.insert(tag.key_offset))
    }))]
    tags: Vec<JxbTag>,
}

#[binread]
#[br(little)]
#[br(import(tag_version: u16))]
#[derive(Debug)]
struct JxbTag {
    key_offset: i32,
    #[br(if(tag_version != 3, 3))]
    type_id: u32,
    value: i32,
}

impl JxbTag {
    fn utf8_offset(&self) -> i32 {
        if self.type_id == 3 {
            std::cmp::max(self.key_offset, self.value)
        } else {
            self.key_offset
        }
    }
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
    fn root_node(&'a self) -> std::io::Result<JxbNode<'a>> {
        JxbNode::new(
            0,
            -1,
            &self.node_data_as,
            &self.node_data_bs,
            &self.utf8_strings,
            &self.utf16_strings,
        )
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
        parent_index: i32,
        node_data_as: &'a [JxbNodeDataA],
        node_data_bs: &'a [JxbNodeDataB],
        utf8_strings: &'a BTreeMap<i32, String>,
        utf16_strings: &'a BTreeMap<i32, String>,
    ) -> std::io::Result<JxbNode<'a>> {
        let a = &node_data_as[index as usize];
        let b = &node_data_bs[index as usize];
        if a.parent_index != parent_index {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Incorrect parent_index on node {:#X}! Expected {:#X}, value on node {:#X}",
                    index, parent_index, a.parent_index
                ),
            ));
        }
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
            children: (b.first_child_index..b.first_child_index + b.child_count)
                .map(|child_index| {
                    JxbNode::new(
                        child_index,
                        index,
                        node_data_as,
                        node_data_bs,
                        utf8_strings,
                        utf16_strings,
                    )
                })
                .collect::<std::io::Result<_>>()?,
        })
    }
}

#[derive(Debug)]
enum JxbValue<'a> {
    Text(&'a str),
    Float(f32),
    Int(i32),
    Bool(bool),
    Other { type_id: u32, value: i32 },
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
            _ => JxbValue::Other {
                type_id: b_tag.type_id,
                value: b_tag.value,
            },
        })
    }
}
