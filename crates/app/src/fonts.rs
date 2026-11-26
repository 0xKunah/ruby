//! Bundled fonts (OFL), registered at startup so the look does not depend on installed fonts.

use std::borrow::Cow;

use gpui_kit::App;

const FONTS: [&[u8]; 7] = [
	include_bytes!("../assets/fonts/Inter-Regular.ttf"),
	include_bytes!("../assets/fonts/Inter-Medium.ttf"),
	include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
	include_bytes!("../assets/fonts/Inter-Bold.ttf"),
	include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
	include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
	include_bytes!("../assets/fonts/Kalam-Regular.ttf"),
];

pub fn register(cx: &mut App) {
	let fonts = FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
	if let Err(e) = cx.text_system().add_fonts(fonts) {
		eprintln!("failed to register bundled fonts: {e}");
	}
}
