use crate::{build_tag_groups, sanitize::sanitize_exported_identifier};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tera::Tera;

pub fn register_filters(tera: &mut Tera) {
    tera.register_filter("go_string", |v: &Value, _: &HashMap<String, Value>| {
        serde_json::to_string(v)
            .map(Value::String)
            .map_err(tera::Error::json)
    });
    tera.register_filter(
        "go_validate_pagination",
        |ir: &Value, _: &HashMap<String, Value>| {
            let operations = ir["operations"].as_array().cloned().unwrap_or_default();
            let paginated: Vec<_> = operations
                .iter()
                .filter(|op| !op["attributes"][arvalez_target_core::PAGINATION].is_null())
                .collect();
            if paginated.is_empty() {
                return Ok(Value::Bool(false));
            }
            // Reject collisions rather than silently generating uncompilable Go.
            let reserved = ["Paginator", "paginationState", "paginationValue"];
            for model in ir["models"].as_array().into_iter().flatten() {
                let name = sanitize_exported_identifier(model["name"].as_str().unwrap_or(""));
                if reserved.contains(&name.as_str()) {
                    return Err(tera::Error::msg(format!(
                        "pagination helper conflicts with model {name}"
                    )));
                }
            }
            let mut methods = HashSet::new();
            for op in &operations {
                let name = sanitize_exported_identifier(op["name"].as_str().unwrap_or(""));
                methods.insert(name.clone());
                methods.insert(format!("{name}Raw"));
                methods.insert(format!("execute{name}"));
            }
            for group in build_tag_groups(&ir["operations"])
                .as_array()
                .into_iter()
                .flatten()
            {
                if let Some(name) = group["field_name"].as_str() {
                    methods.insert(name.to_owned());
                }
            }
            for op in paginated {
                let name = sanitize_exported_identifier(op["name"].as_str().unwrap_or(""));
                for suffix in ["Paginator", "Collect"] {
                    let helper = format!("{name}{suffix}");
                    if !methods.insert(helper.clone()) {
                        return Err(tera::Error::msg(format!(
                            "pagination helper name collision: {helper}"
                        )));
                    }
                }
            }
            Ok(Value::Bool(true))
        },
    );
}
