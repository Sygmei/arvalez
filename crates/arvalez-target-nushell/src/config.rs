use std::path::PathBuf;

/// Configuration for a generated Nushell client module.
#[derive(Debug, Clone)]
pub struct NushellPackageConfig {
    /// Name of the generated Nushell module (e.g. `my-api`).
    pub module_name: String,
    /// Package version string injected into generated files.
    pub version: String,
    /// Optional directory from which to load user-supplied template overrides.
    pub template_dir: Option<PathBuf>,
    /// Default base URL embedded in generated command stubs.
    pub default_base_url: String,
    /// When `true`, commands are emitted as `"tag-name command-name"` subcommands.
    pub group_by_tag: bool,
}

impl NushellPackageConfig {
    pub fn new(module_name: impl Into<String>) -> Self {
        Self {
            module_name: module_name.into(),
            version: "0.1.0".into(),
            template_dir: None,
            default_base_url: String::new(),
            group_by_tag: false,
        }
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_template_dir(mut self, template_dir: Option<PathBuf>) -> Self {
        self.template_dir = template_dir;
        self
    }

    pub fn with_default_base_url(mut self, url: impl Into<String>) -> Self {
        self.default_base_url = url.into();
        self
    }

    pub fn with_group_by_tag(mut self, group_by_tag: bool) -> Self {
        self.group_by_tag = group_by_tag;
        self
    }
}
