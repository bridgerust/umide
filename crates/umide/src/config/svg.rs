use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use include_dir::{Dir, include_dir};

use crate::config::LOGO;

const CODICONS_ICONS_DIR: Dir =
    include_dir!("$CARGO_MANIFEST_DIR/../../icons/codicons");
const UMIDE_ICONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/../../icons/umide");

#[derive(Debug, Clone)]
pub struct SvgStore {
    svgs: HashMap<String, String>,
    svgs_on_disk: HashMap<PathBuf, Option<String>>,
}

impl Default for SvgStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SvgStore {
    fn new() -> Self {
        let mut svgs = HashMap::new();
        svgs.insert("umide_logo".to_string(), LOGO.to_string());

        Self {
            svgs,
            svgs_on_disk: HashMap::new(),
        }
    }

    pub fn logo_svg(&self) -> String {
        self.svgs.get("umide_logo").unwrap().clone()
    }

    pub fn get_default_svg(&mut self, name: &str) -> String {
        if !self.svgs.contains_key(name) {
            // Prefer a bundled UMIDE-specific icon (e.g. ai-assistant.svg,
            // umide_logo.svg, umide_remote.svg), then fall back to codicons.
            let file = UMIDE_ICONS_DIR
                .get_file(name)
                .or_else(|| CODICONS_ICONS_DIR.get_file(name))
                .unwrap_or_else(|| {
                    tracing::error!("Failed to find icon: {}", name);
                    CODICONS_ICONS_DIR.get_file("error.svg").unwrap()
                });
            let content = file.contents_utf8().unwrap();
            self.svgs.insert(name.to_string(), content.to_string());
        }
        self.svgs.get(name).unwrap().clone()
    }

    pub fn get_svg_on_disk(&mut self, path: &Path) -> Option<String> {
        if !self.svgs_on_disk.contains_key(path) {
            let svg = fs::read_to_string(path).ok();
            self.svgs_on_disk.insert(path.to_path_buf(), svg);
        }

        self.svgs_on_disk.get(path).unwrap().clone()
    }
}
