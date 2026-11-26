//! Where the app keeps its own state (outside the vault).

use std::path::{Path, PathBuf};

use ruby_core::workspace_state::workspace_file_name;

/// `%APPDATA%\Ruby`, or `RUBY_CONFIG_DIR` when set (isolated runs and tests).
pub fn config_dir() -> PathBuf {
	if let Some(dir) = std::env::var_os("RUBY_CONFIG_DIR") {
		return PathBuf::from(dir);
	}
	dirs::config_dir()
		.unwrap_or_else(std::env::temp_dir)
		.join("Ruby")
}

pub fn config_file() -> PathBuf {
	config_dir().join("config.json")
}

pub fn workspace_file(vault: &Path) -> PathBuf {
	config_dir()
		.join("workspaces")
		.join(workspace_file_name(vault))
}
