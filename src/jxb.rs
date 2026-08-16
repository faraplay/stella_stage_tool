use std::{collections::{BTreeMap, HashSet}, io::Cursor, path::Path};

use binrw::{
    BinRead, binread, helpers::{args_iter, until_exclusive, until_with},
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
#[brw(magic = b"JXB\0")]
#[derive(Debug)]
struct Jxb {
    unknown_0x4: u32,
    #[br(temp)]
    #[br(assert(a_count != 0))]
    a_count: u32,
    #[br(temp)]
    c_count: u32,
    #[br(temp)]
    b_region_offset: i32,
    #[br(temp)]
    c_region_offset: i32,
    unknown_0x18: u32,
    #[br(temp)]
    d_region_offset: i32,
    unknown_0x20: u32,
    unknown_0x24: u32,
    unknown_0x28: u32,
    unknown_0x2c: u32,

    #[br(args { count: a_count as usize })]
    #[br(assert(reader.stream_position().map_or(false, |pos| pos == b_region_offset as u64)))]
    record_as: Vec<JxbA>,

    #[br(parse_with = args_iter(
        record_as.iter().map(
            |record| (b_region_offset + record.b_offset, record.tag_version, record.b_tag_count)
        )
    ))]
    #[br(align_after = 0x10)]
    #[br(assert(reader.stream_position().map_or(false, |pos| pos == c_region_offset as u64)))]
    bs: Vec<JxbB>,

    #[br(args { count: c_count as usize })]
    #[br(align_after = 0x10)]
    cs: Vec<u32>,

    #[br(temp)]
    #[br(calc = bs
        .iter()
        .flat_map(
            |jxb_b| 
            std::iter::once(jxb_b.node_type_utf8_offset)
            .chain(jxb_b.tags.iter().map(|tag|tag.utf8_offset()))
        )
        .max()
        .unwrap_or(0)
        )]
    d_ascii_max_offset: i32,
    #[br(parse_with = until_with(
        |(offset, _): &(i32, String)| *offset >= d_ascii_max_offset,
        |reader, options, _: ()| {
            let jxb_d = JxbDUtf8::read_options(reader, options, ())?;
            Ok((jxb_d.offset - d_region_offset, jxb_d.text))
        }
    ))]
    d_utf8s: BTreeMap<i32, String>,
    #[br(temp)]
    #[br(calc = bs.iter().map(|jxb_b| jxb_b.d_utf16_offset).max().unwrap_or(0))]
    d_utf16_max_offset: i32,
    #[br(parse_with = until_with(
        |(offset, _): &(i32, String)| *offset >= d_utf16_max_offset,
        |reader, options, _: ()| {
            let jxb_d = JxbDUtf16::read_options(reader, options, ())?;
            Ok((jxb_d.offset - d_region_offset, jxb_d.text))
        }
    ))]
    d_utf16s: BTreeMap<i32, String>,
}

#[binread]
#[br(little)]
#[derive(Debug)]
struct JxbA {
    #[br(temp)]
    #[br(assert(unknown_0x0 == 3))]
    unknown_0x0: u16,
    tag_version: u16,
    b_tag_count: u32,
    b_offset: i32,
    parent_index: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[br(import(offset: i32, tag_version: u16, extra_count: u32))]
#[br(pre_assert(
    reader.stream_position().map_or(false, |pos| pos == offset as u64),
    "incorrect stream position, expected {:X}",
    offset
))]
#[derive(Debug)]
struct JxbB {
    node_type_utf8_offset: i32,
    first_child_index: i32,
    child_count: i32,
    d_utf16_offset: i32,
    #[br(args { count: extra_count as usize, inner: (tag_version,) })]
    #[br(assert(match tag_version {
        0 => tags.is_empty(),
        1 => tags.iter().any(|tag| tag.type_id != 3),
        3 => !tags.is_empty(),
        _ => false,
    }))]
    #[br(assert({
        let mut keys = HashSet::new();
        tags.iter().all(|tag| keys.insert(tag.key_utf8_offset))
    }))]
    tags: Vec<JxbBTag>,
}

#[binread]
#[br(little)]
#[br(import(tag_version: u16))]
#[derive(Debug)]
struct JxbBTag {
    key_utf8_offset: i32,
    #[br(if(tag_version != 3, 3))]
    type_id: u32,
    value: i32,
}

impl JxbBTag {
    fn utf8_offset(&self) -> i32 {
        if self.type_id == 3 {
            std::cmp::max(self.key_utf8_offset, self.value)
        } else {
            self.key_utf8_offset
        }
    }
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbDUtf8 {
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    offset: i32,
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
struct JxbDUtf16 {
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as i32))))]
    offset: i32,
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
            &self.record_as,
            &self.bs,
            &self.d_utf8s,
            &self.d_utf16s
        )
    }
}

#[derive(Debug)]
struct JxbNode<'a> {
    node_type: &'a str,
    text: &'a str,
    tags: IndexMap<&'a str, JxbValue<'a>>,
    children: Vec<JxbNode<'a>>,
}

impl<'a> JxbNode<'a> {
    fn new(
        index: i32,
        parent_index: i32,
        record_as: &'a [JxbA],
        bs: &'a [JxbB],
        utf8_strings: &'a BTreeMap<i32, String>,
        utf16_strings: &'a BTreeMap<i32, String>,
    ) -> std::io::Result<JxbNode<'a>> {
        let a = &record_as[index as usize];
        let b = &bs[index as usize];
        if a.parent_index != parent_index {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "Incorrect parent_index on node {:#X}! Expected {:#X}, value on node {:#X}",
                    index,
                    parent_index,
                    a.parent_index
                )
            ));
        }
        Ok(JxbNode {
            node_type: get_string(b.node_type_utf8_offset, utf8_strings)?,
            text: get_string(b.d_utf16_offset, utf16_strings)?,
            tags: b.tags
                .iter()
                .map(|b_tag| Ok((
                    get_string(b_tag.key_utf8_offset, utf8_strings)?,
                    JxbValue::new(b_tag, utf8_strings)?
                )))
                .collect::<std::io::Result<_>>()?,
            children: (b.first_child_index..b.first_child_index + b.child_count).map(
                |child_index| JxbNode::new(child_index, index, record_as, bs, utf8_strings, utf16_strings)
            ).collect::<std::io::Result<_>>()?
        })
    }
}

#[derive(Debug)]
enum JxbValue<'a> {
    Text(&'a str),
    Float(f32),
    Int(i32),
    Bool(bool),
    Other {
        type_id: u32,
        value: i32,
    }
}

fn get_string(offset: i32, map: &BTreeMap<i32, String>) -> std::io::Result<&str> {
    match map.get(&offset) {
        Some(value) => Ok(value),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Could not find string at {:#X}", offset)
        ))
    }
}

impl<'a> JxbValue<'a> {
    fn new(b_tag: &JxbBTag, strings: &'a BTreeMap<i32, String>) -> std::io::Result<JxbValue<'a>> {
        Ok(match b_tag.type_id {
            3 => JxbValue::Text(get_string(b_tag.value, strings)?),
            4 => JxbValue::Float(f32::from_le_bytes(b_tag.value.to_le_bytes())),
            5 => JxbValue::Int(b_tag.value),
            6 => JxbValue::Bool(match b_tag.value {
                0 => false,
                1 => true,
                _ => return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Invalid boolean value {:#X}!", b_tag.value)
                )),
            }),
            _ => JxbValue::Other { type_id: b_tag.type_id, value: b_tag.value }
        })
    }
}