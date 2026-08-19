use std::collections::{BTreeMap, HashSet};

use binrw::{
    BinRead, BinWrite, binread, binwrite,
    helpers::{args_iter, until, until_exclusive},
};

use self::jxb_node::JxbNode;
use crate::size::get_size;

mod jxb_node;

#[binread]
#[binwrite]
#[brw(little)]
#[br(stream = reader)]
#[derive(Debug)]
pub struct Jxb {
    // some fields have an alignment of 0x10, so we align the whole struct
    #[brw(align_before = 0x10)]
    // record stream position at start of read
    #[br(temp, try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    start_pos: i32,

    #[brw(magic = b"JXB\0\x01\x00\x01")]
    #[br(temp)]
    #[br(assert(uses_utf16 == 1 || uses_utf16 == 2))]
    #[bw(calc(match strings.utf16_strings {
        None => 1,
        Some(_) => 2,
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
    #[br(args(uses_utf16, node_text_offset_min, node_text_offset_max))]
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
#[br(stream = reader)]
#[br(import(uses_utf16: u8, node_text_offset_min: i32, node_text_offset_max: i32))]
#[derive(Debug)]
struct JxbStrings {
    // store current stream position when reading starts
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    start_pos: i32,

    #[br(temp, calc(
        if uses_utf16 == 1 {
            start_pos + node_text_offset_min + 1
        } else {
            start_pos + node_text_offset_min
        }))]
    #[bw(ignore)]
    utf8_region_end_pos: i32,

    #[br(temp)]
    #[br(parse_with = until(
        |string: &JxbStringData<u8>|
    string.pos + string.string_data.len() as i32 + 1 >= utf8_region_end_pos
    ))]
    #[bw(calc(utf8_strings.iter().map(|(_, text)|
    JxbStringData{ pos: 0, string_data: text.bytes().collect() }
    ).collect()))]
    utf8_strings_vec: Vec<JxbStringData<u8>>,

    #[br(temp)]
    #[br(if(uses_utf16 == 2))]
    #[br(parse_with = until(
        |string: &JxbStringData<u16>| string.pos >= start_pos + node_text_offset_max
    ))]
    #[bw(calc(
        if let Some(utf16_strings) = utf16_strings {
            utf16_strings.iter().map(|(_, text)|
                JxbStringData{ pos: 0, string_data: text.encode_utf16().collect() }
            ).collect()
        } else {
            Vec::new()
        }
    ))]
    utf16_strings_vec: Vec<JxbStringData<u16>>,

    #[br(try_calc(
        utf8_strings_vec.into_iter().map(|string| Ok((
            string.pos - start_pos,
            String::from_utf8(string.string_data.clone())?
        ))).collect::<Result<_,std::string::FromUtf8Error>>()
    ))]
    #[bw(ignore)]
    utf8_strings: BTreeMap<i32, String>,

    #[br(if(uses_utf16 == 2))]
    #[br(try_calc(
        utf16_strings_vec
        .into_iter()
        .map(|string|
            Ok((
            string.pos - start_pos,
            String::from_utf16(&string.string_data)?
            ))
        ).collect::<Result<_,std::string::FromUtf16Error>>()
        .and_then(|utf16_strings| Ok(Some(utf16_strings)))
        ))]
    #[bw(ignore)]
    utf16_strings: Option<BTreeMap<i32, String>>,
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbStringData<T>
where
    for<'a> T: BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()> + Default + PartialEq + 'static,
{
    // store current stream position when reading starts
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    pos: i32,

    #[br(parse_with = until_exclusive(|value| *value == T::default()))]
    string_data: Vec<T>,

    // do not include the null terminator in the data
    #[br(temp, ignore)]
    #[bw(calc(T::default()))]
    null_terminator: T,
}

impl<'a> Jxb {
    pub fn get_node(&'a self, index: i32) -> std::io::Result<JxbNode<'a>> {
        JxbNode::new(index, &self.node_data_bs, &self.strings)
    }
    pub fn root_node(&'a self) -> std::io::Result<JxbNode<'a>> {
        self.get_node(0)
    }
}
