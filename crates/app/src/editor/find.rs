//! Find and replace in the current note: a bar above the text (`Ctrl+F`, `Ctrl+H`).

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, IconName};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::editor_state::Selection;
use ruby_core::search::{Matcher, Query, compile};

use super::{Autoscroll, NoteEditor};
use crate::i18n::tr;
use crate::theme::palette;

/// The matches and the index of the selected one.
pub(crate) type Highlights = (Rc<[Range<usize>]>, Option<usize>);

pub(crate) struct FindState {
	query: Entity<InputState>,
	replacement: Entity<InputState>,
	show_replace: bool,
	case_sensitive: bool,
	regex: bool,
	/// `None` for an empty or invalid query.
	matcher: Option<Matcher>,
	invalid: bool,
	matches: Rc<[Range<usize>]>,
	/// Text version `matches` was computed for.
	version: Option<u64>,
	_subscriptions: [Subscription; 2],
}

impl NoteEditor {
	/// Opens the find bar (with the replace row for `Ctrl+H`), seeded with the selection.
	pub(crate) fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
		if self.find.is_none() {
			let t = tr(cx);
			let query = cx.new(|cx| InputState::new(window, cx).placeholder(t.find_placeholder));
			let replacement =
				cx.new(|cx| InputState::new(window, cx).placeholder(t.replace_placeholder));
			let subscriptions = [
				cx.subscribe_in(
					&query,
					window,
					|this, _, event: &InputEvent, _, cx| match event {
						InputEvent::Change => this.find_query_changed(cx),
						InputEvent::PressEnter { shift, .. } => this.find_step(!*shift, cx),
						_ => {}
					},
				),
				cx.subscribe_in(
					&replacement,
					window,
					|this, _, event: &InputEvent, _, cx| {
						if matches!(event, InputEvent::PressEnter { .. }) {
							this.replace_current(cx);
						}
					},
				),
			];
			self.find = Some(FindState {
				query,
				replacement,
				show_replace: replace,
				case_sensitive: false,
				regex: false,
				matcher: None,
				invalid: false,
				matches: Rc::from([]),
				version: None,
				_subscriptions: subscriptions,
			});
		}
		let selected = self.primary().range();
		let seed = self.state.text()[selected].to_string();
		if !seed.is_empty() && !seed.contains('\n') {
			self.set_find_query(&seed, window, cx);
		}
		if let Some(find) = &mut self.find {
			find.show_replace |= replace;
			find.query.update(cx, |input, cx| {
				input.focus(window, cx);
				input.select_all(window, cx);
			});
		}
		cx.notify();
	}

	pub(crate) fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		self.find = None;
		window.focus(&self.focus_handle, cx);
		cx.notify();
	}

	pub(crate) fn set_find_query(
		&mut self,
		query: &str,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if let Some(find) = &self.find {
			let query = query.to_string();
			find.query
				.update(cx, |input, cx| input.set_value(query, window, cx));
			self.find_query_changed(cx);
		}
	}

	#[cfg(test)]
	pub(crate) fn set_find_replacement(
		&mut self,
		replacement: &str,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if let Some(find) = &self.find {
			let replacement = replacement.to_string();
			find.replacement
				.update(cx, |input, cx| input.set_value(replacement, window, cx));
		}
	}

	#[cfg(test)]
	pub(crate) fn set_find_options(
		&mut self,
		case_sensitive: bool,
		regex: bool,
		_window: &mut Window,
		cx: &mut Context<Self>,
	) {
		if let Some(find) = &mut self.find {
			find.case_sensitive = case_sensitive;
			find.regex = regex;
			self.find_query_changed(cx);
		}
	}

	/// Recompiles the query and selects the first match at or after the selection.
	fn find_query_changed(&mut self, cx: &mut Context<Self>) {
		let Some(find) = &mut self.find else {
			return;
		};
		let query = Query {
			pattern: find.query.read(cx).value().to_string(),
			case_sensitive: find.case_sensitive,
			regex: find.regex,
		};
		let compiled = compile(&query);
		find.invalid = compiled.is_err();
		find.matcher = compiled.ok().filter(|_| !query.pattern.is_empty());
		find.version = None;
		let from = self.primary().range().start;
		let matches = self.refresh_find();
		if let Some(ix) = matches
			.iter()
			.position(|m| m.start >= from)
			.or(if matches.is_empty() { None } else { Some(0) })
		{
			self.select_match(&matches[ix], cx);
		}
		cx.notify();
	}

	/// The matches for the current text (recomputed after an edit).
	fn refresh_find(&mut self) -> Rc<[Range<usize>]> {
		let version = self.state.version();
		let Some(find) = &mut self.find else {
			return Rc::from([]);
		};
		if find.version != Some(version) {
			find.matches = match &find.matcher {
				Some(matcher) => matcher.find_all(self.state.text()).into(),
				None => Rc::from([]),
			};
			find.version = Some(version);
		}
		find.matches.clone()
	}

	/// Matches to tint and the index of the one that is selected, while the bar is open.
	pub(crate) fn find_highlights(&mut self) -> Option<Highlights> {
		self.find.as_ref()?;
		let matches = self.refresh_find();
		let selected = self.primary().range();
		let current = matches
			.binary_search_by_key(&selected.start, |m| m.start)
			.ok()
			.filter(|ix| matches[*ix] == selected);
		Some((matches, current))
	}

	#[cfg(test)]
	pub(crate) fn find_matches(&mut self) -> (Vec<Range<usize>>, Option<usize>) {
		self.find_highlights()
			.map(|(matches, current)| (matches.to_vec(), current))
			.unwrap_or_default()
	}

	fn select_match(&mut self, found: &Range<usize>, cx: &mut Context<Self>) {
		self.state.set_selections(vec![Selection {
			anchor: found.start,
			head: found.end,
		}]);
		self.preferred_x = None;
		self.autoscroll = Some(Autoscroll::Fit);
		cx.notify();
	}

	/// Selects the next (or previous) match from the selection, wrapping around.
	pub(crate) fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
		let Some((matches, current)) = self.find_highlights() else {
			return;
		};
		if matches.is_empty() {
			return;
		}
		let selected = self.primary().range();
		let ix = if forward {
			let from = if current.is_some() {
				selected.end
			} else {
				selected.start
			};
			matches.iter().position(|m| m.start >= from).unwrap_or(0)
		} else {
			matches
				.iter()
				.rposition(|m| m.end <= selected.start)
				.unwrap_or(matches.len() - 1)
		};
		self.select_match(&matches[ix], cx);
	}

	/// Replaces the selected match and moves to the next one; selects a match first when
	/// none is selected.
	pub(crate) fn replace_current(&mut self, cx: &mut Context<Self>) {
		if self.doc.read_only {
			return;
		}
		let Some((matches, current)) = self.find_highlights() else {
			return;
		};
		let Some(found) = current.map(|ix| matches[ix].clone()) else {
			return self.find_step(true, cx);
		};
		let Some(find) = &self.find else {
			return;
		};
		let replacement = find.replacement.read(cx).value().to_string();
		let Some(matcher) = &find.matcher else {
			return;
		};
		let text = matcher.replacement_for(self.state.text(), &found, &replacement);
		self.state.break_undo_group();
		self.state.replace_range(found.clone(), &text);
		self.state.break_undo_group();
		self.state
			.set_selections(vec![Selection::caret(found.start + text.len())]);
		self.edited(cx);
		self.find_step(true, cx);
	}

	/// Replaces every match in one undo step.
	pub(crate) fn replace_all(&mut self, cx: &mut Context<Self>) {
		if self.doc.read_only {
			return;
		}
		let Some(find) = &self.find else {
			return;
		};
		let Some(matcher) = &find.matcher else {
			return;
		};
		let replacement = find.replacement.read(cx).value().to_string();
		let (text, count) = matcher.replace_all(self.state.text(), &replacement);
		if count == 0 {
			return;
		}
		let caret = self.primary_head();
		self.state.break_undo_group();
		self.state.select_all();
		self.state.insert(&text);
		self.state.break_undo_group();
		let caret = caret.min(text.len());
		let caret = (0..=caret)
			.rev()
			.find(|o| text.is_char_boundary(*o))
			.unwrap_or(0);
		self.state.set_selections(vec![Selection::caret(caret)]);
		self.edited(cx);
		self.after_command(cx);
	}

	fn find_toggle(
		&self,
		id: &'static str,
		label: &'static str,
		active: bool,
		flip: fn(&mut FindState),
		cx: &mut Context<Self>,
	) -> impl IntoElement {
		div()
			.id(id)
			.h(px(24.))
			.px(px(6.))
			.flex()
			.flex_none()
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
				if let Some(find) = &mut this.find {
					flip(find);
					this.find_query_changed(cx);
				}
			}))
			.child(label)
	}

	fn find_button(
		&self,
		id: &'static str,
		content: impl IntoElement,
		action: fn(&mut Self, &mut Window, &mut Context<Self>),
		cx: &mut Context<Self>,
	) -> impl IntoElement {
		div()
			.id(id)
			.h(px(24.))
			.px(px(6.))
			.flex()
			.flex_none()
			.items_center()
			.rounded(px(4.))
			.text_size(px(12.))
			.text_color(rgb(palette::FG2))
			.cursor_pointer()
			.hover(|d| d.bg(rgb(palette::MUTED2)))
			.on_click(cx.listener(move |this, _, window, cx| action(this, window, cx)))
			.child(content)
	}

	/// The bar shown above the note while searching.
	pub(crate) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
		let (matches, current) = self.find_highlights()?;
		let find = self.find.as_ref()?;
		let t = tr(cx);
		let count = match current {
			Some(ix) => format!("{}/{}", ix + 1, matches.len()),
			None => matches.len().to_string(),
		};
		let field = |input: &Entity<InputState>, invalid: bool| {
			div()
				.flex_1()
				.min_w(px(120.))
				.max_w(px(320.))
				.rounded(px(6.))
				.border_1()
				.border_color(rgb(if invalid {
					palette::ACCENT
				} else {
					palette::BORDER
				}))
				.child(Input::new(input).appearance(false))
		};
		let mut bar = div()
			.flex_none()
			.px(px(12.))
			.py(px(6.))
			.flex()
			.flex_col()
			.gap(px(4.))
			.bg(rgb(palette::SIDEBAR))
			.border_b_1()
			.border_color(rgb(palette::BORDER))
			.on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
				let key = &ev.keystroke;
				match key.key.as_str() {
					"escape" => this.close_find(window, cx),
					"f3" => this.find_step(!key.modifiers.shift, cx),
					"h" if key.modifiers.control => this.open_find(true, window, cx),
					"f" if key.modifiers.control => this.open_find(false, window, cx),
					_ => return,
				}
				cx.stop_propagation();
			}))
			.child(
				div()
					.flex()
					.items_center()
					.gap(px(4.))
					.child(field(&find.query, find.invalid))
					.child(self.find_toggle(
						"find-case",
						"Aa",
						find.case_sensitive,
						|f| f.case_sensitive = !f.case_sensitive,
						cx,
					))
					.child(self.find_toggle(
						"find-regex",
						".*",
						find.regex,
						|f| f.regex = !f.regex,
						cx,
					))
					.child(
						div()
							.min_w(px(44.))
							.px(px(4.))
							.text_size(px(11.))
							.text_color(rgb(palette::FG3))
							.child(count),
					)
					.child(self.find_button(
						"find-prev",
						t.find_previous,
						|this, _, cx| this.find_step(false, cx),
						cx,
					))
					.child(self.find_button(
						"find-next",
						t.find_next,
						|this, _, cx| this.find_step(true, cx),
						cx,
					))
					.child(div().flex_1())
					.child(self.find_button(
						"find-close",
						Icon::new(IconName::Close).size_3p5(),
						|this, window, cx| this.close_find(window, cx),
						cx,
					)),
			);
		if find.show_replace {
			bar = bar.child(
				div()
					.flex()
					.items_center()
					.gap(px(4.))
					.child(field(&find.replacement, false))
					.child(self.find_button(
						"replace-one",
						t.replace_one,
						|this, _, cx| this.replace_current(cx),
						cx,
					))
					.child(self.find_button(
						"replace-all",
						t.replace_all,
						|this, _, cx| this.replace_all(cx),
						cx,
					)),
			);
		}
		Some(bar.into_any_element())
	}
}
