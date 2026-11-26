//! Editing a drawing: selection, the commands of the canvas and their undo history.

use std::collections::HashMap;

use super::geometry::{self, Handle, Point, Rect};
use super::model::{Binding, Element, Scene};
use super::rng::Rng;

/// Space left between an arrow end and the shape it is bound to.
pub const BINDING_GAP: f64 = 4.;
/// How close to a shape an arrow end must be dropped to bind to it.
pub const BINDING_REACH: f64 = 12.;

/// The style given to new elements, shown by the properties panel.
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
	pub stroke_color: String,
	pub background_color: String,
	pub stroke_width: f64,
	pub roughness: f64,
	pub opacity: f64,
}

impl Default for Style {
	fn default() -> Self {
		Style {
			stroke_color: "#1e1e1e".into(),
			background_color: "transparent".into(),
			stroke_width: 2.,
			roughness: 1.,
			opacity: 100.,
		}
	}
}

pub struct Editor {
	pub scene: Scene,
	/// Ids of the selected elements, in selection order.
	selection: Vec<String>,
	style: Style,
	undo: Vec<Vec<Element>>,
	redo: Vec<Vec<Element>>,
	rng: Rng,
	/// Grows with every change (autosave, repaint).
	version: u64,
}

impl Editor {
	/// `seed` feeds the ids and seeds of new elements (the current time is a good one).
	pub fn new(scene: Scene, seed: u64) -> Editor {
		Editor {
			scene,
			selection: Vec::new(),
			style: Style::default(),
			undo: Vec::new(),
			redo: Vec::new(),
			rng: Rng::new(seed),
			version: 0,
		}
	}

	pub fn version(&self) -> u64 {
		self.version
	}

	pub fn is_empty(&self) -> bool {
		self.scene.elements.iter().all(|e| e.is_deleted)
	}

	pub fn index_of(&self, id: &str) -> Option<usize> {
		self.scene.elements.iter().position(|e| e.id == id)
	}

	pub fn selected_ids(&self) -> Vec<String> {
		self.selection.clone()
	}

	pub fn is_selected(&self, id: &str) -> bool {
		self.selection.iter().any(|s| s == id)
	}

	/// Indexes of the selected elements, in scene order.
	pub fn selected_indexes(&self) -> Vec<usize> {
		(0..self.scene.elements.len())
			.filter(|i| self.is_selected(&self.scene.elements[*i].id))
			.collect()
	}

	/// The style of the selection (its first element), or the one new elements get.
	pub fn current_style(&self) -> Style {
		match self.selection.first().and_then(|id| self.index_of(id)) {
			Some(i) => {
				let el = &self.scene.elements[i];
				Style {
					stroke_color: el.stroke_color.clone(),
					background_color: el.background_color.clone(),
					stroke_width: el.stroke_width,
					roughness: el.roughness,
					opacity: el.opacity,
				}
			}
			None => self.style.clone(),
		}
	}

	// History.

	/// Remembers the scene as an undo step. Commands do it themselves; a drag calls it
	/// once when it starts.
	pub fn checkpoint(&mut self) {
		self.undo.push(self.scene.elements.clone());
		self.redo.clear();
	}

	/// Drops the last undo step when nothing changed since (a click without a drag).
	pub fn discard_unchanged_checkpoint(&mut self) {
		if self.undo.last() == Some(&self.scene.elements) {
			self.undo.pop();
		}
	}

	/// Goes back to the last checkpoint and forgets it (a shape that was never drawn).
	pub fn cancel_checkpoint(&mut self) {
		if let Some(previous) = self.undo.pop() {
			self.scene.elements = previous;
			self.after_history();
		}
	}

	/// How many steps can be undone.
	pub fn undo_depth(&self) -> usize {
		self.undo.len()
	}

	/// Makes the last two undo steps one (creating a text, then typing it).
	pub fn merge_checkpoints(&mut self) {
		if self.undo.len() >= 2 {
			self.undo.pop();
		}
	}

	pub fn undo(&mut self) -> bool {
		let Some(previous) = self.undo.pop() else {
			return false;
		};
		self.redo
			.push(std::mem::replace(&mut self.scene.elements, previous));
		self.after_history();
		true
	}

	pub fn redo(&mut self) -> bool {
		let Some(next) = self.redo.pop() else {
			return false;
		};
		self.undo
			.push(std::mem::replace(&mut self.scene.elements, next));
		self.after_history();
		true
	}

	fn after_history(&mut self) {
		let elements = &self.scene.elements;
		self.selection
			.retain(|id| elements.iter().any(|e| &e.id == id));
		self.version += 1;
	}

	fn changed(&mut self, index: usize) {
		self.scene.elements[index].touch(&mut self.rng);
		self.version += 1;
	}

	// Selection.

	pub fn clear_selection(&mut self) {
		self.selection.clear();
	}

	pub fn set_selection(&mut self, ids: &[String]) {
		self.selection = ids.to_vec();
	}

	pub fn select_all(&mut self) {
		self.selection = self
			.scene
			.elements
			.iter()
			.filter(|e| !e.is_deleted && e.container_id.is_none())
			.map(|e| e.id.clone())
			.collect();
	}

	/// The topmost visible element under `p`. Text inside a shape counts as its shape.
	pub fn element_at(&self, p: Point, tolerance: f64) -> Option<usize> {
		let elements = &self.scene.elements;
		let hit = (0..elements.len()).rev().find(|i| {
			!elements[*i].is_deleted && geometry::hit_test(&elements[*i], p, tolerance)
		})?;
		match &elements[hit].container_id {
			Some(container) => self.index_of(container).or(Some(hit)),
			None => Some(hit),
		}
	}

