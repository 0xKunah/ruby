#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod assets;
mod commands;
mod drawing;
mod editor;
mod file_tree;
mod fonts;
mod i18n;
mod keymap;
mod outline;
mod paths;
mod picker;
mod search_panel;
mod settings;
mod status_bar;
mod tab_bar;
#[cfg(test)]
mod test_support;
mod theme;
mod watcher;
mod welcome;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use ruby_core::workspace_state::{AppConfig, load_json, save_json};

use crate::welcome::{VaultChosen, Welcome};
use crate::workspace::Workspace;

/// Window content: the welcome screen until a vault is open, then the workspace.
enum Screen {
	Welcome(Entity<Welcome>),
	Workspace(Entity<Workspace>),
}

struct AppView {
	screen: Screen,
}

impl AppView {
	fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
		let config: AppConfig = load_json(&paths::config_file());
		let screen = match config.last_vault.filter(|p| p.is_dir()) {
			Some(vault) => Self::workspace(vault, window, cx),
			None => Self::welcome(window, cx),
		};
		AppView { screen }
	}

	fn workspace(vault: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Screen {
		let workspace = cx.new(|cx| Workspace::new(vault, window, cx));
		// "Open vault" from the command palette.
		cx.subscribe_in(
			&workspace,
			window,
			|this, _, VaultChosen(path), window, cx| {
				this.open_vault(path.clone(), window, cx);
			},
		)
		.detach();
		Screen::Workspace(workspace)
	}

	fn welcome(window: &mut Window, cx: &mut Context<Self>) -> Screen {
		let welcome = cx.new(|_| Welcome);
		cx.subscribe_in(
			&welcome,
			window,
			|this, _, VaultChosen(path), window, cx| {
				this.open_vault(path.clone(), window, cx);
			},
		)
		.detach();
		Screen::Welcome(welcome)
	}

	fn open_vault(&mut self, vault: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
		let mut config: AppConfig = load_json(&paths::config_file());
		config.last_vault = Some(vault.clone());
		if let Err(e) = save_json(&paths::config_file(), &config) {
			eprintln!("failed to save config: {e}");
		}
		self.screen = Self::workspace(vault, window, cx);
		cx.notify();
	}
}

impl Render for AppView {
	fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
		match &self.screen {
			Screen::Welcome(w) => div()
				.size_full()
				.flex()
				.flex_col()
				.child(TitleBar::new())
				.child(div().flex_1().child(w.clone())),
			Screen::Workspace(w) => div().size_full().child(w.clone()),
		}
	}
}

fn main() {
	gpui_kit::application()
		.with_assets(assets::Assets)
		.run(|cx| {
			gpui_kit::init(cx);
			fonts::register(cx);
			theme::apply(cx);
			i18n::init(cx);
			settings::init(cx);
			keymap::bind(cx);
			let options = WindowOptions {
				window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
					None,
					size(px(1280.), px(800.)),
					cx,
				))),
				window_min_size: Some(size(px(800.), px(500.))),
				..TitleBar::window_options()
			};
			gpui_kit::open_window(options, cx, |window, cx| {
				cx.new(|cx| AppView::new(window, cx))
			})
			.expect("failed to open the main window");
			cx.on_window_closed(|cx, _| {
				if cx.windows().is_empty() {
					cx.quit();
				}
			})
			.detach();
			cx.activate(true);
		});
}
