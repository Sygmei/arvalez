use std::collections::BTreeSet;

use thiserror::Error;

use crate::{CoreIr, TypeRef, CURRENT_IR_VERSION};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

#[derive(Debug, Error)]
#[error("IR validation failed with {} issue(s)", .0.len())]
pub struct ValidationErrors(pub Vec<ValidationIssue>);

pub fn validate_ir(ir: &CoreIr) -> Result<(), ValidationErrors> {
    let mut issues = Vec::new();

    if ir.ir_version != CURRENT_IR_VERSION {
        issues.push(ValidationIssue {
            path: "ir_version".into(),
            message: format!(
                "unsupported IR version {0}, expected {1}",
                ir.ir_version, CURRENT_IR_VERSION
            ),
        });
    }

    let mut model_names = BTreeSet::new();
    for (index, model) in ir.models.iter().enumerate() {
        let path = format!("models[{index}]");
        if model.id.trim().is_empty() {
            issues.push(issue(&format!("{path}.id"), "model id must not be empty"));
        }
        if model.name.trim().is_empty() {
            issues.push(issue(
                &format!("{path}.name"),
                "model name must not be empty",
            ));
        }
        if !model_names.insert(model.name.clone()) {
            issues.push(issue(
                &format!("{path}.name"),
                format!("duplicate model name `{}`", model.name),
            ));
        }

        let mut field_names = BTreeSet::new();
        for (field_index, field) in model.fields.iter().enumerate() {
            let field_path = format!("{path}.fields[{field_index}]");
            if field.name.trim().is_empty() {
                issues.push(issue(
                    &format!("{field_path}.name"),
                    "field name must not be empty",
                ));
            }
            if !field_names.insert(field.name.clone()) {
                issues.push(issue(
                    &format!("{field_path}.name"),
                    format!("duplicate field name `{}`", field.name),
                ));
            }
            validate_type_ref(
                &field.type_ref,
                &format!("{field_path}.type_ref"),
                &mut issues,
            );
        }
    }

    let mut operation_names = BTreeSet::new();
    for (index, operation) in ir.operations.iter().enumerate() {
        let path = format!("operations[{index}]");
        if operation.id.trim().is_empty() {
            issues.push(issue(
                &format!("{path}.id"),
                "operation id must not be empty",
            ));
        }
        if operation.name.trim().is_empty() {
            issues.push(issue(
                &format!("{path}.name"),
                "operation name must not be empty",
            ));
        }
        if !operation_names.insert(operation.name.clone()) {
            issues.push(issue(
                &format!("{path}.name"),
                format!("duplicate operation name `{}`", operation.name),
            ));
        }
        if operation.path.trim().is_empty() {
            issues.push(issue(
                &format!("{path}.path"),
                "operation path must not be empty",
            ));
        }

        for (param_index, param) in operation.params.iter().enumerate() {
            let param_path = format!("{path}.params[{param_index}]");
            if param.name.trim().is_empty() {
                issues.push(issue(
                    &format!("{param_path}.name"),
                    "parameter name must not be empty",
                ));
            }
            validate_type_ref(
                &param.type_ref,
                &format!("{param_path}.type_ref"),
                &mut issues,
            );
        }

        for (response_index, response) in operation.responses.iter().enumerate() {
            let response_path = format!("{path}.responses[{response_index}]");
            if response.status.trim().is_empty() {
                issues.push(issue(
                    &format!("{response_path}.status"),
                    "response status must not be empty",
                ));
            }
            if let Some(type_ref) = &response.type_ref {
                validate_type_ref(type_ref, &format!("{response_path}.type_ref"), &mut issues);
            }
        }
    }

    if issues.is_empty() {
        Ok(())
    } else {
        Err(ValidationErrors(issues))
    }
}

fn validate_type_ref(type_ref: &TypeRef, path: &str, issues: &mut Vec<ValidationIssue>) {
    match type_ref {
        TypeRef::Primitive { name } | TypeRef::Named { name } => {
            if name.trim().is_empty() {
                issues.push(issue(path, "type name must not be empty"));
            }
        }
        TypeRef::Array { item } => validate_type_ref(item, &format!("{path}.item"), issues),
        TypeRef::Map { value } => validate_type_ref(value, &format!("{path}.value"), issues),
        TypeRef::Union { variants } => {
            if variants.is_empty() {
                issues.push(issue(path, "union variants must not be empty"));
            }
            for (index, variant) in variants.iter().enumerate() {
                validate_type_ref(variant, &format!("{path}.variants[{index}]"), issues);
            }
        }
    }
}

fn issue(path: &str, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::Model;

    use super::*;

    #[test]
    fn rejects_duplicate_model_names() {
        let ir = CoreIr {
            models: vec![Model::new("a", "Money"), Model::new("b", "Money")],
            ..Default::default()
        };

        let errors = validate_ir(&ir).expect_err("expected duplicate model name error");
        assert_eq!(errors.0.len(), 1);
    }
}