	/// The elements selected together with `index`: its outermost group.
	fn with_group(&self, index: usize) -> Vec<String> {
		let el = &self.scene.elements[index];
		match el.group_ids.last() {
			Some(group) => self
				.scene
				.elements
				.iter()
				.filter(|e| !e.is_deleted && e.group_ids.contains(group))
				.map(|e| e.id.clone())
				.collect(),
			None => vec![el.id.clone()],
		}
	}

	/// A click: selects what is under `p` (with its group). `additive` (Shift) adds it to
	/// the selection or removes it. Returns whether something was under the pointer.
	pub fn select_at(&mut self, p: Point, tolerance: f64, additive: bool) -> bool {
		let Some(index) = self.element_at(p, tolerance) else {
			if !additive {
				self.selection.clear();
			}
			return false;
		};
		let ids = self.with_group(index);
		if !additive {
			self.selection = ids;
		} else if ids.iter().all(|id| self.is_selected(id)) {
			self.selection.retain(|s| !ids.contains(s));
		} else {
			for id in ids {
				if !self.is_selected(&id) {
					self.selection.push(id);
				}
			}
		}
		true
	}

	/// Marquee: selects the elements entirely inside `rect`.
	pub fn select_in_rect(&mut self, rect: Rect) {
		let elements = &self.scene.elements;
		self.selection = geometry::elements_in_rect(elements, rect)
			.into_iter()
			.filter(|i| elements[*i].container_id.is_none())
			.map(|i| elements[i].id.clone())
			.collect();
	}

	// Commands.

	/// Adds an element with the current style and selects it (one undo step).
	pub fn add(&mut self, kind: &str, x: f64, y: f64) -> usize {
		self.checkpoint();
		let mut el = Element::new(kind, x, y, &mut self.rng);
		el.stroke_color = self.style.stroke_color.clone();
		el.stroke_width = self.style.stroke_width;
		el.roughness = self.style.roughness;
		el.opacity = self.style.opacity;
		if matches!(kind, "rectangle" | "diamond" | "ellipse") {
			el.background_color = self.style.background_color.clone();
		}
		self.selection = vec![el.id.clone()];
		self.scene.elements.push(el);
		self.version += 1;
		self.scene.elements.len() - 1
	}

	/// Removes elements with what depends on them: their text, the bindings of arrows.
	fn remove(&mut self, ids: &[String]) {
		let mut gone: Vec<String> = ids.to_vec();
		for el in &self.scene.elements {
			if el.container_id.as_ref().is_some_and(|c| ids.contains(c)) {
				gone.push(el.id.clone());
			}
		}
		self.scene.elements.retain(|e| !gone.contains(&e.id));
		for i in 0..self.scene.elements.len() {
			let el = &mut self.scene.elements[i];
			let before = (
				el.bound_elements.len(),
				el.start_binding.is_some(),
				el.end_binding.is_some(),
			);
			el.bound_elements.retain(|(id, _)| !gone.contains(id));
			if el
				.start_binding
				.as_ref()
				.is_some_and(|b| gone.contains(&b.element_id))
			{
				el.start_binding = None;
			}
			if el
				.end_binding
				.as_ref()
				.is_some_and(|b| gone.contains(&b.element_id))
			{
				el.end_binding = None;
			}
			let after = (
				el.bound_elements.len(),
				el.start_binding.is_some(),
				el.end_binding.is_some(),
			);
			if before != after {
				self.changed(i);
			}
		}
		self.selection.retain(|id| !gone.contains(id));
		self.version += 1;
	}

	pub fn delete_selected(&mut self) {
		if self.selection.is_empty() {
			return;
		}
		self.checkpoint();
		let ids = self.selection.clone();
		self.remove(&ids);
	}

	/// Eraser: removes what is under `p`. The caller checkpoints when the drag starts.
	pub fn erase_at(&mut self, p: Point, tolerance: f64) -> bool {
		let Some(index) = self.element_at(p, tolerance) else {
			return false;
		};
		let id = self.scene.elements[index].id.clone();
		self.remove(&[id]);
		true
	}

	/// Moves the selection (and the text inside moved shapes); bound arrows follow.
	pub fn move_selected(&mut self, dx: f64, dy: f64) {
		if dx == 0. && dy == 0. {
			return;
		}
		let selected = self.selection.clone();
		for i in 0..self.scene.elements.len() {
			let el = &self.scene.elements[i];
			let moves = selected.contains(&el.id)
				|| el
					.container_id
					.as_ref()
					.is_some_and(|c| selected.contains(c));
			if moves {
				let el = &mut self.scene.elements[i];
				el.x += dx;
				el.y += dy;
				self.changed(i);
			}
		}
		self.update_bound_arrows();
	}

	/// Copies the selection `(dx, dy)` away and selects the copies (one undo step).
	pub fn duplicate_selected(&mut self, dx: f64, dy: f64) {
		let mut sources = self.selected_indexes();
		if sources.is_empty() {
			return;
		}
		self.checkpoint();
		// The text of a copied shape is copied with it.
		for i in 0..self.scene.elements.len() {
			let el = &self.scene.elements[i];
			if !sources.contains(&i)
				&& el
					.container_id
					.as_ref()
					.is_some_and(|c| self.is_selected(c))
			{
				sources.push(i);
			}
		}
		sources.sort_unstable();
		let originals = sources
			.iter()
			.map(|i| self.scene.elements[*i].clone())
			.collect();
		self.insert_copies(originals, dx, dy);
	}

	/// The selected elements with the text inside them, in scene order.
	fn selected_with_text(&self) -> Vec<Element> {
		self.scene
			.elements
			.iter()
			.filter(|el| {
				!el.is_deleted
					&& (self.is_selected(&el.id)
						|| el
							.container_id
							.as_ref()
							.is_some_and(|c| self.is_selected(c)))
			})
			.cloned()
			.collect()
	}

