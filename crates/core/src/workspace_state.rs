//! App config and per-vault workspace state, persisted as JSON outside the vault.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::save::atomic_write;

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct AppConfig {
	pub last_vault: Option<PathBuf>,
	/// UI language code (`en`, `fr`); `None` follows the system language.
	pub language: Option<String>,
	/// Editor text size, in pixels.
	pub font_size: f32,
	/// Widest the note text gets, in pixels.
	pub max_width: f32,
	/// Where pasted images go, relative to the vault.
	pub attachments_folder: String,
	/// New tabs open in live preview (otherwise in source mode).
	pub default_live: bool,
}

impl Default for AppConfig {
	fn default() -> Self {
		AppConfig {
			last_vault: None,
			language: None,
			font_size: 15.,
			max_width: 720.,
			attachments_folder: "attachments".to_string(),
			default_live: true,
		}
	}
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Clone)]
pub struct TabState {
	pub path: PathBuf,
	pub cursor: usize,
	pub scroll_y: f32,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
#[serde(default)]
pub struct WorkspaceState {
	pub tabs: Vec<TabState>,
	pub active: Option<usize>,
	pub expanded: Vec<PathBuf>,
	pub left_width: f32,
	pub right_width: f32,
	pub left_visible: bool,
	pub right_visible: bool,
}

impl Default for WorkspaceState {
	fn default() -> Self {
		WorkspaceState {
			tabs: Vec::new(),
			active: None,
			expanded: Vec::new(),
			left_width: 200.0,
			right_width: 170.0,
			left_visible: true,
			right_visible: true,
		}
	}
}

/// Readable, file-system-safe name for a vault's state file, made unique by a hash of the
/// (case-insensitive) path: `d__notes_vault-<16 hex>.json`.
pub fn workspace_file_name(vault: &Path) -> String {
	let path = vault.to_string_lossy().to_lowercase();
	let safe: String = path
		.chars()
		.map(|c| {
			if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-') {
				c
			} else {
				'_'
			}
		})
		.collect();
	// Keep the end of the path (the vault folder name); the hash keeps names unique and the
	// whole file name well under the 255-char limit of Windows.
	let tail = safe
		.get(safe.len().saturating_sub(60)..)
		.unwrap_or(&safe)
		.trim_start_matches('_');
	format!("{tail}-{:016x}.json", fnv1a(path.as_bytes()))
}

/// FNV-1a 64: stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
	bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, b| {
		(h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3)
	})
}

/// Missing or unparsable files give the default value.
pub fn load_json<T: DeserializeOwned + Default>(path: &Path) -> T {
	std::fs::read(path)
		.ok()
		.and_then(|bytes| serde_json::from_slice(&bytes).ok())
		.unwrap_or_default()
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
	atomic_write(path, &bytes)
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::Path;

	#[test]
	fn file_name_is_stable_and_safe() {
		let name = workspace_file_name(Path::new(r"D:\Notes\Mon Vault"));
		assert!(
			name.starts_with("d__notes_mon_vault-") && name.ends_with(".json"),
			"{name}"
		);
		assert_eq!(name, workspace_file_name(Path::new(r"d:\notes\mon vault")));
		assert_ne!(
			workspace_file_name(Path::new(r"D:\Été")),
			workspace_file_name(Path::new(r"D:\Àté"))
		);
	}

	#[test]
	fn load_missing_or_corrupt_gives_default() {
		let d = tempfile::tempdir().unwrap();
		assert_eq!(
			load_json::<WorkspaceState>(&d.path().join("none.json")),
			WorkspaceState::default()
		);
		std::fs::write(d.path().join("bad.json"), "{not json").unwrap();
		assert_eq!(
			load_json::<AppConfig>(&d.path().join("bad.json")),
			AppConfig::default()
		);
	}

	#[test]
	fn roundtrip_creates_parents() {
		let d = tempfile::tempdir().unwrap();
		let path = d.path().join("a/b/ws.json");
		let mut ws = WorkspaceState::default();
		ws.tabs.push(TabState {
			path: "n.md".into(),
			cursor: 3,
			scroll_y: 10.0,
		});
		save_json(&path, &ws).unwrap();
		assert_eq!(load_json::<WorkspaceState>(&path), ws);
	}

	#[test]
	fn file_name_stays_short_for_deep_vaults() {
		let deep = format!(r"C:\{}\My Vault", "very long folder name\\".repeat(20));
		let name = workspace_file_name(std::path::Path::new(&deep));
		assert!(name.len() <= 100, "{} chars: {name}", name.len());
		assert!(name.contains("my_vault"), "{name}");
	}

	#[test]
	fn config_defaults_and_round_trip() {
		let d = tempfile::tempdir().unwrap();
		let path = d.path().join("config.json");
		std::fs::write(&path, r#"{"language":"fr"}"#).unwrap();
		let mut config: super::AppConfig = super::load_json(&path);
		assert_eq!(config.language.as_deref(), Some("fr"));
		assert_eq!(
			(config.font_size, config.max_width, config.default_live),
			(15., 720., true)
		);
		assert_eq!(config.attachments_folder, "attachments");
		config.font_size = 18.;
		config.attachments_folder = "files".into();
		super::save_json(&path, &config).unwrap();
		assert_eq!(super::load_json::<super::AppConfig>(&path), config);
	}
}
