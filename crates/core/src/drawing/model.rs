//! The `.excalidraw` file format: a JSON scene of elements. Every field this app does not
//! use is kept as read, so a file edited here still opens unchanged on excalidraw.com.

use serde_json::{Map, Value, json};

use super::rng::Rng;

/// An arrow end attached to a shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
	pub element_id: String,
	pub focus: f64,
	pub gap: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Element {
	pub id: String,
	/// `rectangle`, `diamond`, `ellipse`, `arrow`, `line`, `freedraw`, `text`, `image`, or a
	/// type this app does not draw.
	pub kind: String,
	pub x: f64,
	pub y: f64,
	pub width: f64,
	pub height: f64,
	/// Radians, around the center.
	pub angle: f64,
	pub stroke_color: String,
	pub background_color: String,
	pub fill_style: String,
	pub stroke_width: f64,
	pub stroke_style: String,
	pub roughness: f64,
	/// 0 to 100.
	pub opacity: f64,
	pub seed: i64,
	pub version: i64,
	pub version_nonce: i64,
	pub is_deleted: bool,
	pub group_ids: Vec<String>,
	/// `(id, type)` of the arrows and text attached to this element.
	pub bound_elements: Vec<(String, String)>,
	/// Lines, arrows, freedraw: points relative to `(x, y)`.
	pub points: Option<Vec<[f64; 2]>>,
	pub start_binding: Option<Binding>,
	pub end_binding: Option<Binding>,
	pub start_arrowhead: Option<String>,
	pub end_arrowhead: Option<String>,
	pub text: Option<String>,
	pub font_size: f64,
	/// 1 hand-drawn, 2 normal, 3 code.
	pub font_family: i64,
	pub text_align: String,
	pub vertical_align: String,
	pub container_id: Option<String>,
	/// Images: key of the picture in the scene's `files`.
	pub file_id: Option<String>,
	/// The element as read, with every field this struct does not model.
	raw: Map<String, Value>,
}

fn num(raw: &Map<String, Value>, key: &str, default: f64) -> f64 {
	raw.get(key).and_then(Value::as_f64).unwrap_or(default)
}

fn int(raw: &Map<String, Value>, key: &str, default: i64) -> i64 {
	raw.get(key)
		.and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
		.unwrap_or(default)
}

fn string(raw: &Map<String, Value>, key: &str, default: &str) -> String {
	raw.get(key)
		.and_then(Value::as_str)
		.unwrap_or(default)
		.to_string()
}

fn opt_string(raw: &Map<String, Value>, key: &str) -> Option<String> {
	raw.get(key).and_then(Value::as_str).map(str::to_string)
}

fn binding(raw: &Map<String, Value>, key: &str) -> Option<Binding> {
	let b = raw.get(key)?.as_object()?;
	Some(Binding {
		element_id: b.get("elementId")?.as_str()?.to_string(),
		focus: num(b, "focus", 0.),
		gap: num(b, "gap", 0.),
	})
}

/// A number as JSON: whole numbers are written without a fraction.
fn number(v: f64) -> Value {
	if v.fract() == 0. && v.abs() < 9e15 {
		json!(v as i64)
	} else {
		json!(v)
	}
}

/// Writes a number, keeping the JSON value as read when it did not change.
fn put_num(raw: &mut Map<String, Value>, key: &str, v: f64) {
	if raw.get(key).and_then(Value::as_f64) != Some(v) {
		raw.insert(key.to_string(), number(v));
	}
}

fn put_str(raw: &mut Map<String, Value>, key: &str, v: &str) {
	if raw.get(key).and_then(Value::as_str) != Some(v) {
		raw.insert(key.to_string(), json!(v));
	}
}

/// Writes an optional string: `null` when it had a value (or the key exists), otherwise the
/// key stays absent.
fn put_opt_str(raw: &mut Map<String, Value>, key: &str, v: Option<&str>) {
	match v {
		Some(v) => put_str(raw, key, v),
		None => {
			if raw.contains_key(key) {
				raw.insert(key.to_string(), Value::Null);
			}
		}
	}
}

