mod types;
mod validation;

pub use types::{
    Attributes, CoreIr, Field, HttpMethod, Model, Operation, Parameter, ParameterLocation,
    RequestBody, Response, SourceRef, TypeRef, CURRENT_IR_VERSION,
};
pub use validation::{ValidationErrors, ValidationIssue, validate_ir};
