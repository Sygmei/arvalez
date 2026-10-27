use std::collections::BTreeMap;
use std::{fs, process::Command};

use arvalez_ir::{
    Attributes, CoreIr, Field, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    Response, TypeRef,
};
use arvalez_target_core::{CommonConfig, PackageConfig};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::{TargetConfig, generate_go_package, write_go_package};

fn sample_ir() -> CoreIr {
    CoreIr {
        models: vec![arvalez_ir::Model {
            id: "model.widget".into(),
            name: "Widget".into(),
            kind: arvalez_ir::ModelKind::Object,
            fields: vec![
                Field::new("id", TypeRef::primitive("string")),
                Field {
                    name: "count".into(),
                    type_ref: TypeRef::primitive("integer"),
                    optional: true,
                    nullable: false,
                    attributes: Attributes::default(),
                },
                Field {
                    name: "picture_url".into(),
                    type_ref: TypeRef::primitive("string"),
                    optional: true,
                    nullable: true,
                    attributes: Attributes::default(),
                },
            ],
            attributes: Attributes::default(),
            source: None,
        }],
        operations: vec![Operation {
            id: "operation.get_widget".into(),
            name: "get_widget".into(),
            method: HttpMethod::Get,
            path: "/widgets/{widget_id}".into(),
            params: vec![
                Parameter {
                    name: "widget_id".into(),
                    location: ParameterLocation::Path,
                    type_ref: TypeRef::primitive("string"),
                    required: true,
                    attributes: BTreeMap::from([(
                        "description".into(),
                        Value::String("Unique widget identifier.".into()),
                    )]),
                },
                Parameter {
                    name: "include_count".into(),
                    location: ParameterLocation::Query,
                    type_ref: TypeRef::primitive("boolean"),
                    required: false,
                    attributes: BTreeMap::from([(
                        "description".into(),
                        Value::String("Whether to include the total count.".into()),
                    )]),
                },
            ],
            request_body: Some(RequestBody {
                required: false,
                media_type: "application/json".into(),
                type_ref: Some(TypeRef::named("Widget")),
                attributes: BTreeMap::new(),
            }),
            responses: vec![Response {
                status: "200".into(),
                media_type: Some("application/json".into()),
                type_ref: Some(TypeRef::named("Widget")),
                attributes: Attributes::default(),
            }],
            attributes: Attributes::from([
                ("tags".into(), json!(["widgets"])),
                ("deprecated".into(), json!(true)),
            ]),
            source: None,
        }],
        ..Default::default()
    }
}

fn default_common() -> CommonConfig {
    CommonConfig { package: PackageConfig { name: "client".into(), version: "0.1.0".into(), description: None } }
}

#[test]
fn renders_basic_go_package() {
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), ..Default::default() },
    )
    .expect("package should render");

    let go_mod = files
        .iter()
        .find(|file| file.path.ends_with("go.mod"))
        .expect("go.mod");
    let models = files
        .iter()
        .find(|file| file.path.ends_with("models.go"))
        .expect("models.go");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");
    let utils = files
        .iter()
        .find(|file| file.path.ends_with("utils.go"))
        .expect("utils.go");

    let params = &files.iter().find(|f| f.path.ends_with("params.go")).unwrap().contents;
    assert!(go_mod.contents.contains("module github.com/demo/client"));
    assert!(models.contents.contains("type Widget struct"));
    assert!(models.contents.contains("Count *int64 `json:\"count,omitempty\"`"));
    assert!(models.contents.contains("PictureURL *Nullable[string] `json:\"picture_url,omitempty\"`"));
    assert!(models.contents.contains("type Nullable[T any] struct"));
    assert!(models.contents.contains("func Null[T any]() *Nullable[T]"));
    assert!(client.contents.contains("type ErrorHandler func(*http.Response) error"));
    assert!(client.contents.contains("type RequestOptions struct"));
    assert!(!client.contents.contains("Context                  context.Context"));
    assert!(utils.contents.contains("type APIError struct"));
    assert!(utils.contents.contains("Body       []byte"));
    assert!(!utils.contents.contains("func (c *Client) resolveContext("));
    assert!(utils.contents.contains("func (c *Client) encodeMultipartBody(payload any) (io.Reader, string, error) {"));
    assert!(client.contents.contains("func (c *Client) GetWidgetRaw("));
    assert!(client.contents.contains("func (c *Client) GetWidget("));
    assert!(params.contains("Body *Widget"));
    assert!(client.contents.contains("// Deprecated: This operation is deprecated."));
    assert!(client.contents.contains("GetWidget parameter params.WidgetID: Unique widget identifier."));
    assert!(client.contents.contains("requestOptions *RequestOptions"));
    assert!(client.contents.contains("if err := client.handleError(response, requestOptions); err != nil {"));
    assert!(client.contents.contains("response, err := c.executeGetWidget("));
    assert!(client.contents.contains(
        "// GetWidgetRaw parameter params.WidgetID: Unique widget identifier.\n// GetWidgetRaw parameter params.IncludeCount: Whether to include the total count.\nfunc (c *Client) GetWidgetRaw("
    ));
    assert!(client.contents.contains(
        "// GetWidget parameter params.WidgetID: Unique widget identifier.\n// GetWidget parameter params.IncludeCount: Whether to include the total count.\nfunc (c *Client) GetWidget("
    ));
    assert!(client.contents.contains(
        "if params.IncludeCount != nil {\n\t\tquery.Set(\"include_count\", fmt.Sprint(*params.IncludeCount))\n\t}"
    ));
}

