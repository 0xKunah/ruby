//! The drawing canvas: tools, selection, pan and zoom, and the floating panels.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ruby_core::drawing::color::{BACKGROUNDS, STROKES};
use ruby_core::drawing::edit::Editor;
use ruby_core::drawing::geometry::{self, Handle, Rect};
use ruby_core::drawing::model::Element;
use ruby_core::drawing::rough;

use super::paint::{self, MAX_ZOOM, MIN_ZOOM, ViewTransform, display_color};
use super::{DrawingModel, encode_data_url};
use crate::i18n::tr;
use crate::theme::palette;

/// How close to an element a click must be, in screen pixels.
const HIT_TOLERANCE: f64 = 6.;
/// Longest side of a picture when it is inserted, in scene units.
const PICTURE_SIZE: f64 = 480.;
/// Background of the embedded canvas and of drawing cards in notes.
pub const CARD_BG: u32 = 0x19191b;
const ACTIVE_BG: u32 = 0xdc26262e;
const ACTIVE_BORDER: u32 = 0xdc262680;
const ACTIVE_FG: u32 = 0xfca5a5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
	Hand,
	Select,
	Rectangle,
	Diamond,
	Ellipse,
	Arrow,
	Line,
	Pencil,
	Text,
	Image,
	Eraser,
}

impl Tool {
	pub const ALL: [Tool; 11] = [
		Tool::Hand,
		Tool::Select,
		Tool::Rectangle,
		Tool::Diamond,
		Tool::Ellipse,
		Tool::Arrow,
		Tool::Line,
		Tool::Pencil,
		Tool::Text,
		Tool::Image,
		Tool::Eraser,
	];

	pub fn key(self) -> &'static str {
		match self {
			Tool::Hand => "h",
			Tool::Select => "v",
			Tool::Rectangle => "r",
			Tool::Diamond => "d",
			Tool::Ellipse => "o",
			Tool::Arrow => "a",
			Tool::Line => "l",
			Tool::Pencil => "p",
			Tool::Text => "t",
			Tool::Image => "i",
			Tool::Eraser => "e",
		}
	}

	fn from_key(key: &str) -> Option<Tool> {
		Tool::ALL.into_iter().find(|t| t.key() == key)
	}

	fn icon(self) -> &'static str {
		match self {
			Tool::Hand => "icons/hand.svg",
			Tool::Select => "icons/mouse-pointer.svg",
			Tool::Rectangle => "icons/square.svg",
			Tool::Diamond => "icons/diamond.svg",
			Tool::Ellipse => "icons/circle.svg",
			Tool::Arrow => "icons/arrow-up-right.svg",
			Tool::Line => "icons/slash.svg",
			Tool::Pencil => "icons/pencil.svg",
			Tool::Text => "icons/type.svg",
			Tool::Image => "icons/image.svg",
			Tool::Eraser => "icons/eraser.svg",
		}
	}

	/// The element type the tool draws by dragging out a box.
	fn shape(self) -> Option<&'static str> {
		match self {
			Tool::Rectangle => Some("rectangle"),
			Tool::Diamond => Some("diamond"),
			Tool::Ellipse => Some("ellipse"),
			_ => None,
		}
	}

	fn creates(self) -> bool {
		!matches!(self, Tool::Hand | Tool::Select | Tool::Eraser | Tool::Image)
	}
}

/// What the pressed mouse button is doing.
enum Drag {
	None,
	Pan {
		last: Point<Pixels>,
	},
	Marquee {
		start: [f64; 2],
		current: [f64; 2],
	},
	Move {
		last: [f64; 2],
	},
	Resize {
		original: Box<Element>,
		handle: Handle,
	},
	Rotate,
	/// One end of a line or arrow.
	End {
		index: usize,
		start: bool,
	},
	Create {
		index: usize,
		origin: [f64; 2],
	},
	Line {
		index: usize,
	},
	Pencil {
		index: usize,
	},
	Erase,
}

struct TextEdit {
	id: String,
	/// The element was created for this edit: both are one undo step.
	created: bool,
	input: Entity<TextareaState>,
	_subscription: Subscription,
}

pub enum DrawingViewEvent {
	/// Escape in an embedded canvas: back to the preview.
	Close,
}

pub struct DrawingView {
	pub model: Entity<DrawingModel>,
	focus_handle: FocusHandle,
	pub tool: Tool,
	pub pan: [f64; 2],
	pub zoom: f64,
	/// Where the canvas was last painted (window coordinates).
	pub bounds: Bounds<Pixels>,
	drag: Drag,
	space: bool,
	text_edit: Option<TextEdit>,
	/// Inside a note: Escape closes it.
	embedded: bool,
	needs_fit: bool,
	last_mouse: Option<Point<Pixels>>,
	/// Undo depth when an opacity drag started (the drag is one undo step).
	opacity_drag: Option<usize>,
	_observe: Subscription,
}

impl EventEmitter<DrawingViewEvent> for DrawingView {}

impl Focusable for DrawingView {
	fn focus_handle(&self, _: &App) -> FocusHandle {
		self.focus_handle.clone()
	}
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
	((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

fn format_for_extension(path: &std::path::Path) -> Option<ImageFormat> {
	let ext = path.extension()?.to_str()?.to_ascii_lowercase();
	Some(match ext.as_str() {
		"png" => ImageFormat::Png,
		"jpg" | "jpeg" => ImageFormat::Jpeg,
		"webp" => ImageFormat::Webp,
		"gif" => ImageFormat::Gif,
		"svg" => ImageFormat::Svg,
		"bmp" => ImageFormat::Bmp,
		_ => return None,
	})
}

impl DrawingView {
	pub fn new(model: Entity<DrawingModel>, embedded: bool, cx: &mut Context<Self>) -> Self {
		let observe = cx.observe(&model, |_, _, cx| cx.notify());
		let needs_fit = !model.read(cx).editor.is_empty();
		DrawingView {
			model,
			focus_handle: cx.focus_handle(),
			tool: Tool::Select,
			pan: [0., 0.],
			zoom: 1.,
			bounds: Bounds::default(),
			drag: Drag::None,
			space: false,
			text_edit: None,
			embedded,
			needs_fit,
			last_mouse: None,
			opacity_drag: None,
			_observe: observe,
		}
	}

	pub fn transform(&self) -> ViewTransform {
		ViewTransform {
			origin: self.bounds.origin,
			pan: self.pan,
			zoom: self.zoom,
		}
	}

	/// Zoom in percent (status bar, zoom indicator).
	pub fn zoom_percent(&self) -> i64 {
		(self.zoom * 100.).round() as i64
	}

	fn set_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
		self.bounds = bounds;
		if self.needs_fit && bounds.size.width > px(0.) {
			self.needs_fit = false;
			self.fit_scene(1., cx);
		}
	}

	fn edit<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor) -> R) -> R {
		self.model.update(cx, |m, cx| m.edit(cx, f))
	}

	// Pan and zoom.

