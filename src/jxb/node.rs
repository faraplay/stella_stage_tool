use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    sync::Arc,
};

use indexmap::IndexMap;
use quick_xml::{
    Reader, Writer,
    events::{BytesText, Event, attributes::Attributes},
    name::QName,
};
use tokio::io::{AsyncBufRead, AsyncWrite};

use super::{Jxb, NodeDataA, NodeDataB, StringPool, TagData, TagDatas};
use crate::size::get_size;

#[derive(Debug, Default)]
pub struct NodeData<'a> {
    node_type: Cow<'a, str>,
    tags: IndexMap<Cow<'a, str>, Value<'a>>,
    text: Cow<'a, str>,
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
        let node_type = Cow::Borrowed(match b {
            NodeDataB::Version1 { .. } => "",
            NodeDataB::Version2 { .. } => "",
            NodeDataB::Version3 {
                node_type_offset, ..
            } => get_string(*node_type_offset, key_value_strings)?,
        });
        let tags = b
            .tags()
            .iter()
            .map(|tag| {
                Ok((
                    Cow::Borrowed(get_string(tag.key_offset, key_value_strings)?),
                    Value::new(tag, key_value_strings)?,
                ))
            })
            .collect::<std::io::Result<_>>()?;
        let text = Cow::Borrowed(match b {
            NodeDataB::Version1 { .. } => "",
            NodeDataB::Version2 { .. } => "",
            NodeDataB::Version3 { text_offset, .. } => get_string(*text_offset, text_strings)?,
        });
        Ok(NodeData {
            node_type,
            tags,
            text,
        })
    }

    pub fn get_type(&self) -> &str {
        &self.node_type
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

    pub fn get_inner_text(&self) -> &str {
        &self.text
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
            children_start_index: b.children_start_index(),
            child_count: b.child_count(),
        })
    }

    pub fn inject_text(&mut self, text: Cow<'a, str>) {
        if !text.is_empty() {
            self.data.text = text;
        }
    }

    pub fn into_jxb(node_list: Vec<NodeDataWithPointers<'a>>) -> Jxb {
        let mut key_strings = BTreeSet::<Cow<'a, str>>::new();
        let mut value_strings = BTreeSet::<Cow<'a, str>>::new();
        let mut text_strings = BTreeSet::<Cow<'a, str>>::new();
        for node in &node_list {
            key_strings.insert(Cow::Borrowed(&node.data.node_type));
            for (key, value) in &node.data.tags {
                key_strings.insert(Cow::Borrowed(&key));
                if let Value::Text(text) = value {
                    value_strings.insert(Cow::Borrowed(text));
                }
            }
            text_strings.insert(Cow::Borrowed(&node.data.text));
        }
        let mut utf8_strings = BTreeMap::new();
        let mut offset = 0;
        for string in key_strings {
            utf8_strings
                .entry(offset)
                .or_insert_with(|| string.to_string());
            offset += string.as_bytes().len() as i32 + 1;
        }
        let key_string_offsets = utf8_strings.keys().copied().collect();
        for string in value_strings {
            utf8_strings
                .entry(offset)
                .or_insert_with(|| string.to_string());
            offset += string.as_bytes().len() as i32 + 1;
        }
        let mut utf16_strings = BTreeMap::new();
        for string in text_strings {
            utf16_strings
                .entry(offset)
                .or_insert_with(|| string.to_string());
            offset += (string.encode_utf16().count() as i32 + 1) * 2;
        }
        let utf8_offset_lookup = offset_lookup(&utf8_strings);
        let utf16_offset_lookup = offset_lookup(&utf16_strings);
        let mut node_data_as = Vec::new();
        let mut node_data_bs = Vec::new();
        let mut b_offset = 0;
        for node in node_list {
            let tags: TagDatas = TagDatas {
                tags: node
                    .data
                    .tags
                    .into_iter()
                    .map(|(key, value)| TagData {
                        key_offset: *utf8_offset_lookup.get(&key as &str).unwrap(),
                        type_id: value.type_id(),
                        value: value.to_i32(&utf8_offset_lookup).unwrap(),
                    })
                    .collect(),
            };
            let tags_type_id = tags.tag_type_id();
            let a: NodeDataA;
            let b: NodeDataB;
            if node.data.node_type.is_empty() && node.data.text.is_empty() {
                if tags.tags.is_empty() {
                    a = NodeDataA {
                        node_version: 2,
                        tags_type_id: 2,
                        tag_count: node.child_count as u32,
                        b_offset: node.children_start_index,
                        parent_index: node.parent_index,
                    };
                    b = NodeDataB::Version2 {
                        child_indexes: (node.children_start_index
                            ..node.children_start_index + node.child_count)
                            .collect(),
                    };
                } else {
                    a = NodeDataA {
                        node_version: 1,
                        tags_type_id,
                        tag_count: tags.tags.len() as u32,
                        b_offset,
                        parent_index: node.parent_index,
                    };
                    b = NodeDataB::Version1 { tags };
                }
            } else {
                a = NodeDataA {
                    node_version: 3,
                    tags_type_id,
                    tag_count: tags.tags.len() as u32,
                    b_offset,
                    parent_index: node.parent_index,
                };
                b = NodeDataB::Version3 {
                    node_type_offset: *utf8_offset_lookup
                        .get(&node.data.node_type as &str)
                        .unwrap(),
                    children_start_index: node.children_start_index,
                    child_count: node.child_count,
                    text_offset: *utf16_offset_lookup.get(&node.data.text as &str).unwrap(),
                    tags,
                };
            }
            b_offset += get_size(&b) as i32;
            node_data_as.push(a);
            node_data_bs.push(b);
        }
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

    pub async fn read_xml<R>(reader: &mut Reader<R>) -> quick_xml::Result<Vec<Node<'a>>>
    where
        R: AsyncBufRead + Unpin,
    {
        let mut buf = Vec::new();
        // this must always have at least 1 element
        let mut nodes_stack = Vec::new();
        // bottom element of the stack is NOT the final result
        // instead its children field is the vector of final results
        nodes_stack.push(Node {
            data: NodeData::default(),
            children: Vec::new(),
        });
        loop {
            let event = reader.read_event_into_async(&mut buf).await?;
            match event {
                Event::Empty(e) => {
                    let node = start_bytes_to_node(e)?;
                    let parent = nodes_stack.last_mut().unwrap();
                    if !parent.data.text.is_empty() {
                        return Err(quick_xml::Error::Io(Arc::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Node {:?} contains both text and child nodes", parent),
                        ))));
                    }
                    parent.children.push(node);
                }
                Event::Start(e) => {
                    let node = start_bytes_to_node(e)?;
                    let parent = nodes_stack.last_mut().unwrap();
                    if !parent.data.text.is_empty() {
                        return Err(quick_xml::Error::Io(Arc::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Node {:?} contains both text and child nodes", parent),
                        ))));
                    }
                    nodes_stack.push(node);
                }
                Event::End(e) => {
                    let name = qname_to_str(e.name())?;
                    // need to be careful and check nodes_stack still has 1 element after this!
                    let Some(node) = nodes_stack.pop() else {
                        return Err(quick_xml::Error::IllFormed(
                            quick_xml::errors::IllFormedError::UnmatchedEndTag(name.to_string()),
                        ));
                    };
                    let Some(parent) = nodes_stack.last_mut() else {
                        return Err(quick_xml::Error::IllFormed(
                            quick_xml::errors::IllFormedError::UnmatchedEndTag(name.to_string()),
                        ));
                    };
                    if node.data.node_type != name {
                        return Err(quick_xml::Error::IllFormed(
                            quick_xml::errors::IllFormedError::MismatchedEndTag {
                                expected: node.data.node_type.to_string(),
                                found: name.to_string(),
                            },
                        ));
                    }
                    parent.children.push(node);
                }
                Event::Text(e) => {
                    let string = e
                        .xml_content(quick_xml::XmlVersion::Implicit1_0)?
                        .to_string();
                    if string.chars().all(|char| char.is_whitespace()) {
                        continue;
                    }
                    let node = nodes_stack.last_mut().unwrap();
                    let text = Cow::Owned(string);
                    if !node.children.is_empty() {
                        return Err(quick_xml::Error::Io(Arc::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Node {:?} contains both text and child nodes", node),
                        ))));
                    }
                    if !node.data.text.is_empty() {
                        return Err(quick_xml::Error::Io(Arc::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("Node {:?} contains both text and child nodes", node),
                        ))));
                    }
                    node.data.text = text;
                }
                Event::Eof => break,
                _ => {}
            }
        }
        if nodes_stack.len() != 1 {
            return Err(quick_xml::Error::IllFormed(
                quick_xml::errors::IllFormedError::MissingEndTag(
                    nodes_stack.last().unwrap().data.node_type.to_string(),
                ),
            ));
        }
        Ok(nodes_stack.pop().unwrap().children)
    }

    pub async fn write_xml<W>(&self, writer: &mut Writer<W>) -> quick_xml::Result<()>
    where
        W: AsyncWrite + Unpin,
    {
        let element_writer = writer
            .create_element(&self.data.node_type as &str)
            .with_attributes(
                self.data
                    .tags
                    .iter()
                    .map(|(key, value)| (key as &str, value.to_string())),
            );
        if !self.data.text.is_empty() {
            element_writer
                .write_text_content_async(BytesText::new(&self.data.text))
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

fn start_bytes_to_node<'a>(e: quick_xml::events::BytesStart) -> Result<Node<'a>, quick_xml::Error> {
    let data = NodeData {
        node_type: qname_to_str(e.name())?,
        tags: to_tags(e.attributes())?,
        text: Cow::Borrowed(""),
    };
    let node = Node {
        data,
        children: Vec::new(),
    };
    Ok(node)
}

fn qname_to_str<'a>(qname: QName) -> std::io::Result<Cow<'a, str>> {
    match str::from_utf8(qname.into_inner()) {
        Ok(str) => Ok(Cow::Owned(str.to_string())),
        Err(error) => Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error)),
    }
}

