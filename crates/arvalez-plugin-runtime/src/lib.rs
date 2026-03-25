use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use arvalez_ir::{
    CoreIr, PluginContextEnvelope, PluginRequest, PluginResponse, Target, validate_ir,
};
use serde_json::Value;
use tempfile::tempdir;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx};

#[derive(Debug, Clone)]
pub struct WasmPluginDefinition {
    pub path: PathBuf,
    pub options: Value,
}

pub struct WasmPluginRunner {
    engine: Engine,
}

impl WasmPluginRunner {
    pub fn new() -> Result<Self> {
        Ok(Self {
            engine: Engine::default(),
        })
    }

    pub fn run(
        &self,
        plugin_name: &str,
        definition: &WasmPluginDefinition,
        target: Option<Target>,
        ir: &CoreIr,
    ) -> Result<PluginResponse> {
        validate_ir(ir).context("refusing to send invalid IR to plugin")?;

        let plugin_path = canonicalize(&definition.path)?;
        let workspace = tempdir().context("failed to create plugin workspace")?;
        let request_path = workspace.path().join("request.json");
        let response_path = workspace.path().join("response.json");

        let request = PluginRequest {
            context: PluginContextEnvelope {
                plugin_name: plugin_name.to_owned(),
                target,
                options: definition.options.clone(),
            },
            ir: ir.clone(),
        };

        let request_bytes =
            serde_json::to_vec_pretty(&request).context("failed to serialize plugin request")?;
        fs::write(&request_path, request_bytes).with_context(|| {
            format!("failed to write request file `{}`", request_path.display())
        })?;

        let module = Module::from_file(&self.engine, &plugin_path)
            .with_context(|| format!("failed to load plugin module `{}`", plugin_path.display()))?;

        let mut linker = Linker::new(&self.engine);
        wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |wasi| wasi)
            .context("failed to add WASI functions to linker")?;

        let mut wasi = WasiCtx::builder();
        wasi.arg(plugin_name);
        wasi.env("ARVALEZ_REQUEST_PATH", "request.json");
        wasi.env("ARVALEZ_RESPONSE_PATH", "response.json");
        wasi.preopened_dir(workspace.path(), ".", DirPerms::all(), FilePerms::all())
            .context("failed to preopen plugin workspace")?;

        let mut store = Store::new(&self.engine, wasi.build_p1());
        linker
            .module(&mut store, "", &module)
            .context("failed to instantiate plugin module")?;
        let start = linker
            .get_default(&mut store, "")
            .context("failed to locate plugin entrypoint")?
            .typed::<(), ()>(&store)
            .context("failed to type-check plugin entrypoint")?;
        start
            .call(&mut store, ())
            .context("plugin execution trapped")?;

        let response_bytes = fs::read(&response_path).with_context(|| {
            format!(
                "plugin did not write a response file at `{}`",
                response_path.display()
            )
        })?;
        let response: PluginResponse = serde_json::from_slice(&response_bytes)
            .context("failed to deserialize plugin response")?;
        validate_ir(&response.ir).context("plugin returned invalid IR")?;

        Ok(response)
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path)
        .with_context(|| format!("failed to resolve plugin path `{}`", path.display()))
}
