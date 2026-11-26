//! Vault-wide search shown in the left sidebar in place of the file tree (`Ctrl+Shift+F`).

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::links::slash;
use ruby_core::search::{FileResult, Query, compile, search_vault};

use crate::i18n::{fmt, tr};
use crate::theme::palette;

/// The search starts this long after the last keystroke.
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Matches listed at most.
const MAX_MATCHES: usize = 500;
const MATCH_TINT: u32 = 0xdc262655;

pub enum SearchPanelEvent {
	/// The query or an option changed: the owner calls `search` with the open notes' text.
	QueryChanged,
	Open {
		path: PathBuf,
		range: Range<usize>,
	},
	Close,
}

pub struct SearchPanel {
	vault: PathBuf,
	input: Entity<InputState>,
	case_sensitive: bool,
	regex: bool,
	results: Vec<FileResult>,
	truncated: bool,
	error: Option<String>,
	searching: bool,
	task: Option<Task<()>>,
	_subscription: Subscription,
}

impl EventEmitter<SearchPanelEvent> for SearchPanel {}

impl SearchPanel {
	pub fn new(vault: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
		let placeholder = tr(cx).search_notes_placeholder;
		let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
		let subscription = cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
			if matches!(event, InputEvent::Change) {
				cx.emit(SearchPanelEvent::QueryChanged);
			}
		});
		SearchPanel {
			vault,
			input,
			case_sensitive: false,
			regex: false,
			results: Vec::new(),
			truncated: false,
			error: None,
			searching: false,
			task: None,
			_subscription: subscription,
		}
	}

	pub fn focus_input(&self, window: &mut Window, cx: &mut App) {
		self.input.update(cx, |input, cx| input.focus(window, cx));
	}

	#[cfg(test)]
	pub fn results(&self) -> &[FileResult] {
		&self.results
	}

	/// Runs the current query off the UI thread. `open_texts` holds the text of open notes
	/// (their edits may not be saved yet), by vault-relative path.
	pub fn search(&mut self, open_texts: HashMap<PathBuf, String>, cx: &mut Context<Self>) {
		let query = Query {
			pattern: self.input.read(cx).value().to_string(),
			case_sensitive: self.case_sensitive,
			regex: self.regex,
		};
		self.task = None;
		self.error = None;
		self.searching = false;
		let matcher = match compile(&query) {
			Ok(matcher) if !query.pattern.is_empty() => matcher,
			other => {
				self.error = other.err();
				self.results.clear();
				self.truncated = false;
				cx.notify();
				return;
			}
		};
		self.searching = true;
		let vault = self.vault.clone();
		self.task = Some(cx.spawn(async move |this, cx| {
			cx.background_executor().timer(DEBOUNCE).await;
			let found = cx
				.background_spawn(async move {
					search_vault(&vault, &matcher, &open_texts, MAX_MATCHES)
				})
				.await;
			this.update(cx, |panel, cx| {
				(panel.results, panel.truncated) = found;
				panel.searching = false;
				cx.notify();
			})
			.ok();
		}));
		cx.notify();
	}

	/// Opens the note of a result with the match selected.
	pub fn open_result(&mut self, file: usize, index: usize, cx: &mut Context<Self>) {
		if let Some(result) = self.results.get(file)
			&& let Some(found) = result.matches.get(index)
		{
			cx.emit(SearchPanelEvent::Open {
				path: result.path.clone(),
				range: found.range.clone(),
			});
		}
	}

	fn toggle(
		&self,
		id: &'static str,
		label: &'static str,
		active: bool,
		flip: fn(&mut Self),
		cx: &mut Context<Self>,
	) -> impl IntoElement {
		div()
			.id(id)
			.h(px(22.))
			.px(px(6.))
			.flex()
			.items_center()
			.rounded(px(4.))
			.border_1()
			.border_color(rgb(palette::BORDER))
			.text_size(px(11.))
			.font_family("JetBrains Mono")
			.cursor_pointer()
			.text_color(rgb(if active { palette::FG } else { palette::FG3 }))
			.when(active, |d| {
				d.bg(rgb(palette::ACCENT))
					.border_color(rgb(palette::ACCENT))
			})
			.on_click(cx.listener(move |this, _, _, cx| {
				flip(this);
				cx.emit(SearchPanelEvent::QueryChanged);
				cx.notify();
			}))
			.child(label)
	}

	fn status(&self, cx: &App) -> Option<String> {
		let t = tr(cx);
		if let Some(error) = &self.error {
			return Some(error.lines().last().unwrap_or(error).to_string());
		}
		if self.input.read(cx).value().is_empty() || self.searching {
			return None;
		}
		let total: usize = self.results.iter().map(|r| r.matches.len()).sum();
		Some(if total == 0 {
			t.no_results.to_string()
		} else if self.truncated {
			fmt(t.results_truncated, &[&total])
		} else {
			fmt(t.results_count, &[&total, &self.results.len()])
		})
	}

	fn render_results(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
		let mut rows = Vec::new();
		for (file, result) in self.results.iter().enumerate() {
			let path = slash(&result.path);
			let name = path.strip_suffix(".md").unwrap_or(&path).to_string();
			rows.push(
				div()
					.px(px(10.))
					.pt(px(8.))
					.pb(px(2.))
					.flex()
					.flex_none()
					.items_center()
					.gap(px(6.))
					.text_size(px(12.))
					.font_weight(FontWeight::MEDIUM)
					.text_color(rgb(palette::FG))
					.child(Icon::new(IconName::FileText).size_3p5())
					.child(
						div()
							.flex_1()
							.min_w_0()
							.overflow_hidden()
							.whitespace_nowrap()
							.child(name),
					)
					.child(
						div()
							.text_size(px(10.5))
							.text_color(rgb(palette::FG3))
							.child(result.matches.len().to_string()),
					)
					.into_any_element(),
			);
			for (index, found) in result.matches.iter().enumerate() {
				let highlight = HighlightStyle {
					background_color: Some(rgba(MATCH_TINT).into()),
					color: Some(rgb(palette::FG).into()),
					..Default::default()
				};
				rows.push(
					div()
						.id(("search-result", file * (MAX_MATCHES + 1) + index))
						.mx(px(6.))
						.pl(px(24.))
						.pr(px(6.))
						.py(px(3.))
						.flex_none()
						.rounded(px(4.))
						.text_size(px(12.))
						.text_color(rgb(palette::FG2))
						.overflow_hidden()
						.whitespace_nowrap()
						.cursor_pointer()
						.hover(|d| d.bg(rgb(palette::MUTED2)))
						.on_click(
							cx.listener(move |this, _, _, cx| this.open_result(file, index, cx)),
						)
						.child(
							StyledText::new(found.snippet.clone())
								.with_highlights(vec![(found.highlight.clone(), highlight)]),
						)
						.into_any_element(),
				);
			}
		}
		rows
	}
}