fn put_binding(raw: &mut Map<String, Value>, key: &str, v: &Option<Binding>) {
	match v {
		Some(b) => {
			// Unknown binding fields are kept while it points to the same element.
			let mut object = raw
				.get(key)
				.and_then(Value::as_object)
				.filter(|o| {
					o.get("elementId").and_then(Value::as_str) == Some(b.element_id.as_str())
				})
				.cloned()
				.unwrap_or_default();
			put_str(&mut object, "elementId", &b.element_id);
			put_num(&mut object, "focus", b.focus);
			put_num(&mut object, "gap", b.gap);
			raw.insert(key.to_string(), Value::Object(object));
		}
		None => {
			if raw.contains_key(key) {
				raw.insert(key.to_string(), Value::Null);
			}
		}
	}
}

impl Element {
	/// Reads an element; missing fields take Excalidraw's defaults.
	pub fn from_raw(raw: Map<String, Value>) -> Element {
		let points = raw.get("points").and_then(Value::as_array).map(|points| {
			points
				.iter()
				.filter_map(|p| Some([p.get(0)?.as_f64()?, p.get(1)?.as_f64()?]))
				.collect()
		});
		let strings = |key: &str| -> Vec<String> {
			raw.get(key)
				.and_then(Value::as_array)
				.map(|a| {
					a.iter()
						.filter_map(Value::as_str)
						.map(str::to_string)
						.collect()
				})
				.unwrap_or_default()
		};
		let bound_elements = raw
			.get("boundElements")
			.and_then(Value::as_array)
			.map(|a| {
				a.iter()
					.filter_map(|b| {
						Some((
							b.get("id")?.as_str()?.to_string(),
							b.get("type")
								.and_then(Value::as_str)
								.unwrap_or("arrow")
								.to_string(),
						))
					})
					.collect()
			})
			.unwrap_or_default();
		Element {
			id: string(&raw, "id", ""),
			kind: string(&raw, "type", ""),
			x: num(&raw, "x", 0.),
			y: num(&raw, "y", 0.),
			width: num(&raw, "width", 0.),
			height: num(&raw, "height", 0.),
			angle: num(&raw, "angle", 0.),
			stroke_color: string(&raw, "strokeColor", "#1e1e1e"),
			background_color: string(&raw, "backgroundColor", "transparent"),
			fill_style: string(&raw, "fillStyle", "solid"),
			stroke_width: num(&raw, "strokeWidth", 2.),
			stroke_style: string(&raw, "strokeStyle", "solid"),
			roughness: num(&raw, "roughness", 1.),
			opacity: num(&raw, "opacity", 100.),
			seed: int(&raw, "seed", 1),
			version: int(&raw, "version", 1),
			version_nonce: int(&raw, "versionNonce", 0),
			is_deleted: raw
				.get("isDeleted")
				.and_then(Value::as_bool)
				.unwrap_or(false),
			group_ids: strings("groupIds"),
			bound_elements,
			points,
			start_binding: binding(&raw, "startBinding"),
			end_binding: binding(&raw, "endBinding"),
			start_arrowhead: opt_string(&raw, "startArrowhead"),
			end_arrowhead: opt_string(&raw, "endArrowhead"),
			text: opt_string(&raw, "text"),
			font_size: num(&raw, "fontSize", 20.),
			font_family: int(&raw, "fontFamily", 1),
			text_align: string(&raw, "textAlign", "left"),
			vertical_align: string(&raw, "verticalAlign", "top"),
			container_id: opt_string(&raw, "containerId"),
			file_id: opt_string(&raw, "fileId"),
			raw,
		}
	}

