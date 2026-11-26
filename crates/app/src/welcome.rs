//! First-launch screen: pick the vault folder.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::*;

use crate::i18n::tr;
use crate::theme::palette;

pub struct VaultChosen(pub PathBuf);

pub struct Welcome;

impl EventEmitter<VaultChosen> for Welcome {}

impl Welcome {
	fn choose(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let rx = cx.prompt_for_paths(PathPromptOptions {
			files: false,
			directories: true,
			multiple: false,
			prompt: Some(tr(cx).open_vault_prompt.into()),
		});
		cx.spawn_in(window, async move |this, cx| {
			if let Ok(Ok(Some(paths))) = rx.await
				&& let Some(path) = paths.into_iter().next()
			{
				this.update(cx, |_, cx| cx.emit(VaultChosen(path))).ok();
			}
		})
		.detach();
	}
}

impl Render for Welcome {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		div()
			.size_full()
			.flex()
			.flex_col()
			.items_center()
			.justify_center()
			.gap_3()
			.bg(rgb(palette::BG))
			.child(
				div()
					.text_size(px(26.))
					.font_weight(FontWeight::BOLD)
					.text_color(rgb(palette::FG))
					.child("Ruby"),
			)
			.child(
				div()
					.text_size(px(13.))
					.text_color(rgb(palette::FG2))
					.child(tr(cx).welcome_tagline),
			)
			.child(
				Button::new("open-vault")
					.primary()
					.label(tr(cx).open_vault)
					.on_click(cx.listener(|this, _, window, cx| this.choose(window, cx))),
			)
	}
}
