//! Drawings: the shared model of an open `.excalidraw` file, and its canvas view.
//!
//! A file has one [`DrawingModel`], whether it is shown in a tab, embedded in notes, or
//! both: every view shows the same edits live.

pub mod paint;
pub mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use gpui_kit::*;
use ruby_core::document::{AUTOSAVE_DELAY_MS, ExternalOutcome, OpenDocument};
use ruby_core::drawing::edit::Editor;
use ruby_core::drawing::model::{Element, Scene};
use ruby_core::drawing::rough::{Shape, rough_shape};

pub use view::DrawingView;

#[derive(Debug, Clone)]
pub enum DrawingEvent {
	/// The disk version was kept as this conflict copy.
	Conflict(PathBuf),
	SaveFailed(String),
}

pub struct DrawingModel {
	pub editor: Editor,
	pub doc: OpenDocument,
	/// The editor version last seen: a newer one is an edit to save.
	seen_version: u64,
	save_task: Option<Task<()>>,
	/// Hand-drawn geometry by element id, valid for one element version.
	shapes: RefCell<HashMap<String, (i64, Rc<Shape>)>>,
	/// Decoded pictures by file id (`None`: not a picture this app can show).
	pictures: RefCell<HashMap<String, Option<Arc<RenderImage>>>>,
}

impl EventEmitter<DrawingEvent> for DrawingModel {}

/// Bytes and format of a `data:image/...;base64,...` URL.
fn decode_data_url(url: &str) -> Option<Image> {
	let rest = url.strip_prefix("data:")?;
	let (meta, data) = rest.split_once(',')?;
	let mime = meta.strip_suffix(";base64")?;
	let format = ImageFormat::from_mime_type(mime)?;
	let bytes = base64::engine::general_purpose::STANDARD
		.decode(data.trim())
		.ok()?;
	Some(Image::from_bytes(format, bytes))
}

pub fn encode_data_url(mime: &str, bytes: &[u8]) -> String {
	format!(
		"data:{mime};base64,{}",
		base64::engine::general_purpose::STANDARD.encode(bytes)
	)
}

impl DrawingModel {
	/// Reads a drawing. A file that is not a drawing is an error and is never written to.
	pub fn load(path: &Path) -> Result<DrawingModel, String> {
		let (doc, text) = OpenDocument::load(path).map_err(|e| e.to_string())?;
		if doc.read_only {
			return Err("not a drawing: the file is not UTF-8 text".to_string());
		}
		let scene = Scene::parse(&text)?;
		let seed = std::time::SystemTime::now()
			.duration_since(std::time::UNIX_EPOCH)
			.map_or(1, |d| d.as_nanos() as u64);
		let editor = Editor::new(scene, seed);
		Ok(DrawingModel {
			seen_version: editor.version(),
			editor,
			doc,
			save_task: None,
			shapes: RefCell::default(),
			pictures: RefCell::default(),
		})
	}