	/// Shows the whole scene, never magnified beyond `max_zoom`.
	fn fit_scene(&mut self, max_zoom: f64, cx: &mut Context<Self>) {
		let scene = geometry::scene_bounds(&self.model.read(cx).editor.scene.elements);
		match scene {
			Some(scene) => {
				(self.pan, self.zoom) = paint::fit(scene, self.bounds.size, 48., max_zoom)
			}
			None => (self.pan, self.zoom) = ([0., 0.], 1.),
		}
		cx.notify();
	}

	/// Multiplies the zoom, keeping the scene point under `anchor` where it is.
	fn zoom_at(&mut self, factor: f64, anchor: Point<Pixels>, cx: &mut Context<Self>) {
		let scene = self.transform().to_scene(anchor);
		self.zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
		let local = anchor - self.bounds.origin;
		self.pan = [
			f64::from(f32::from(local.x)) - scene[0] * self.zoom,
			f64::from(f32::from(local.y)) - scene[1] * self.zoom,
		];
		cx.notify();
	}

	fn zoom_step(&mut self, factor: f64, cx: &mut Context<Self>) {
		self.zoom_at(factor, self.bounds.center(), cx);
	}

	fn reset_zoom(&mut self, cx: &mut Context<Self>) {
		let factor = 1. / self.zoom;
		self.zoom_step(factor, cx);
	}

	fn scroll(&mut self, ev: &ScrollEvent, cx: &mut Context<Self>) {
		let delta = ev.delta.pixel_delta(px(40.));
		let (dx, dy) = (f64::from(f32::from(delta.x)), f64::from(f32::from(delta.y)));
		if ev.modifiers.control {
			self.zoom_at((dy * 0.0025).exp(), ev.position, cx);
		} else if ev.modifiers.shift && dx == 0. {
			self.pan[0] += dy;
		} else {
			self.pan[0] += dx;
			self.pan[1] += dy;
		}
		cx.notify();
	}

	// Tools.

	pub fn set_tool(&mut self, tool: Tool, window: &mut Window, cx: &mut Context<Self>) {
		self.commit_text(window, cx);
		if tool == Tool::Image {
			self.pick_image(window, cx);
			return;
		}
		self.tool = tool;
		if tool != Tool::Select && tool != Tool::Hand {
			self.edit(cx, |ed| ed.clear_selection());
		}
		cx.notify();
	}

	/// The image tool: a file picker, then the picture in the middle of the view.
	fn pick_image(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let rx = cx.prompt_for_paths(PathPromptOptions {
			files: true,
			directories: false,
			multiple: false,
			prompt: None,
		});
		cx.spawn_in(window, async move |this, cx| {
			let Ok(Ok(Some(paths))) = rx.await else {
				return;
			};
			let Some(path) = paths.into_iter().next() else {
				return;
			};
			let Some(format) = format_for_extension(&path) else {
				return;
			};
			let bytes = cx
				.background_spawn(async move { std::fs::read(&path) })
				.await;
			if let Ok(bytes) = bytes {
				this.update(cx, |this, cx| {
					this.insert_image(&Image::from_bytes(format, bytes), cx)
				})
				.ok();
			}
		})
		.detach();
	}

	/// Adds a picture in the middle of the view and selects it.
	pub fn insert_image(&mut self, image: &Image, cx: &mut Context<Self>) {
		let Ok(decoded) = image.to_image_data(cx.svg_renderer()) else {
			return;
		};
		let size = decoded.size(0);
		let (w, h) = (
			f64::from(size.width.0.max(1)),
			f64::from(size.height.0.max(1)),
		);
		let scale = (PICTURE_SIZE / w.max(h)).min(1.);
		let (w, h) = (w * scale, h * scale);
		let center = self.transform().to_scene(self.bounds.center());
		let url = encode_data_url(image.format.mime_type(), &image.bytes);
		let mime = image.format.mime_type();
		self.edit(cx, |ed| {
			ed.add_image(mime, &url, center[0] - w / 2., center[1] - h / 2., w, h)
		});
		self.tool = Tool::Select;
	}

	// Text editing.

