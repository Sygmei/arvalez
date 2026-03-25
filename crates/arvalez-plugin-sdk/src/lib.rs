use std::{env, fs};

use anyhow::{Context, Result};
use arvalez_ir::{CoreIr, PluginRequest, PluginResponse, Target, validate_ir};
use serde::de::DeserializeOwned;

const REQUEST_PATH_ENV: &str = "ARVALEZ_REQUEST_PATH";
const RESPONSE_PATH_ENV: &str = "ARVALEZ_RESPONSE_PATH";

pub trait Plugin {
    type Options: DeserializeOwned + Default;

    fn transform_core(
        &self,
        ctx: &PluginContext<Self::Options>,
        ir: CoreIr,
    ) -> Result<TransformOutput>;
}

#[derive(Debug, Clone)]
pub struct PluginContext<Options> {
    plugin_name: String,
    target: Option<Target>,
    options: Options,
}

impl<Options> PluginContext<Options> {
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }

    pub fn target(&self) -> Option<Target> {
        self.target
    }

    pub fn options(&self) -> &Options {
        &self.options
    }
}

#[derive(Debug, Clone)]
pub struct TransformOutput {
    pub ir: CoreIr,
    pub warnings: Vec<String>,
}

impl TransformOutput {
    pub fn ok(ir: CoreIr) -> Self {
        Self {
            ir,
            warnings: Vec::new(),
        }
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

pub fn run_plugin<P>(plugin: P) -> Result<()>
where
    P: Plugin,
{
    let request_path =
        env::var(REQUEST_PATH_ENV).context("missing ARVALEZ_REQUEST_PATH environment variable")?;
    let response_path = env::var(RESPONSE_PATH_ENV)
        .context("missing ARVALEZ_RESPONSE_PATH environment variable")?;

    let request_bytes = fs::read(&request_path)
        .with_context(|| format!("failed to read plugin request from `{request_path}`"))?;
    let request: PluginRequest =
        serde_json::from_slice(&request_bytes).context("failed to deserialize plugin request")?;
    validate_ir(&request.ir).context("input IR is invalid")?;

    let PluginRequest { context, ir } = request;
    let options = if context.options.is_null() {
        P::Options::default()
    } else {
        serde_json::from_value(context.options).context("failed to deserialize plugin options")?
    };

    let plugin_context = PluginContext {
        plugin_name: context.plugin_name,
        target: context.target,
        options,
    };

    let output = plugin.transform_core(&plugin_context, ir)?;
    validate_ir(&output.ir).context("plugin returned an invalid IR")?;

    let response = PluginResponse {
        ir: output.ir,
        warnings: output.warnings,
    };
    let response_bytes =
        serde_json::to_vec_pretty(&response).context("failed to serialize plugin response")?;
    fs::write(&response_path, response_bytes)
        .with_context(|| format!("failed to write plugin response to `{response_path}`"))?;

    Ok(())
}
