use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct TypeScriptPackageConfig {
    pub package_name: String,
    pub version: String,
    pub template_dir: Option<PathBuf>,
    pub group_by_tag: bool,
}

impl TypeScriptPackageConfig {
    pub fn new(package_name: impl Into<String>) -> Self {
        Self {
            package_name: package_name.into(),
            version: "0.1.0".into(),
            template_dir: None,
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

    pub fn with_group_by_tag(mut self, group_by_tag: bool) -> Self {
        self.group_by_tag = group_by_tag;
        self
    }
}
