use std::{borrow::Cow, collections::BTreeMap};

use indexmap::IndexMap;
use quick_xml::{Writer, events::BytesText};
use tokio::io::AsyncWrite;

use super::TagData;
use crate::extract::jxb::{NodeDataA, NodeDataB, StringPool};

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
}

impl<'a> Node<'a> {
    pub fn new(mut node_list: Vec<NodeDataWithPointers<'a>>, index: i32) -> Node<'a> {
        Node::from_node_list(&mut node_list, index)
    }

    fn from_node_list(node_list: &mut [NodeDataWithPointers<'a>], index: i32) -> Node<'a> {
        let node = &mut node_list[index as usize];
        Node {
            data: std::mem::take(&mut node.data),
            children: (node.children_start_index..node.children_start_index + node.child_count)
                .map(|child_index| Node::from_node_list(node_list, child_index))
                .collect(),
        }
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

    fn to_string(&'a self) -> Cow<'a, str> {
        match self {
            Value::Text(text) => Cow::Borrowed(*text),
            Value::Float(value) => Cow::Owned(value.to_string()),
            Value::Int(value) => Cow::Owned(value.to_string()),
            Value::Bool(value) => Cow::Owned(value.to_string()),
        }
    }
}
