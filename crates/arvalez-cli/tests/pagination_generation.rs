use std::{fs, path::Path, process::Command};
use tempfile::tempdir;

fn checked(output: std::process::Output) {
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn available(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[test]
fn all_language_paginators_execute_in_flat_and_grouped_clients() {
    let root = tempdir().unwrap();
    fs::write(
        root.path().join("openapi.json"),
        include_str!("fixtures/pagination.json"),
    )
    .unwrap();
    let python = std::env::var("ARVALEZ_TEST_PYTHON").unwrap_or_else(|_| "python3".into());
    for grouped in [false, true] {
        let config = format!(
            "{}\n[common.output]\ngroup_by_tag = {grouped}\n",
            include_str!("fixtures/pagination.toml")
        );
        fs::write(root.path().join("arvalez.toml"), config).unwrap();
        for target in ["python", "python-mini", "typescript", "nushell"] {
            let out = root.path().join(format!("{target}-{grouped}"));
            checked(
                Command::new(env!("CARGO_BIN_EXE_arvalez-cli"))
                    .args([
                        &format!("generate-{target}"),
                        "--openapi",
                        "openapi.json",
                        "--output-directory",
                    ])
                    .arg(&out)
                    .current_dir(root.path())
                    .output()
                    .unwrap(),
            );
            let runtime_available = match target {
                "python" | "python-mini" => available(
                    &python,
                    &[
                        "-c",
                        "import httpx, pydantic; assert pydantic.__version__.startswith('2.')",
                    ],
                ),
                "typescript" => {
                    available("tsc", &["--version"]) && available("node", &["--version"])
                }
                "nushell" => available("nu", &["--version"]),
                _ => unreachable!(),
            };
            if !runtime_available {
                assert!(
                    std::env::var_os("ARVALEZ_REQUIRE_PAGINATION_RUNTIMES").is_none()
                        && (!target.starts_with("python")
                            || std::env::var_os("ARVALEZ_TEST_PYTHON").is_none()),
                    "{target} runtime/dependencies are required but unavailable"
                );
                eprintln!(
                    "{target} generated; runtime checks skipped because its runtime/dependencies are unavailable"
                );
                continue;
            }
            match target {
                "python" | "python-mini" => {
                    let script = root.path().join("check.py");
                    fs::write(&script, include_str!("check_python_pagination.py")).unwrap();
                    checked(
                        Command::new(&python)
                            .arg(script)
                            .arg(out.join("src"))
                            .arg(target)
                            .arg(grouped.to_string())
                            .output()
                            .unwrap(),
                    );
                }
                "typescript" => {
                    checked(
                        Command::new("tsc")
                            .args([
                                "-p",
                                "tsconfig.json",
                                "--module",
                                "commonjs",
                                "--moduleResolution",
                                "node",
                            ])
                            .current_dir(&out)
                            .output()
                            .unwrap(),
                    );
                    // package.json declares ESM; isolate the CommonJS test output.
                    fs::write(out.join("dist/package.json"), "{\"type\":\"commonjs\"}").unwrap();
                    let script = root.path().join("check.cjs");
                    fs::write(&script, include_str!("check_typescript_pagination.cjs")).unwrap();
                    checked(
                        Command::new("node")
                            .arg(script)
                            .arg(out.join("dist/client.js"))
                            .arg(grouped.to_string())
                            .output()
                            .unwrap(),
                    );
                }
                "nushell" => run_nushell(&out, grouped),
                _ => unreachable!(),
            }
        }
    }
}

fn run_nushell(out: &Path, grouped: bool) {
    checked(
        Command::new("nu")
            .args(["--no-config-file", "-c", "use mod.nu *"])
            .current_dir(out)
            .output()
            .unwrap(),
    );
    let tests = include_str!("check_nushell_pagination.nu")
        .replace("@GROUP@", if grouped { "widgets " } else { "" });
    fs::write(out.join("check.nu"), tests).unwrap();
    checked(
        Command::new("nu")
            .args(["--no-config-file", "check.nu"])
            .current_dir(out)
            .output()
            .unwrap(),
    );
}

#[test]
fn every_language_rejects_pagination_name_collisions_before_writing() {
    let dir = tempdir().unwrap();
    let mut spec: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/pagination.json")).unwrap();
    let mut operation = spec["paths"]["/{tenant}/PageZero"].clone();
    operation["get"]["operationId"] = "PageZeroCollect".into();
    spec["paths"]["/{tenant}/Collision"] = operation;
    fs::write(dir.path().join("openapi.json"), spec.to_string()).unwrap();
    fs::write(
        dir.path().join("arvalez.toml"),
        include_str!("fixtures/pagination.toml"),
    )
    .unwrap();
    for target in ["go", "python", "python-mini", "typescript", "nushell"] {
        let out = dir.path().join(target);
        let output = Command::new(env!("CARGO_BIN_EXE_arvalez-cli"))
            .args([
                &format!("generate-{target}"),
                "--openapi",
                "openapi.json",
                "--output-directory",
            ])
            .arg(&out)
            .current_dir(dir.path())
            .output()
            .unwrap();
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && error.contains("pagination helper name collision"),
            "{target}: {error}"
        );
        assert!(!out.exists());
    }
}