#[test]
fn generated_package_builds_and_vets_when_go_is_available() {
    if Command::new("go").arg("version").output().is_err() {
        return;
    }

    let output_dir = tempdir().expect("tempdir");
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), ..Default::default() },
    )
    .expect("package should render");
    write_go_package(output_dir.path(), &files).expect("package should be written");
    let go_cache = output_dir.path().join("go-cache");
    fs::create_dir_all(&go_cache).expect("Go cache directory should be created");

    for command in ["test", "vet"] {
        let output = Command::new("go")
            .args([command, "./..."])
            .current_dir(output_dir.path())
            .env("GOCACHE", &go_cache)
            .output()
            .expect("go command should run");
        assert!(
            output.status.success(),
            "go {command} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn avoids_collisions_between_parameters_and_generated_locals() {
    let mut ir = sample_ir();
    ir.operations[0].params[1].name = "query".into();

    let files = generate_go_package(&ir, None, &default_common(), &TargetConfig::default())
        .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(files.iter().find(|f| f.path.ends_with("params.go")).unwrap().contents.contains("Query *bool"));
    assert!(client.contents.contains("query_.Set(\"query\", fmt.Sprint(*params.Query))"));
    assert!(client.contents.contains("query_ := url.Values{}"));
}

#[test]
fn required_named_request_bodies_are_values() {
    let mut ir = sample_ir();
    ir.operations[0].request_body.as_mut().expect("request body").required = true;

    let files = generate_go_package(
        &ir,
        None,
        &default_common(),
        &TargetConfig::default(),
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    let params = &files.iter().find(|f| f.path.ends_with("params.go")).unwrap().contents;
    assert!(params.contains("Body Widget"));
    assert!(!params.contains("Body *Widget"));
    assert!(client.contents.contains("params.Body"));
}

#[test]
fn preserves_common_go_initialisms() {
    assert_eq!(crate::sanitize::sanitize_exported_identifier("picture_url"), "PictureURL");
    assert_eq!(crate::sanitize::sanitize_exported_identifier("get_item_by_id"), "GetItemByID");
    assert_eq!(crate::sanitize::sanitize_identifier("item_id"), "itemID");
    assert_eq!(crate::sanitize::sanitize_identifier("x_admin_token"), "xAdminToken");
}

#[test]
fn groups_operations_by_tag_when_enabled() {
    let files = generate_go_package(
        &sample_ir(),
        None,
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("Widgets *WidgetsService"));
    assert!(client.contents.contains("client.Widgets = &WidgetsService{client: client}"));
    assert!(client.contents.contains("type WidgetsService struct"));
    assert!(client.contents.contains("func (s *WidgetsService) GetWidgetRaw("));
}

#[test]
fn supports_selective_template_overrides() {
    let tempdir = tempdir().expect("tempdir");
    let partial_dir = tempdir.path().join("partials");
    fs::create_dir_all(&partial_dir).expect("partials dir");
    fs::write(
        partial_dir.join("service.go.tera"),
        "type {{ service.struct_name }} struct { Overridden bool }\n",
    )
    .expect("override template");

    let files = generate_go_package(
        &sample_ir(),
        Some(tempdir.path()),
        &default_common(),
        &TargetConfig { module_path: "github.com/demo/client".into(), group_by_tag: true },
    )
    .expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.go"))
        .expect("client.go");

    assert!(client.contents.contains("type WidgetsService struct { Overridden bool }"));
}

#[test]
fn imported_array_parameters_build_and_serialize_on_the_wire() {
    let output_dir = tempdir().expect("tempdir");
    let mut paths = serde_json::Map::new();
    let mut calls = String::new();
    let mut case = 0;
    for location in ["query", "header"] {
        for required in [false, true] {
            for explode in [None, Some(false), Some(true)] {
                for (item_type, go_values, expected, param_name) in [
                    ("string", "[]string{\"active\", \"paused\"}", "active,paused", "item"),
                    ("integer", "[]int64{2, 7}", "2,7", "values"),
                    ("number", "[]float64{1.5, 2.5}", "1.5,2.5", "query"),
                    ("boolean", "[]bool{true, false}", "true,false", "headers"),
                ] {
                    let mut parameter = json!({
                        "name": param_name, "in": location, "required": required,
                        "schema": {"type": ["array", "null"], "items": {"type": item_type}},
                    });
                    if let Some(explode) = explode {
                        parameter["explode"] = json!(explode);
                    }
                    paths.insert(format!("/case-{case}"), json!({"get": {
                        "operationId": format!("case_{case}"), "tags": ["arrays"],
                        "parameters": [parameter], "responses": {"204": {"description": "ok"}},
                    }}));
                    let repeated = location == "query" && explode.unwrap_or(true);
                    let expected_values: Vec<_> = if repeated {
                        expected.split(',').collect()
                    } else {
                        vec![expected]
                    };
                    let expected_go = expected_values.iter().map(|v| format!("{v:?}"))
                        .collect::<Vec<_>>().join(", ");
                    let empty_values = format!("{}{{}}", go_values.split('{').next().unwrap());
                    let field_name = crate::sanitize::sanitize_exported_identifier(param_name);
                    for (values, want) in [(go_values, format!("[]string{{{expected_go}}}")),
                        ("nil", "nil".into()), (empty_values.as_str(), "nil".into())] {
                        calls.push_str(&format!(r#"
    response, err = client.Arrays.Case{case}Raw(context.Background(), &Case{case}Params{{{field_name}: {values}}})
    if err != nil {{ t.Fatal(err) }}
    response.Body.Close()
    check(t, captured, "{location}", "{param_name}", {want})
"#));
                    }
                    case += 1;
                }
            }
        }
    }
    for (index, metadata, expected) in [
        (0, json!({"style": "spaceDelimited", "explode": false}), vec!["a b"]),
        (1, json!({"style": "pipeDelimited", "explode": false}), vec!["a|b"]),
        (2, json!({"collectionFormat": "csv"}), vec!["a,b"]),
        (3, json!({"collectionFormat": "multi"}), vec!["a", "b"]),
        (4, json!({"collectionFormat": "ssv"}), vec!["a b"]),
        (5, json!({"collectionFormat": "tsv"}), vec!["a\tb"]),
        (6, json!({"collectionFormat": "pipes"}), vec!["a|b"]),
        (7, json!({}), vec!["a,b"]),
    ] {
        let mut parameter = json!({
            "name": "values", "in": "query", "type": "array", "items": {"type": "string"},
        });
        if index < 2 {
            parameter.as_object_mut().unwrap().remove("type");
            parameter.as_object_mut().unwrap().remove("items");
            parameter["schema"] = json!({"type": "array", "items": {"type": "string"}});
        }
        parameter.as_object_mut().unwrap().extend(metadata.as_object().unwrap().clone());
        paths.insert(format!("/style-{index}"), json!({"get": {
            "operationId": format!("style_{index}"), "tags": ["arrays"],
            "parameters": [parameter], "responses": {"204": {"description": "ok"}},
        }}));
        let expected_go = expected.iter().map(|v| format!("{v:?}")).collect::<Vec<_>>().join(", ");
        calls.push_str(&format!(r#"
    response, err = client.Arrays.Style{index}Raw(context.Background(), (&Style{index}Params{{}}).WithValues([]string{{"a", "b"}}))
    if err != nil {{ t.Fatal(err) }}
    response.Body.Close()
    check(t, captured, "query", "values", []string{{{expected_go}}})
"#));
    }
    // Reproduce the five optional ramen filters together in a single operation.
    paths.insert("/v1/workspaces".into(), json!({"get": {
        "operationId": "list_workspaces", "tags": ["workspace"],
        "parameters": (["status", "namespace", "tofu_version", "action", "wid"].map(|name| json!({
            "name": name, "in": "query", "required": false, "explode": false,
            "schema": {"type": ["array", "null"], "items": {"type": "string"}},
        }))), "responses": {"204": {"description": "ok"}},
    }}));
    calls.push_str(r#"
    response, err = client.Workspace.ListWorkspacesRaw(context.Background(),
        (&ListWorkspacesParams{}).WithStatus([]string{"active", "paused"}).WithNamespace([]string{"a", "b"}).
            WithTofuVersion([]string{"1", "2"}).WithAction([]string{"plan", "apply"}).WithWid([]string{"w1", "w2"}))
    if err != nil { t.Fatal(err) }
    response.Body.Close()
    check(t, captured, "query", "status", []string{"active,paused"})
    check(t, captured, "query", "namespace", []string{"a,b"})
    check(t, captured, "query", "tofu_version", []string{"1,2"})
    check(t, captured, "query", "action", []string{"plan,apply"})
    check(t, captured, "query", "wid", []string{"w1,w2"})
"#);
    // Deliberately declare path parameters in the opposite order to the route.
    paths.insert("/v1/workspaces/{wid}/actions/{jid}".into(), json!({"patch": {
        "operationId": "update_actions_for_workspace", "tags": ["action"],
        "parameters": (["jid", "wid"].map(|name| json!({
            "name": name, "in": "path", "required": true, "schema": {"type": "string"},
        }))), "responses": {"204": {"description": "ok"}},
    }}));
    calls.push_str(r#"
    response, err = client.Action.UpdateActionsForWorkspaceRaw(context.Background(), &UpdateActionsForWorkspaceParams{Jid: "job/1", Wid: "workspace/2"})
    if err != nil { t.Fatal(err) }
    response.Body.Close()
    if got := captured.URL.EscapedPath(); got != "/v1/workspaces/workspace%2F2/actions/job%2F1" {
        t.Fatalf("wrong path argument order or escaping: %s", got)
    }
"#);
    let spec_path = output_dir.path().join("openapi.json");
    fs::write(&spec_path, json!({"openapi": "3.1.0", "paths": paths}).to_string()).unwrap();
    let ir = arvalez_openapi::load_openapi_to_ir(&spec_path).expect("OpenAPI should import");
    let files = generate_go_package(&ir, None, &default_common(), &TargetConfig {
        group_by_tag: true, ..Default::default()
    }).expect("arrays should render");
    let source = &files.iter().find(|f| f.path.ends_with("client.go")).unwrap().contents;
    for name in ["status", "namespace", "tofuVersion", "action", "wid"] {
        assert!(source.contains(&format!("strings.Join(params.{}, \",\")", crate::sanitize::sanitize_exported_identifier(name))));
        assert!(!source.contains(&format!("fmt.Sprint(*{name})")));
    }
    write_go_package(output_dir.path(), &files).unwrap();
    fs::write(output_dir.path().join("arrays_test.go"), format!(r#"package client
import (
    "context"
    "io"
    "net/http"
    "reflect"
    "strings"
    "testing"
)
type captureTransport func(*http.Request) (*http.Response, error)
func (f captureTransport) RoundTrip(r *http.Request) (*http.Response, error) {{ return f(r) }}
func check(t *testing.T, r *http.Request, location, name string, want []string) {{
    t.Helper()
    got := r.URL.Query()[name]
    if location == "header" {{ got = r.Header.Values(name) }}
    if !reflect.DeepEqual(got, want) {{ t.Fatalf("%s %s: got %#v, want %#v", r.URL.Path, name, got, want) }}
}}
func TestArrayWireEncoding(t *testing.T) {{
    var captured *http.Request
    client := NewClient("https://example.test", &ClientOptions{{HTTPClient: &http.Client{{
        Transport: captureTransport(func(r *http.Request) (*http.Response, error) {{
            captured = r
            return &http.Response{{StatusCode: 204, Header: http.Header{{}}, Body: io.NopCloser(strings.NewReader(""))}}, nil
        }}),
    }}}})
    var response *http.Response
    var err error
    {calls}
}}
"#)).unwrap();
    if Command::new("go").arg("version").output().is_err() {
        eprintln!("Go unavailable; generated wire tests were not executed");
        return;
    }
    let go_files: Vec<_> = fs::read_dir(output_dir.path()).unwrap().map(|f| f.unwrap().path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "go")).collect();
    let formatted = Command::new("gofmt").arg("-w").args(&go_files).output().unwrap();
    assert!(formatted.status.success(), "{}", String::from_utf8_lossy(&formatted.stderr));
    let checked = Command::new("gofmt").arg("-l").args(&go_files).output().unwrap();
    assert!(checked.status.success() && checked.stdout.is_empty());
    for command in ["build", "test", "vet"] {
        let output = Command::new("go").args([command, "./..."])
            .current_dir(output_dir.path()).env("GOCACHE", output_dir.path().join("go-cache"))
            .output().unwrap();
        assert!(output.status.success(), "go {command}:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn rejects_missing_or_extra_path_parameter_declarations() {
    for (path, expected) in [
        ("/workspaces/{wid}/widgets/{widget_id}", "path {wid} not declared as a path parameter"),
        ("/widgets", "path parameter widget_id has no matching placeholder"),
        ("/widgets/{widget_id", "unclosed path placeholder"),
    ] {
        let mut ir = sample_ir();
        ir.operations[0].path = path.into();
        let error = generate_go_package(&ir, None, &default_common(), &TargetConfig::default())
            .expect_err("invalid paths must fail generation");
        let message = format!("{error:#}");
        assert!(message.contains("get_widget") && message.contains(expected), "{message}");
    }
    let mut ir = sample_ir();
    ir.operations[0].params.clear();
    let error = generate_go_package(&ir, None, &default_common(), &TargetConfig::default()).unwrap_err();
    assert!(format!("{error:#}").contains("path {widget_id} not declared"));
}
