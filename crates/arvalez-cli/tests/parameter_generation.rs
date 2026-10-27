use serde_json::Value;
use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

const TARGETS: [&str; 5] = ["go", "python", "python-mini", "typescript", "nushell"];
const SPEC: &str = include_str!("fixtures/array_parameters.json");

fn generate(target: &str, spec: &Path, output: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_arvalez-cli"))
        .arg(format!("generate-{target}"))
        .arg("--openapi")
        .arg(spec)
        .arg("--output-directory")
        .arg(output)
        .output()
        .unwrap()
}

#[test]
fn every_target_rejects_undeclared_path_parameters_before_writing_files() {
    let temp = tempdir().unwrap();
    let mut spec: Value = serde_json::from_str(SPEC).unwrap();
    spec["paths"]["/v1/workspaces/{wid}/actions/{jid}"]["patch"]["parameters"]
        .as_array_mut()
        .unwrap()
        .pop();
    let input = temp.path().join("invalid.json");
    fs::write(&input, spec.to_string()).unwrap();
    for target in TARGETS {
        let dir = temp.path().join(target);
        let result = generate(target, &input, &dir);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            !result.status.success(),
            "{target} accepted an undeclared path parameter"
        );
        assert!(
            error.contains("operation update-actions-for-workspace path {wid} not declared"),
            "{target}: {error}"
        );
        assert!(
            !dir.exists(),
            "{target} wrote output before rejecting the spec"
        );
    }
}

