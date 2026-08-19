use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
};

use indexmap::IndexMap;
use quick_xml::{Writer, events::BytesText};
use tokio::io::AsyncWrite;

use super::{Jxb, TagData};
use crate::{
    extract::jxb::{NodeDataA, NodeDataB, StringPool},
    size::get_size,
};

#[derive(Debug, Default)]
pub struct NodeData<'a> {
    node_type: &'a str,
    tags: IndexMap<&'a str, Value<'a>>,
    text: &'a str,
}

#[derive(Debug)]
pub struct NodeDataWithPointers<'a> {
    data: NodeData<'a>,
    parent_index: i32,
    children_start_index: i32,
    child_count: i32,
}

#[derive(Debug)]
pub struct Node<'a> {
    data: NodeData<'a>,
    children: Vec<Node<'a>>,
}

impl<'a> NodeData<'a> {
    pub(super) fn new(
        b: &'a NodeDataB,
        string_pool: &'a StringPool,
    ) -> std::io::Result<NodeData<'a>> {
        let key_value_strings = &string_pool.utf8_strings;
        let text_strings = string_pool
            .utf16_strings
            .as_ref()
            .unwrap_or(key_value_strings);
        let node_type = get_string(b.node_type_offset, key_value_strings)?;
        let tags = b
            .tags
            .iter()
            .map(|tag| {
                Ok((
                    get_string(tag.key_offset, key_value_strings)?,
                    Value::new(tag, key_value_strings)?,
                ))
            })
            .collect::<std::io::Result<_>>()?;
        let text = get_string(b.text_offset, text_strings)?;
        Ok(NodeData {
            node_type,
            tags,
            text,
        })
    }

    pub fn get_type(&self) -> &str {
        self.node_type
    }

    pub fn get_text_tag(&self, key: &str) -> std::io::Result<&str> {
        let Value::Text(value) = self.tags.get(key).ok_or(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Could not find tag with key {key} on node"),
        ))?
        else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Tag with key {key} on node is not a string"),
            ));
        };
        Ok(value)
    }
}

impl<'a> NodeDataWithPointers<'a> {
    pub(super) fn new(
        a: &'a NodeDataA,
        b: &'a NodeDataB,
        string_pool: &'a StringPool,
    ) -> std::io::Result<NodeDataWithPointers<'a>> {
        Ok(NodeDataWithPointers {
            data: NodeData::new(b, string_pool)?,
            parent_index: a.parent_index,
            children_start_index: b.children_start_index,
            child_count: b.child_count,
        })
    }

    pub fn into_jxb(node_list: Vec<NodeDataWithPointers<'a>>) -> Jxb {
        let mut key_strings = BTreeSet::new();
        let mut value_strings = BTreeSet::new();
        let mut text_strings = BTreeSet::new();
        for node in &node_list {
            key_strings.insert(node.data.node_type);
            for (&key, value) in &node.data.tags {
                key_strings.insert(key);
                if let Value::Text(text) = value {
                    value_strings.insert(text);
                }
            }
            text_strings.insert(node.data.text);
        }
        let (utf8_strings, utf16_start_offset) = offsets_and_utf8_strings(
            key_strings
                .iter()
                .copied()
                .chain(value_strings.iter().map(|cow| cow as &str)),
            0,
        );
        let (utf16_strings, _) =
            offsets_and_utf16_strings(text_strings.iter().copied(), utf16_start_offset);
        let utf8_offset_lookup = offset_lookup(&utf8_strings);
        let utf16_offset_lookup = offset_lookup(&utf16_strings);
        let mut node_data_as = Vec::new();
        let mut node_data_bs = Vec::new();
        let mut b_offset = 0;
        for node in node_list {
            let tags: Vec<_> = node
                .data
                .tags
                .into_iter()
                .map(|(key, value)| TagData {
                    key_offset: *utf8_offset_lookup.get(key).unwrap(),
                    type_id: value.type_id(),
                    value: value.to_i32(&utf8_offset_lookup).unwrap(),
                })
                .collect();
            let tags_type_id = if tags.is_empty() {
                0
            } else if tags.iter().all(|tag| tag.type_id == tags[0].type_id) {
                tags[0].type_id as u16
            } else {
                1
            };
            let a = NodeDataA {
                tags_type_id,
                tag_count: tags.len() as u32,
                b_offset,
                parent_index: node.parent_index,
            };
            let b = NodeDataB {
                node_type_offset: *utf8_offset_lookup.get(node.data.node_type).unwrap(),
                children_start_index: node.children_start_index,
                child_count: node.child_count,
                text_offset: *utf16_offset_lookup.get(node.data.text).unwrap(),
                tags,
            };
            b_offset += get_size(&b) as i32;
            node_data_as.push(a);
            node_data_bs.push(b);
        }
        let key_string_offsets = key_strings
            .iter()
            .map(|key| *utf8_offset_lookup.get(key).unwrap())
            .collect();
        let string_pool = StringPool {
            utf8_strings,
            utf16_strings: Some(utf16_strings),
        };
        Jxb {
            node_data_as,
            node_data_bs,
            key_string_offsets,
            string_pool,
        }
    }
}