	/// A new element of `kind` at `(x, y)` with Excalidraw's defaults, a fresh id and seed.
	pub fn new(kind: &str, x: f64, y: f64, rng: &mut Rng) -> Element {
		let mut raw = Map::new();
		raw.insert("id".into(), json!(rng.next_id()));
		raw.insert("type".into(), json!(kind));
		for (key, value) in [
			("x", number(x)),
			("y", number(y)),
			("width", json!(0)),
			("height", json!(0)),
			("angle", json!(0)),
			("strokeColor", json!("#1e1e1e")),
			("backgroundColor", json!("transparent")),
			("fillStyle", json!("solid")),
			("strokeWidth", json!(2)),
			("strokeStyle", json!("solid")),
			("roughness", json!(1)),
			("opacity", json!(100)),
			("groupIds", json!([])),
			("frameId", Value::Null),
			("roundness", Value::Null),
			("seed", json!(rng.next_seed())),
			("version", json!(1)),
			("versionNonce", json!(rng.next_seed())),
			("isDeleted", json!(false)),
			("boundElements", Value::Null),
			("updated", json!(1)),
			("link", Value::Null),
			("locked", json!(false)),
		] {
			raw.insert(key.into(), value);
		}
		let extra: &[(&str, Value)] = match kind {
			"arrow" | "line" => &[
				("points", json!([[0, 0], [0, 0]])),
				("lastCommittedPoint", Value::Null),
				("startBinding", Value::Null),
				("endBinding", Value::Null),
				("startArrowhead", Value::Null),
				(
					"endArrowhead",
					if kind == "arrow" {
						json!("arrow")
					} else {
						Value::Null
					},
				),
			],
			"freedraw" => &[
				("points", json!([])),
				("pressures", json!([])),
				("simulatePressure", json!(true)),
				("lastCommittedPoint", Value::Null),
			],
			"text" => &[
				("text", json!("")),
				("fontSize", json!(20)),
				("fontFamily", json!(1)),
				("textAlign", json!("left")),
				("verticalAlign", json!("top")),
				("containerId", Value::Null),
				("originalText", json!("")),
				("autoResize", json!(true)),
				("lineHeight", json!(1.25)),
			],
			"image" => &[
				("fileId", Value::Null),
				("status", json!("saved")),
				("scale", json!([1, 1])),
			],
			_ => &[],
		};
		for (key, value) in extra {
			raw.insert((*key).into(), value.clone());
		}
		Element::from_raw(raw)
	}

	/// Rounded corners (rectangles) or a curve through the points (lines).
	pub fn rounded(&self) -> bool {
		self.raw.get("roundness").is_some_and(|v| !v.is_null())
	}

	/// Marks the element as changed, the way Excalidraw tracks edits.
	pub fn touch(&mut self, rng: &mut Rng) {
		self.version += 1;
		self.version_nonce = rng.next_seed();
	}

	/// The element as JSON: the fields as read, with this struct's values written over.
	pub fn to_value(&self) -> Value {
		let mut raw = self.raw.clone();
		put_str(&mut raw, "id", &self.id);
		put_str(&mut raw, "type", &self.kind);
		for (key, v) in [
			("x", self.x),
			("y", self.y),
			("width", self.width),
			("height", self.height),
			("angle", self.angle),
			("strokeWidth", self.stroke_width),
			("roughness", self.roughness),
			("opacity", self.opacity),
			("seed", self.seed as f64),
			("version", self.version as f64),
			("versionNonce", self.version_nonce as f64),
		] {
			// A field that was absent and still has its default is not added.
			if raw.contains_key(key) || Element::default_num(key) != Some(v) {
				put_num(&mut raw, key, v);
			}
		}
		for (key, v, default) in [
			("strokeColor", &self.stroke_color, "#1e1e1e"),
			("backgroundColor", &self.background_color, "transparent"),
			("fillStyle", &self.fill_style, "solid"),
			("strokeStyle", &self.stroke_style, "solid"),
		] {
			if raw.contains_key(key) || v != default {
				put_str(&mut raw, key, v);
			}
		}
		if raw.contains_key("isDeleted") || self.is_deleted {
			raw.insert("isDeleted".into(), json!(self.is_deleted));
		}
		let groups = |raw: &Map<String, Value>| -> Vec<String> {
			raw.get("groupIds")
				.and_then(Value::as_array)
				.map(|a| {
					a.iter()
						.filter_map(Value::as_str)
						.map(str::to_string)
						.collect()
				})
				.unwrap_or_default()
		};
		if groups(&raw) != self.group_ids {
			raw.insert("groupIds".into(), json!(self.group_ids));
		}
		let read = Element::from_raw(raw.clone());
		if read.bound_elements != self.bound_elements {
			let bound: Vec<Value> = self
				.bound_elements
				.iter()
				.map(|(id, kind)| json!({"id": id, "type": kind}))
				.collect();
			raw.insert("boundElements".into(), Value::Array(bound));
		}
		if read.points != self.points
			&& let Some(points) = &self.points
		{
			let points: Vec<Value> = points
				.iter()
				.map(|p| Value::Array(vec![number(p[0]), number(p[1])]))
				.collect();
			raw.insert("points".into(), Value::Array(points));
		}
		if read.start_binding != self.start_binding {
			put_binding(&mut raw, "startBinding", &self.start_binding);
		}
		if read.end_binding != self.end_binding {
			put_binding(&mut raw, "endBinding", &self.end_binding);
		}
		put_opt_str(&mut raw, "startArrowhead", self.start_arrowhead.as_deref());
		put_opt_str(&mut raw, "endArrowhead", self.end_arrowhead.as_deref());
		put_opt_str(&mut raw, "containerId", self.container_id.as_deref());
		put_opt_str(&mut raw, "fileId", self.file_id.as_deref());
		if let Some(text) = &self.text {
			put_str(&mut raw, "text", text);
			if raw.contains_key("originalText") && read.text.as_deref() != Some(text) {
				raw.insert("originalText".into(), json!(text));
			}
		}
		if self.text.is_some() || raw.contains_key("fontSize") {
			put_num(&mut raw, "fontSize", self.font_size);
			put_num(&mut raw, "fontFamily", self.font_family as f64);
			put_str(&mut raw, "textAlign", &self.text_align);
			put_str(&mut raw, "verticalAlign", &self.vertical_align);
		}
		Value::Object(raw)
	}