	/// Adds copies of `originals` moved by `(dx, dy)`, with fresh ids, and selects them.
	fn insert_copies(&mut self, originals: Vec<Element>, dx: f64, dy: f64) {
		let mut ids: HashMap<String, String> = HashMap::new();
		let mut groups: HashMap<String, String> = HashMap::new();
		let mut copies: Vec<Element> = Vec::new();
		for original in originals {
			let mut copy = original;
			let old_id = std::mem::replace(&mut copy.id, self.rng.next_id());
			copy.seed = self.rng.next_seed();
			copy.version = 1;
			copy.version_nonce = self.rng.next_seed();
			copy.x += dx;
			copy.y += dy;
			ids.insert(old_id, copy.id.clone());
			for group in &mut copy.group_ids {
				let rng = &mut self.rng;
				*group = groups
					.entry(group.clone())
					.or_insert_with(|| rng.next_id())
					.clone();
			}
			copies.push(copy);
		}
		// References between copies point to the copies; references to the rest are cut.
		for copy in &mut copies {
			let remap = |b: &Option<Binding>| -> Option<Binding> {
				let b = b.as_ref()?;
				Some(Binding {
					element_id: ids.get(&b.element_id)?.clone(),
					..b.clone()
				})
			};
			copy.start_binding = remap(&copy.start_binding);
			copy.end_binding = remap(&copy.end_binding);
			copy.container_id = copy.container_id.as_ref().and_then(|c| ids.get(c).cloned());
			copy.bound_elements = copy
				.bound_elements
				.iter()
				.filter_map(|(id, kind)| Some((ids.get(id)?.clone(), kind.clone())))
				.collect();
		}
		self.selection = copies
			.iter()
			.filter(|c| c.container_id.is_none())
			.map(|c| c.id.clone())
			.collect();
		self.scene.elements.extend(copies);
		self.version += 1;
	}

	/// The selection as Excalidraw clipboard JSON (with the pictures it uses).
	pub fn copy_selected(&self) -> Option<String> {
		let elements = self.selected_with_text();
		if elements.is_empty() {
			return None;
		}
		let mut files = serde_json::Map::new();
		for el in &elements {
			if let Some(id) = &el.file_id
				&& let Some(file) = self.scene.files().and_then(|f| f.get(id))
			{
				files.insert(id.clone(), file.clone());
			}
		}
		let elements: Vec<serde_json::Value> = elements.iter().map(Element::to_value).collect();
		let value = serde_json::json!({
			"type": "excalidraw/clipboard",
			"elements": elements,
			"files": files,
		});
		Some(value.to_string())
	}

	/// Adds the elements of clipboard JSON (or of a whole drawing), centered on `at` or
	/// slightly off their place, and selects them (one undo step). Other text is refused.
	pub fn paste(&mut self, json: &str, at: Option<Point>) -> bool {
		let Ok(serde_json::Value::Object(mut value)) = serde_json::from_str(json) else {
			return false;
		};
		let is_drawing = matches!(
			value.get("type").and_then(|t| t.as_str()),
			Some("excalidraw/clipboard" | "excalidraw")
		);
		let Some(serde_json::Value::Array(items)) = value.remove("elements") else {
			return false;
		};
		let elements: Vec<Element> = items
			.into_iter()
			.filter_map(|item| match item {
				serde_json::Value::Object(raw) => Some(Element::from_raw(raw)),
				_ => None,
			})
			.filter(|el| !el.is_deleted)
			.collect();
		if !is_drawing || elements.is_empty() {
			return false;
		}
		let (dx, dy) = match (at, geometry::scene_bounds(&elements)) {
			(Some(at), Some(bounds)) => {
				let c = bounds.center();
				(at[0] - c[0], at[1] - c[1])
			}
			_ => (10., 10.),
		};
		self.checkpoint();
		if let Some(serde_json::Value::Object(files)) = value.remove("files") {
			for (id, file) in files {
				let known = self.scene.files().is_some_and(|f| f.contains_key(&id));
				let text = |key: &str| file.get(key).and_then(|v| v.as_str());
				if !known && let (Some(mime), Some(url)) = (text("mimeType"), text("dataURL")) {
					self.scene.add_file(&id, mime, url);
				}
			}
		}
		self.insert_copies(elements, dx, dy);
		true
	}

	/// Adds a picture (a `data:` URL) as an image element of the given box (one undo step).
	pub fn add_image(
		&mut self,
		mime: &str,
		data_url: &str,
		x: f64,
		y: f64,
		width: f64,
		height: f64,
	) -> usize {
		let file = self.rng.next_id();
		self.scene.add_file(&file, mime, data_url);
		let index = self.add("image", x, y);
		let el = &mut self.scene.elements[index];
		el.file_id = Some(file);
		el.width = width;
		el.height = height;
		el.stroke_color = "transparent".into();
		index
	}

	/// While a new shape is dragged out: its box goes from `a` to `b`.
	pub fn set_box(&mut self, index: usize, a: Point, b: Point) {
		let rect = Rect::from_points(a, b);
		let el = &mut self.scene.elements[index];
		(el.x, el.y, el.width, el.height) = (rect.x, rect.y, rect.w, rect.h);
		self.changed(index);
	}

	/// While the pencil is dragged: adds `p` to the stroke (repeated points are skipped).
	pub fn add_freedraw_point(&mut self, index: usize, p: Point) {
		let el = &mut self.scene.elements[index];
		let rel = [p[0] - el.x, p[1] - el.y];
		let points = el.points.get_or_insert_with(Vec::new);
		if points.last() == Some(&rel) {
			return;
		}
		points.push(rel);
		geometry::fit_linear(el);
		self.changed(index);
	}

	/// Shows another scene (the file changed on disk): history and selection are forgotten.
	pub fn replace_scene(&mut self, scene: Scene) {
		self.scene = scene;
		self.selection.clear();
		self.undo.clear();
		self.redo.clear();
		self.version += 1;
	}

