//! Dark theme from the design (spec section 4).
//!
//! gpui-component colors come from `assets/theme.json`; views that draw their own chrome use
//! `palette` directly.

use gpui_kit::App;
use gpui_kit::component::{Theme, ThemeMode, ThemeRegistry};

pub mod palette {
	pub const BG: u32 = 0x1c1c1e;
	pub const SIDEBAR: u32 = 0x19191b;
	pub const MUTED: u32 = 0x242427;
	pub const MUTED2: u32 = 0x2c2c30;
	pub const BORDER: u32 = 0x2e2e32;
	pub const FG: u32 = 0xfafafa;
	pub const FG2: u32 = 0xa3a3a3;
	pub const FG3: u32 = 0x737373;
	pub const ACCENT: u32 = 0xdc2626;
	pub const LINK: u32 = 0xf87171;
	/// Text color of note content.
	pub const TEXT: u32 = 0xe5e5e5;
	/// RGBA.
	pub const SELECTION: u32 = 0xdc262647;
}

const THEME_JSON: &str = include_str!("../assets/theme.json");
const THEME_NAME: &str = "Ruby Dark";

pub fn apply(cx: &mut App) {
	if let Err(e) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME_JSON) {
		eprintln!("invalid bundled theme: {e}");
	}
	Theme::change(ThemeMode::Dark, None, cx);
	let config = ThemeRegistry::global(cx).themes().get(THEME_NAME).cloned();
	if let Some(config) = config {
		Theme::update(cx, |theme| theme.apply_config(&config));
	}
}