#[test]
fn all_targets_generate_array_parameters_and_valid_source() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("openapi.json");
    fs::write(&input, SPEC).unwrap();
    for target in TARGETS {
        let dir = temp.path().join(target);
        let result = generate(target, &input, &dir);
        assert!(
            result.status.success(),
            "{target}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let (tool, args) = match target {
            "go" => ("go", vec!["test", "./..."]),
            "typescript" => ("tsc", vec!["--noEmit", "-p", "tsconfig.json"]),
            "nushell" => ("nu", vec!["--no-config-file", "-c", "use mod.nu *"]),
            _ => ("python3", vec!["-m", "compileall", "-q", "src"]),
        };
        if Command::new(tool).arg("--version").output().is_err() {
            eprintln!("{tool} unavailable; skipping {target} source check");
            continue;
        }
        let result = Command::new(tool)
            .args(args)
            .current_dir(&dir)
            .env("GOCACHE", temp.path().join("go-cache"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{target}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let python = std::env::var("ARVALEZ_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    if Command::new(&python)
        .args(["-c", "import httpx, pydantic"])
        .output()
        .is_ok_and(|output| output.status.success())
    {
        let script = temp.path().join("check_python.py");
        fs::write(&script, include_str!("check_python_parameters.py")).unwrap();
        let result = Command::new(python)
            .arg(script)
            .arg(temp.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Python wire tests: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    } else {
        eprintln!("HTTPX/Pydantic unavailable; set ARVALEZ_TEST_PYTHON to run Python wire checks");
    }
    if Command::new("tsc").arg("--version").output().is_ok()
        && Command::new("node").arg("--version").output().is_ok()
    {
        let runtime = temp.path().join("ts-runtime");
        let result = Command::new("tsc")
            .args([
                "-p",
                "tsconfig.json",
                "--module",
                "commonjs",
                "--moduleResolution",
                "node",
                "--outDir",
            ])
            .arg(&runtime)
            .current_dir(temp.path().join("typescript"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let script = temp.path().join("check_typescript.cjs");
        fs::write(&script, include_str!("check_typescript_parameters.cjs")).unwrap();
        let result = Command::new("node")
            .arg(script)
            .arg(runtime.join("client.js"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "TypeScript wire tests: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    if Command::new("nu").arg("--version").output().is_ok() {
        let dir = temp.path().join("nushell");
        fs::write(
            dir.join("check.nu"),
            include_str!("check_nushell_parameters.nu"),
        )
        .unwrap();
        let result = Command::new("nu")
            .args(["--no-config-file", "check.nu"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "Nushell request tests: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
}

#[test]
fn named_inputs_compile_and_preserve_request_behavior() {
    let temp = tempdir().unwrap();
    let spec = temp.path().join("named-inputs.json");
    fs::write(&spec, include_str!("fixtures/named_inputs.json")).unwrap();
    for grouped in [false, true] {
        for target in ["go", "typescript"] {
            let dir = temp.path().join(format!("{target}-{grouped}"));
            let mut command = Command::new(env!("CARGO_BIN_EXE_arvalez-cli"));
            command
                .arg(format!("generate-{target}"))
                .arg("--openapi")
                .arg(&spec)
                .arg("--output-directory")
                .arg(&dir);
            if grouped {
                command.arg("--group-by-tag");
            }
            let result = command.output().unwrap();
            assert!(
                result.status.success(),
                "generation: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let tools = if target == "go" {
                vec!["go", "gofmt"]
            } else {
                vec!["tsc", "node"]
            };
            if tools
                .iter()
                .any(|tool| Command::new(tool).arg("--version").output().is_err())
            {
                eprintln!("{target} toolchain unavailable; skipping named input runtime checks");
                continue;
            }
            if target == "go" {
                let mut source = include_str!("check_go_named_inputs.go").to_owned();
                if grouped {
                    for method in [
                        "GetWidget",
                        "GetWidgetRaw",
                        "Ping",
                        "ListWidgets",
                        "RequiredBody",
                        "UntypedBody",
                        "Stream",
                        "Multipart",
                        "Collision",
                    ] {
                        source = source.replace(
                            &format!("client.{method}("),
                            &format!("client.Widgets.{method}("),
                        );
                    }
                }
                fs::write(dir.join("named_inputs_test.go"), source).unwrap();
                assert!(
                    Command::new("go").arg("version").output().is_ok(),
                    "Go is required for named input integration checks"
                );
                let files: Vec<_> = fs::read_dir(&dir)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "go"))
                    .collect();
                let result = Command::new("gofmt")
                    .arg("-w")
                    .args(&files)
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "gofmt: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                for check in ["test", "vet"] {
                    let result = Command::new("go")
                        .args([check, "./..."])
                        .current_dir(&dir)
                        .env("GOCACHE", temp.path().join("go-cache"))
                        .output()
                        .unwrap();
                    assert!(
                        result.status.success(),
                        "Go {check} ({grouped}):\n{}\n{}",
                        String::from_utf8_lossy(&result.stdout),
                        String::from_utf8_lossy(&result.stderr)
                    );
                }
            } else {
                let mut source = include_str!("check_typescript_named_inputs.ts").to_owned();
                if grouped {
                    for method in [
                        "getWidget",
                        "_getWidgetRaw",
                        "listWidgets",
                        "ping",
                        "requiredBody",
                        "untypedBody",
                        "stream",
                        "multipart",
                        "collision",
                    ] {
                        source = source.replace(
                            &format!("client.{method}("),
                            &format!("client.widgets.{method}("),
                        );
                    }
                }
                fs::write(dir.join("src/named-inputs-test.ts"), source).unwrap();
                let runtime = temp.path().join(format!("runtime-{grouped}"));
                let result = Command::new("tsc")
                    .args([
                        "-p",
                        "tsconfig.json",
                        "--module",
                        "commonjs",
                        "--moduleResolution",
                        "node",
                        "--outDir",
                    ])
                    .arg(&runtime)
                    .current_dir(&dir)
                    .output()
                    .expect("TypeScript is required for named input integration checks");
                assert!(
                    result.status.success(),
                    "tsc ({grouped}):\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
                let result = Command::new("node")
                    .arg(runtime.join("named-inputs-test.js"))
                    .output()
                    .unwrap();
                assert!(
                    result.status.success(),
                    "TypeScript runtime ({grouped}):\n{}\n{}",
                    String::from_utf8_lossy(&result.stdout),
                    String::from_utf8_lossy(&result.stderr)
                );
            }
        }
    }
}

#[test]
fn named_inputs_compile_for_empty_and_minimal_apis() {
    let temp = tempdir().unwrap();
    let full: Value = serde_json::from_str(include_str!("fixtures/named_inputs.json")).unwrap();
    // Exercise imports without model, path, query, or required-input usage.
    for endpoint in [None, Some("/ping"), Some("/stream")] {
        let mut spec = full.clone();
        spec["paths"] = serde_json::json!({});
        if let Some(path) = endpoint {
            spec["paths"][path] = full["paths"][path].clone();
            let method = if path == "/ping" { "get" } else { "post" };
            spec["paths"][path][method]["responses"] =
                serde_json::json!({"204": {"description": "No content"}});
        }
        spec["components"] = serde_json::json!({});
        let input = temp.path().join("minimal.json");
        fs::write(&input, spec.to_string()).unwrap();
        for target in ["go", "typescript"] {
            let dir = temp.path().join(format!(
                "{target}-{}",
                endpoint.unwrap_or("empty").trim_start_matches('/')
            ));
            let result = generate(target, &input, &dir);
            assert!(
                result.status.success(),
                "{target}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let (tool, args) = if target == "go" {
                ("go", vec!["test", "./..."])
            } else {
                ("tsc", vec!["--noEmit", "-p", "tsconfig.json"])
            };
            if Command::new(tool).arg("--version").output().is_err() {
                eprintln!("{tool} unavailable; skipping minimal API source check");
                continue;
            }
            let result = Command::new(tool)
                .args(args)
                .current_dir(dir)
                .env("GOCACHE", temp.path().join("go-cache"))
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{target} ({endpoint:?}):\n{}\n{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}
