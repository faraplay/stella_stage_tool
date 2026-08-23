use std::{collections::BTreeMap, iter::zip};

use binrw::{
    BinRead, BinWrite, binread, binwrite,
    helpers::{args_iter, until, until_exclusive},
};

use self::node::{Node, NodeData, NodeDataWithPointers};
use crate::size::get_size;

mod node;

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
    #[bw(calc(match string_pool.utf16_strings {
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
    node_data_as: Vec<NodeDataA>,

    #[br(parse_with = args_iter(
        node_data_as.iter().map(
            |a| (a.node_version, b_region_pos + a.b_offset, a.tags_type_id, a.tag_count)
        )
    ))]
    node_data_bs: Vec<NodeDataB>,

    #[brw(align_after = 0x10)]
    #[br(temp, try_calc(
        {
            let mut child_index = 1;
            node_data_bs.iter().enumerate().map(|(parent_index, b)| {
                if let NodeDataB::Version3{child_count, children_start_index, ..} = b {
                    if *child_count == 0 {
                        if *children_start_index != -1 {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("node {:X} has no children but children_start_index is not -1", parent_index),
                            ));
                        } else {
                            return Ok(());
                        }
                    }
                    if *children_start_index != child_index {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!(
                                "node {:X} has children_start_index {:X}, expected {:X}",
                                parent_index,
                                *children_start_index,
                                child_index,
                            ),
                        ));
                    }
                    child_index = *children_start_index + *child_count;
                    for index in *children_start_index..*children_start_index + *child_count {
                        if node_data_as[index as usize].parent_index as usize != parent_index {
                            return Err(std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("node {:X} has incorrect parent index", index),
                            ));
                        }
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
    key_string_offsets: Vec<i32>,

    #[br(temp, calc(node_data_bs.iter().flat_map(|b| {
        match b {
            NodeDataB::Version1{tags} => &tags.tags,
            NodeDataB::Version3{tags, ..} => &tags.tags,
        }
    }).flat_map(|tag| {
        std::iter::once(tag.key_offset).chain(if tag.type_id == 3 {
            Some(tag.value)
        } else {
            None
        })
    }).max().unwrap_or(0)))]
    #[bw(ignore)]
    key_value_offset_max: i32,

    #[br(temp, calc(node_data_bs.iter().flat_map(
        |b| if let NodeDataB::Version3{text_offset, ..} = b { Some(*text_offset) } else { None }
    ).min()))]
    #[bw(ignore)]
    node_text_offset_min: Option<i32>,
    #[br(temp, calc(node_data_bs.iter().flat_map(
        |b| if let NodeDataB::Version3{text_offset, ..} = b { Some(*text_offset) } else { None }
    ).max()))]
    #[br(assert(
        if uses_utf16 == 1 { node_text_offset_min == node_text_offset_max } else { true }
    ))]
    #[bw(ignore)]
    node_text_offset_max: Option<i32>,
    #[br(temp, calc(
        if let Some(node_text_offset_min) = node_text_offset_min {
            if uses_utf16 == 1 {
                string_region_pos + node_text_offset_min + 1
            } else {
                string_region_pos + node_text_offset_min
            }
        } else {
            key_value_offset_max + 1
        }
    ))]
    #[bw(ignore)]
    utf8_region_end_pos: i32,

    #[brw(align_before = 0x10)]
    #[br(args(uses_utf16, utf8_region_end_pos, node_text_offset_max.unwrap_or(0)))]
    string_pool: StringPool,
}

#[binread]
#[binwrite]
#[brw(little)]
#[derive(Debug)]
struct NodeDataA {
    node_version: u16,
    tags_type_id: u16,
    tag_count: u32,
    b_offset: i32,
    parent_index: i32,
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(import(node_version: u16, expected_offset: i32, tags_type_id: u16, tag_count: u32))]
#[derive(Debug)]
enum NodeDataB {
    #[br(assert(node_version == 1))]
    Version1 {
        #[br(args(tags_type_id, tag_count))]
        tags: TagDatas,
    },
    #[br(assert(node_version == 3))]
    Version3 {
        node_type_offset: i32,
        children_start_index: i32,
        child_count: i32,
        text_offset: i32,

        #[br(args(tags_type_id, tag_count))]
        tags: TagDatas,
    },
}