impl Render for SearchPanel {
	fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let t = tr(cx);
		let status = self.status(cx);
		let rows = self.render_results(cx);
		div()
			.size_full()
			.flex()
			.flex_col()
			.bg(rgb(palette::SIDEBAR))
			.on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
				if ev.keystroke.key == "escape" {
					cx.emit(SearchPanelEvent::Close);
					cx.stop_propagation();
				}
			}))
			.child(
				div()
					.px(px(12.))
					.pt(px(10.))
					.pb(px(8.))
					.flex()
					.items_center()
					.text_size(px(13.))
					.font_weight(FontWeight::SEMIBOLD)
					.child(div().flex_1().child(t.search))
					.child(
						div()
							.id("search-close")
							.p(px(3.))
							.rounded(px(4.))
							.cursor_pointer()
							.text_color(rgb(palette::FG2))
							.hover(|d| d.bg(rgb(palette::MUTED2)))
							.on_click(cx.listener(|_, _, _, cx| cx.emit(SearchPanelEvent::Close)))
							.child(Icon::new(IconName::Close).size_3p5()),
					),
			)
			.child(div().px(px(10.)).child(Input::new(&self.input)))
			.child(
				div()
					.px(px(10.))
					.pt(px(6.))
					.flex()
					.items_center()
					.gap(px(4.))
					.child(self.toggle(
						"search-case",
						"Aa",
						self.case_sensitive,
						|p| p.case_sensitive = !p.case_sensitive,
						cx,
					))
					.child(self.toggle(
						"search-regex",
						".*",
						self.regex,
						|p| p.regex = !p.regex,
						cx,
					))
					.children(status.map(|s| {
						div()
							.ml(px(4.))
							.flex_1()
							.min_w_0()
							.overflow_hidden()
							.whitespace_nowrap()
							.text_size(px(11.))
							.text_color(rgb(if self.error.is_some() {
								palette::LINK
							} else {
								palette::FG3
							}))
							.child(s)
					})),
			)
			.child(
				div()
					.id("search-results")
					.flex_1()
					.min_h_0()
					.pb(px(8.))
					.flex()
					.flex_col()
					.overflow_y_scroll()
					.children(rows),
			)
	}
}
