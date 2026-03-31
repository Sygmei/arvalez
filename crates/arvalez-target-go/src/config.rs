use std::path::PathBuf;

use crate::sanitize::sanitize_package_name;

#[derive(Debug, Clone)]
pub struct GoPackageConfig {
    pub module_path: String,
    pub package_name: String,
    pub version: String,
    pub template_dir: Option<PathBuf>,
    pub group_by_tag: bool,
}

impl GoPackageConfig {
    pub fn new(module_path: impl Into<String>) -> Self {
        let module_path = module_path.into();
        let package_name = default_package_name(&module_path);
        Self {
            module_path,
            package_name,
            version: "0.1.0".into(),
            template_dir: None,
            group_by_tag: false,
        }
    }

    pub fn with_package_name(mut self, package_name: impl Into<String>) -> Self {
        self.package_name = sanitize_package_name(&package_name.into());
        self
    }

    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn with_template_dir(mut self, template_dir: Option<PathBuf>) -> Self {
        self.template_dir = template_dir;
        self
    }

    pub fn with_group_by_tag(mut self, group_by_tag: bool) -> Self {
        self.group_by_tag = group_by_tag;
        self
    }
}

pub(crate) fn default_package_name(module_path: &str) -> String {
    module_path
        .rsplit('/')
        .next()
        .map(sanitize_package_name)
        .unwrap_or_else(|| "client".into())
}