	fn start_text_edit(
		&mut self,
		id: String,
		created: bool,
		window: &mut Window,
		cx: &mut Context<Self>,
	) {
		let text = {
			let m = self.model.read(cx);
			m.editor
				.index_of(&id)
				.and_then(|i| m.editor.scene.elements[i].text.clone())
				.unwrap_or_default()
		};
		let input = cx.new(|cx| {
			TextareaState::new(window, cx)
				.default_value(text)
				.auto_grow(1, 30)
		});
		let subscription =
			cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
				if matches!(event, InputEvent::Blur) {
					this.commit_text(window, cx);
				}
			});
		input.update(cx, |state, cx| state.focus(window, cx));
		self.text_edit = Some(TextEdit {
			id,
			created,
			input,
			_subscription: subscription,
		});
		self.tool = Tool::Select;
		cx.notify();
	}

	/// Ends text editing: the typed text goes to the element (an empty text removes it).
	pub(crate) fn commit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
		let Some(edit) = self.text_edit.take() else {
			return;
		};
		let text = edit.input.read(cx).value().to_string();
		let (old, font_size, family) = {
			let m = self.model.read(cx);
			match m.editor.index_of(&edit.id) {
				Some(i) => {
					let el = &m.editor.scene.elements[i];
					(
						el.text.clone().unwrap_or_default(),
						el.font_size,
						el.font_family,
					)
				}
				None => return,
			}
		};
		if !edit.created && text == old {
			cx.notify();
			return;
		}
		let (w, h) = paint::measure_text(&text, font_size, family, window);
		self.edit(cx, |ed| {
			ed.set_text(&edit.id, &text, w, h);
			if edit.created {
				ed.merge_checkpoints();
				// An empty new text leaves no trace.
				ed.discard_unchanged_checkpoint();
			}
		});
	}

	fn double_click(&mut self, p: [f64; 2], window: &mut Window, cx: &mut Context<Self>) {
		let tolerance = HIT_TOLERANCE / self.zoom;
		let target = self.edit(cx, |ed| match ed.element_at(p, tolerance) {
			Some(i) => {
				let el = &ed.scene.elements[i];
				let id = el.id.clone();
				match el.kind.as_str() {
					"text" => Some((id, false)),
					"rectangle" | "diamond" | "ellipse" => {
						let created = ed.text_of(&id).is_none();
						ed.add_text_in(&id).map(|text| (text, created))
					}
					_ => None,
				}
			}
			None => {
				let i = ed.add("text", p[0], p[1]);
				Some((ed.scene.elements[i].id.clone(), true))
			}
		});
		if let Some((id, created)) = target {
			self.start_text_edit(id, created, window, cx);
		}
	}

	// Mouse.

	fn mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
		self.commit_text(window, cx);
		window.focus(&self.focus_handle, cx);
		let p = self.transform().to_scene(ev.position);
		let zoom = self.zoom;
		let tolerance = HIT_TOLERANCE / zoom;
		let panning = ev.button == MouseButton::Middle
			|| (ev.button == MouseButton::Left && (self.space || self.tool == Tool::Hand));
		if panning {
			self.drag = Drag::Pan { last: ev.position };
			return;
		}
		if ev.button != MouseButton::Left {
			return;
		}
		let tool = self.tool;
		if tool == Tool::Select && ev.click_count >= 2 {
			self.drag = Drag::None;
			self.double_click(p, window, cx);
			return;
		}
		if tool == Tool::Text {
			let id = self.edit(cx, |ed| {
				let i = ed.add("text", p[0], p[1]);
				ed.scene.elements[i].id.clone()
			});
			self.start_text_edit(id, true, window, cx);
			return;
		}
		let (shift, alt) = (ev.modifiers.shift, ev.modifiers.alt);
		self.drag = self.edit(cx, |ed| match tool {
			Tool::Select => {
				if let [i] = ed.selected_indexes()[..] {
					let el = &ed.scene.elements[i];
					if matches!(el.kind.as_str(), "line" | "arrow") {
						let points = geometry::abs_points(el);
						let reach = 8. / zoom;
						let near =
							|q: Option<&[f64; 2]>| q.is_some_and(|q| distance(*q, p) <= reach);
						let end = if near(points.last()) {
							Some(false)
						} else if near(points.first()) {
							Some(true)
						} else {
							None
						};
						if let Some(start) = end {
							ed.checkpoint();
							return Drag::End { index: i, start };
						}
					} else if el.kind != "freedraw"
						&& let Some(handle) = geometry::handle_at(el, p, zoom)
					{
						let original = Box::new(el.clone());
						ed.checkpoint();
						return match handle {
							Handle::Rotate => Drag::Rotate,
							handle => Drag::Resize { original, handle },
						};
					}
				}
				match ed.element_at(p, tolerance) {
					Some(_) if shift => {
						ed.select_at(p, tolerance, true);
						Drag::None
					}
					Some(i) => {
						if !ed.is_selected(&ed.scene.elements[i].id) {
							ed.select_at(p, tolerance, false);
						}
						if alt {
							ed.duplicate_selected(0., 0.);
						} else {
							ed.checkpoint();
						}
						Drag::Move { last: p }
					}
					None => {
						if !shift {
							ed.clear_selection();
						}
						Drag::Marquee {
							start: p,
							current: p,
						}
					}
				}
			}
			Tool::Arrow | Tool::Line => {
				let kind = if tool == Tool::Arrow { "arrow" } else { "line" };
				Drag::Line {
					index: ed.add(kind, p[0], p[1]),
				}
			}
			Tool::Pencil => {
				let index = ed.add("freedraw", p[0], p[1]);
				ed.add_freedraw_point(index, p);
				Drag::Pencil { index }
			}
			Tool::Eraser => {
				ed.checkpoint();
				ed.erase_at(p, tolerance);
				Drag::Erase
			}
			tool => match tool.shape() {
				Some(kind) => Drag::Create {
					index: ed.add(kind, p[0], p[1]),
					origin: p,
				},
				None => Drag::None,
			},
		});
	}

	fn mouse_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
		self.last_mouse = Some(ev.position);
		if matches!(self.drag, Drag::None) {
			return;
		}
		if ev.pressed_button.is_none() {
			// The button was released outside the window.
			self.mouse_up(window, cx);
			return;
		}
		let p = self.transform().to_scene(ev.position);
		let tolerance = HIT_TOLERANCE / self.zoom;
		let shift = ev.modifiers.shift;
		let model = self.model.clone();
		let mut edit = |f: &mut dyn FnMut(&mut Editor)| model.update(cx, |m, cx| m.edit(cx, f));
		match &mut self.drag {
			Drag::None => {}
			Drag::Pan { last } => {
				let delta = ev.position - *last;
				*last = ev.position;
				self.pan[0] += f64::from(f32::from(delta.x));
				self.pan[1] += f64::from(f32::from(delta.y));
			}
			Drag::Marquee { start, current } => {
				*current = p;
				let rect = Rect::from_points(*start, p);
				edit(&mut |ed| ed.select_in_rect(rect));
			}
			Drag::Move { last } => {
				let (dx, dy) = (p[0] - last[0], p[1] - last[1]);
				*last = p;
				edit(&mut |ed| ed.move_selected(dx, dy));
			}
			Drag::Resize { original, handle } => {
				let handle = *handle;
				edit(&mut |ed| ed.resize_selected(original, handle, p, shift));
			}
			Drag::Rotate => edit(&mut |ed| ed.rotate_selected(p, shift)),
			Drag::End { index, start } => {
				let (index, start) = (*index, *start);
				edit(&mut |ed| ed.move_arrow_end(index, start, p));
			}
			Drag::Create { index, origin } => {
				let (index, origin) = (*index, *origin);
				let mut to = p;
				if shift {
					// Shift draws a square or a circle.
					let side = (p[0] - origin[0]).abs().max((p[1] - origin[1]).abs());
					to = [
						origin[0] + side * (p[0] - origin[0]).signum(),
						origin[1] + side * (p[1] - origin[1]).signum(),
					];
				}
				edit(&mut |ed| ed.set_box(index, origin, to));
			}
			Drag::Line { index } => {
				let index = *index;
				edit(&mut |ed| ed.move_arrow_end(index, false, p));
			}
			Drag::Pencil { index } => {
				let index = *index;
				edit(&mut |ed| ed.add_freedraw_point(index, p));
			}
			Drag::Erase => edit(&mut |ed| {
				ed.erase_at(p, tolerance);
			}),
		}
		cx.notify();
	}

	fn mouse_up(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
		let drag = std::mem::replace(&mut self.drag, Drag::None);
		let zoom = self.zoom;
		// A shape smaller than this on screen was a click, not a drag.
		let too_small = |el: &Element| {
			let b = geometry::bounds(el);
			b.w.max(b.h) * zoom < 3.
		};
		match drag {
			Drag::None | Drag::Pan { .. } => return,
			Drag::Marquee { .. } => {}
			Drag::Move { .. } | Drag::Resize { .. } | Drag::Rotate | Drag::Erase => {
				self.edit(cx, |ed| ed.discard_unchanged_checkpoint())
			}
			Drag::End { index, .. } => self.edit(cx, |ed| {
				ed.bind_arrow(index);
				ed.discard_unchanged_checkpoint();
			}),
			Drag::Create { index, .. } => {
				self.edit(cx, |ed| {
					if too_small(&ed.scene.elements[index]) {
						ed.cancel_checkpoint();
					}
				});
				self.tool = Tool::Select;
			}
			Drag::Line { index } => {
				self.edit(cx, |ed| {
					if too_small(&ed.scene.elements[index]) {
						ed.cancel_checkpoint();
					} else {
						ed.bind_arrow(index);
					}
				});
				self.tool = Tool::Select;
			}
			Drag::Pencil { index } => self.edit(cx, |ed| {
				let dot = ed.scene.elements[index]
					.points
					.as_ref()
					.is_none_or(|p| p.len() < 2);
				if dot {
					ed.cancel_checkpoint();
				} else {
					ed.clear_selection();
				}
			}),
		}
		cx.notify();
	}

	// Keyboard.

	fn copy(&mut self, cut: bool, cx: &mut Context<Self>) {
		let Some(json) = self.model.read(cx).editor.copy_selected() else {
			return;
		};
		cx.write_to_clipboard(ClipboardItem::new_string(json));
		if cut {
			self.edit(cx, |ed| ed.delete_selected());
		}
	}

	fn paste(&mut self, cx: &mut Context<Self>) {
		let Some(item) = cx.read_from_clipboard() else {
			return;
		};
		// Under the pointer when it is over the canvas, in the middle otherwise.
		let anchor = self
			.last_mouse
			.filter(|p| self.bounds.contains(p))
			.unwrap_or(self.bounds.center());
		let at = self.transform().to_scene(anchor);
		for entry in item.entries() {
			match entry {
				ClipboardEntry::String(text) => {
					if self.edit(cx, |ed| ed.paste(text.text(), Some(at))) {
						self.tool = Tool::Select;
						return;
					}
				}
				ClipboardEntry::Image(image) => {
					self.insert_image(image, cx);
					return;
				}
				_ => {}
			}
		}
	}

	fn key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
		let key = ev.keystroke.key.as_str();
		if !self.focus_handle.is_focused(window) {
			// Typing in the text field: only Escape is for the canvas.
			if key == "escape" && self.text_edit.is_some() {
				self.commit_text(window, cx);
				window.focus(&self.focus_handle, cx);
				cx.stop_propagation();
			}
			return;
		}
		let m = ev.keystroke.modifiers;
		let (ctrl, shift) = (m.control, m.shift);
		if m.alt {
			return;
		}
		let step = if shift { 10. } else { 1. };
		let nudge = |this: &mut Self, dx: f64, dy: f64, cx: &mut Context<Self>| {
			this.edit(cx, |ed| {
				if !ed.selected_ids().is_empty() {
					ed.checkpoint();
					ed.move_selected(dx * step, dy * step);
				}
			})
		};
		match (ctrl, shift, key) {
			(false, _, "space") => self.space = true,
			(false, false, "escape") => {
				let idle = self.tool == Tool::Select
					&& self.model.read(cx).editor.selected_ids().is_empty();
				if idle && self.embedded {
					cx.emit(DrawingViewEvent::Close);
				}
				self.tool = Tool::Select;
				self.edit(cx, |ed| ed.clear_selection());
			}
			(false, _, "delete" | "backspace") => self.edit(cx, |ed| ed.delete_selected()),
			(true, false, "a") => {
				self.tool = Tool::Select;
				self.edit(cx, |ed| ed.select_all());
			}
			(true, false, "c") => self.copy(false, cx),
			(true, false, "x") => self.copy(true, cx),
			(true, false, "v") => self.paste(cx),
			(true, false, "d") => self.edit(cx, |ed| ed.duplicate_selected(10., 10.)),
			(true, false, "g") => self.edit(cx, |ed| ed.group()),
			(true, true, "g") => self.edit(cx, |ed| ed.ungroup()),
			(true, false, "z") => {
				self.edit(cx, |ed| ed.undo());
			}
			(true, false, "y") | (true, true, "z") => {
				self.edit(cx, |ed| ed.redo());
			}
			(true, false, "0") => self.reset_zoom(cx),
			(false, true, "1") | (false, _, "!") => self.fit_scene(MAX_ZOOM, cx),
			(false, _, "left") => nudge(self, -1., 0., cx),
			(false, _, "right") => nudge(self, 1., 0., cx),
			(false, _, "up") => nudge(self, 0., -1., cx),
			(false, _, "down") => nudge(self, 0., 1., cx),
			(false, false, key) => match Tool::from_key(key) {
				Some(tool) => self.set_tool(tool, window, cx),
				None => return,
			},
			_ => return,
		}
		cx.stop_propagation();
		cx.notify();
	}

	fn key_up(&mut self, ev: &KeyUpEvent, _: &mut Window, cx: &mut Context<Self>) {
		if ev.keystroke.key == "space" {
			self.space = false;
			cx.notify();
		}
	}

	// Properties.

	fn set_opacity_at(&mut self, x: Pixels, track: Bounds<Pixels>, cx: &mut Context<Self>) {
		let ratio =
			f64::from(f32::from(x - track.left())) / f64::from(f32::from(track.size.width)).max(1.);
		let opacity = (ratio.clamp(0., 1.) * 10.).round() * 10.;
		let depth = self.opacity_drag;
		self.edit(cx, |ed| {
			ed.set_opacity(opacity);
			// The whole drag is one undo step.
			if let Some(depth) = depth {
				while ed.undo_depth() > depth + 1 {
					ed.merge_checkpoints();
				}
			}
		});
	}

	// Painting.

	fn paint_canvas(
		view: &Entity<Self>,
		bounds: Bounds<Pixels>,
		hitbox: &Hitbox,
		window: &mut Window,
		cx: &mut App,
	) {
		let this = view.read(cx);
		let transform = this.transform();
		let model = this.model.clone();
		let hidden = this.text_edit.as_ref().map(|e| e.id.clone());
		let marquee = match &this.drag {
			Drag::Marquee { start, current } => Some(Rect::from_points(*start, *current)),
			_ => None,
		};
		let cursor = match this.tool {
			Tool::Hand => CursorStyle::OpenHand,
			Tool::Select => CursorStyle::Arrow,
			_ => CursorStyle::Crosshair,
		};
		let cursor = if this.space || matches!(this.drag, Drag::Pan { .. }) {
			CursorStyle::ClosedHand
		} else {
			cursor
		};
		let embedded = this.embedded;
		window.set_cursor_style(cursor, hitbox);
		window.with_content_mask(Some(ContentMask { bounds }), |window| {
			if !embedded {
				paint::paint_grid(&transform, bounds, window);
			}
			model.update(cx, |m, cx| {
				paint::paint_scene(m, &transform, bounds, hidden.as_deref(), window, cx);
				paint_chrome(m, &transform, marquee, window);
			});
		});

		let (v, h) = (view.clone(), hitbox.clone());
		window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
			if phase == DispatchPhase::Bubble && h.is_hovered(window) {
				v.update(cx, |this, cx| this.mouse_down(ev, window, cx));
				cx.stop_propagation();
			}
		});
		let v = view.clone();
		window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
			if phase == DispatchPhase::Bubble {
				v.update(cx, |this, cx| this.mouse_move(ev, window, cx));
			}
		});
		let v = view.clone();
		window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
			if phase == DispatchPhase::Bubble {
				v.update(cx, |this, cx| this.mouse_up(window, cx));
			}
		});
		let (v, h) = (view.clone(), hitbox.clone());
		window.on_mouse_event(move |ev: &ScrollEvent, phase, window, cx| {
			if phase == DispatchPhase::Bubble && h.is_hovered(window) {
				v.update(cx, |this, cx| this.scroll(ev, cx));
				cx.stop_propagation();
			}
		});
	}

	// Panels.

	fn panel(id: &'static str) -> Stateful<Div> {
		div()
			.id(id)
			.occlude()
			.bg(rgb(palette::MUTED))
			.border_1()
			.border_color(rgb(palette::BORDER))
			.shadow_lg()
	}

	fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let separator = || {
			div()
				.w(px(1.))
				.my(px(4.))
				.mx(px(3.))
				.bg(rgb(palette::BORDER))
		};
		let mut bar = Self::panel("drawing-toolbar")
			.flex()
			.gap(px(2.))
			.p(px(4.))
			.rounded(px(10.));
		for (ix, tool) in Tool::ALL.into_iter().enumerate() {
			if tool == Tool::Rectangle || tool == Tool::Eraser {
				bar = bar.child(separator());
			}
			let active = self.tool == tool;
			bar = bar.child(
				div()
					.id(("tool", ix))
					.relative()
					.size(px(32.))
					.flex()
					.items_center()
					.justify_center()
					.rounded(px(7.))
					.cursor_pointer()
					.text_color(rgb(palette::FG2))
					.map(|d| {
						if active {
							d.bg(rgba(ACTIVE_BG)).text_color(rgb(ACTIVE_FG))
						} else {
							d.hover(|s| s.bg(rgb(palette::MUTED2)))
						}
					})
					.child(Icon::empty().path(tool.icon()).small())
					.child(
						div()
							.absolute()
							.bottom(px(0.))
							.right(px(3.))
							.text_size(px(8.5))
							.text_color(rgb(palette::FG3))
							.child(tool.key().to_ascii_uppercase()),
					)
					.on_click(cx.listener(move |this, _, window, cx| {
						this.set_tool(tool, window, cx);
						window.focus(&this.focus_handle, cx);
					})),
			);
		}
		div()
			.absolute()
			.top(px(12.))
			.left_0()
			.right_0()
			.flex()
			.justify_center()
			.child(bar)
	}

	fn render_properties(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
		let (style, has_selection) = {
			let editor = &self.model.read(cx).editor;
			(editor.current_style(), !editor.selected_ids().is_empty())
		};
		if !self.tool.creates() && !has_selection {
			return None;
		}
		let t = tr(cx);
		let label = |text: &'static str| {
			div()
				.text_size(px(10.5))
				.text_color(rgb(palette::FG3))
				.mt(px(8.))
				.mb(px(5.))
				.child(text)
		};
		let swatch = |id: ElementId, color: &'static str, selected: bool| {
			div()
				.id(id)
				.size(px(20.))
				.rounded(px(5.))
				.border_1()
				.border_color(rgb(palette::BORDER))
				.cursor_pointer()
				.when_some(display_color(color, 100.), |d, c| d.bg(c))
				.when(selected, |d| {
					d.border_2().border_color(rgb(palette::ACCENT))
				})
		};
		let strokes = STROKES.iter().enumerate().map(|(ix, color)| {
			swatch(("stroke", ix).into(), color, style.stroke_color == *color).on_click(
				cx.listener(move |this, _, _, cx| this.edit(cx, |ed| ed.set_stroke_color(color))),
			)
		});
		let backgrounds = BACKGROUNDS.iter().enumerate().map(|(ix, color)| {
			swatch(
				("background", ix).into(),
				color,
				style.background_color == *color,
			)
			.on_click(
				cx.listener(move |this, _, _, cx| this.edit(cx, |ed| ed.set_background(color))),
			)
		});
		let segment = |id: ElementId, text: &'static str, selected: bool| {
			div()
				.id(id)
				.flex_1()
				.h(px(24.))
				.flex()
				.items_center()
				.justify_center()
				.rounded(px(6.))
				.border_1()
				.cursor_pointer()
				.map(|d| {
					if selected {
						d.bg(rgba(ACTIVE_BG))
							.border_color(rgba(ACTIVE_BORDER))
							.text_color(rgb(ACTIVE_FG))
					} else {
						d.border_color(rgb(palette::BORDER))
					}
				})
				.child(text)
		};
		let widths = [(1., t.draw_thin), (2., t.draw_medium), (4., t.draw_bold)]
			.into_iter()
			.enumerate()
			.map(|(ix, (width, text))| {
				segment(("width", ix).into(), text, style.stroke_width == width).on_click(
					cx.listener(move |this, _, _, cx| {
						this.edit(cx, |ed| ed.set_stroke_width(width))
					}),
				)
			});
		let styles = [(0., t.draw_sharp), (1., t.draw_hand), (2., t.draw_sketch)]
			.into_iter()
			.enumerate()
			.map(|(ix, (roughness, text))| {
				segment(("rough", ix).into(), text, style.roughness == roughness).on_click(
					cx.listener(move |this, _, _, cx| {
						this.edit(cx, |ed| ed.set_roughness(roughness))
					}),
				)
			});
		let view = cx.entity();
		let opacity = style.opacity;
		let slider = canvas(
			|bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
			move |bounds, hitbox, window, _| {
				let track = Bounds::new(
					point(bounds.left(), bounds.center().y - px(2.)),
					size(bounds.size.width, px(4.)),
				);
				window.paint_quad(fill(track, rgb(palette::MUTED2)).corner_radii(px(2.)));
				let filled = Bounds::new(
					track.origin,
					size(track.size.width * (opacity / 100.) as f32, px(4.)),
				);
				window.paint_quad(fill(filled, rgb(palette::ACCENT)).corner_radii(px(2.)));
				window.set_cursor_style(CursorStyle::PointingHand, &hitbox);
				let v = view.clone();
				window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
					if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
						v.update(cx, |this, cx| {
							let depth = this.model.read(cx).editor.undo_depth();
							this.opacity_drag = Some(depth);
							this.set_opacity_at(ev.position.x, bounds, cx);
						});
						cx.stop_propagation();
					}
				});
				let v = view.clone();
				window.on_mouse_event(move |ev: &MouseMoveEvent, phase, _, cx| {
					if phase == DispatchPhase::Bubble && ev.dragging() {
						v.update(cx, |this, cx| {
							if this.opacity_drag.is_some() {
								this.set_opacity_at(ev.position.x, bounds, cx);
							}
						});
					}
				});
				let v = view.clone();
				window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
					if phase == DispatchPhase::Bubble {
						v.update(cx, |this, _| this.opacity_drag = None);
					}
				});
			},
		)
		.w_full()
		.h(px(14.));
		Some(
			Self::panel("drawing-properties")
				.absolute()
				.top(px(12.))
				.left(px(12.))
				.w(px(180.))
				.p(px(12.))
				.rounded(px(10.))
				.text_size(px(11.5))
				.text_color(rgb(palette::FG2))
				.child(label(t.draw_stroke).mt(px(0.)))
				.child(div().flex().gap(px(5.)).children(strokes))
				.child(label(t.draw_background))
				.child(div().flex().gap(px(5.)).children(backgrounds))
				.child(label(t.draw_width))
				.child(div().flex().gap(px(4.)).children(widths))
				.child(label(t.draw_style))
				.child(div().flex().gap(px(4.)).children(styles))
				.child(label(t.draw_opacity))
				.child(slider),
		)
	}

	fn render_zoom(&self, cx: &mut Context<Self>) -> impl IntoElement {
		let button = |id: &'static str| {
			div()
				.id(id)
				.w(px(26.))
				.h(px(24.))
				.flex()
				.items_center()
				.justify_center()
				.rounded(px(5.))
				.cursor_pointer()
				.hover(|s| s.bg(rgb(palette::MUTED2)))
		};
		div()
			.absolute()
			.bottom(px(12.))
			.left(px(12.))
			.flex()
			.gap(px(8.))
			.text_size(px(11.5))
			.text_color(rgb(palette::FG2))
			.child(
				Self::panel("drawing-zoom")
					.flex()
					.items_center()
					.p(px(3.))
					.rounded(px(8.))
					.child(
						button("zoom-out")
							.child("-")
							.on_click(cx.listener(|this, _, _, cx| this.zoom_step(1. / 1.1, cx))),
					)
					.child(
						div()
							.id("zoom-reset")
							.w(px(44.))
							.text_center()
							.cursor_pointer()
							.child(format!("{}%", self.zoom_percent()))
							.on_click(cx.listener(|this, _, _, cx| this.reset_zoom(cx))),
					)
					.child(
						button("zoom-in")
							.child("+")
							.on_click(cx.listener(|this, _, _, cx| this.zoom_step(1.1, cx))),
					),
			)
			.child(
				Self::panel("drawing-history")
					.flex()
					.gap(px(2.))
					.p(px(3.))
					.rounded(px(8.))
					.child(
						button("undo")
							.child(Icon::empty().path("icons/undo-2.svg").small())
							.on_click(cx.listener(|this, _, _, cx| {
								this.edit(cx, |ed| ed.undo());
							})),
					)
					.child(
						button("redo")
							.child(Icon::empty().path("icons/redo-2.svg").small())
							.on_click(cx.listener(|this, _, _, cx| {
								this.edit(cx, |ed| ed.redo());
							})),
					),
			)
	}

	/// The text field over the element being edited.
	fn render_text_edit(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
		let edit = self.text_edit.as_ref()?;
		let m = self.model.read(cx);
		let el = &m.editor.scene.elements[m.editor.index_of(&edit.id)?];
		let font_size = el.font_size * self.zoom;
		// Text in a shape is typed across the shape, centered.
		let container = el
			.container_id
			.as_ref()
			.and_then(|c| m.editor.index_of(c))
			.map(|i| geometry::bounds(&m.editor.scene.elements[i]));
		let (x, y, width) = match container {
			Some(b) => (b.x, b.center()[1] - el.font_size * 0.625, b.w),
			None => (el.x, el.y, (el.width + 40.).max(240.)),
		};
		let at = self.transform().to_screen([x, y]) - self.bounds.origin;
		Some(
			div()
				.id("drawing-text")
				.occlude()
				.absolute()
				.left(at.x)
				.top(at.y)
				.w(px((width * self.zoom).max(80.) as f32))
				.font_family(paint::font_family(el.font_family))
				.text_size(px(font_size as f32))
				.when(container.is_some(), |d| d.text_center())
				.when_some(display_color(&el.stroke_color, 100.), |d, c| {
					d.text_color(c)
				})
				.child(Textarea::new(&edit.input).appearance(false)),
		)
	}
}