	fn default_num(key: &str) -> Option<f64> {
		Some(match key {
			"x" | "y" | "width" | "height" | "angle" | "versionNonce" => 0.,
			"strokeWidth" => 2.,
			"roughness" | "seed" | "version" => 1.,
			"opacity" => 100.,
			_ => return None,
		})
	}
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
	pub elements: Vec<Element>,
	/// Everything else in the file (`appState`, `files`, ...), in file order, with a
	/// placeholder where `elements` goes.
	raw: Map<String, Value>,
}

impl Scene {
	pub fn empty() -> Scene {
		Scene::parse(crate::fs_ops::EMPTY_DRAWING).expect("the empty drawing is valid")
	}

	/// Parses a `.excalidraw` file. An empty file is an empty drawing.
	pub fn parse(json: &str) -> Result<Scene, String> {
		if json.trim().is_empty() {
			return Ok(Scene::empty());
		}
		let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
		let Value::Object(mut raw) = value else {
			return Err("not a drawing: the file is not a JSON object".to_string());
		};
		let elements = match raw.get_mut("elements").map(Value::take) {
			None | Some(Value::Null) => Vec::new(),
			Some(Value::Array(items)) => items
				.into_iter()
				.map(|item| match item {
					Value::Object(raw) => Ok(Element::from_raw(raw)),
					_ => Err("not a drawing: an element is not an object".to_string()),
				})
				.collect::<Result<_, _>>()?,
			Some(_) => return Err("not a drawing: `elements` is not a list".to_string()),
		};
		Ok(Scene { elements, raw })
	}

	/// The file content: indented JSON like Excalidraw writes, with a final new line.
	pub fn to_json(&self) -> String {
		let mut raw = self.raw.clone();
		let elements: Vec<Value> = self.elements.iter().map(Element::to_value).collect();
		// Inserting over the placeholder keeps the key where it was in the file.
		raw.insert("elements".into(), Value::Array(elements));
		let mut out = serde_json::to_string_pretty(&Value::Object(raw)).unwrap_or_default();
		out.push('\n');
		out
	}

	/// The scene's `files` map (pictures by file id).
	pub fn files(&self) -> Option<&Map<String, Value>> {
		self.raw.get("files").and_then(Value::as_object)
	}

	/// Adds a picture (`data:` URL) under `id`.
	pub fn add_file(&mut self, id: &str, mime: &str, data_url: &str) {
		let files = self
			.raw
			.entry("files")
			.or_insert_with(|| Value::Object(Map::new()));
		if !files.is_object() {
			*files = Value::Object(Map::new());
		}
		if let Some(files) = files.as_object_mut() {
			files.insert(
				id.to_string(),
				json!({"mimeType": mime, "id": id, "dataURL": data_url, "created": 1}),
			);
		}
	}
}

#[cfg(test)]
mod tests {
	use serde_json::Value;

	use super::{Binding, Element, Scene};
	use crate::drawing::rng::Rng;

