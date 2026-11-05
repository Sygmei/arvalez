use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

const SPEC: &str = include_str!("fixtures/response_fields.json");
const CONFIG: &str = r#"
[common.models]
response_fields = "always_present"
[common.models.overrides.Partial]
response_fields = "schema"
[common.package]
name = "sdk"
[target.go]
module_path = "example.com/sdk"
[target.terraform]
module_path = "example.com/adapters"
sdk_module_path = "example.com/sdk"
sdk_version = "v1.0.0"
[target.terraform.package]
name = "adapters"
[target.terraform.models.Widget.fields.nickname]
name = "display_name"
null_behavior = "explicit_null"
[target.terraform.models.WidgetResponse.fields.quantity]
name = "amount"
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
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn setup(dir: &Path, config: &str) {
    fs::write(dir.join("openapi.json"), SPEC).unwrap();
    fs::write(dir.join("arvalez.toml"), config).unwrap();
}
fn generate(dir: &Path, target: &str, dest: &str) {
    success(run(
        dir,
        &[
            &format!("generate-{target}"),
            "--openapi",
            "openapi.json",
            "--output-directory",
            dest,
        ],
    ));
}
fn source(dir: &Path, file: &str) -> String {
    fs::read_to_string(dir.join(file)).unwrap()
}
fn available(tool: &str, arg: &str) -> bool {
    Command::new(tool)
        .arg(arg)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn common_policies_reach_every_target_and_terraform_follows_go() {
    let dir = tempdir().unwrap();
    setup(dir.path(), CONFIG);
    for (target, dest) in [
        ("go", "sdk"),
        ("typescript", "ts"),
        ("python", "py"),
        ("python-mini", "mini"),
        ("nushell", "nu"),
        ("terraform", "tf"),
    ] {
        generate(dir.path(), target, dest);
    }
    let go = source(dir.path(), "sdk/models.go");
    assert!(go.contains("Nickname *string `json:\"nickname\"`"));
    assert!(go.contains("Nickname *Nullable[string] `json:\"nickname,omitempty\"`"));
    assert!(go.contains("Labels []string `json:\"labels\"`"));
    assert!(go.contains("Note *Nullable[string] `json:\"note,omitempty\"`"));
    assert!(go.contains("Message *string `json:\"message\"`"));
    assert!(go.contains("Input *Nullable[string] `json:\"input,omitempty\"`"));
    assert!(source(dir.path(), "ts/src/models.ts").contains("nickname: string | null;"));
    for path in ["py/src/sdk/models.py", "mini/src/sdk/models.py"] {
        let py = source(dir.path(), path);
        let response = py.split("class WidgetResponse(").nth(1).unwrap();
        assert!(response.contains("nickname: str | None\n"));
        assert!(response.contains("title: str\n"));
        assert!(!response.contains("title: str ="));
    }
    assert!(source(dir.path(), "nu/models.nu").contains("nickname: any"));
    let adapters = source(dir.path(), "tf/adapters.go");
    assert!(adapters.contains("WidgetResponseModelFromAPI"));
    assert!(adapters.contains("\"display_name\""));
    assert!(adapters.contains("\"amount\""));
    // Aggregate and standalone paths must resolve identical models.
    success(run(
        dir.path(),
        &[
            "generate",
            "--openapi",
            "openapi.json",
            "--output-directory",
            "all",
        ],
    ));
    assert_eq!(go, source(dir.path(), "all/go-client/models.go"));
    assert_eq!(adapters, source(dir.path(), "all/terraform/adapters.go"));
    // Adapter-only operation still inherits Go settings even when Go is disabled.
    let config = CONFIG.replace("[target.go]\n", "[target.go]\ndisabled = true\n");
    fs::write(dir.path().join("arvalez.toml"), config).unwrap();
    generate(dir.path(), "terraform", "tf-alone");
    assert_eq!(adapters, source(dir.path(), "tf-alone/adapters.go"));
}

#[test]
fn target_and_model_overrides_and_invalid_configuration() {
    let dir = tempdir().unwrap();
    setup(
        dir.path(),
        &format!(
            "{CONFIG}\n[target.go.models]\nresponse_fields = 'schema'\n[target.go.models.overrides.Partial]\nresponse_fields = 'always_present'\n[target.typescript.models]\nresponse_fields = 'schema'\n"
        ),
    );
    generate(dir.path(), "go", "sdk");
    let go = source(dir.path(), "sdk/models.go");
    assert!(go.contains("Note *string `json:\"note\"`"));
    assert!(go.contains("Nickname *Nullable[string] `json:\"nickname,omitempty\"`"));
    // A parent needs a response variant when a nested model changes, even under schema policy.
    assert!(go.contains("type WidgetResponse struct"));
    assert!(go.contains("Partial *PartialResponse"));
    generate(dir.path(), "typescript", "ts");
    let ts = source(dir.path(), "ts/src/models.ts");
    assert!(!ts.contains("interface WidgetResponse"));
    assert!(ts.contains("nickname?: string | null;"));
    for (config, message) in [
        (
            "[common.models]\nresponse_fields = 'typo'",
            "unknown variant",
        ),
        (
            "[common.models.overrides.NoSuchModel]\nresponse_fields = 'schema'",
            "unknown model policy override",
        ),
    ] {
        fs::write(dir.path().join("arvalez.toml"), config).unwrap();
        for target in ["go", "python", "python-mini", "typescript", "nushell"] {
            let out = run(
                dir.path(),
                &[
                    &format!("generate-{target}"),
                    "--openapi",
                    "openapi.json",
                    "--output-directory",
                    "invalid",
                ],
            );
            assert!(!out.status.success());
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(message),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}

#[test]
fn explicit_schema_and_target_opt_out_preserve_default_generation() {
    let dir = tempdir().unwrap();
    setup(dir.path(), "[common.package]\nname = 'sdk'\n");
    for (target, key, file) in [
        ("go", "go", "models.go"),
        ("typescript", "typescript", "src/models.ts"),
        ("python", "python", "src/sdk/models.py"),
        ("python-mini", "pythonmini", "src/sdk/models.py"),
        ("nushell", "nushell", "models.nu"),
    ] {
        fs::write(
            dir.path().join("arvalez.toml"),
            "[common.package]\nname = 'sdk'\n",
        )
        .unwrap();
        generate(dir.path(), target, "default");
        let baseline = source(dir.path(), &format!("default/{file}"));
        fs::write(dir.path().join("arvalez.toml"), format!("[common.package]\nname = 'sdk'\n[common.models]\nresponse_fields = 'always_present'\n[target.{key}.models]\nresponse_fields = 'schema'\n")).unwrap();
        generate(dir.path(), target, "opt-out");
        assert_eq!(
            baseline,
            source(dir.path(), &format!("opt-out/{file}")),
            "{target}"
        );
    }
}

#[test]
fn nullable_openapi_30_and_31_project_identically() {
    let dir = tempdir().unwrap();
    setup(
        dir.path(),
        "[common.models]\nresponse_fields = 'always_present'\n",
    );
    let mut spec = serde_json::json!({
        "openapi":"3.1.0", "info":{"title":"nullable", "version":"1"},
        "paths":{"/value":{"get":{"operationId":"getValue", "responses":{"200":{"description":"ok", "content":{"application/json":{"schema":{"$ref":"#/components/schemas/Value"}}}}}}}},
        "components":{"schemas":{"Value":{"type":"object", "properties":{
            "text":{"type":["string", "null"]},
            "items":{"type":["array", "null"], "items":{"type":"integer"}}
        }}}}
    });
    fs::write(dir.path().join("openapi.json"), spec.to_string()).unwrap();
    generate(dir.path(), "go", "v31");
    spec["openapi"] = serde_json::json!("3.0.3");
    for (field, ty) in [("text", "string"), ("items", "array")] {
        spec["components"]["schemas"]["Value"]["properties"][field]["type"] = serde_json::json!(ty);
        spec["components"]["schemas"]["Value"]["properties"][field]["nullable"] =
            serde_json::json!(true);
    }
    fs::write(dir.path().join("openapi.json"), spec.to_string()).unwrap();
    generate(dir.path(), "go", "v30");
    assert_eq!(
        source(dir.path(), "v30/models.go"),
        source(dir.path(), "v31/models.go")
    );
}

#[test]
fn generated_clients_and_adapters_execute_response_policy() {
    let dir = tempdir().unwrap();
    setup(dir.path(), CONFIG);
    for (target, dest) in [
        ("go", "sdk"),
        ("terraform", "tf"),
        ("typescript", "ts"),
        ("python", "py"),
        ("python-mini", "mini"),
        ("nushell", "nu"),
    ] {
        generate(dir.path(), target, dest);
    }
    if available("go", "version") {
        fs::write(
            dir.path().join("sdk/response_test.go"),
            include_str!("check_response_fields.go"),
        )
        .unwrap();
        fs::write(
            dir.path().join("tf/response_test.go"),
            include_str!("check_response_adapters.go"),
        )
        .unwrap();
        let modfile = dir.path().join("tf/go.mod");
        fs::write(
            &modfile,
            format!(
                "{}\nreplace example.com/sdk => ../sdk\n",
                fs::read_to_string(&modfile).unwrap()
            ),
        )
        .unwrap();
        for (subdir, args) in [
            ("sdk", vec!["test", "./..."]),
            ("sdk", vec!["vet", "./..."]),
            ("tf", vec!["mod", "tidy"]),
            ("tf", vec!["test", "./..."]),
            ("tf", vec!["vet", "./..."]),
        ] {
            success(
                Command::new("go")
                    .args(args)
                    .current_dir(dir.path().join(subdir))
                    .env("GOWORK", "off")
                    .output()
                    .unwrap(),
            );
        }
    } else {
        assert!(
            std::env::var_os("ARVALEZ_REQUIRE_GO").is_none(),
            "Go required"
        );
    }
    if available("tsc", "--version") {
        fs::write(
            dir.path().join("ts/src/policy_test.ts"),
            include_str!("check_response_fields.ts"),
        )
        .unwrap();
        success(
            Command::new("tsc")
                .arg("--noEmit")
                .current_dir(dir.path().join("ts"))
                .output()
                .unwrap(),
        );
    }
    let python = std::env::var("ARVALEZ_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    if Command::new(&python)
        .args(["-c", "import pydantic,httpx"])
        .output()
        .is_ok_and(|o| o.status.success())
    {
        for target in ["py", "mini"] {
            success(
                Command::new(&python)
                    .args(["-c", include_str!("check_response_fields.py")])
                    .env("PYTHONPATH", dir.path().join(target).join("src"))
                    .output()
                    .unwrap(),
            );
        }
    } else {
        assert!(
            std::env::var_os("ARVALEZ_TEST_PYTHON").is_none(),
            "Configured Python must provide Pydantic and HTTPX"
        );
    }
    if available("nu", "--version") {
        success(Command::new("nu").args(["--no-config-file", "-c", "use models.nu *; let p = (make-problem null); if $p.message != null { error make {msg: 'null lost'} }; let p = (make-problem 'failure'); if $p.message != 'failure' { error make {msg: 'value lost'} }"])
            .current_dir(dir.path().join("nu")).output().unwrap());
    }
}