/// A closed dashed outline through `corners` (window coordinates).
fn dashed_outline(corners: &[Point<Pixels>], window: &mut Window) {
	let mut points: Vec<[f64; 2]> = corners
		.iter()
		.map(|p| [f64::from(f32::from(p.x)), f64::from(f32::from(p.y))])
		.collect();
	if let Some(first) = points.first().copied() {
		points.push(first);
	}
	let mut builder = PathBuilder::stroke(px(1.));
	let mut any = false;
	for piece in rough::dash(&points, 4., 3.) {
		let Some((first, rest)) = piece.split_first() else {
			continue;
		};
		builder.move_to(point(px(first[0] as f32), px(first[1] as f32)));
		for p in rest {
			builder.line_to(point(px(p[0] as f32), px(p[1] as f32)));
		}
		any = true;
	}
	if any && let Ok(path) = builder.build() {
		window.paint_path(path, rgb(palette::ACCENT));
	}
}

/// Selection boxes, handles and the marquee.
fn paint_chrome(m: &DrawingModel, t: &ViewTransform, marquee: Option<Rect>, window: &mut Window) {
	let elements = &m.editor.scene.elements;
	let selected = m.editor.selected_indexes();
	let pad = 4. / t.zoom;
	for &i in &selected {
		let el = &elements[i];
		let b = geometry::bounds(el);
		let c = b.center();
		let corners = [
			[b.x - pad, b.y - pad],
			[b.x + b.w + pad, b.y - pad],
			[b.x + b.w + pad, b.y + b.h + pad],
			[b.x - pad, b.y + b.h + pad],
		]
		.map(|p| t.to_screen(geometry::rotate(p, c, el.angle)));
		dashed_outline(&corners, window);
	}
	let handle = |at: Point<Pixels>, round: bool, window: &mut Window| {
		let bounds = Bounds::new(point(at.x - px(4.), at.y - px(4.)), size(px(8.), px(8.)));
		window.paint_quad(quad(
			bounds,
			if round { px(4.) } else { px(2.) },
			rgb(palette::BG),
			px(1.2),
			rgb(palette::ACCENT),
			BorderStyle::Solid,
		));
	};
	if let [i] = selected[..] {
		let el = &elements[i];
		match el.kind.as_str() {
			"line" | "arrow" => {
				let points = geometry::abs_points(el);
				for p in points.first().into_iter().chain(points.last()) {
					handle(t.to_screen(*p), true, window);
				}
			}
			"freedraw" => {}
			_ => {
				for (kind, p) in geometry::handle_positions(el, t.zoom) {
					handle(t.to_screen(p), kind == Handle::Rotate, window);
				}
			}
		}
	}
	if let Some(rect) = marquee {
		let bounds = Bounds::from_corners(
			t.to_screen([rect.x, rect.y]),
			t.to_screen([rect.x + rect.w, rect.y + rect.h]),
		);
		window.paint_quad(quad(
			bounds,
			px(0.),
			rgba(0xdc262618),
			px(1.),
			rgb(palette::ACCENT),
			BorderStyle::Solid,
		));
	}
}