	/// Groups the selection (two elements or more).
	pub fn group(&mut self) {
		let indexes = self.selected_indexes();
		if indexes.len() < 2 {
			return;
		}
		self.checkpoint();
		let group = self.rng.next_id();
		for i in indexes {
			self.scene.elements[i].group_ids.push(group.clone());
			self.changed(i);
		}
	}

	/// Dissolves the outermost group of each selected element.
	pub fn ungroup(&mut self) {
		let indexes = self.selected_indexes();
		if indexes
			.iter()
			.all(|i| self.scene.elements[*i].group_ids.is_empty())
		{
			return;
		}
		self.checkpoint();
		for i in indexes {
			if self.scene.elements[i].group_ids.pop().is_some() {
				self.changed(i);
			}
		}
	}

	/// Applies a style change to the selection (one undo step).
	fn restyle(&mut self, apply: impl Fn(&mut Element)) {
		let indexes = self.selected_indexes();
		if indexes.is_empty() {
			return;
		}
		self.checkpoint();
		for i in indexes {
			apply(&mut self.scene.elements[i]);
			self.changed(i);
		}
	}

	pub fn set_stroke_color(&mut self, color: &str) {
		self.style.stroke_color = color.to_string();
		self.restyle(|e| e.stroke_color = color.to_string());
	}

	pub fn set_background(&mut self, color: &str) {
		self.style.background_color = color.to_string();
		self.restyle(|e| {
			if matches!(e.kind.as_str(), "rectangle" | "diamond" | "ellipse") {
				e.background_color = color.to_string();
			}
		});
	}

	pub fn set_stroke_width(&mut self, width: f64) {
		self.style.stroke_width = width;
		self.restyle(|e| e.stroke_width = width);
	}

	pub fn set_roughness(&mut self, roughness: f64) {
		self.style.roughness = roughness;
		self.restyle(|e| e.roughness = roughness);
	}

	pub fn set_opacity(&mut self, opacity: f64) {
		let opacity = opacity.clamp(0., 100.);
		self.style.opacity = opacity;
		self.restyle(|e| e.opacity = opacity);
	}

	// Text.

	/// The text element inside the shape `container`.
	pub fn text_of(&self, container: &str) -> Option<String> {
		self.scene
			.elements
			.iter()
			.find(|e| {
				!e.is_deleted && e.kind == "text" && e.container_id.as_deref() == Some(container)
			})
			.map(|e| e.id.clone())
	}

	/// The text of a shape, created empty and centered when it has none yet.
	pub fn add_text_in(&mut self, container: &str) -> Option<String> {
		if let Some(existing) = self.text_of(container) {
			return Some(existing);
		}
		let index = self.index_of(container)?;
		let c = geometry::center(&self.scene.elements[index]);
		let angle = self.scene.elements[index].angle;
		let i = self.add("text", c[0], c[1]);
		let id = self.scene.elements[i].id.clone();
		let el = &mut self.scene.elements[i];
		el.container_id = Some(container.to_string());
		el.text_align = "center".into();
		el.vertical_align = "middle".into();
		el.angle = angle;
		self.scene.elements[index]
			.bound_elements
			.push((id.clone(), "text".into()));
		self.selection = vec![container.to_string()];
		Some(id)
	}

	/// Keeps the text of the shape at `container` centered in it.
	fn center_text(&mut self, container: usize) {
		let shape = &self.scene.elements[container];
		let (c, angle) = (geometry::center(shape), shape.angle);
		let Some(text) = self.text_of(&shape.id).and_then(|id| self.index_of(&id)) else {
			return;
		};
		let el = &mut self.scene.elements[text];
		let (x, y) = (c[0] - el.width / 2., c[1] - el.height / 2.);
		if (el.x, el.y, el.angle) != (x, y, angle) {
			(el.x, el.y, el.angle) = (x, y, angle);
			self.changed(text);
		}
	}

	/// Sets the text of a text element and its measured size (one undo step). An empty
	/// text removes the element.
	pub fn set_text(&mut self, id: &str, text: &str, width: f64, height: f64) {
		let Some(index) = self.index_of(id) else {
			return;
		};
		self.checkpoint();
		if text.trim().is_empty() {
			self.remove(&[id.to_string()]);
			return;
		}
		let el = &mut self.scene.elements[index];
		el.text = Some(text.to_string());
		el.width = width;
		el.height = height;
		let container = el.container_id.clone();
		self.changed(index);
		if let Some(container) = container.and_then(|c| self.index_of(&c)) {
			self.center_text(container);
		}
	}

	// Resize and rotate (one selected element).

	/// Resizes the selected element by a handle; `original` is the element when the drag
	/// started.
	pub fn resize_selected(
		&mut self,
		original: &Element,
		handle: Handle,
		to: Point,
		keep_ratio: bool,
	) {
		let Some(index) = self.index_of(&original.id) else {
			return;
		};
		geometry::resize(
			&mut self.scene.elements[index],
			original,
			handle,
			to,
			keep_ratio,
		);
		self.changed(index);
		self.center_text(index);
		self.update_bound_arrows();
	}

	/// Turns the first selected element toward `p`.
	pub fn rotate_selected(&mut self, p: Point, snap: bool) {
		let Some(index) = self.selection.first().and_then(|id| self.index_of(id)) else {
			return;
		};
		geometry::rotate_to(&mut self.scene.elements[index], p, snap);
		self.changed(index);
		self.center_text(index);
		self.update_bound_arrows();
	}

	// Arrows.

	/// The shape an arrow end at `p` attaches to.
	fn bind_target(&self, p: Point, arrow: &str) -> Option<usize> {
		let elements = &self.scene.elements;
		(0..elements.len()).rev().find(|i| {
			let el = &elements[*i];
			el.id != arrow
				&& el.container_id.is_none()
				&& geometry::bindable_at(std::slice::from_ref(el), p, BINDING_REACH).is_some()
		})
	}

