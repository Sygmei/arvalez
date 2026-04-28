use crate::{TargetConfig, generate};
use arvalez_target_core::GeneratedFile;
use arvalez_ir::{
    Attributes, CoreIr, HttpMethod, Operation, Parameter, ParameterLocation, Response, TypeRef,
};

fn make_package_from_ir(ir: CoreIr, package_name: &str) -> anyhow::Result<Vec<GeneratedFile>> {
    let common = arvalez_target_core::CommonConfig {
        package: arvalez_target_core::PackageConfig {
            name: package_name.into(),
            version: "0.1.0".into(),
            description: None,
        },
    };
    generate(&ir, None, &common, &TargetConfig {})
}

#[test]
fn stringifies_header_values_before_passing_them_to_httpx() {
    let ir = CoreIr {
        operations: vec![Operation {
            id: "operation.list_widgets".into(),
            name: "list_widgets".into(),
            method: HttpMethod::Get,
            path: "/widgets".into(),
            params: vec![
                Parameter {
                    name: "x-enabled".into(),
                    location: ParameterLocation::Header,
                    type_ref: TypeRef::primitive("boolean"),
                    required: true,
                    attributes: Attributes::default(),
                },
                Parameter {
                    name: "x-attempts".into(),
                    location: ParameterLocation::Header,
                    type_ref: TypeRef::primitive("integer"),
                    required: false,
                    attributes: Attributes::default(),
                },
            ],
            request_body: None,
            responses: vec![Response {
                status: "200".into(),
                media_type: Some("application/json".into()),
                type_ref: Some(TypeRef::primitive("string")),
                attributes: Attributes::default(),
            }],
            attributes: Attributes::default(),
            source: None,
        }],
        ..Default::default()
    };

    let files = make_package_from_ir(ir, "demo_client").expect("package should render");
    let client = files
        .iter()
        .find(|file| file.path.ends_with("client.py"))
        .expect("client.py");

    assert!(
        client
            .contents
            .contains("def _stringify_header_value(value: Any) -> str:")
    );
    assert!(
        client
            .contents
            .contains("headers=self._stringify_headers(headers),")
    );
}
