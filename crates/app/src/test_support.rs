//! Shared setup for GPUI tests.

use std::sync::OnceLock;

use gpui_kit::App;

/// Initializes components, key bindings and the app theme (no fonts, no window).
/// App state goes to a throwaway config dir shared by all tests of the process.
pub fn init(cx: &mut App) {
	static CONFIG: OnceLock<tempfile::TempDir> = OnceLock::new();
	let dir = CONFIG.get_or_init(|| tempfile::tempdir().expect("temp config dir"));
	// SAFETY: set once to the same value before any test reads it.
	unsafe { std::env::set_var("RUBY_CONFIG_DIR", dir.path()) };
	gpui_kit::init(cx);
	crate::theme::apply(cx);
	crate::keymap::bind(cx);
	crate::i18n::init(cx);
	crate::settings::init(cx);
}