	/// Runs an editing command, then schedules the save and repaints the views.
	pub fn edit<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor) -> R) -> R {
		let result = f(&mut self.editor);
		if self.editor.version() != self.seen_version {
			self.seen_version = self.editor.version();
			self.doc.mark_dirty();
			self.schedule_save(cx);
		}
		cx.notify();
		result
	}

	fn schedule_save(&mut self, cx: &mut Context<Self>) {
		self.save_task = Some(cx.spawn(async move |this, cx| {
			cx.background_executor()
				.timer(Duration::from_millis(AUTOSAVE_DELAY_MS))
				.await;
			this.update(cx, |this, cx| this.flush(cx)).ok();
		}));
	}

	/// Writes pending edits now. Errors keep the drawing dirty and are reported.
	pub fn flush(&mut self, cx: &mut Context<Self>) {
		self.save_task = None;
		match self.save(cx) {
			Ok(Some(copy)) => cx.emit(DrawingEvent::Conflict(copy)),
			Ok(None) => {}
			Err(e) => cx.emit(DrawingEvent::SaveFailed(e.to_string())),
		}
	}

	fn save(&mut self, cx: &App) -> std::io::Result<Option<PathBuf>> {
		if !self.doc.is_dirty() {
			return Ok(None);
		}
		let tag = crate::i18n::conflict_tag(cx);
		self.doc.save(&self.editor.scene.to_json(), &tag)
	}

	/// The file as it would be written now.
	pub fn json(&self) -> String {
		self.editor.scene.to_json()
	}

	/// The file changed on disk: reloads a clean drawing, keeps both versions otherwise.
	pub fn disk_changed(&mut self, cx: &mut Context<Self>) -> std::io::Result<ExternalOutcome> {
		let tag = crate::i18n::conflict_tag(cx);
		// A clean drawing never needs its JSON: nothing of it would be written.
		let json = if self.doc.is_dirty() {
			self.json()
		} else {
			String::new()
		};
		let outcome = self.doc.on_disk_changed(&json, &tag)?;
		if let ExternalOutcome::Reloaded(text) = &outcome {
			// A file caught half written is shown when its next change arrives.
			if let Ok(scene) = Scene::parse(text) {
				self.editor.replace_scene(scene);
				self.seen_version = self.editor.version();
				self.shapes.borrow_mut().clear();
				self.pictures.borrow_mut().clear();
			}
			cx.notify();
		}
		Ok(outcome)
	}

	/// The hand-drawn geometry of `el`, computed once per element version.
	pub fn shape(&self, el: &Element) -> Rc<Shape> {
		let mut shapes = self.shapes.borrow_mut();
		if let Some((nonce, shape)) = shapes.get(&el.id)
			&& *nonce == el.version_nonce
		{
			return shape.clone();
		}
		if shapes.len() > self.editor.scene.elements.len() * 2 + 64 {
			shapes.clear();
		}
		let shape = Rc::new(rough_shape(el));
		shapes.insert(el.id.clone(), (el.version_nonce, shape.clone()));
		shape
	}

	/// The picture stored under `file_id`, decoded once.
	pub fn picture(&self, file_id: &str, cx: &App) -> Option<Arc<RenderImage>> {
		if let Some(known) = self.pictures.borrow().get(file_id) {
			return known.clone();
		}
		let image = self
			.editor
			.scene
			.files()
			.and_then(|files| files.get(file_id))
			.and_then(|file| file.get("dataURL"))
			.and_then(|url| url.as_str())
			.and_then(decode_data_url)
			.and_then(|image| image.to_image_data(cx.svg_renderer()).ok());
		self.pictures
			.borrow_mut()
			.insert(file_id.to_string(), image.clone());
		image
	}

	/// Visible elements (status bar).
	pub fn element_count(&self) -> usize {
		self.editor
			.scene
			.elements
			.iter()
			.filter(|e| !e.is_deleted)
			.count()
	}
}

/// Every drawing open in the vault, by file: tabs and embeds share the models.
#[derive(Default)]
pub struct DrawingStore {
	models: Vec<WeakEntity<DrawingModel>>,
}

impl EventEmitter<DrawingEvent> for DrawingStore {}

impl DrawingStore {
	pub fn live(&self) -> Vec<Entity<DrawingModel>> {
		self.models.iter().filter_map(|m| m.upgrade()).collect()
	}

	/// The model of the drawing at `abs`, loading it when it is not open yet.
	pub fn open(
		&mut self,
		abs: &Path,
		cx: &mut Context<Self>,
	) -> Result<Entity<DrawingModel>, String> {
		self.models.retain(|m| m.upgrade().is_some());
		if let Some(model) = self.live().into_iter().find(|m| m.read(cx).doc.path == abs) {
			return Ok(model);
		}
		let model = DrawingModel::load(abs)?;
		let model = cx.new(|cx| {
			// The last view going away never loses an edit still waiting for its save.
			cx.on_release(|this: &mut DrawingModel, cx| {
				if let Err(e) = this.save(cx) {
					eprintln!("cannot save {}: {e}", this.doc.path.display());
				}
			})
			.detach();
			model
		});
		cx.subscribe(&model, |_, _, event: &DrawingEvent, cx| {
			cx.emit(event.clone())
		})
		.detach();
		self.models.push(model.downgrade());
		Ok(model)
	}

	/// A file or folder moved from `old` to `new` (absolute): open drawings follow.
	pub fn renamed(&mut self, old: &Path, new: &Path, cx: &mut App) {
		for model in self.live() {
			model.update(cx, |m, cx| {
				if let Ok(rest) = m.doc.path.strip_prefix(old) {
					let path: PathBuf = new.join(rest).components().collect();
					m.doc.set_path(path);
					cx.notify();
				}
			});
		}
	}
}