	/// What excalidraw.com writes, shortened, plus fields and a type this app does not know.
	const SAMPLE: &str = r##"{
  "type": "excalidraw",
  "version": 2,
  "source": "https://excalidraw.com",
  "elements": [
    {"id": "r1", "type": "rectangle", "x": 10, "y": 20.5, "width": 100, "height": 50, "angle": 0,
     "strokeColor": "#e03131", "backgroundColor": "#ffc9c9", "fillStyle": "hachure", "strokeWidth": 2,
     "strokeStyle": "dashed", "roughness": 1, "opacity": 80, "groupIds": ["g1"], "frameId": null,
     "roundness": {"type": 3}, "seed": 1234, "version": 7, "versionNonce": 99, "isDeleted": false,
     "boundElements": [{"id": "a1", "type": "arrow"}], "updated": 1700000000000, "link": null,
     "locked": false, "futureField": {"keep": [1, 2]}},
    {"id": "a1", "type": "arrow", "x": 110, "y": 45, "width": 80, "height": 0, "angle": 0,
     "strokeColor": "#1e1e1e", "backgroundColor": "transparent", "fillStyle": "solid", "strokeWidth": 2,
     "strokeStyle": "solid", "roughness": 1, "opacity": 100, "groupIds": [], "seed": 5, "version": 3,
     "versionNonce": 4, "isDeleted": false, "boundElements": null, "points": [[0, 0], [80, 0]],
     "startBinding": {"elementId": "r1", "focus": 0.1, "gap": 4, "fixedPoint": null},
     "endBinding": null, "startArrowhead": null, "endArrowhead": "arrow"},
    {"id": "t1", "type": "text", "x": 0, "y": 0, "width": 40, "height": 25, "text": "Café",
     "fontSize": 20, "fontFamily": 1, "textAlign": "center", "verticalAlign": "middle",
     "containerId": "r1", "originalText": "Café", "lineHeight": 1.25},
    {"id": "f1", "type": "frame", "x": 1, "y": 2, "name": "later"}
  ],
  "appState": {"gridSize": null, "viewBackgroundColor": "#ffffff"},
  "files": {"abc": {"mimeType": "image/png", "id": "abc", "dataURL": "data:image/png;base64,AAAA"}}
}"##;

	/// JSON equality where `10` and `10.0` are the same number.
	fn same(a: &Value, b: &Value) -> bool {
		match (a, b) {
			(Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
			(Value::Array(x), Value::Array(y)) => {
				x.len() == y.len() && x.iter().zip(y).all(|(x, y)| same(x, y))
			}
			(Value::Object(x), Value::Object(y)) => {
				x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
			}
			_ => a == b,
		}
	}

	#[test]
	fn fields_are_read() {
		let scene = Scene::parse(SAMPLE).unwrap();
		assert_eq!(scene.elements.len(), 4);
		let r = &scene.elements[0];
		assert_eq!((r.id.as_str(), r.kind.as_str()), ("r1", "rectangle"));
		assert_eq!((r.x, r.y, r.width, r.height), (10., 20.5, 100., 50.));
		assert_eq!(r.stroke_color, "#e03131");
		assert_eq!(r.fill_style, "hachure");
		assert_eq!(r.stroke_style, "dashed");
		assert_eq!((r.opacity, r.seed, r.version), (80., 1234, 7));
		assert_eq!(r.group_ids, ["g1"]);
		assert_eq!(r.bound_elements, [("a1".to_string(), "arrow".to_string())]);
		let a = &scene.elements[1];
		assert_eq!(a.points.as_deref(), Some(&[[0., 0.], [80., 0.]][..]));
		assert_eq!(
			a.start_binding,
			Some(Binding {
				element_id: "r1".into(),
				focus: 0.1,
				gap: 4.
			})
		);
		assert_eq!(a.end_binding, None);
		assert_eq!(a.end_arrowhead.as_deref(), Some("arrow"));
		let t = &scene.elements[2];
		assert_eq!(t.text.as_deref(), Some("Caf\u{e9}"));
		assert_eq!((t.font_size, t.font_family), (20., 1));
		assert_eq!(t.container_id.as_deref(), Some("r1"));
		// Missing fields get Excalidraw's defaults.
		assert_eq!((t.opacity, t.stroke_width, t.roughness), (100., 2., 1.));
		assert_eq!(t.stroke_color, "#1e1e1e");
		assert!(!t.is_deleted);
	}

	#[test]
	fn round_trip_keeps_everything() {
		let scene = Scene::parse(SAMPLE).unwrap();
		let out = scene.to_json();
		let (before, after): (Value, Value) = (
			serde_json::from_str(SAMPLE).unwrap(),
			serde_json::from_str(&out).unwrap(),
		);
		assert!(same(&before, &after), "{out}");
		assert!(out.ends_with("}\n"));
		// Key order is kept: the file diffs cleanly.
		assert!(out.find("\"type\"").unwrap() < out.find("\"elements\"").unwrap());
		assert!(out.find("\"elements\"").unwrap() < out.find("\"appState\"").unwrap());
	}

	#[test]
	fn edits_are_written_and_unknown_fields_survive() {
		let mut scene = Scene::parse(SAMPLE).unwrap();
		let r = &mut scene.elements[0];
		r.x = 42.5;
		r.bound_elements.clear();
		r.touch(&mut Rng::new(1));
		assert_eq!(r.version, 8);
		let a = &mut scene.elements[1];
		a.start_binding = None;
		a.end_binding = Some(Binding {
			element_id: "r1".into(),
			focus: 0.,
			gap: 2.,
		});
		a.points = Some(vec![[0., 0.], [10., 5.]]);
		scene.elements[2].text = Some("x".into());
		let out: Value = serde_json::from_str(&scene.to_json()).unwrap();
		let els = out["elements"].as_array().unwrap();
		assert_eq!(els[0]["x"], 42.5);
		assert_eq!(els[0]["version"], 8);
		assert_ne!(els[0]["versionNonce"], 99);
		assert_eq!(els[0]["futureField"]["keep"][1], 2);
		assert_eq!(els[0]["boundElements"], Value::Array(vec![]));
		assert!(els[1]["startBinding"].is_null());
		assert_eq!(els[1]["endBinding"]["elementId"], "r1");
		assert_eq!(els[1]["points"][1][1], 5.0);
		assert_eq!(els[2]["text"], "x");
		assert_eq!(els[3]["name"], "later");
		// Untouched integers stay integers.
		assert!(scene.to_json().contains("\"width\": 100,"));
	}

	#[test]
	fn new_elements_have_excalidraw_defaults() {
		let mut rng = Rng::new(7);
		let rect = Element::new("rectangle", 1., 2., &mut rng);
		let arrow = Element::new("arrow", 0., 0., &mut rng);
		let text = Element::new("text", 0., 0., &mut rng);
		assert_eq!(rect.id.len(), 20);
		assert_ne!(rect.id, arrow.id);
		assert_ne!(rect.seed, arrow.seed);
		let v = rect.to_value();
		for key in [
			"id",
			"type",
			"x",
			"y",
			"width",
			"height",
			"angle",
			"strokeColor",
			"backgroundColor",
			"fillStyle",
			"strokeWidth",
			"strokeStyle",
			"roughness",
			"opacity",
			"groupIds",
			"frameId",
			"roundness",
			"seed",
			"version",
			"versionNonce",
			"isDeleted",
			"boundElements",
			"updated",
			"link",
			"locked",
		] {
			assert!(v.get(key).is_some(), "rectangle misses {key}");
		}
		assert_eq!(v["strokeColor"], "#1e1e1e");
		assert_eq!(v["backgroundColor"], "transparent");
		let v = arrow.to_value();
		assert_eq!(v["endArrowhead"], "arrow");
		assert!(v["startBinding"].is_null() && v.get("points").is_some());
		let v = text.to_value();
		assert_eq!(v["fontFamily"], 1);
		assert_eq!(v["text"], "");
		assert!(v.get("lineHeight").is_some() && v.get("originalText").is_some());
	}

	#[test]
	fn empty_scene_and_errors() {
		let scene = Scene::empty();
		let v: Value = serde_json::from_str(&scene.to_json()).unwrap();
		assert_eq!(v["type"], "excalidraw");
		assert_eq!(v["version"], 2);
		assert!(v["elements"].as_array().unwrap().is_empty());
		assert!(v["files"].is_object());
		assert!(Scene::parse("not json").is_err());
		assert!(Scene::parse("[1]").is_err());
		assert!(Scene::parse(r#"{"elements": 3}"#).is_err());
		// An empty file is an empty drawing.
		assert!(Scene::parse("").unwrap().elements.is_empty());
		assert_eq!(
			Scene::parse(crate::fs_ops::EMPTY_DRAWING)
				.unwrap()
				.elements
				.len(),
			0
		);
	}
}