impl Render for DrawingView {
	fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
		let (prepaint_view, paint_view) = (cx.entity(), cx.entity());
		div()
			.id("drawing-view")
			.key_context("DrawingCanvas")
			.track_focus(&self.focus_handle)
			.on_key_down(cx.listener(Self::key_down))
			.on_key_up(cx.listener(Self::key_up))
			.relative()
			.size_full()
			.overflow_hidden()
			.bg(rgb(if self.embedded { CARD_BG } else { palette::BG }))
			.font_family("Inter")
			.child(
				canvas(
					move |bounds, window, cx| {
						prepaint_view.update(cx, |this, cx| this.set_bounds(bounds, cx));
						window.insert_hitbox(bounds, HitboxBehavior::Normal)
					},
					move |bounds, hitbox, window, cx| {
						Self::paint_canvas(&paint_view, bounds, &hitbox, window, cx)
					},
				)
				.absolute()
				.size_full(),
			)
			.child(self.render_toolbar(cx))
			.children(self.render_properties(cx))
			.child(self.render_zoom(cx))
			.children(self.render_text_edit(cx))
	}
}

#[cfg(test)]
mod tests {
	use std::path::Path;
	use std::time::Duration;

	use gpui_kit::{
		AppContext as _, Entity, Modifiers, MouseButton, Pixels, Point, TestAppContext,
		VisualTestContext, point, px,
	};
	use ruby_core::drawing::model::Scene;
	use ruby_core::fs_ops::EMPTY_DRAWING;