#[binread]
#[binwrite]
#[brw(little)]
#[br(import(tags_type_id: u16, tag_count: u32))]
#[derive(Debug)]
struct TagDatas {
    #[br(args { count: tag_count as usize, inner: (tags_type_id,) })]
    #[bw(args (self.tag_type_id()))]
    tags: Vec<TagData>,
}

impl TagDatas {
    fn tag_type_id(&self) -> u16 {
        if self.tags.is_empty() {
            0
        } else if self
            .tags
            .windows(2)
            .all(|window| window[0].type_id == window[1].type_id)
        {
            self.tags[0].type_id as u16
        } else {
            1
        }
    }
}

#[binread]
#[binwrite]
#[brw(little)]
#[brw(import(tags_type_id: u16))]
#[derive(Debug)]
struct TagData {
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
#[br(import(uses_utf16: u8, utf8_region_end_pos: i32, node_text_offset_max: i32))]
#[derive(Debug)]
struct StringPool {
    // store current stream position when reading starts
    #[br(temp, try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    start_pos: i32,

    #[br(temp)]
    #[br(parse_with = until(
        |string: &StringData<u8>|
    string.pos + string.data.len() as i32 + 1 >= utf8_region_end_pos
    ))]
    #[bw(calc(utf8_strings.iter().map(|(_, text)|
    StringData{ pos: 0, data: text.bytes().collect() }
    ).collect()))]
    utf8_strings_vec: Vec<StringData<u8>>,

    #[br(temp)]
    #[br(if(uses_utf16 == 2))]
    #[br(parse_with = until(
        |string: &StringData<u16>| string.pos >= start_pos + node_text_offset_max
    ))]
    #[bw(calc(
        if let Some(utf16_strings) = utf16_strings {
            utf16_strings.iter().map(|(_, text)|
                StringData{ pos: 0, data: text.encode_utf16().collect() }
            ).collect()
        } else {
            Vec::new()
        }
    ))]
    utf16_strings_vec: Vec<StringData<u16>>,

    #[br(try_calc(
        utf8_strings_vec.into_iter().map(|string| Ok((
            string.pos - start_pos,
            String::from_utf8(string.data.clone())?
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
            String::from_utf16(&string.data)?
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
struct StringData<T>
where
    for<'a> T: BinRead<Args<'a> = ()> + BinWrite<Args<'a> = ()> + Default + PartialEq + 'static,
{
    // store current stream position when reading starts
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    #[bw(ignore)]
    pos: i32,

    #[br(parse_with = until_exclusive(|value| *value == T::default()))]
    data: Vec<T>,

    // do not include the null terminator in the data
    #[br(temp, ignore)]
    #[bw(calc(T::default()))]
    null_terminator: T,
}

impl<'a> Jxb {
    pub fn node_list(&'a self) -> std::io::Result<Vec<NodeData<'a>>> {
        self.node_data_bs
            .iter()
            .map(|b| NodeData::new(b, &self.string_pool))
            .collect()
    }

    pub fn get_node_data(&'a self, index: i32) -> std::io::Result<NodeData<'a>> {
        NodeData::new(&self.node_data_bs[index as usize], &self.string_pool)
    }

    pub fn root_node(&'a self) -> std::io::Result<Node<'a>> {
        let node_list = zip(&self.node_data_as, &self.node_data_bs)
            .map(|(a, b)| NodeDataWithPointers::new(a, b, &self.string_pool))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Node::new(node_list, 0))
    }

    pub async fn from_xml<R>(reader: &mut quick_xml::Reader<R>) -> std::io::Result<Jxb>
    where
        R: tokio::io::AsyncBufRead + Unpin,
    {
        let nodes = Node::read_xml(reader)
            .await
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let Ok([node]) = TryInto::<[Node; 1]>::try_into(nodes) else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "xml file contains more than one top-level node",
            ));
        };
        let node_list = node.into_node_list();
        let jxb = NodeDataWithPointers::into_jxb(node_list);
        Ok(jxb)
    }
}
