//! App settings (editor text size and width, default mode, attachments folder, language)
//! and the dialog that edits them.

use std::path::{Component, Path};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::*;
use ruby_core::workspace_state::{AppConfig, load_json, save_json};

use crate::i18n::{self, I18n, Language, fmt, tr};
use crate::paths;
use crate::theme::palette;

#[derive(Debug, Clone, PartialEq)]
pub struct AppSettings {
	pub font_size: f32,
	pub max_width: f32,
	/// Relative to the vault, with `/` separators.
	pub attachments_folder: String,
	pub default_live: bool,
}

impl Global for AppSettings {}

impl AppSettings {
	fn from_config(config: &AppConfig) -> Self {
		let mut settings = AppSettings {
			font_size: config.font_size,
			max_width: config.max_width,
			attachments_folder: config.attachments_folder.clone(),
			default_live: config.default_live,
		};
		settings.sanitize(&AppConfig::default().attachments_folder);
		settings
	}

	/// Keeps every value in its range; a folder outside the vault falls back to `folder`.
	fn sanitize(&mut self, folder: &str) {
		self.font_size = self.font_size.clamp(12., 24.);
		self.max_width = self.max_width.clamp(500., 1400.);
		let cleaned = self
			.attachments_folder
			.trim()
			.replace('\\', "/")
			.trim_matches('/')
			.to_string();
		let inside_vault = !cleaned.contains(':')
			&& Path::new(&cleaned)
				.components()
				.all(|c| matches!(c, Component::Normal(_)));
		self.attachments_folder = if inside_vault {
			cleaned
		} else {
			folder.to_string()
		};
	}
}

/// Loads the settings from the config file (tests always start from the defaults).
pub fn init(cx: &mut App) {
	let config = if cfg!(test) {
		AppConfig::default()
	} else {
		load_json(&paths::config_file())
	};
	cx.set_global(AppSettings::from_config(&config));
}

pub fn get(cx: &App) -> &AppSettings {
	cx.global::<AppSettings>()
}

/// Changes settings, saves them and repaints every window.
pub fn update(cx: &mut App, change: impl FnOnce(&mut AppSettings)) {
	let mut settings = get(cx).clone();
	let previous_folder = settings.attachments_folder.clone();
	change(&mut settings);
	settings.sanitize(&previous_folder);
	if settings == *get(cx) {
		return;
	}
	// Tests share one config dir: they never write settings to it.
	if !cfg!(test) {
		let mut config: AppConfig = load_json(&paths::config_file());
		config.font_size = settings.font_size;
		config.max_width = settings.max_width;
		config.attachments_folder = settings.attachments_folder.clone();
		config.default_live = settings.default_live;
		if let Err(e) = save_json(&paths::config_file(), &config) {
			eprintln!("failed to save config: {e}");
		}
	}
	cx.set_global(settings);
	cx.refresh_windows();
}

fn label(text: &'static str) -> Div {
	div()
		.text_size(px(12.))
		.font_weight(FontWeight::MEDIUM)
		.text_color(rgb(palette::FG2))
		.child(text)
}

/// A value with `-` and `+` buttons.
fn stepper(
	id: &'static str,
	value: String,
	step: f32,
	field: fn(&mut AppSettings) -> &mut f32,
) -> Div {
	let button = |suffix: &'static str, text: &'static str, delta: f32| {
		Button::new((id, if suffix == "minus" { 0usize } else { 1 }))
			.label(text)
			.outline()
			.on_click(move |_, _, cx| update(cx, |s| *field(s) += delta))
	};
	div()
		.flex()
		.items_center()
		.gap(px(6.))
		.child(button("minus", "-", -step))
		.child(
			div()
				.w(px(64.))
				.text_size(px(13.))
				.text_color(rgb(palette::FG))
				.flex()
				.justify_center()
				.child(value),
		)
		.child(button("plus", "+", step))
}

