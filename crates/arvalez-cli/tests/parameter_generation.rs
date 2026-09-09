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