fn to_tags<'a>(attributes: Attributes) -> quick_xml::Result<IndexMap<Cow<'a, str>, Value<'a>>> {
    attributes
        .map(|attribute| {
            let attribute = attribute?;
            let key = qname_to_str(attribute.key)?;
            let value_str = attribute
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)?
                .into_owned();
            let value = Value::from_str(Cow::Owned(value_str));
            Ok((key, value))
        })
        .collect()
}

#[derive(Debug)]
pub enum Value<'a> {
    Node(i32),
    Text(Cow<'a, str>),
    Float(f32),
    Int(i32),
    Bool(bool),
}

fn get_string(offset: i32, strings: &BTreeMap<i32, String>) -> std::io::Result<&str> {
    if offset == -1 {
        return Ok("");
    }
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
            2 => Value::Node(b_tag.value),
            3 => Value::Text(Cow::Borrowed(get_string(b_tag.value, strings)?)),
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

    fn from_str(string: Cow<'a, str>) -> Self {
        if string == "true" {
            return Value::Bool(true);
        }
        if string == "false" {
            return Value::Bool(false);
        }
        if let Some(suffix) = string.strip_prefix("NODE")
            && let Ok(value) = suffix.parse::<i32>()
        {
            return Value::Node(value);
        }
        if let Ok(value) = string.parse::<i32>() {
            return Value::Int(value);
        }
        if let Ok(value) = string.parse::<f32>() {
            return Value::Float(value);
        }
        return Value::Text(string);
    }

    fn to_i32(&self, offset_lookup: &HashMap<&str, i32>) -> Option<i32> {
        match self {
            Value::Node(value) => Some(*value),
            Value::Text(text) => offset_lookup.get(text as &str).copied(),
            Value::Float(value) => Some(i32::from_le_bytes(value.to_le_bytes())),
            Value::Int(value) => Some(*value),
            Value::Bool(value) => Some(if *value { 1 } else { 0 }),
        }
    }

    fn type_id(&self) -> u32 {
        match self {
            Value::Node(_) => 2,
            Value::Text(_) => 3,
            Value::Float(_) => 4,
            Value::Int(_) => 5,
            Value::Bool(_) => 6,
        }
    }

    fn to_string(&'a self) -> Cow<'a, str> {
        match self {
            Value::Node(value) => Cow::Owned(format!("NODE{value}")),
            Value::Text(text) => Cow::Borrowed(text),
            Value::Float(value) => Cow::Owned(format!("{value:.6}")),
            Value::Int(value) => Cow::Owned(value.to_string()),
            Value::Bool(value) => Cow::Owned(value.to_string()),
        }
    }
}
