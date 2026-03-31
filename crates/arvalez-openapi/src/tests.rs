use arvalez_ir::{ParameterLocation, TypeRef};
use serde_json::{Value, json};
use std::path::Path;

use crate::document::OpenApiDocument;
use crate::importer::OpenApiImporter;
use crate::source::{OpenApiSource, SourceFormat};
use crate::parse::{parse_json_openapi_document, parse_yaml_openapi_document};
use crate::diagnostic::DiagnosticKind;
use crate::LoadOpenApiOptions;

fn json_test_source(spec: &str) -> OpenApiSource {
    OpenApiSource::new(SourceFormat::Json, spec.to_owned())
}

#[test]
    fn imports_minimal_openapi_document() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets/{widget_id}": {
      "get": {
        "operationId": "get_widget",
        "parameters": [
          {
            "name": "widget_id",
            "in": "path",
            "required": true,
            "schema": { "type": "string" }
          }
        ],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Widget" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "required": ["id"],
        "properties": {
          "status": {
            "$ref": "#/components/schemas/WidgetStatus"
          },
          "id": { "type": "string" },
          "count": { "anyOf": [{ "type": "integer" }, { "type": "null" }] },
          "labels": {
            "type": "object",
            "additionalProperties": { "type": "string" }
          },
          "metadata": {
            "type": "object",
            "additionalProperties": true
          }
        }
      },
      "WidgetStatus": {
        "type": "string",
        "enum": ["READY", "PAUSED"]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("should import successfully");
        let ir = result.ir;

        assert_eq!(ir.models.len(), 2);
        assert_eq!(ir.operations.len(), 1);
        assert_eq!(ir.operations[0].name, "get_widget");
        assert!(ir.models.iter().any(|model| model.name == "Widget"));
        assert!(ir.models.iter().any(|model| model.name == "WidgetStatus"));
        let widget = ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("widget model");
        assert!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "count")
                .expect("count field")
                .nullable
        );
        assert!(matches!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "metadata")
                .expect("metadata field")
                .type_ref,
            TypeRef::Map { .. }
        ));
        assert_eq!(
            widget
                .fields
                .iter()
                .find(|field| field.name == "status")
                .expect("status field")
                .type_ref,
            TypeRef::named("WidgetStatus")
        );
    }

    #[test]
    fn supports_parameter_refs() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/key/{PK}": {
      "delete": {
        "operationId": "delete_key",
        "parameters": [
          { "$ref": "#/components/parameters/PK" }
        ],
        "responses": {
          "204": { "description": "deleted" }
        }
      }
    }
  },
  "components": {
    "parameters": {
      "PK": {
        "name": "PK",
        "in": "path",
        "required": true,
        "schema": { "type": "string" }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("parameter refs should be supported");

        assert_eq!(result.ir.operations.len(), 1);
        let operation = &result.ir.operations[0];
        assert_eq!(operation.params.len(), 1);
        let param = &operation.params[0];
        assert_eq!(param.name, "PK");
        assert_eq!(param.location, ParameterLocation::Path);
        assert!(param.required);
        assert_eq!(param.type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn supports_swagger_root_parameter_refs_with_type() {
        let spec = r##"
{
  "swagger": "2.0",
  "paths": {
    "/widgets/{id}": {
      "get": {
        "operationId": "get_widget",
        "parameters": [
          { "$ref": "#/parameters/ApiVersionParameter" },
          { "$ref": "#/parameters/IdParameter" }
        ],
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  },
  "parameters": {
    "ApiVersionParameter": {
      "name": "api-version",
      "in": "query",
      "required": true,
      "type": "string"
    },
    "IdParameter": {
      "name": "id",
      "in": "path",
      "required": true,
      "type": "integer",
      "format": "int64"
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("swagger root parameter refs should be supported");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "get_widget")
            .expect("operation should exist");
        assert_eq!(operation.params.len(), 2);
        assert_eq!(operation.params[0].name, "api-version");
        assert_eq!(operation.params[0].location, ParameterLocation::Query);
        assert_eq!(operation.params[0].type_ref, TypeRef::primitive("string"));
        assert_eq!(operation.params[1].name, "id");
        assert_eq!(operation.params[1].location, ParameterLocation::Path);
        assert_eq!(operation.params[1].type_ref, TypeRef::primitive("integer"));
    }

    #[test]
    fn supports_references_into_parameter_schemas() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "companyId": {
            "$ref": "#/components/parameters/companyId/schema"
          }
        }
      }
    },
    "parameters": {
      "companyId": {
        "name": "companyId",
        "in": "path",
        "required": true,
        "schema": {
          "type": "string"
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("schema refs into reusable parameters should resolve");

        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model should exist");
        assert_eq!(widget.fields[0].name, "companyId");
        assert_eq!(widget.fields[0].type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn preserves_external_file_references_as_named_types() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Route": {
        "type": "object",
        "properties": {
          "subnet": {
            "$ref": "./virtualNetwork.json#/definitions/Subnet"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("external file refs should remain importable as named types");

        let route = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Route")
            .expect("Route model should exist");
        assert_eq!(route.fields[0].name, "subnet");
        assert_eq!(route.fields[0].type_ref, TypeRef::named("Subnet"));
    }

    #[test]
    fn normalizes_swagger_body_parameters_into_request_bodies() {
        let spec = r##"
{
  "swagger": "2.0",
  "consumes": ["application/json"],
  "paths": {
    "/widgets/{id}": {
      "patch": {
        "operationId": "patch_widget",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "type": "string"
          },
          {
            "name": "widget",
            "in": "body",
            "required": true,
            "description": "Widget update payload.",
            "schema": {
              "type": "object",
              "properties": {
                "name": { "type": "string" }
              }
            }
          }
        ],
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("swagger body parameters should become request bodies");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "patch_widget")
            .expect("operation should exist");
        assert_eq!(operation.params.len(), 1);
        assert_eq!(operation.params[0].name, "id");

        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/json");
        assert_eq!(
            request_body.attributes.get("description"),
            Some(&Value::String("Widget update payload.".into()))
        );
        assert!(matches!(request_body.type_ref, Some(TypeRef::Named { .. })));
    }

    #[test]
    fn supports_request_body_refs() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/events": {
      "post": {
        "operationId": "create_event",
        "requestBody": {
          "$ref": "#/components/requestBodies/EventRequest"
        },
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  },
  "components": {
    "requestBodies": {
      "EventRequest": {
        "$ref": "#/components/requestBodies/BaseEventRequest"
      },
      "BaseEventRequest": {
        "required": true,
        "content": {
          "application/json": {
            "schema": { "type": "string" }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("request body refs should be supported");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_event")
            .expect("operation should exist");
        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/json");
        assert_eq!(request_body.type_ref, Some(TypeRef::primitive("string")));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn defaults_empty_request_body_content_to_untyped_octet_stream() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/events": {
      "post": {
        "operationId": "create_event",
        "requestBody": {
          "required": true,
          "content": {}
        },
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("empty request body content should be normalized");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_event")
            .expect("operation should exist");
        let request_body = operation.request_body.as_ref().expect("request body");
        assert!(request_body.required);
        assert_eq!(request_body.media_type, "application/octet-stream");
        assert_eq!(request_body.type_ref, None);
        assert_eq!(result.warnings.len(), 1);
        assert!(matches!(
            result.warnings[0].kind,
            DiagnosticKind::EmptyRequestBodyContent
        ));
        assert_eq!(
            result.warnings[0].pointer.as_deref(),
            Some("#/paths/~1events/post/requestBody/content")
        );
    }

    #[test]
    fn supports_const_scalar_fields() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchOp": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "replace"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("const should be supported");
        let patch_op = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchOp")
            .expect("PatchOp model");
        assert!(
            patch_op
                .fields
                .iter()
                .any(|field| field.name == "op" && field.type_ref == TypeRef::primitive("string"))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn supports_type_array_with_nullability() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "name": {
            "type": ["string", "null"]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("type arrays with null should be supported");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let name = widget
            .fields
            .iter()
            .find(|field| field.name == "name")
            .expect("name field");
        assert_eq!(name.type_ref, TypeRef::primitive("string"));
        assert!(name.nullable);
    }

    #[test]
    fn falls_back_when_operation_id_is_empty() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("empty operation ids should fall back");
        let operation = &result.ir.operations[0];
        assert_eq!(operation.name, "get_widgets");
    }

    #[test]
    fn supports_implicit_enum_and_items_schema_shapes() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "status": {
            "enum": ["ready", "pending"]
          },
          "children": {
            "items": {
              "type": "string"
            }
          },
          "withTrial": {
            "format": "boolean"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("implicit enum/items/format schema shapes should be supported");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let status = widget
            .fields
            .iter()
            .find(|field| field.name == "status")
            .expect("status field");
        assert_eq!(status.type_ref, TypeRef::primitive("string"));

        let children = widget
            .fields
            .iter()
            .find(|field| field.name == "children")
            .expect("children field");
        assert_eq!(
            children.type_ref,
            TypeRef::array(TypeRef::primitive("string"))
        );

        let with_trial = widget
            .fields
            .iter()
            .find(|field| field.name == "withTrial")
            .expect("withTrial field");
        assert_eq!(with_trial.type_ref, TypeRef::primitive("boolean"));
    }

    #[test]
    fn supports_object_schemas_with_validation_only_any_of() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchGist": {
        "type": "object",
        "properties": {
          "description": { "type": "string" },
          "files": { "type": "object" }
        },
        "anyOf": [
          { "required": ["description"] },
          { "required": ["files"] }
        ],
        "nullable": true
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("object schemas with validation-only anyOf should be supported");
        let patch_gist = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchGist")
            .expect("PatchGist model");
        let field_names = patch_gist
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["description", "files"]);
    }

    #[test]
    fn preserves_schema_property_order() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "zebra": { "type": "string" },
          "alpha": { "type": "string" },
          "middle": { "type": "string" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("property order should be preserved");
        let widget = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Widget")
            .expect("Widget model");
        let field_names = widget
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["zebra", "alpha", "middle"]);
    }

    #[test]
    fn supports_metadata_only_property_schema_as_any() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "ErrorDetail": {
        "type": "object",
        "properties": {
          "value": {
            "description": "The value at the given location"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("metadata-only schema should be treated as any");
        let error_detail = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "ErrorDetail")
            .expect("ErrorDetail model");
        let value = error_detail
            .fields
            .iter()
            .find(|field| field.name == "value")
            .expect("value field");
        assert_eq!(value.type_ref, TypeRef::primitive("any"));
    }

    #[test]
    fn supports_discriminator_on_unions() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "AddOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "add"
          }
        }
      },
      "RemoveOperation": {
        "type": "object",
        "properties": {
          "op": {
            "type": "string",
            "const": "remove"
          }
        }
      },
      "PatchSchema": {
        "type": "object",
        "properties": {
          "patches": {
            "type": "array",
            "items": {
              "oneOf": [
                { "$ref": "#/components/schemas/AddOperation" },
                { "$ref": "#/components/schemas/RemoveOperation" }
              ],
              "discriminator": {
                "propertyName": "op",
                "mapping": {
                  "add": "#/components/schemas/AddOperation",
                  "remove": "#/components/schemas/RemoveOperation"
                }
              }
            }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("discriminator unions should be supported");
        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let patches = patch_schema
            .fields
            .iter()
            .find(|field| field.name == "patches")
            .expect("patches field");
        assert!(matches!(
            &patches.type_ref,
            TypeRef::Array { item }
                if matches!(
                    item.as_ref(),
                    TypeRef::Union { variants }
                        if variants == &vec![
                            TypeRef::named("AddOperation"),
                            TypeRef::named("RemoveOperation")
                        ]
                )
        ));
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn supports_all_of_object_composition() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Cursor": {
        "type": "object",
        "properties": {
          "cursor": { "type": "string" }
        },
        "required": ["cursor"]
      },
      "PatchSchema": {
        "allOf": [
          { "$ref": "#/components/schemas/Cursor" },
          {
            "type": "object",
            "properties": {
              "items": {
                "type": "array",
                "items": { "type": "string" }
              }
            },
            "required": ["items"]
          }
        ]
      },
      "BaseId": { "type": "string" },
      "WrappedId": {
        "allOf": [
          { "$ref": "#/components/schemas/BaseId" },
          { "description": "Identifier wrapper" }
        ]
      },
      "Status": {
        "type": "string",
        "enum": ["ready", "pending", "failed"]
      },
      "RetryableStatus": {
        "allOf": [
          { "$ref": "#/components/schemas/Status" },
          { "enum": ["pending", "failed"] }
        ]
      },
      "TitledCursor": {
        "allOf": [
          {
            "$ref": "#/components/schemas/Cursor",
            "title": "Cursor Base"
          },
          {
            "type": "object",
            "title": "Cursor Overlay",
            "properties": {
              "nextCursor": { "type": "string" }
            }
          }
        ]
      },
      "Wrapper": {
        "type": "object",
        "properties": {
          "cursorRef": {
            "allOf": [
              { "$ref": "#/components/schemas/Cursor" },
              { "description": "Keep the named component reference" }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("allOf should be supported");

        let patch_schema = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PatchSchema")
            .expect("PatchSchema model");
        let field_names = patch_schema
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(field_names, vec!["cursor", "items"]);

        let titled_cursor = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "TitledCursor")
            .expect("TitledCursor model");
        let titled_cursor_fields = titled_cursor
            .fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(titled_cursor_fields, vec!["cursor", "nextCursor"]);

        let retryable_status = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "RetryableStatus")
            .expect("RetryableStatus model");
        assert_eq!(
            retryable_status.attributes.get("enum_values"),
            Some(&Value::Array(vec![
                Value::String("pending".into()),
                Value::String("failed".into())
            ]))
        );
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "cursor")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );
        assert!(
            patch_schema
                .fields
                .iter()
                .find(|field| field.name == "items")
                .map(|field| !field.optional)
                .unwrap_or(false)
        );

        let wrapped_id = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "WrappedId")
            .expect("WrappedId model");
        assert_eq!(
            wrapped_id.attributes.get("alias_type_ref"),
            Some(&json!(TypeRef::primitive("string")))
        );
        let wrapper = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Wrapper")
            .expect("Wrapper model");
        assert_eq!(
            wrapper
                .fields
                .iter()
                .find(|field| field.name == "cursorRef")
                .map(|field| &field.type_ref),
            Some(&TypeRef::named("Cursor"))
        );
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn errors_on_recursive_all_of_reference_cycles() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Node": {
        "allOf": [
          { "$ref": "#/components/schemas/Node" }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("recursive allOf cycles should fail cleanly");

        let rendered = format!("{error:#}");
        assert!(
            rendered.contains("recursive reference cycle"),
            "unexpected error: {rendered}"
        );
        assert!(rendered.contains("#/components/schemas/Node"));
    }

    #[test]
    fn errors_on_unhandled_elements_by_default_and_warns_when_ignored() {
        // `not` is now silently ignored (mapped to `any`). Verify a genuinely
        // unsupported-but-declared keyword (`if`) still triggers the unhandled path.
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "if": {
          "properties": { "foo": { "type": "string" } }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let strict_error = OpenApiImporter::new(
            document.clone(),
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("strict mode should fail");
        assert!(
            strict_error
                .to_string()
                .contains("`if` is not supported yet")
        );

        let warning_result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions {
                ignore_unhandled: true,
                ..Default::default()
            },
        )
        .build_ir()
        .expect("ignore mode should succeed");
        assert!(
            warning_result
                .warnings
                .iter()
                .any(|warning| matches!(&warning.kind, DiagnosticKind::UnsupportedSchemaKeyword { keyword } if keyword == "if"))
        );

        // Verify `not` is silently ignored (no error, no warning).
        let not_spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "NotSchema": {
        "not": { "type": "object" }
      }
    }
  }
}
"##;
        let not_document: OpenApiDocument =
            serde_json::from_str(not_spec).expect("valid test spec");
        let not_result = OpenApiImporter::new(
            not_document,
            json_test_source(not_spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("`not` keyword should be silently ignored");
        assert!(
            not_result.warnings.is_empty(),
            "`not` should produce no warnings"
        );
    }

    #[test]
    fn errors_on_unknown_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "frobnicate": true
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let error = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("unknown keyword should fail");
        assert!(
            error
                .to_string()
                .contains("unknown schema keyword `frobnicate`")
        );
    }

    #[test]
    fn ignores_known_non_codegen_schema_keywords() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PatchSchema": {
        "type": "string",
        "description": "some text",
        "default": "value",
        "minLength": 1,
        "contentEncoding": "base64",
        "externalDocs": {
          "description": "More details",
          "url": "https://example.com/schema-docs"
        },
        "xml": {
          "name": "patchSchema"
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("known ignored keywords should not fail");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn json_parse_errors_include_schema_path_and_source_context() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Broken": {
        "type": "object",
        "title": ["not", "a", "string"]
      }
    }
  }
}
"##;

        let error = parse_json_openapi_document(Path::new("broken.json"), spec)
            .expect_err("invalid schema shape should fail during deserialization");
        let message = error.to_string();
        assert!(message.contains("failed to parse JSON OpenAPI document `broken.json`"));
        assert!(message.contains("schema mismatch at `components.schemas.Broken.title`"));
        assert!(message.contains("invalid type"));
        assert!(message.contains("source:         \"title\": [\"not\", \"a\", \"string\"]"));
        assert!(message.contains("note: this usually means"));
    }

    #[test]
    fn yaml_loader_ignores_tab_only_blank_lines_in_block_scalars() {
        let spec = r##"
openapi: 3.1.0
paths: {}
components:
  schemas:
    AdditionalDataAirline:
      type: object
      properties:
        airline.leg.date_of_travel:
          description: |-
            	
            Date and time of travel in ISO 8601 format.
          type: string
"##;

        let loaded = parse_yaml_openapi_document(Path::new("broken.yaml"), spec)
            .expect("tab-only blank lines should be normalized before YAML parsing");
        let result = OpenApiImporter::new(
            loaded.document,
            loaded.source,
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("normalized YAML should import");

        let model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "AdditionalDataAirline")
            .expect("model should exist");
        assert!(
            model
                .fields
                .iter()
                .any(|field| field.name == "airline.leg.date_of_travel")
        );
    }

    #[test]
    fn preserves_content_encoding_metadata_in_ir_attributes() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "post": {
        "operationId": "create_widget",
        "parameters": [
          {
            "name": "token",
            "in": "query",
            "required": true,
            "schema": {
              "type": "string",
              "contentEncoding": "base64"
            }
          }
        ],
        "requestBody": {
          "required": true,
          "content": {
            "application/json": {
              "schema": {
                "type": "string",
                "contentEncoding": "base64",
                "contentMediaType": "application/octet-stream"
              }
            }
          }
        },
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": {
                  "$ref": "#/components/schemas/EncodedValue"
                }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "EncodedValue": {
        "type": "object",
        "properties": {
          "payload": {
            "type": "string",
            "contentEncoding": "base64"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid test spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("content encoding metadata should be preserved");

        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_widget")
            .expect("operation should exist");
        assert_eq!(
            operation.params[0]
                .attributes
                .get("content_encoding")
                .and_then(Value::as_str),
            Some("base64")
        );
        assert_eq!(
            operation
                .request_body
                .as_ref()
                .and_then(|request_body| request_body.attributes.get("content_media_type"))
                .and_then(Value::as_str),
            Some("application/octet-stream")
        );
        let response = operation
            .responses
            .iter()
            .find(|response| response.status == "200")
            .expect("response should exist");
        assert_eq!(
            response.type_ref.as_ref(),
            Some(&TypeRef::named("EncodedValue"))
        );
        let model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "EncodedValue")
            .expect("model should exist");
        assert_eq!(
            model.fields[0]
                .attributes
                .get("content_encoding")
                .and_then(Value::as_str),
            Some("base64")
        );
    }

    #[test]
    fn supports_swagger_form_data_parameters() {
        let spec = r##"
{
  "swagger": "2.0",
  "consumes": ["application/x-www-form-urlencoded"],
  "paths": {
    "/widgets": {
      "post": {
        "operationId": "create_widget",
        "parameters": [
          { "$ref": "#/parameters/form_name" }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  },
  "parameters": {
    "form_name": {
      "name": "name",
      "in": "formData",
      "description": "Widget name",
      "required": true,
      "type": "string"
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid swagger spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("formData parameters should normalize into a request body");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "create_widget")
            .expect("operation should exist");
        assert!(operation.params.is_empty());
        let request_body = operation
            .request_body
            .as_ref()
            .expect("formData should create a request body");
        assert_eq!(request_body.media_type, "application/x-www-form-urlencoded");
        assert_eq!(
            request_body.type_ref.as_ref(),
            Some(&TypeRef::named("CreateWidgetRequest"))
        );
        let body_model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "CreateWidgetRequest")
            .expect("inline form body model should exist");
        assert_eq!(body_model.fields[0].name, "name");
        assert!(!body_model.fields[0].optional);
        assert_eq!(
            body_model.fields[0]
                .attributes
                .get("description")
                .and_then(Value::as_str),
            Some("Widget name")
        );
    }

    #[test]
    fn supports_path_local_parameter_references() {
        let spec = r##"
{
  "swagger": "2.0",
  "definitions": {
    "Widget": {
      "type": "object",
      "properties": {
        "id": { "type": "string" }
      }
    }
  },
  "paths": {
    "/widgets/{id}": {
      "post": {
        "operationId": "get_widget",
        "parameters": [
          {
            "name": "id",
            "in": "path",
            "required": true,
            "type": "string"
          }
        ],
        "responses": {
          "200": {
            "description": "ok",
            "schema": {
              "$ref": "#/definitions/Widget"
            }
          }
        }
      }
    },
    "/widget-ids": {
      "get": {
        "operationId": "list_widget_ids",
        "parameters": [
          {
            "$ref": "#/paths/~1widgets~1%7Bid%7D/post/parameters/0"
          }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid swagger spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("path-local parameter refs should resolve");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "list_widget_ids")
            .expect("operation should exist");
        assert_eq!(operation.params[0].name, "id");
        assert_eq!(operation.params[0].location, ParameterLocation::Path);
        assert_eq!(
            result
                .ir
                .models
                .iter()
                .find(|model| model.name == "Widget")
                .map(|model| model.name.as_str()),
            Some("Widget")
        );
    }

    #[test]
    fn de_duplicates_operation_names() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "get_widgets",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    },
    "/users": {
      "get": {
        "operationId": "get_widgets",
        "responses": {
          "200": { "description": "ok" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("duplicate operation ids should be disambiguated");
        let names = result
            .ir
            .operations
            .iter()
            .map(|operation| operation.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["get_widgets", "get_widgets_2"]);
    }

    #[test]
    fn supports_numeric_all_of_type_widening() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "WidgetEvent": {
        "type": "object",
        "properties": {
          "payload": {
            "allOf": [
              {
                "type": "object",
                "properties": {
                  "count": { "type": "integer" }
                }
              },
              {
                "type": "object",
                "properties": {
                  "count": { "type": "number" }
                }
              }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("numeric allOf overlays should merge");
        assert!(result.warnings.is_empty());
        let payload_model = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "WidgetEventPayload")
            .expect("inline payload model should exist");
        assert_eq!(
            payload_model.fields[0].type_ref,
            TypeRef::primitive("number")
        );
    }

    #[test]
    fn supports_nested_schema_definitions_references() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Transfer": {
        "type": "object",
        "definitions": {
          "money": {
            "type": "object",
            "properties": {
              "currency": { "type": "string" }
            }
          }
        },
        "properties": {
          "amount": {
            "$ref": "#/components/schemas/Transfer/definitions/money"
          },
          "currency": {
            "$ref": "#/components/schemas/Transfer/definitions/money/properties/currency"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("nested schema definitions refs should resolve");
        let transfer = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Transfer")
            .expect("Transfer model should exist");
        assert_eq!(transfer.fields[0].name, "amount");
        let amount_type_name = match &transfer.fields[0].type_ref {
            TypeRef::Named { name } => name.clone(),
            other => panic!("expected named type for nested definition, got {other:?}"),
        };
        assert!(
            amount_type_name.starts_with("TransferAmount"),
            "nested definition should be materialized as a TransferAmount* inline model"
        );
        assert!(
            result
                .ir
                .models
                .iter()
                .any(|model| model.name == amount_type_name),
            "nested local definition model should be imported"
        );
        assert_eq!(transfer.fields[1].type_ref, TypeRef::primitive("string"));
    }

    #[test]
    fn supports_nullable_all_of_overlays_on_referenced_scalars() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Transfer": {
        "type": "object",
        "definitions": {
          "money": {
            "type": "string"
          }
        }
      },
      "Bill": {
        "type": "object",
        "properties": {
          "currency": {
            "allOf": [
              { "$ref": "#/components/schemas/Transfer/definitions/money" },
              { "type": "null" }
            ]
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("nullable allOf overlay should merge");
        let bill = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Bill")
            .expect("Bill model should exist");
        assert_eq!(bill.fields[0].name, "currency");
        assert_eq!(bill.fields[0].type_ref, TypeRef::primitive("string"));
        assert!(bill.fields[0].nullable);
    }

    #[test]
    fn supports_recursive_local_object_references_without_unbounded_inline_models() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "PushOption": {
        "definitions": {
          "pushOptionProperty": {
            "type": "object",
            "properties": {
              "properties": {
                "type": "object",
                "additionalProperties": {
                  "$ref": "#/components/schemas/PushOption/definitions/pushOptionProperty"
                }
              }
            }
          }
        },
        "type": "object",
        "properties": {
          "properties": {
            "type": "object",
            "additionalProperties": {
              "$ref": "#/components/schemas/PushOption/definitions/pushOptionProperty"
            }
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("recursive local refs should not recurse forever");

        let push_option = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "PushOption")
            .expect("PushOption model should exist");
        let properties_field = push_option
            .fields
            .iter()
            .find(|field| field.name == "properties")
            .expect("properties field should exist");
        assert!(matches!(properties_field.type_ref, TypeRef::Map { .. }));

        let inline_models = result
            .ir
            .models
            .iter()
            .filter(|model| model.name.contains("Properties"))
            .collect::<Vec<_>>();
        assert!(
            inline_models.len() <= 2,
            "recursive local refs should reuse an inline model instead of generating an unbounded chain"
        );
    }

    #[test]
    fn supports_collection_format_metadata() {
        let spec = r##"
{
  "swagger": "2.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [
          {
            "name": "categories",
            "in": "query",
            "type": "array",
            "collectionFormat": "csv",
            "items": {
              "type": "string",
              "collectionFormat": "csv"
            }
          }
        ],
        "responses": {
          "200": {
            "description": "ok"
          }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("collectionFormat should be accepted");
        let operation = result
            .ir
            .operations
            .iter()
            .find(|operation| operation.name == "list_widgets")
            .expect("operation should exist");
        assert_eq!(
            operation.params[0]
                .attributes
                .get("collection_format")
                .and_then(Value::as_str),
            Some("csv")
        );
    }

    #[test]
    fn supports_all_of_with_multiple_discriminators() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Base": {
        "type": "object",
        "discriminator": {
          "propertyName": "serviceType"
        },
        "properties": {
          "serviceType": { "type": "string" }
        }
      },
      "Derived": {
        "allOf": [
          { "$ref": "#/components/schemas/Base" },
          {
            "type": "object",
            "discriminator": {
              "propertyName": "credentialType"
            },
            "properties": {
              "credentialType": { "type": "string" }
            }
          }
        ]
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("allOf discriminator metadata should not fail");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn empty_property_names_fail_cleanly_or_warn_when_ignored() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Broken": {
        "type": "object",
        "properties": {
          "": { "type": "string" }
        }
      }
    }
  }
}
"##;

        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let error = OpenApiImporter::new(
            document.clone(),
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect_err("empty property names should fail by default");
        assert!(error.to_string().contains("property #1 has an empty name"));

        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions {
                ignore_unhandled: true,
                emit_timings: false,
            },
        )
        .build_ir()
        .expect("empty property names should be synthesized when warnings are allowed");
        let broken = result
            .ir
            .models
            .iter()
            .find(|model| model.name == "Broken")
            .expect("Broken model should exist");
        assert_eq!(broken.fields[0].name, "unnamed_field_1");
    }

    #[test]
    fn supports_ref_to_components_responses() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/responses/WidgetList/content/application~1json/schema" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "responses": {
      "WidgetList": {
        "description": "A list of widgets",
        "content": {
          "application/json": {
            "schema": {
              "type": "array",
              "items": { "type": "string" }
            }
          }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("$ref to components/responses should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "list_widgets")
            .expect("op should exist");
        let response = op.responses.first().expect("should have a response");
        // The $ref resolves to an array type; type_ref should be Some (not None).
        assert!(
            response.type_ref.is_some(),
            "response type_ref should be resolved, got: {response:?}"
        );
    }

    #[test]
    fn supports_ref_to_path_response_schema() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [],
        "responses": {
          "200": {
            "description": "ok",
            "content": {
              "application/json": {
                "schema": { "type": "array", "items": { "type": "string" } }
              }
            }
          }
        }
      },
      "post": {
        "operationId": "create_widget",
        "parameters": [],
        "responses": {
          "201": {
            "description": "created",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/paths/~1widgets/get/responses/200/content/application~1json/schema" }
              }
            }
          }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("$ref to path response schema should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "create_widget")
            .expect("op should exist");
        let response = op.responses.first().expect("should have a response");
        // The $ref resolves to an array-of-string type; type_ref should be Some.
        assert!(
            response.type_ref.is_some(),
            "response type_ref should be resolved, got: {response:?}"
        );
    }

    #[test]
    fn supports_content_based_parameters() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {
    "/widgets": {
      "get": {
        "operationId": "list_widgets",
        "parameters": [
          {
            "name": "filter",
            "in": "query",
            "content": {
              "application/json": {
                "schema": { "type": "object", "properties": { "name": { "type": "string" } } }
              }
            }
          }
        ],
        "responses": { "200": { "description": "ok" } }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("content-based parameter should succeed");
        let op = result
            .ir
            .operations
            .iter()
            .find(|o| o.name == "list_widgets")
            .expect("op should exist");
        assert_eq!(op.params.len(), 1);
        assert_eq!(op.params[0].name, "filter");
    }

    #[test]
    fn supports_format_string_as_type_inference() {
        let spec = r##"
{
  "openapi": "3.1.0",
  "paths": {},
  "components": {
    "schemas": {
      "Widget": {
        "type": "object",
        "properties": {
          "name": { "format": "string", "description": "The widget name" },
          "score": { "format": "float" }
        }
      }
    }
  }
}
"##;
        let document: OpenApiDocument = serde_json::from_str(spec).expect("valid spec");
        let result = OpenApiImporter::new(
            document,
            json_test_source(spec),
            LoadOpenApiOptions::default(),
        )
        .build_ir()
        .expect("format:string schema shape should succeed");
        let model = result
            .ir
            .models
            .iter()
            .find(|m| m.name == "Widget")
            .expect("model should exist");
        let name_field = model
            .fields
            .iter()
            .find(|f| f.name == "name")
            .expect("name field should exist");
        assert!(matches!(&name_field.type_ref, t if format!("{t:?}").contains("string")));
        let score_field = model
            .fields
            .iter()
            .find(|f| f.name == "score")
            .expect("score field should exist");
        assert!(matches!(&score_field.type_ref, t if format!("{t:?}").contains("number")));
    }