	/// Moves one end of a line or arrow to `p`.
	pub fn move_arrow_end(&mut self, index: usize, start: bool, p: Point) {
		let el = &mut self.scene.elements[index];
		let mut points = geometry::abs_points(el);
		let Some(target) = (if start {
			points.first_mut()
		} else {
			points.last_mut()
		}) else {
			return;
		};
		*target = p;
		let origin = points[0];
		el.points = Some(
			points
				.iter()
				.map(|q| [q[0] - origin[0], q[1] - origin[1]])
				.collect(),
		);
		(el.x, el.y) = (origin[0], origin[1]);
		geometry::fit_linear(el);
		self.changed(index);
	}

	/// After an arrow was drawn or one of its ends dragged: binds each end to the shape it
	/// is on or near, lets go of the others, and snaps bound ends to the outlines.
	pub fn bind_arrow(&mut self, index: usize) {
		let arrow = &self.scene.elements[index];
		if arrow.kind != "arrow" {
			return;
		}
		let id = arrow.id.clone();
		let points = geometry::abs_points(arrow);
		let (Some(first), Some(last)) = (points.first().copied(), points.last().copied()) else {
			return;
		};
		let start = self.bind_target(first, &id);
		// Both ends on one shape: only the start is bound.
		let end = self.bind_target(last, &id).filter(|e| Some(*e) != start);
		let binding = |i: Option<usize>| {
			i.map(|i| Binding {
				element_id: self.scene.elements[i].id.clone(),
				focus: 0.,
				gap: BINDING_GAP,
			})
		};
		let (start_binding, end_binding) = (binding(start), binding(end));
		let bound: Vec<usize> = start.into_iter().chain(end).collect();
		let arrow = &mut self.scene.elements[index];
		arrow.start_binding = start_binding;
		arrow.end_binding = end_binding;
		self.changed(index);
		for i in 0..self.scene.elements.len() {
			let el = &mut self.scene.elements[i];
			let listed = el.bound_elements.iter().any(|(b, _)| b == &id);
			if bound.contains(&i) && !listed {
				el.bound_elements.push((id.clone(), "arrow".into()));
				self.changed(i);
			} else if !bound.contains(&i) && listed {
				el.bound_elements.retain(|(b, _)| b != &id);
				self.changed(i);
			}
		}
		self.update_bound_arrows();
	}

	/// Puts the bound ends of every arrow on the outline of their shape, facing the other
	/// end.
	pub fn update_bound_arrows(&mut self) {
		for index in 0..self.scene.elements.len() {
			let arrow = &self.scene.elements[index];
			if arrow.is_deleted || (arrow.start_binding.is_none() && arrow.end_binding.is_none()) {
				continue;
			}
			let points = geometry::abs_points(arrow);
			let (Some(first), Some(last)) = (points.first().copied(), points.last().copied())
			else {
				continue;
			};
			let shape = |b: &Option<Binding>| b.as_ref().and_then(|b| self.index_of(&b.element_id));
			let (from, to) = (shape(&arrow.start_binding), shape(&arrow.end_binding));
			let center = |i: usize| geometry::center(&self.scene.elements[i]);
			// Each end looks at the other shape's center, or at the other end when free.
			let straight = points.len() == 2;
			let toward_start = match (to, straight) {
				(Some(t), true) => center(t),
				_ => points.get(1).copied().unwrap_or(last),
			};
			let toward_end = match (from, straight) {
				(Some(f), true) => center(f),
				_ => points
					.get(points.len().saturating_sub(2))
					.copied()
					.unwrap_or(first),
			};
			let gap = |b: &Option<Binding>| b.as_ref().map_or(BINDING_GAP, |b| b.gap);
			let new_first = from.map(|f| {
				geometry::outline_point(
					&self.scene.elements[f],
					toward_start,
					gap(&arrow.start_binding),
				)
			});
			let new_last = to.map(|t| {
				geometry::outline_point(
					&self.scene.elements[t],
					toward_end,
					gap(&arrow.end_binding),
				)
			});
			if let Some(p) = new_first.filter(|p| *p != first) {
				self.move_arrow_end(index, true, p);
			}
			if let Some(p) = new_last.filter(|p| *p != last) {
				self.move_arrow_end(index, false, p);
			}
		}
	}
}

#[cfg(test)]
#[allow(clippy::cloned_ref_to_slice_refs)]
mod tests {
	use super::*;
	use crate::drawing::geometry::{self, Handle, Rect};
	use crate::drawing::model::Scene;

	fn editor() -> Editor {
		Editor::new(Scene::empty(), 1)
	}

	/// Adds a shape of the given box and returns its id.
	fn shape(ed: &mut Editor, kind: &str, x: f64, y: f64, w: f64, h: f64) -> String {
		let i = ed.add(kind, x, y);
		let el = &mut ed.scene.elements[i];
		el.width = w;
		el.height = h;
		el.id.clone()
	}

	fn arrow(ed: &mut Editor, from: [f64; 2], to: [f64; 2]) -> String {
		let i = ed.add("arrow", from[0], from[1]);
		let el = &mut ed.scene.elements[i];
		el.points = Some(vec![[0., 0.], [to[0] - from[0], to[1] - from[1]]]);
		geometry::fit_linear(el);
		ed.bind_arrow(i);
		ed.scene.elements[i].id.clone()
	}

