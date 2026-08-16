use std::{collections::BTreeMap, io::Cursor, path::Path};

use binrw::{
    BinRead, binread, helpers::{args_iter, until_exclusive, until_with},
};
use tokio::{fs::File, io::AsyncReadExt};

pub async fn check_file(in_path: &Path) -> std::io::Result<()> {
    let mut reader = File::open(in_path).await?;
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).await?;
    let mut cursor = Cursor::new(buffer);

    let jxb = Jxb::read(&mut cursor).expect("Jxb parsing failure");
    eprintln!("Parsed jxb!");
    println!("{jxb:#X?}");

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
    b_region_offset: u32,
    #[br(temp)]
    c_region_offset: u32,
    unknown_0x18: u32,
    #[br(temp)]
    d_region_offset: u32,
    unknown_0x20: u32,
    unknown_0x24: u32,
    unknown_0x28: u32,
    unknown_0x2c: u32,

    #[br(temp)]
    #[br(args { count: a_count as usize })]
    #[br(assert(reader.stream_position().map_or(false, |pos| pos == b_region_offset.into())))]
    record_as: Vec<JxbA>,

    #[br(temp)]
    #[br(parse_with = args_iter(
        record_as.iter().map(
            |record| (b_region_offset + record.b_offset, record.tag_version, record.b_extra_count)
        )
    ))]
    #[br(align_after = 0x10)]
    #[br(assert(reader.stream_position().map_or(false, |pos| pos == c_region_offset.into())))]
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
        .unwrap()
        )]
    d_ascii_max_offset: u32,
    #[br(parse_with = until_with(
        |(offset, _): &(u32, String)| *offset >= d_ascii_max_offset,
        |reader, options, _: ()| {
            let jxb_d = JxbDUtf8::read_options(reader, options, ())?;
            Ok((jxb_d.offset - d_region_offset, jxb_d.text))
        }
    ))]
    d_utf8s: BTreeMap<u32, String>,
    #[br(temp)]
    #[br(calc = bs.iter().map(|jxb_b| jxb_b.d_utf16_offset).max().unwrap())]
    d_utf16_max_offset: u32,
    #[br(parse_with = until_with(
        |(offset, _): &(u32, String)| *offset >= d_utf16_max_offset,
        |reader, options, _: ()| {
            let jxb_d = JxbDUtf16::read_options(reader, options, ())?;
            Ok((jxb_d.offset - d_region_offset, jxb_d.text))
        }
    ))]
    d_utf16s: BTreeMap<u32, String>,

    #[br(calc = std::iter::zip(record_as, bs).collect())]
    records: Vec<(JxbA, JxbB)>,
}

#[binread]
#[br(little)]
#[derive(Debug)]
struct JxbA {
    unknown_0x0: u16,
    tag_version: u16,
    b_extra_count: u32,
    b_offset: u32,
    parent_index: i32,
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[br(import(offset: u32, version: u16, extra_count: u32))]
#[br(pre_assert(
    reader.stream_position().map_or(false, |pos| pos == offset.into()),
    "incorrect stream position, expected {:X}",
    offset
))]
#[derive(Debug)]
struct JxbB {
    node_type_utf8_offset: u32,
    first_child_index: i32,
    child_count: u32,
    d_utf16_offset: u32,
    #[br(args { count: extra_count as usize, inner: (version,) })]
    tags: Vec<JxbBTag>,
}

#[binread]
#[br(little)]
#[br(import(version: u16))]
#[derive(Debug)]
enum JxbBTag {
    #[br(assert(version == 1))]
    Type1 {
        type_utf8_offset: u32,
        type_id: u32,
        value: u32,
    },
    #[br(assert(version == 3))]
    Type3 {
        type_utf8_offset: u32,
        value_utf8_offset: u32,
    },
}

impl JxbBTag {
    fn utf8_offset(&self) -> u32 {
        match self {
            JxbBTag::Type1 { type_utf8_offset, type_id, value } => {
                if *type_id == 3 {
                    std::cmp::max(*type_utf8_offset, *value)
                } else {
                    *type_utf8_offset
                }
            },
            JxbBTag::Type3 { type_utf8_offset, value_utf8_offset } => 
                std::cmp::max(*type_utf8_offset, *value_utf8_offset),
        }
    }
}

#[binread]
#[br(little)]
#[br(stream = reader)]
#[derive(Debug)]
struct JxbDUtf8 {
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as u32))))]
    offset: u32,
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
    #[br(try_calc(reader.stream_position().and_then(|pos| Ok(pos as u32))))]
    offset: u32,
    #[br(temp)]
    #[br(parse_with = until_exclusive(|&value| value == 0))]
    utf16_values: Vec<u16>,
    #[br(try_calc(String::from_utf16(&utf16_values)))]
    text: String,
}
