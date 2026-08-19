use std::{borrow::Cow, collections::BTreeMap};

use indexmap::IndexMap;
use quick_xml::{Writer, events::BytesText};
use tokio::io::AsyncWrite;

use super::JxbTag;
use crate::extract::jxb::{JxbNodeDataB, JxbStrings};

#[derive(Debug)]
pub struct JxbNode<'a> {
    node_type: &'a str,
    tags: IndexMap<&'a str, JxbValue<'a>>,
    text: &'a str,
    children: Vec<JxbNode<'a>>,
}

impl<'a> JxbNode<'a> {
    pub(super) fn new(
        index: i32,
        node_data_bs: &[JxbNodeDataB],
        strings: &'a JxbStrings,
    ) -> std::io::Result<JxbNode<'a>> {
        let b = &node_data_bs[index as usize];
        let text_strings = strings
            .utf16_strings
            .as_ref()
            .unwrap_or(&strings.utf8_strings);
        Ok(JxbNode {
            node_type: get_string(b.node_type_offset, &strings.utf8_strings)?,
            tags: b
                .tags
                .iter()
                .map(|tag| {
                    Ok((
                        get_string(tag.key_offset, &strings.utf8_strings)?,
                        JxbValue::new(tag, &strings.utf8_strings)?,
                    ))
                })
                .collect::<std::io::Result<_>>()?,
            text: get_string(b.text_offset, text_strings)?,
            children: (b.children_start_index..b.children_start_index + b.child_count)
                .map(|child_index| JxbNode::new(child_index, node_data_bs, strings))
                .collect::<std::io::Result<_>>()?,
        })
    }

    pub fn get_type(&self) -> &str {
        self.node_type
    }

    pub fn get_text_tag(&self, key: &str) -> std::io::Result<&str> {
        let JxbValue::Text(value) = self.tags.get(key).ok_or(std::io::Error::new(
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

    pub async fn write_xml<W>(&self, writer: &mut Writer<W>) -> quick_xml::Result<()>
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
pub enum JxbValue<'a> {
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
    fn new(
        b_tag: &JxbTag,
        strings: &'a BTreeMap<i32, String>,
    ) -> std::io::Result<JxbValue<'a>> {
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
