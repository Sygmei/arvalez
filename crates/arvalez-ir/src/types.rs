use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CURRENT_IR_VERSION: u32 = 1;

pub type Attributes = BTreeMap<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CoreIr {
    #[serde(default = "default_ir_version")]
    pub ir_version: u32,
    #[serde(default)]
    pub models: Vec<Model>,
    #[serde(default)]
    pub operations: Vec<Operation>,
}

impl Default for CoreIr {
    fn default() -> Self {
        Self {
            ir_version: CURRENT_IR_VERSION,
            models: Vec::new(),
            operations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub fields: Vec<Field>,
    #[serde(default)]
    pub attributes: Attributes,
    #[serde(default)]
    pub source: Option<SourceRef>,
}

impl Model {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            fields: Vec::new(),
            attributes: Attributes::default(),
            source: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Field {
    pub name: String,
    pub type_ref: TypeRef,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub attributes: Attributes,
}

impl Field {
    pub fn new(name: impl Into<String>, type_ref: TypeRef) -> Self {
        Self {
            name: name.into(),
            type_ref,
            optional: false,
            nullable: false,
            attributes: Attributes::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Operation {
    pub id: String,
    pub name: String,
    pub method: HttpMethod,
    pub path: String,
    #[serde(default)]
    pub params: Vec<Parameter>,
    #[serde(default)]
    pub request_body: Option<RequestBody>,
    #[serde(default)]
    pub responses: Vec<Response>,
    #[serde(default)]
    pub attributes: Attributes,
    #[serde(default)]
    pub source: Option<SourceRef>,
}

impl Default for Operation {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            method: HttpMethod::Get,
            path: String::new(),
            params: Vec::new(),
            request_body: None,
            responses: Vec::new(),
            attributes: Attributes::default(),
            source: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestBody {
    #[serde(default)]
    pub required: bool,
    pub media_type: String,
    #[serde(default)]
    pub type_ref: Option<TypeRef>,
    #[serde(default)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Parameter {
    pub name: String,
    pub location: ParameterLocation,
    pub type_ref: TypeRef,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Response {
    pub status: String,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub type_ref: Option<TypeRef>,
    #[serde(default)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TypeRef {
    Primitive { name: String },
    Named { name: String },
    Array { item: Box<TypeRef> },
    Map { value: Box<TypeRef> },
    Union { variants: Vec<TypeRef> },
}

impl TypeRef {
    pub fn primitive(name: impl Into<String>) -> Self {
        Self::Primitive { name: name.into() }
    }

    pub fn named(name: impl Into<String>) -> Self {
        Self::Named { name: name.into() }
    }

    pub fn array(item: TypeRef) -> Self {
        Self::Array {
            item: Box::new(item),
        }
    }

    pub fn map(value: TypeRef) -> Self {
        Self::Map {
            value: Box::new(value),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceRef {
    pub pointer: String,
    #[serde(default)]
    pub line: Option<u32>,
}

pub(crate) fn default_ir_version() -> u32 {
    CURRENT_IR_VERSION
}
