//! Asset source: the app's extra lucide icons, falling back to gpui-kit's default assets.

use std::borrow::Cow;

use gpui_kit::{AssetSource, Result, SharedString};

const EXTRA_ICONS: [(&str, &[u8]); 16] = [
	(
		"icons/shapes.svg",
		include_bytes!("../assets/icons/shapes.svg"),
	),
	(
		"icons/image.svg",
		include_bytes!("../assets/icons/image.svg"),
	),
	(
		"icons/file-plus.svg",
		include_bytes!("../assets/icons/file-plus.svg"),
	),
	(
		"icons/folder-plus.svg",
		include_bytes!("../assets/icons/folder-plus.svg"),
	),
	("icons/hand.svg", include_bytes!("../assets/icons/hand.svg")),
	(
		"icons/mouse-pointer.svg",
		include_bytes!("../assets/icons/mouse-pointer.svg"),
	),
	(
		"icons/square.svg",
		include_bytes!("../assets/icons/square.svg"),
	),
	(
		"icons/diamond.svg",
		include_bytes!("../assets/icons/diamond.svg"),
	),
	(
		"icons/circle.svg",
		include_bytes!("../assets/icons/circle.svg"),
	),
	(
		"icons/arrow-up-right.svg",
		include_bytes!("../assets/icons/arrow-up-right.svg"),
	),
	(
		"icons/slash.svg",
		include_bytes!("../assets/icons/slash.svg"),
	),
	(
		"icons/pencil.svg",
		include_bytes!("../assets/icons/pencil.svg"),
	),
	("icons/type.svg", include_bytes!("../assets/icons/type.svg")),
	(
		"icons/eraser.svg",
		include_bytes!("../assets/icons/eraser.svg"),
	),
	(
		"icons/undo-2.svg",
		include_bytes!("../assets/icons/undo-2.svg"),
	),
	(
		"icons/redo-2.svg",
		include_bytes!("../assets/icons/redo-2.svg"),
	),
];

pub struct Assets;

impl AssetSource for Assets {
	fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
		if let Some((_, bytes)) = EXTRA_ICONS.iter().find(|(p, _)| *p == path) {
			return Ok(Some(Cow::Borrowed(*bytes)));
		}
		gpui_kit::assets::Assets.load(path)
	}

	fn list(&self, path: &str) -> Result<Vec<SharedString>> {
		let mut out = gpui_kit::assets::Assets.list(path)?;
		out.extend(
			EXTRA_ICONS
				.iter()
				.filter(|(p, _)| p.starts_with(path))
				.map(|(p, _)| SharedString::from(*p)),
		);
		Ok(out)
	}
}
