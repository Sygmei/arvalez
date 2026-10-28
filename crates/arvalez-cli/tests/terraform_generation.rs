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
fn complex_openapi_generates_without_terraform_representation_annotations() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("arvalez.toml"),
        r#"
[common.package]
version = "2.0.0"
[target.terraform]
module_path = "example.com/tfmodels"
sdk_module_path = "example.com/sdk/v2"
[target.terraform.package]
name = "tfmodels"
"#,
    )
    .unwrap();
    fs::write(dir.path().join("openapi.json"), r##"{
      "openapi":"3.1.0", "paths":{}, "components":{"schemas":{
        "Node":{"type":"object","properties":{
          "label":{"type":"string"},
          "payload":{},
          "choice":{"oneOf":[{"type":"string"},{"type":"integer"}]},
          "children":{"type":"array","items":{"$ref":"#/components/schemas/Node"}},
          "tuple":{"type":"array","prefixItems":[{"type":"string"},{"type":"integer"}],"items":false,"minItems":2},
          "metadata":{"type":"object","additionalProperties":true}
        }}
      }}
    }"##).unwrap();
    success(run(
        dir.path(),
        &["generate-terraform", "--openapi", "openapi.json"],
    ));
    let output = dir.path().join("generated/terraform");
    let adapters = fs::read_to_string(output.join("adapters.go")).unwrap();
    assert!(adapters.contains("jsonCodec[[]sdk.Node]()"));
    assert!(adapters.contains("jsonCodec[any]()"));
    assert!(adapters.contains("jsonCodec[[2]any]()"));
    assert!(adapters.contains("jsonCodec[map[string]any]()"));
    let readme = fs::read_to_string(output.join("README.md")).unwrap();
    assert!(readme.contains("`Node.children`: recursive model reference"));
    assert!(!readme.contains("`Node.label`:"));
}

#[test]
fn terraform_current_sdk_version_follows_go_version_precedence() {
    let common = "[common.package]\nversion = \"2.3.0\"\n";
    let go = "[common.package]\nversion = \"2.3.0\"\n[target.go.package]\nversion = \"2.4.0\"\n";
    for (prefix, sdk_version, flags, expected) in [
        ("", None, vec![], "v0.1.0"),
        (common, None, vec![], "v2.3.0"),
        (common, Some("current"), vec![], "v2.3.0"),
        (go, None, vec![], "v2.4.0"),
        (
            go,
            Some("current"),
            vec!["--output-version", "2.5.0"],
            "v2.5.0",
        ),
        (
            go,
            None,
            vec!["--output-version", "v2.5.0-rc.1"],
            "v2.5.0-rc.1",
        ),
        (
            go,
            Some("v2.0.0"),
            vec!["--output-version", "2.5.0"],
            "v2.0.0",
        ),
        (
            go,
            Some("v2.0.0"),
            vec!["--sdk-version", "current"],
            "v2.4.0",
        ),
        (
            go,
            Some("current"),
            vec!["--sdk-version", "v2.6.0"],
            "v2.6.0",
        ),
    ] {
        let dir = tempdir().unwrap();
        let setting = sdk_version
            .map(|v| format!("sdk_version = \"{v}\"\n"))
            .unwrap_or_default();
        let sdk_module = if expected.starts_with("v2.") {
            "example.com/sdk/v2"
        } else {
            "example.com/sdk"
        };
        let config = format!(
            "{prefix}{}",
            CONFIG
                .replace("sdk_version = \"v2.0.0\"\n", &setting)
                .replace("example.com/sdk/v2", sdk_module)
                .replace(
                    "name = \"adapters\"",
                    "name = \"adapters\"\nversion = \"9.0.0\""
                )
        );
        fs::write(dir.path().join("arvalez.toml"), config).unwrap();
        fs::write(dir.path().join("openapi.json"), SPEC).unwrap();
        let mut args = vec!["generate-terraform", "--openapi", "openapi.json"];
        args.extend(flags);
        success(run(dir.path(), &args));
        let gomod = fs::read_to_string(dir.path().join("generated/terraform/go.mod")).unwrap();
        assert!(
            gomod.contains(&format!("{sdk_module} {expected}\n")),
            "{gomod}"
        );
    }
}

#[test]
fn aggregate_generation_resolves_current_and_rejects_invalid_versions_before_writing() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("openapi.json"), SPEC).unwrap();
    fs::write(
        dir.path().join("arvalez.toml"),
        CONFIG.replace("sdk_version = \"v2.0.0\"\n", ""),
    )
    .unwrap();
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--output-version",
            "2.6.0",
            "--no-python",
            "--no-typescript",
            "--no-nushell",
        ],
    ));
    let gomod = fs::read_to_string(dir.path().join("generated/terraform/go.mod")).unwrap();
    assert!(gomod.contains("example.com/sdk/v2 v2.6.0\n"));
    assert!(dir.path().join("generated/go-client/models.go").exists());
    for command in ["generate", "generate-terraform"] {
        let output = run(
            dir.path(),
            &[
                command,
                "--openapi",
                "openapi.json",
                "--output-version",
                "not-a-version",
                "--output-directory",
                "invalid",
            ],
        );
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("sdk_version must resolve"));
        assert!(!dir.path().join("invalid").exists());
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
