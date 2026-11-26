//! A floating fuzzy picker: one text field over a filtered list. Used by the quick
//! switcher (`Ctrl+P`) and the command palette (`Ctrl+Shift+P`).

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::fuzzy::rank;

use crate::theme::palette;

/// Rows shown at most (the query narrows the list down).
const MAX_ROWS: usize = 50;

pub struct PickerItem {
	/// The text matched against the query.
	pub label: String,
	/// Dim text at the right end of the row (a shortcut).
	pub detail: String,
}

pub enum PickerEvent {
	/// Enter on an item (`secondary`: with Ctrl).
	Confirm {
		index: usize,
		secondary: bool,
	},
	/// Shift+Enter: use the typed text itself.
	Create(String),
	Dismiss,
}

pub struct Picker {
	input: Entity<InputState>,
	items: Vec<PickerItem>,
	/// Index in `items` and matched byte positions of each visible row.
	rows: Vec<(usize, Vec<usize>)>,
	selected: usize,
	scroll: ScrollHandle,
	_subscription: Subscription,
}

impl EventEmitter<PickerEvent> for Picker {}

impl Picker {
	pub fn new(
		items: Vec<PickerItem>,
		placeholder: &'static str,
		window: &mut Window,
		cx: &mut Context<Self>,
	) -> Self {
		let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
		let subscription =
			cx.subscribe_in(
				&input,
				window,
				|this, _, event: &InputEvent, _, cx| match event {
					InputEvent::Change => this.refilter(cx),
					InputEvent::PressEnter { secondary, shift } => {
						if *shift {
							this.create(cx);
						} else {
							this.confirm(*secondary, cx);
						}
					}
					_ => {}
				},
			);
		input.update(cx, |input, cx| input.focus(window, cx));
		let mut picker = Picker {
			input,
			items,
			rows: Vec::new(),
			selected: 0,
			scroll: ScrollHandle::new(),
			_subscription: subscription,
		};
		picker.refilter(cx);
		picker
	}

	fn query(&self, cx: &App) -> String {
		self.input.read(cx).value().to_string()
	}

	fn refilter(&mut self, cx: &mut Context<Self>) {
		let query = self.query(cx);
		self.rows = rank(
			&query,
			self.items.iter().map(|i| i.label.as_str()),
			MAX_ROWS,
		)
		.into_iter()
		.map(|(i, m)| (i, m.positions))
		.collect();
		self.selected = 0;
		self.scroll.scroll_to_item(0);
		cx.notify();
	}

	fn select(&mut self, delta: isize, cx: &mut Context<Self>) {
		if self.rows.is_empty() {
			return;
		}
		let len = self.rows.len() as isize;
		self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
		self.scroll.scroll_to_item(self.selected);
		cx.notify();
	}

	fn confirm(&mut self, secondary: bool, cx: &mut Context<Self>) {
		if let Some((index, _)) = self.rows.get(self.selected) {
			cx.emit(PickerEvent::Confirm {
				index: *index,
				secondary,
			});
		}
	}

	fn create(&mut self, cx: &mut Context<Self>) {
		let query = self.query(cx).trim().to_string();
		if !query.is_empty() {
			cx.emit(PickerEvent::Create(query));
		}
	}

	fn key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
		match ev.keystroke.key.as_str() {
			"up" => self.select(-1, cx),
			"down" => self.select(1, cx),
			"escape" => cx.emit(PickerEvent::Dismiss),
			// The field reports a plain Enter itself; these come with modifiers.
			"enter" if ev.keystroke.modifiers.shift => self.create(cx),
			"enter" if ev.keystroke.modifiers.control => self.confirm(true, cx),
			_ => return,
		}
		cx.stop_propagation();
	}

	fn render_row(&self, row: usize, cx: &mut Context<Self>) -> AnyElement {
		let (index, positions) = &self.rows[row];
		let item = &self.items[*index];
		let highlight = HighlightStyle {
			color: Some(rgb(palette::LINK).into()),
			font_weight: Some(FontWeight::SEMIBOLD),
			..Default::default()
		};
		let highlights: Vec<_> = positions
			.iter()
			.filter_map(|p| {
				let len = item.label[*p..].chars().next()?.len_utf8();
				Some((*p..*p + len, highlight))
			})
			.collect();
		let selected = row == self.selected;
		let index = *index;
		div()
			.id(("picker-row", row))
			.h(px(32.))
			.px(px(10.))
			.mx(px(6.))
			.flex()
			.flex_none()
			.items_center()
			.gap(px(12.))
			.rounded(px(6.))
			.text_size(px(13.))
			.text_color(rgb(palette::TEXT))
			.when(selected, |d| d.bg(rgb(palette::MUTED2)))
			.hover(|d| d.bg(rgb(palette::MUTED2)))
			.cursor_pointer()
			.on_click(cx.listener(move |_, ev: &ClickEvent, _, cx| {
				cx.emit(PickerEvent::Confirm {
					index,
					secondary: ev.modifiers().control,
				});
			}))
			.child(
				div()
					.flex_1()
					.min_w_0()
					.overflow_hidden()
					.whitespace_nowrap()
					.child(StyledText::new(item.label.clone()).with_highlights(highlights)),
			)
			.when(!item.detail.is_empty(), |d| {
				d.child(
					div()
						.flex_none()
						.text_size(px(11.))
						.text_color(rgb(palette::FG3))
						.child(item.detail.clone()),
				)
			})
			.into_any_element()
	}
}

impl Render for Picker {
	fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let rows: Vec<AnyElement> = (0..self.rows.len())
			.map(|row| self.render_row(row, cx))
			.collect();
		div()
			.id("picker-backdrop")
			.absolute()
			.inset_0()
			.flex()
			.justify_center()
			.items_start()
			.pt(px(84.))
			.bg(rgba(0x00000066))
			.occlude()
			.on_mouse_down(
				MouseButton::Left,
				cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Dismiss)),
			)
			.child(
				div()
					.id("picker")
					.w(px(560.))
					.max_w(relative(0.9))
					.flex()
					.flex_col()
					.rounded(px(10.))
					.border_1()
					.border_color(rgb(palette::BORDER))
					.bg(rgb(palette::SIDEBAR))
					.shadow_lg()
					.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
					.on_key_down(cx.listener(Self::key_down))
					.child(
						div()
							.p(px(8.))
							.border_b_1()
							.border_color(rgb(palette::BORDER))
							.child(Input::new(&self.input).appearance(false)),
					)
					.child(
						div()
							.id("picker-list")
							.max_h(px(364.))
							.py(px(6.))
							.flex()
							.flex_col()
							.overflow_y_scroll()
							.track_scroll(&self.scroll)
							.children(rows),
					),
			)
	}
}