	fn get<'a>(ed: &'a Editor, id: &str) -> &'a Element {
		ed.scene.elements.iter().find(|e| e.id == id).unwrap()
	}

	fn ends(ed: &Editor, id: &str) -> ([f64; 2], [f64; 2]) {
		let points = geometry::abs_points(get(ed, id));
		(points[0], *points.last().unwrap())
	}

	#[test]
	fn add_undo_redo() {
		let mut ed = editor();
		let v0 = ed.version();
		let a = shape(&mut ed, "rectangle", 0., 0., 10., 10.);
		assert!(ed.version() > v0);
		assert_eq!(ed.selected_ids(), [a.clone()]);
		shape(&mut ed, "ellipse", 20., 0., 10., 10.);
		assert_eq!(ed.scene.elements.len(), 2);
		assert!(ed.undo());
		assert_eq!(ed.scene.elements.len(), 1);
		assert!(ed.undo());
		assert!(ed.scene.elements.is_empty());
		assert!(
			ed.selected_ids().is_empty(),
			"a selected element that is gone is deselected"
		);
		assert!(!ed.undo());
		assert!(ed.redo() && ed.redo() && !ed.redo());
		assert_eq!(ed.scene.elements.len(), 2);
		// A new edit drops the redo history.
		ed.undo();
		shape(&mut ed, "diamond", 0., 0., 5., 5.);
		assert!(!ed.redo());
	}

	#[test]
	fn selecting() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let b = shape(&mut ed, "rectangle", 50., 50., 100., 100.);
		for id in [&a, &b] {
			let i = ed.index_of(id).unwrap();
			ed.scene.elements[i].background_color = "#ffc9c9".into();
		}
		ed.clear_selection();
		assert!(ed.select_at([75., 75.], 2., false));
		assert_eq!(ed.selected_ids(), [b.clone()], "the topmost element wins");
		assert!(ed.select_at([10., 10.], 2., true));
		assert_eq!(ed.selected_ids(), [b.clone(), a.clone()]);
		assert!(
			ed.select_at([10., 10.], 2., true),
			"shift+click on a selected element removes it"
		);
		assert_eq!(ed.selected_ids(), [b.clone()]);
		assert!(!ed.select_at([500., 500.], 2., false));
		assert!(ed.selected_ids().is_empty());
		ed.select_in_rect(Rect {
			x: -5.,
			y: -5.,
			w: 120.,
			h: 120.,
		});
		assert_eq!(ed.selected_ids(), [a.clone()]);
		ed.select_all();
		assert_eq!(ed.selected_ids().len(), 2);
		// One click selects a whole group.
		ed.group();
		ed.clear_selection();
		ed.select_at([10., 10.], 2., false);
		assert_eq!(ed.selected_ids().len(), 2);
		ed.ungroup();
		ed.clear_selection();
		ed.select_at([10., 10.], 2., false);
		assert_eq!(ed.selected_ids(), [a]);
	}

	#[test]
	fn moving_takes_bound_text_and_arrows_along() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let b = shape(&mut ed, "rectangle", 300., 0., 100., 100.);
		let t = ed.add_text_in(&a).unwrap();
		ed.set_text(&t, "hi", 20., 25.);
		assert_eq!(
			(get(&ed, &t).x, get(&ed, &t).y),
			(40., 37.5),
			"centered in its shape"
		);
		let arr = arrow(&mut ed, [100., 50.], [300., 50.]);
		let (start, end) = ends(&ed, &arr);
		assert_eq!(get(&ed, &arr).start_binding.as_ref().unwrap().element_id, a);
		assert_eq!(get(&ed, &arr).end_binding.as_ref().unwrap().element_id, b);
		assert!(
			get(&ed, &a)
				.bound_elements
				.contains(&(arr.clone(), "arrow".to_string()))
		);
		assert!((start[0] - (100. + BINDING_GAP)).abs() < 1e-6 && (start[1] - 50.).abs() < 1e-6);
		assert!((end[0] - (300. - BINDING_GAP)).abs() < 1e-6);
		ed.set_selection(&[a.clone()]);
		ed.checkpoint();
		ed.move_selected(0., 200.);
		assert_eq!(get(&ed, &a).y, 200.);
		assert_eq!(get(&ed, &t).y, 237.5);
		let (start, end) = ends(&ed, &arr);
		// The arrow now runs between the two shapes' outlines, toward each other's center.
		assert!(
			geometry::hit_test(get(&ed, &a), start, BINDING_GAP + 1.),
			"{start:?}"
		);
		assert!(
			geometry::hit_test(get(&ed, &b), end, BINDING_GAP + 1.),
			"{end:?}"
		);
		assert!(start[1] > 200. && end[1] < 100.);
		ed.undo();
		assert_eq!(get(&ed, &a).y, 0.);
		assert_eq!(ends(&ed, &arr).0[1], 50.);
	}

	#[test]
	fn deleting() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let t = ed.add_text_in(&a).unwrap();
		ed.set_text(&t, "hi", 20., 25.);
		let arr = arrow(&mut ed, [100., 50.], [300., 50.]);
		ed.set_selection(&[a.clone()]);
		ed.delete_selected();
		assert!(
			ed.index_of(&a).is_none() && ed.index_of(&t).is_none(),
			"its text goes with it"
		);
		assert!(
			get(&ed, &arr).start_binding.is_none(),
			"arrows let go of a deleted shape"
		);
		assert!(ed.selected_ids().is_empty());
		ed.undo();
		assert!(ed.index_of(&a).is_some() && ed.index_of(&t).is_some());
		assert!(get(&ed, &arr).start_binding.is_some());
		// Deleting an arrow removes it from the shapes it was bound to.
		ed.set_selection(&[arr.clone()]);
		ed.delete_selected();
		assert!(get(&ed, &a).bound_elements.iter().all(|(id, _)| id != &arr));
		// Erasing works on what is under the pointer.
		ed.checkpoint();
		assert!(ed.erase_at([0., 50.], 3.));
		assert!(ed.index_of(&a).is_none());
		assert!(!ed.erase_at([0., 50.], 3.));
	}

	#[test]
	fn duplicating_and_grouping() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let b = shape(&mut ed, "ellipse", 200., 0., 100., 100.);
		let arr = arrow(&mut ed, [100., 50.], [200., 50.]);
		ed.select_all();
		ed.group();
		let group = get(&ed, &a).group_ids.clone();
		assert_eq!(group.len(), 1);
		ed.duplicate_selected(10., 10.);
		assert_eq!(ed.scene.elements.len(), 6);
		let copies = ed.selected_ids();
		assert_eq!(copies.len(), 3);
		assert!(copies.iter().all(|id| ![&a, &b, &arr].contains(&id)));
		let copy_a = get(&ed, &copies[0]);
		assert_eq!((copy_a.x, copy_a.y), (10., 10.));
		assert_ne!(copy_a.seed, get(&ed, &a).seed);
		assert_eq!(copy_a.group_ids.len(), 1);
		assert_ne!(copy_a.group_ids, group, "the copies are their own group");
		assert_eq!(get(&ed, &copies[1]).group_ids, copy_a.group_ids);
		// The copied arrow is bound to the copied shapes.
		let copy_arrow = get(&ed, &copies[2]);
		assert_eq!(
			copy_arrow.start_binding.as_ref().unwrap().element_id,
			copies[0]
		);
		assert_eq!(
			copy_arrow.end_binding.as_ref().unwrap().element_id,
			copies[1]
		);
		// Copying an arrow alone leaves it unbound.
		ed.set_selection(&[arr.clone()]);
		ed.ungroup();
		ed.set_selection(&[arr.clone()]);
		ed.duplicate_selected(0., 30.);
		let lone = ed.selected_ids();
		assert!(get(&ed, &lone[0]).start_binding.is_none());
		ed.undo();
		assert_eq!(ed.scene.elements.len(), 6);
	}

	#[test]
	fn styles_apply_to_the_selection_and_to_new_elements() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 10., 10.);
		let v = get(&ed, &a).version;
		ed.set_stroke_color("#ea4d4d");
		ed.set_background("#fcd2d2");
		ed.set_stroke_width(4.);
		ed.set_roughness(0.);
		ed.set_opacity(50.);
		let el = get(&ed, &a);
		assert_eq!(
			(el.stroke_color.as_str(), el.background_color.as_str()),
			("#ea4d4d", "#fcd2d2")
		);
		assert_eq!((el.stroke_width, el.roughness, el.opacity), (4., 0., 50.));
		assert!(el.version > v);
		let b = shape(&mut ed, "ellipse", 0., 0., 10., 10.);
		assert_eq!(get(&ed, &b).stroke_color, "#ea4d4d");
		assert_eq!(get(&ed, &b).opacity, 50.);
		// The panel shows the style of the selection.
		ed.set_selection(&[a]);
		assert_eq!(ed.current_style().stroke_width, 4.);
		for _ in 0..7 {
			ed.undo();
		}
		assert!(ed.scene.elements.is_empty());
	}

	#[test]
	fn text_editing() {
		let mut ed = editor();
		let i = ed.add("text", 5., 5.);
		let t = ed.scene.elements[i].id.clone();
		ed.set_text(&t, "hello", 50., 25.);
		let el = get(&ed, &t);
		assert_eq!(el.text.as_deref(), Some("hello"));
		assert_eq!((el.width, el.height), (50., 25.));
		// Emptied text disappears.
		ed.set_text(&t, "  ", 0., 25.);
		assert!(ed.index_of(&t).is_none());
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let bound = ed.add_text_in(&a).unwrap();
		assert_eq!(
			ed.add_text_in(&a).as_deref(),
			Some(bound.as_str()),
			"one text per shape"
		);
		assert_eq!(ed.text_of(&a).as_deref(), Some(bound.as_str()));
		let el = get(&ed, &bound);
		assert_eq!(el.container_id.as_deref(), Some(a.as_str()));
		assert_eq!(
			(el.text_align.as_str(), el.vertical_align.as_str()),
			("center", "middle")
		);
		ed.set_text(&bound, "", 0., 25.);
		assert!(ed.text_of(&a).is_none());
		assert!(get(&ed, &a).bound_elements.is_empty());
		assert!(ed.add_text_in("missing").is_none());
	}

	#[test]
	fn arrow_ends_bind_and_let_go() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let arr = arrow(&mut ed, [200., 50.], [108., 50.]);
		let el = get(&ed, &arr);
		assert!(el.start_binding.is_none());
		assert_eq!(el.end_binding.as_ref().unwrap().element_id, a);
		assert!(
			(ends(&ed, &arr).1[0] - (100. + BINDING_GAP)).abs() < 1e-6,
			"snapped to the outline"
		);
		// Dragging the end away lets go.
		let i = ed.index_of(&arr).unwrap();
		ed.checkpoint();
		ed.move_arrow_end(i, false, [250., 300.]);
		ed.bind_arrow(i);
		assert!(get(&ed, &arr).end_binding.is_none());
		assert!(get(&ed, &a).bound_elements.is_empty());
		assert_eq!(ends(&ed, &arr).1, [250., 300.]);
		assert_eq!(ends(&ed, &arr).0, [200., 50.]);
		// An arrow never binds both ends to the same shape.
		let both = arrow(&mut ed, [10., 10.], [90., 90.]);
		let el = get(&ed, &both);
		assert!(!(el.start_binding.is_some() && el.end_binding.is_some()));
	}

	#[test]
	fn resizing_and_rotating_the_selection() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 100.);
		let t = ed.add_text_in(&a).unwrap();
		ed.set_text(&t, "hi", 20., 20.);
		let arr = arrow(&mut ed, [300., 50.], [108., 50.]);
		ed.set_selection(&[a.clone()]);
		ed.checkpoint();
		let original = get(&ed, &a).clone();
		ed.resize_selected(&original, Handle::SE, [200., 200.], false);
		assert_eq!((get(&ed, &a).width, get(&ed, &a).height), (200., 200.));
		assert_eq!(
			(get(&ed, &t).x, get(&ed, &t).y),
			(90., 90.),
			"its text stays centered"
		);
		let end = ends(&ed, &arr).1;
		assert!(
			end[0] > 200. && end[0] <= 200. + BINDING_GAP,
			"its arrow follows: {end:?}"
		);
		ed.rotate_selected([500., 100.], false);
		assert!((get(&ed, &a).angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
		assert_eq!(get(&ed, &t).angle, get(&ed, &a).angle);
		ed.undo();
		assert_eq!(get(&ed, &a).width, 100.);
		assert_eq!(get(&ed, &a).angle, 0.);
	}

	#[test]
	fn a_drag_that_changes_nothing_leaves_no_undo_step() {
		let mut ed = editor();
		shape(&mut ed, "rectangle", 0., 0., 10., 10.);
		let v = ed.version();
		ed.checkpoint();
		ed.move_selected(0., 0.);
		ed.discard_unchanged_checkpoint();
		assert_eq!(ed.version(), v);
		assert!(ed.undo());
		assert!(
			ed.scene.elements.is_empty(),
			"the only step is the creation"
		);
		assert!(ed.is_empty());
	}

	#[test]
	fn creation_drags() {
		let mut ed = editor();
		let i = ed.add("rectangle", 50., 50.);
		ed.set_box(i, [50., 50.], [20., 90.]);
		let el = &ed.scene.elements[i];
		assert_eq!((el.x, el.y, el.width, el.height), (20., 50., 30., 40.));
		let f = ed.add("freedraw", 10., 10.);
		ed.add_freedraw_point(f, [10., 10.]);
		ed.add_freedraw_point(f, [5., 30.]);
		ed.add_freedraw_point(f, [5., 30.]);
		let el = &ed.scene.elements[f];
		assert_eq!(geometry::abs_points(el), [[10., 10.], [5., 30.]]);
		assert_eq!((el.width, el.height), (5., 20.));
		// A shape that was never given a size is dropped without a trace in the history.
		let n = ed.scene.elements.len();
		ed.add("ellipse", 0., 0.);
		ed.cancel_checkpoint();
		assert_eq!(ed.scene.elements.len(), n);
		assert!(!ed.redo());
		assert!(ed.selected_ids().is_empty());
	}

	#[test]
	fn merged_checkpoints_undo_as_one_step() {
		let mut ed = editor();
		let i = ed.add("text", 0., 0.);
		let id = ed.scene.elements[i].id.clone();
		ed.set_text(&id, "hello", 50., 25.);
		assert_eq!(ed.undo_depth(), 2);
		ed.merge_checkpoints();
		assert_eq!(ed.undo_depth(), 1);
		assert!(ed.undo());
		assert!(ed.scene.elements.is_empty());
		assert!(!ed.undo());
	}

	#[test]
	fn copy_and_paste() {
		let mut ed = editor();
		let a = shape(&mut ed, "rectangle", 0., 0., 100., 50.);
		let t = ed.add_text_in(&a).unwrap();
		ed.set_text(&t, "hi", 20., 10.);
		let b = shape(&mut ed, "ellipse", 300., 0., 100., 50.);
		let arr = arrow(&mut ed, [100., 25.], [300., 25.]);
		assert!(editor().copy_selected().is_none(), "nothing selected");
		ed.set_selection(&[a.clone(), arr.clone()]);
		let json = ed.copy_selected().unwrap();
		// Pasted into another drawing, centered on a point.
		let mut other = Editor::new(Scene::empty(), 2);
		assert!(other.paste(&json, Some([1000., 1000.])));
		let els = &other.scene.elements;
		assert_eq!(els.len(), 3, "the shape, its text and the arrow");
		let rect = els.iter().find(|e| e.kind == "rectangle").unwrap();
		let text = els.iter().find(|e| e.kind == "text").unwrap();
		let arrow = els.iter().find(|e| e.kind == "arrow").unwrap();
		assert_ne!(rect.id, a);
		assert_eq!(text.container_id.as_deref(), Some(rect.id.as_str()));
		assert_eq!(
			arrow.start_binding.as_ref().map(|b| b.element_id.as_str()),
			Some(rect.id.as_str())
		);
		assert!(arrow.end_binding.is_none(), "{b} was not copied");
		let bounds = geometry::scene_bounds(els).unwrap();
		let c = bounds.center();
		assert!((c[0] - 1000.).abs() < 1e-6 && (c[1] - 1000.).abs() < 1e-6);
		assert_eq!(other.selected_ids().len(), 2);
		assert!(other.undo());
		assert!(other.scene.elements.is_empty());
		// Pasting in place offsets the copies; text that is not a drawing is refused.
		assert!(ed.paste(&json, None));
		assert_eq!(ed.scene.elements.len(), 7);
		assert!(!ed.paste("just text", None));
		assert!(!ed.paste(r#"{"type":"excalidraw/clipboard","elements":[]}"#, None));
	}

	#[test]
	fn images_are_added_with_their_file_and_follow_copies() {
		let mut ed = editor();
		let i = ed.add_image(
			"image/png",
			"data:image/png;base64,AAAA",
			10.,
			20.,
			30.,
			40.,
		);
		let el = ed.scene.elements[i].clone();
		assert_eq!(el.kind, "image");
		assert_eq!((el.x, el.y, el.width, el.height), (10., 20., 30., 40.));
		let file = el.file_id.clone().unwrap();
		assert!(ed.scene.files().unwrap().contains_key(&file));
		let json = ed.copy_selected().unwrap();
		let mut other = editor();
		assert!(other.paste(&json, None));
		assert!(other.scene.files().unwrap().contains_key(&file));
		assert!(ed.undo());
		assert!(ed.is_empty());
	}

	#[test]
	fn replacing_the_scene_forgets_history_and_selection() {
		let mut ed = editor();
		shape(&mut ed, "rectangle", 0., 0., 10., 10.);
		let v = ed.version();
		ed.replace_scene(Scene::empty());
		assert!(ed.version() > v);
		assert!(ed.is_empty() && ed.selected_ids().is_empty());
		assert!(!ed.undo());
	}
}