	use super::{DrawingView, Tool};
	use crate::workspace::Workspace;

	/// A vault with the drawing `d.excalidraw` open in a tab.
	fn open_drawing<'a>(
		cx: &'a mut TestAppContext,
		json: &str,
	) -> (
		tempfile::TempDir,
		Entity<Workspace>,
		Entity<DrawingView>,
		&'a mut VisualTestContext,
	) {
		let dir = tempfile::tempdir().unwrap();
		std::fs::write(dir.path().join("d.excalidraw"), json).unwrap();
		cx.update(crate::test_support::init);
		let vault = dir.path().to_path_buf();
		let (root, cx) = cx.add_window_view(|window, cx| {
			let ws = cx.new(|cx| Workspace::new(vault, window, cx));
			gpui_kit::component::Root::new(ws, window, cx)
		});
		let ws: Entity<Workspace> = root.read_with(cx, |r, _| r.view().clone().downcast().unwrap());
		ws.update_in(cx, |ws, window, cx| {
			ws.open("d.excalidraw".into(), false, window, cx)
		});
		cx.run_until_parked();
		let view = ws.read_with(cx, |ws, _| ws.active_drawing().expect("a drawing tab"));
		(dir, ws, view, cx)
	}

	/// Opening and repainting a scene of 2000 elements, shown whole. Run with
	/// `cargo test --release -p ruby perf_ -- --ignored --nocapture`.
	#[gpui_kit::test]
	#[ignore]
	fn perf_painting_2000_elements(cx: &mut TestAppContext) {
		let mut rng = ruby_core::drawing::rng::Rng::new(7);
		let kinds = ["rectangle", "ellipse", "diamond", "arrow"];
		let mut scene = Scene::empty();
		for i in 0..2000 {
			let mut el = ruby_core::drawing::model::Element::new(
				kinds[i % 4],
				(i % 50) as f64 * 120.,
				(i / 50) as f64 * 90.,
				&mut rng,
			);
			el.width = 100.;
			el.height = 60.;
			if el.kind == "arrow" {
				el.points = Some(vec![[0., 0.], [100., 60.]]);
			}
			scene.elements.push(el);
		}
		let start = std::time::Instant::now();
		let (_dir, _ws, view, cx) = open_drawing(cx, &scene.to_json());
		println!("open + first paint of 2000 elements: {:?}", start.elapsed());
		let mut times = Vec::new();
		for _ in 0..20 {
			let start = std::time::Instant::now();
			view.update(cx, |_, cx| cx.notify());
			cx.run_until_parked();
			times.push(start.elapsed());
		}
		times.sort();
		let median = times[times.len() / 2];
		println!(
			"repaint of 2000 elements: median {median:?}, worst {:?}",
			times[19]
		);
		if !cfg!(debug_assertions) {
			assert!(median < Duration::from_millis(50), "{median:?}");
		}
	}

	/// A window position from canvas coordinates.
	fn at(view: &Entity<DrawingView>, cx: &mut VisualTestContext, x: f32, y: f32) -> Point<Pixels> {
		let origin = view.read_with(cx, |v, _| v.bounds.origin);
		point(origin.x + px(x), origin.y + px(y))
	}

	fn drag(
		view: &Entity<DrawingView>,
		cx: &mut VisualTestContext,
		from: (f32, f32),
		to: (f32, f32),
	) {
		let (a, b) = (at(view, cx, from.0, from.1), at(view, cx, to.0, to.1));
		let none = Modifiers::default();
		cx.simulate_mouse_down(a, MouseButton::Left, none);
		cx.simulate_mouse_move(b, MouseButton::Left, none);
		cx.simulate_mouse_up(b, MouseButton::Left, none);
		cx.run_until_parked();
	}

	/// `(kind, x, y, width, height)` of every element.
	fn boxes(
		view: &Entity<DrawingView>,
		cx: &mut VisualTestContext,
	) -> Vec<(String, f64, f64, f64, f64)> {
		view.read_with(cx, |v, cx| {
			v.model
				.read(cx)
				.editor
				.scene
				.elements
				.iter()
				.map(|e| (e.kind.clone(), e.x, e.y, e.width, e.height))
				.collect()
		})
	}

	fn draw_rectangle(view: &Entity<DrawingView>, cx: &mut VisualTestContext) {
		cx.simulate_keystrokes("r");
		drag(view, cx, (400., 100.), (500., 180.));
	}

	#[gpui_kit::test]
	fn a_rectangle_is_drawn_with_the_mouse(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		cx.simulate_keystrokes("r");
		view.read_with(cx, |v, _| assert_eq!(v.tool, Tool::Rectangle));
		drag(&view, cx, (400., 100.), (500., 180.));
		assert_eq!(
			boxes(&view, cx),
			[("rectangle".to_string(), 400., 100., 100., 80.)]
		);
		view.read_with(cx, |v, cx| {
			assert_eq!(v.tool, Tool::Select, "back to the selection tool");
			assert_eq!(v.model.read(cx).editor.selected_ids().len(), 1);
		});
		// A click without a drag draws nothing.
		cx.simulate_keystrokes("o");
		drag(&view, cx, (600., 300.), (600., 300.));
		assert_eq!(boxes(&view, cx).len(), 1);
	}

	#[gpui_kit::test]
	fn an_element_is_selected_moved_and_the_move_undone(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		draw_rectangle(&view, cx);
		// Clicking the empty canvas deselects; the outline of the shape selects it again.
		drag(&view, cx, (700., 400.), (700., 400.));
		view.read_with(cx, |v, cx| {
			assert!(v.model.read(cx).editor.selected_ids().is_empty())
		});
		drag(&view, cx, (400., 140.), (450., 160.));
		assert_eq!(
			boxes(&view, cx),
			[("rectangle".to_string(), 450., 120., 100., 80.)]
		);
		cx.simulate_keystrokes("ctrl-z");
		assert_eq!(boxes(&view, cx)[0].1, 400.);
		cx.simulate_keystrokes("ctrl-z");
		assert!(boxes(&view, cx).is_empty());
		cx.simulate_keystrokes("ctrl-y ctrl-y");
		assert_eq!(boxes(&view, cx)[0].1, 450.);
	}

	#[gpui_kit::test]
	fn a_drawing_is_saved_and_read_back(cx: &mut TestAppContext) {
		let (dir, ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		let file = dir.path().join("d.excalidraw");
		draw_rectangle(&view, cx);
		assert_eq!(std::fs::read_to_string(&file).unwrap(), EMPTY_DRAWING);
		// Autosave, after the delay.
		cx.executor().advance_clock(Duration::from_millis(600));
		cx.run_until_parked();
		let saved = Scene::parse(&std::fs::read_to_string(&file).unwrap()).unwrap();
		assert_eq!(saved.elements.len(), 1);
		assert_eq!(saved.elements[0].kind, "rectangle");
		// Closing the tab writes what is pending; reopening shows the same drawing.
		cx.simulate_keystrokes("d");
		drag(&view, cx, (600., 100.), (660., 160.));
		cx.simulate_keystrokes("ctrl-w");
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| assert!(ws.tabs.tabs().is_empty()));
		drop(view);
		ws.update_in(cx, |ws, window, cx| {
			ws.open("d.excalidraw".into(), false, window, cx)
		});
		cx.run_until_parked();
		let view = ws.read_with(cx, |ws, _| ws.active_drawing().unwrap());
		let kinds: Vec<String> = boxes(&view, cx).into_iter().map(|b| b.0).collect();
		assert_eq!(kinds, ["rectangle", "diamond"]);
	}

	#[gpui_kit::test]
	fn keys_switch_tools_and_edit_the_selection(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		for tool in Tool::ALL {
			// The image tool opens a file picker.
			if tool == Tool::Image {
				continue;
			}
			cx.simulate_keystrokes(tool.key());
			view.read_with(cx, |v, _| assert_eq!(v.tool, tool));
		}
		draw_rectangle(&view, cx);
		cx.simulate_keystrokes("ctrl-d");
		assert_eq!(boxes(&view, cx).len(), 2);
		cx.simulate_keystrokes("ctrl-a shift-right");
		let xs: Vec<f64> = boxes(&view, cx).into_iter().map(|b| b.1).collect();
		assert_eq!(xs, [410., 420.]);
		cx.simulate_keystrokes("ctrl-c ctrl-v");
		assert_eq!(boxes(&view, cx).len(), 4);
		cx.simulate_keystrokes("ctrl-a delete");
		assert!(boxes(&view, cx).is_empty());
		cx.simulate_keystrokes("escape");
		view.read_with(cx, |v, _| assert_eq!(v.tool, Tool::Select));
	}

	#[gpui_kit::test]
	fn an_arrow_dropped_on_a_shape_follows_it(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		draw_rectangle(&view, cx);
		cx.simulate_keystrokes("a");
		drag(&view, cx, (700., 140.), (504., 140.));
		let bound = view.read_with(cx, |v, cx| {
			let els = &v.model.read(cx).editor.scene.elements;
			els[1].end_binding.as_ref().map(|b| b.element_id.clone()) == Some(els[0].id.clone())
		});
		assert!(bound, "the arrow end is bound to the rectangle");
		let end_before = boxes(&view, cx)[1].clone();
		// Moving the rectangle (by its top edge) takes the arrow end along.
		drag(&view, cx, (450., 100.), (450., 300.));
		assert_ne!(boxes(&view, cx)[1], end_before);
	}

	#[gpui_kit::test]
	fn zoom_keeps_the_point_under_the_pointer(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		let anchor = at(&view, cx, 200., 100.);
		let before = view.read_with(cx, |v, _| v.transform().to_scene(anchor));
		cx.simulate_event(gpui_kit::ScrollEvent {
			position: anchor,
			delta: gpui_kit::ScrollDelta::Lines(point(0., 3.)),
			modifiers: Modifiers::control(),
			..Default::default()
		});
		cx.run_until_parked();
		view.read_with(cx, |v, _| {
			assert!(v.zoom > 1.2, "zoomed in: {}", v.zoom);
			let after = v.transform().to_scene(anchor);
			assert!((after[0] - before[0]).abs() < 1e-3 && (after[1] - before[1]).abs() < 1e-3);
		});
		cx.simulate_keystrokes("ctrl-0");
		view.read_with(cx, |v, _| assert_eq!(v.zoom_percent(), 100));
	}

	#[gpui_kit::test]
	fn an_external_change_reloads_a_clean_drawing(cx: &mut TestAppContext) {
		let (dir, ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		let file = dir.path().join("d.excalidraw");
		let theirs = EMPTY_DRAWING.replace(
			r#""elements":[]"#,
			r#""elements":[{"id":"a","type":"ellipse","x":1,"y":2,"width":30,"height":40}]"#,
		);
		std::fs::write(&file, &theirs).unwrap();
		ws.update_in(cx, |ws, window, cx| {
			ws.on_fs_events(vec![file.clone()], window, cx)
		});
		cx.run_until_parked();
		assert_eq!(
			boxes(&view, cx),
			[("ellipse".to_string(), 1., 2., 30., 40.)]
		);
		assert_eq!(
			std::fs::read_to_string(&file).unwrap(),
			theirs,
			"a reload never rewrites the file"
		);
	}

	#[gpui_kit::test]
	fn a_file_that_is_not_a_drawing_is_not_opened_nor_touched(cx: &mut TestAppContext) {
		let dir = tempfile::tempdir().unwrap();
		let file = dir.path().join("d.excalidraw");
		std::fs::write(&file, "{ not json").unwrap();
		cx.update(crate::test_support::init);
		let vault = dir.path().to_path_buf();
		let (root, cx) = cx.add_window_view(|window, cx| {
			let ws = cx.new(|cx| Workspace::new(vault, window, cx));
			gpui_kit::component::Root::new(ws, window, cx)
		});
		let ws: Entity<Workspace> = root.read_with(cx, |r, _| r.view().clone().downcast().unwrap());
		ws.update_in(cx, |ws, window, cx| {
			ws.open(Path::new("d.excalidraw").into(), false, window, cx)
		});
		cx.run_until_parked();
		ws.read_with(cx, |ws, _| assert!(ws.active_drawing().is_none()));
		assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
	}

	#[gpui_kit::test]
	fn double_click_types_a_text_and_escape_keeps_it(cx: &mut TestAppContext) {
		let (_dir, _ws, view, cx) = open_drawing(cx, EMPTY_DRAWING);
		let p = at(&view, cx, 500., 300.);
		for click_count in [1, 2] {
			cx.simulate_event(gpui_kit::MouseDownEvent {
				position: p,
				modifiers: Modifiers::default(),
				button: MouseButton::Left,
				click_count,
				first_mouse: false,
			});
			cx.simulate_mouse_up(p, MouseButton::Left, Modifiers::default());
		}
		cx.run_until_parked();
		cx.simulate_input("hello");
		// Tool keys are text while typing.
		view.read_with(cx, |v, _| assert_eq!(v.tool, Tool::Select));
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		let texts: Vec<(Option<String>, bool)> = view.read_with(cx, |v, cx| {
			let els = &v.model.read(cx).editor.scene.elements;
			els.iter().map(|e| (e.text.clone(), e.width > 0.)).collect()
		});
		assert_eq!(texts, [(Some("hello".to_string()), true)]);
		// Creating and typing the text is one undo step.
		cx.simulate_keystrokes("ctrl-z");
		assert!(boxes(&view, cx).is_empty());
		// A text left empty leaves nothing behind.
		cx.simulate_keystrokes("t");
		drag(&view, cx, (500., 300.), (500., 300.));
		cx.simulate_keystrokes("escape");
		cx.run_until_parked();
		assert!(boxes(&view, cx).is_empty());
		view.read_with(cx, |v, cx| {
			assert_eq!(v.model.read(cx).editor.undo_depth(), 0)
		});
	}
}