fn offsets_and_utf8_strings<'a>(
    strings: impl Iterator<Item = &'a str>,
    start_offset: i32,
) -> (BTreeMap<i32, String>, i32) {
    let mut map = BTreeMap::new();
    let mut offset = start_offset;
    for string in strings {
        map.entry(offset).or_insert_with(|| string.to_string());
        offset += string.as_bytes().len() as i32 + 1;
    }
    (map, offset)
}

fn offsets_and_utf16_strings<'a>(
    strings: impl Iterator<Item = &'a str>,
    start_offset: i32,
) -> (BTreeMap<i32, String>, i32) {
    let mut map = BTreeMap::new();
    let mut offset = start_offset;
    for string in strings {
        map.entry(offset).or_insert_with(|| string.to_string());
        offset += (string.encode_utf16().count() as i32 + 1) * 2;
    }
    (map, offset)
}

fn offset_lookup<'a>(strings: &'a BTreeMap<i32, String>) -> HashMap<&'a str, i32> {
    let mut map: HashMap<&'a str, i32> = HashMap::new();
    for (&offset, string) in strings {
        map.entry(string).or_insert(offset);
    }
    map
}

impl<'a> Node<'a> {
    pub fn new(mut node_list: Vec<NodeDataWithPointers<'a>>, index: i32) -> Node<'a> {
        Node::from_node_list(&mut node_list, index)
    }

    pub fn from_node_list(node_list: &mut [NodeDataWithPointers<'a>], index: i32) -> Node<'a> {
        let node = &mut node_list[index as usize];
        Node {
            data: std::mem::take(&mut node.data),
            children: (node.children_start_index..node.children_start_index + node.child_count)
                .map(|child_index| Node::from_node_list(node_list, child_index))
                .collect(),
        }
    }

    pub fn into_node_list(mut self) -> Vec<NodeDataWithPointers<'a>> {
        let mut node_list = Vec::new();
        let mut nodes_to_process = VecDeque::new();
        nodes_to_process.push_back((&mut self, -1));
        while let Some((node, parent_index)) = nodes_to_process.pop_front() {
            let index = node_list.len() as i32;
            let child_count = node.children.len() as i32;
            let children_start_index = if child_count == 0 {
                -1
            } else {
                index + nodes_to_process.len() as i32 + 1
            };
            for child in &mut node.children {
                nodes_to_process.push_back((child, index));
            }
            node_list.push(NodeDataWithPointers {
                data: std::mem::take(&mut node.data),
                parent_index,
                children_start_index,
                child_count,
            })
        }
        node_list
    }

    pub async fn write_xml<W>(&self, writer: &mut Writer<W>) -> quick_xml::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let element_writer = writer.create_element(self.data.node_type).with_attributes(
            self.data
                .tags
                .iter()
                .map(|(key, value)| (*key, value.to_string())),
        );
        if !self.data.text.is_empty() {
            element_writer
                .write_text_content_async(BytesText::new(self.data.text))
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
pub enum Value<'a> {
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

impl<'a> Value<'a> {
    fn new(b_tag: &TagData, strings: &'a BTreeMap<i32, String>) -> std::io::Result<Value<'a>> {
        Ok(match b_tag.type_id {
            3 => Value::Text(get_string(b_tag.value, strings)?),
            4 => Value::Float(f32::from_le_bytes(b_tag.value.to_le_bytes())),
            5 => Value::Int(b_tag.value),
            6 => Value::Bool(match b_tag.value {
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

    fn to_i32(&self, offset_lookup: &HashMap<&str, i32>) -> Option<i32> {
        match self {
            Value::Text(text) => offset_lookup.get(text).copied(),
            Value::Float(value) => Some(i32::from_le_bytes(value.to_le_bytes())),
            Value::Int(value) => Some(*value),
            Value::Bool(value) => Some(if *value { 1 } else { 0 }),
        }
    }

    fn type_id(&self) -> u32 {
        match self {
            Value::Text(_) => 3,
            Value::Float(_) => 4,
            Value::Int(_) => 5,
            Value::Bool(_) => 6,
        }
    }

    fn to_string(&'a self) -> Cow<'a, str> {
        match self {
            Value::Text(text) => Cow::Borrowed(*text),
            Value::Float(value) => Cow::Owned(value.to_string()),
            Value::Int(value) => Cow::Owned(value.to_string()),
            Value::Bool(value) => Cow::Owned(value.to_string()),
        }
    }
}
