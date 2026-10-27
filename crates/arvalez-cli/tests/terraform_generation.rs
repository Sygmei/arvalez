use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

const SPEC: &str = r##"{
  "openapi":"3.1.0","info":{"title":"Terraform test","version":"1"},
  "paths":{"/widgets":{"get":{"operationId":"getWidgets","responses":{"200":{"description":"ok","content":{"application/json":{"schema":{"type":"array","items":{"$ref":"#/components/schemas/Widget"}}}}}}}}},
  "components":{"schemas":{"Widget":{"type":"object","required":["action"],"properties":{"action":{"type":"string"},"count":{"type":"integer"},"displayName":{"type":["string","null"]}}}}}
}"##;
const CONFIG: &str = r#"
[target.terraform]
module_path = "example.com/terraform"
sdk_module_path = "example.com/sdk/v2"
sdk_version = "v2.0.0"
[target.terraform.package]
name = "adapters"
[target.terraform.models.Widget.fields.displayName]
name = "display_name"
null_behavior = "explicit_null"
"#;
fn run(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arvalez-cli"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap()
}
fn success(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn openapi_30_nullable_generates_matching_sdk_and_terraform_models() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("arvalez.toml"), CONFIG).unwrap();
    let legacy = SPEC.replace("3.1.0", "3.0.3").replace(
        "\"type\":[\"string\",\"null\"]",
        "\"type\":\"string\",\"nullable\":true",
    );
    for (version, spec) in [("30", legacy.as_str()), ("31", SPEC)] {
        fs::write(dir.path().join("openapi.json"), spec).unwrap();
        for (command, target) in [("generate-go", "sdk"), ("generate-terraform", "terraform")] {
            success(run(
                dir.path(),
                &[
                    command,
                    "--openapi",
                    "openapi.json",
                    "--output-directory",
                    &format!("{target}{version}"),
                ],
            ));
        }
    }
    for (target, file) in [
        ("sdk", "models.go"),
        ("terraform", "models.go"),
        ("terraform", "adapters.go"),
    ] {
        let legacy = fs::read_to_string(dir.path().join(format!("{target}30/{file}"))).unwrap();
        let modern = fs::read_to_string(dir.path().join(format!("{target}31/{file}"))).unwrap();
        assert_eq!(legacy, modern, "{target}/{file}");
        if target == "sdk" {
            assert!(legacy.contains("DisplayName *Nullable[string]"));
        }
    }
}
#[test]
fn terraform_cli_resolves_config_and_overrides_without_installing_dependencies() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("openapi.json"), SPEC).unwrap();
    fs::write(dir.path().join("arvalez.toml"), CONFIG).unwrap();
    success(run(
        dir.path(),
        &["generate-terraform", "--openapi", "openapi.json"],
    ));
    let output = dir.path().join("generated/terraform");
    let models = fs::read_to_string(output.join("models.go")).unwrap();
    assert!(models.contains("package adapters"));
    assert!(models.contains("tfsdk:\"count_value\""));
    assert!(models.contains("tfsdk:\"display_name\""));
    assert!(
        fs::read_to_string(output.join("runtime.go"))
            .unwrap()
            .contains("sdk \"example.com/sdk/v2\"")
    );
    assert!(!output.join("go.sum").exists());
    success(run(
        dir.path(),
        &[
            "generate-terraform",
            "--openapi",
            "openapi.json",
            "--output-directory",
            "override",
            "--package-name",
            "tfmodels",
            "--module-path",
            "example.com/override",
            "--sdk-version",
            "v2.1.0",
            "--sdk-import-path",
            "example.com/sdk/v2/client",
        ],
    ));
    let gomod = fs::read_to_string(dir.path().join("override/go.mod")).unwrap();
    assert!(gomod.contains("module example.com/override"));
    assert!(gomod.contains("v2.1.0"));
    assert!(
        fs::read_to_string(dir.path().join("override/runtime.go"))
            .unwrap()
            .contains("example.com/sdk/v2/client")
    );
    success(run(
        dir.path(),
        &[
            "dump-templates",
            "--target",
            "terraform",
            "--output-directory",
            "templates",
        ],
    ));
    fs::write(
        dir.path().join("templates/root/README.md.tera"),
        "custom {{ package_name }}",
    )
    .unwrap();
    success(run(
        dir.path(),
        &[
            "generate-terraform",
            "--openapi",
            "openapi.json",
            "--template-dir",
            "templates",
            "--output-directory",
            "custom",
        ],
    ));
    assert_eq!(
        fs::read_to_string(dir.path().join("custom/README.md")).unwrap(),
        "custom adapters"
    );
    success(run(
        dir.path(),
        &[
            "dump-erasers",
            "--target",
            "terraform",
            "--output-directory",
            "erasers",
        ],
    ));
    assert!(dir.path().join("erasers/root/~schema.go.tera").exists());
}
#[test]
fn terraform_is_opt_in_and_respects_disable_flags() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("openapi.json"), SPEC).unwrap();
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--no-python",
            "--no-typescript",
            "--no-nushell",
            "--output-directory",
            "normal",
        ],
    ));
    assert!(dir.path().join("normal/go-client/models.go").exists());
    assert!(!dir.path().join("normal/terraform").exists());
    fs::write(dir.path().join("arvalez.toml"), CONFIG).unwrap();
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--no-go",
            "--no-python",
            "--no-typescript",
            "--no-nushell",
            "--output-directory",
            "aggregate",
        ],
    ));
    assert!(dir.path().join("aggregate/terraform/models.go").exists());
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--no-terraform",
            "--no-python",
            "--no-typescript",
            "--no-nushell",
            "--output-directory",
            "disabled",
        ],
    ));
    assert!(!dir.path().join("disabled/terraform").exists());
    fs::write(
        dir.path().join("arvalez.toml"),
        CONFIG.replace("[target.terraform]", "[target.terraform]\ndisabled = true"),
    )
    .unwrap();
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--no-python",
            "--no-typescript",
            "--no-nushell",
            "--output-directory",
            "configdisabled",
        ],
    ));
    assert!(!dir.path().join("configdisabled/terraform").exists());
}
#[test]
fn terraform_rejects_missing_settings_and_invalid_models_before_writing() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("openapi.json"), SPEC).unwrap();
    let output = run(
        dir.path(),
        &[
            "generate-terraform",
            "--openapi",
            "openapi.json",
            "--output-directory",
            "missing",
        ],
    );
    assert!(!output.status.success());
    assert!(!dir.path().join("missing").exists());
    fs::write(
        dir.path().join("arvalez.toml"),
        CONFIG.replace("name = \"display_name\"", "name = \"action\""),
    )
    .unwrap();
    let output = run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--output-directory",
            "invalid",
        ],
    );
    assert!(!output.status.success());
    assert!(!dir.path().join("invalid").exists());
    assert!(String::from_utf8_lossy(&output.stderr).contains("colliding Terraform"));
}