fn choice(id: &'static str, text: &'static str, selected: bool, live: bool) -> Button {
	let button = Button::new(id)
		.label(text)
		.on_click(move |_, _, cx| update(cx, |s| s.default_live = live));
	if selected {
		button.primary()
	} else {
		button.outline()
	}
}

fn language_button(
	id: &'static str,
	label: String,
	selected: bool,
	setting: Option<Language>,
) -> Button {
	let button = Button::new(id)
		.label(label)
		.on_click(move |_, _, cx| i18n::set_language(setting, cx));
	if selected {
		button.primary()
	} else {
		button.outline()
	}
}

pub fn open_settings(window: &mut Window, cx: &mut App) {
	let folder = get(cx).attachments_folder.clone();
	let folder_input = cx.new(|cx| InputState::new(window, cx).default_value(folder));
	cx.subscribe(&folder_input, |input, event: &InputEvent, cx| {
		if matches!(event, InputEvent::Change) {
			let value = input.read(cx).value().to_string();
			update(cx, |s| s.attachments_folder = value);
		}
	})
	.detach();
	window.open_dialog(cx, move |dialog, _, cx| {
		let t = tr(cx);
		let settings = get(cx).clone();
		let current = cx.global::<I18n>().setting;
		let system = i18n::system_locale()
			.as_deref()
			.and_then(Language::from_locale)
			.unwrap_or(Language::En);
		let system_label = fmt(t.system_language, &[&system.native_name()]);
		let mut buttons = vec![language_button(
			"lang-system",
			system_label,
			current.is_none(),
			None,
		)];
		for (id, lang) in [("lang-en", Language::En), ("lang-fr", Language::Fr)] {
			buttons.push(language_button(
				id,
				lang.native_name().to_string(),
				current == Some(lang),
				Some(lang),
			));
		}
		dialog.title(t.settings).w(px(440.)).child(
			div()
				.flex()
				.flex_col()
				.gap(px(8.))
				.child(
					div()
						.text_size(px(12.))
						.font_weight(FontWeight::MEDIUM)
						.text_color(rgb(palette::FG2))
						.child(t.language),
				)
				.child(div().flex().gap(px(6.)).children(buttons))
				.child(label(t.setting_font_size).mt(px(8.)))
				.child(stepper(
					"font-size",
					format!("{} px", settings.font_size),
					1.,
					|s| &mut s.font_size,
				))
				.child(label(t.setting_max_width).mt(px(8.)))
				.child(stepper(
					"max-width",
					format!("{} px", settings.max_width),
					40.,
					|s| &mut s.max_width,
				))
				.child(label(t.setting_default_mode).mt(px(8.)))
				.child(
					div()
						.flex()
						.gap(px(6.))
						.child(choice(
							"mode-live",
							t.mode_live,
							settings.default_live,
							true,
						))
						.child(choice(
							"mode-source",
							t.mode_source,
							!settings.default_live,
							false,
						)),
				)
				.child(label(t.setting_attachments).mt(px(8.)))
				.child(Input::new(&folder_input)),
		)
	});
}

#[cfg(test)]
mod tests {
	use crate::i18n::{self, I18n, Language, tr};
	use gpui_kit::TestAppContext;
	use ruby_core::workspace_state::{AppConfig, load_json};

	#[gpui_kit::test]
	fn changing_language_applies_and_persists(cx: &mut TestAppContext) {
		cx.update(crate::test_support::init);
		cx.update(|cx| i18n::set_language(Some(Language::Fr), cx));
		cx.update(|cx| {
			assert_eq!(tr(cx).new_note, "Nouvelle note");
			assert_eq!(cx.global::<I18n>().setting, Some(Language::Fr));
		});
		let config: AppConfig = load_json(&crate::paths::config_file());
		assert_eq!(config.language.as_deref(), Some("fr"));
		cx.update(|cx| i18n::set_language(Some(Language::En), cx));
		cx.update(|cx| assert_eq!(tr(cx).new_note, "New note"));
	}
}
