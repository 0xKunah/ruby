//! UI languages: English and French, system language detected, user override in settings.

use std::fmt::Display;

use gpui_kit::{App, Global};
use ruby_core::fs_ops::FsError;
use ruby_core::workspace_state::{AppConfig, load_json, save_json};

use crate::paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
	En,
	Fr,
}

impl Language {
	pub const ALL: [Language; 2] = [Language::En, Language::Fr];

	pub fn code(self) -> &'static str {
		match self {
			Language::En => "en",
			Language::Fr => "fr",
		}
	}

	pub fn from_code(code: &str) -> Option<Language> {
		Language::ALL.into_iter().find(|l| l.code() == code)
	}

	/// `fr-FR`, `fr_CA`, `FR` -> French; unknown languages -> `None`.
	pub fn from_locale(locale: &str) -> Option<Language> {
		let primary = locale.split(['-', '_']).next()?.to_ascii_lowercase();
		Language::from_code(&primary)
	}

	/// The language's own name, shown in the language picker.
	pub fn native_name(self) -> &'static str {
		match self {
			Language::En => "English",
			Language::Fr => "Français",
		}
	}

	pub fn strings(self) -> &'static Strings {
		match self {
			Language::En => &EN,
			Language::Fr => &FR,
		}
	}
}

/// The user's choice wins, then the system language, then English.
pub fn resolve(setting: Option<Language>, system_locale: Option<&str>) -> Language {
	setting
		.or_else(|| system_locale.and_then(Language::from_locale))
		.unwrap_or(Language::En)
}

pub fn system_locale() -> Option<String> {
	sys_locale::get_locale()
}

/// Replaces each `{}` in `template` with the next argument.
pub fn fmt(template: &str, args: &[&dyn Display]) -> String {
	let mut out = String::with_capacity(template.len() + 16);
	let mut args = args.iter();
	let mut rest = template;
	while let Some(i) = rest.find("{}") {
		out.push_str(&rest[..i]);
		if let Some(arg) = args.next() {
			out.push_str(&arg.to_string());
		}
		rest = &rest[i + 2..];
	}
	out.push_str(rest);
	out
}

/// Current language (global): `setting` is the user's choice, `None` = follow the system.
pub struct I18n {
	pub setting: Option<Language>,
	pub language: Language,
}

impl Global for I18n {}

pub fn init(cx: &mut App) {
	let config: AppConfig = load_json(&paths::config_file());
	let setting = config.language.as_deref().and_then(Language::from_code);
	let language = resolve(setting, system_locale().as_deref());
	cx.set_global(I18n { setting, language });
}

pub fn tr(cx: &App) -> &'static Strings {
	cx.global::<I18n>().language.strings()
}

/// Changes the UI language, saves the choice and repaints every window.
pub fn set_language(setting: Option<Language>, cx: &mut App) {
	let language = resolve(setting, system_locale().as_deref());
	cx.set_global(I18n { setting, language });
	let mut config: AppConfig = load_json(&paths::config_file());
	config.language = setting.map(|l| l.code().to_string());
	if let Err(e) = save_json(&paths::config_file(), &config) {
		eprintln!("failed to save config: {e}");
	}
	cx.refresh_windows();
}

/// Name tag of a conflict copy: `conflict 2026-11-09 1430` / `conflit 2026-11-09 1430`.
pub fn conflict_tag(cx: &App) -> String {
	format!(
		"{} {}",
		tr(cx).conflict_tag,
		chrono::Local::now().format("%Y-%m-%d %H%M")
	)
}

/// Every user-visible string. Placeholders `{}` are filled with [`fmt`].
pub struct Strings {
	pub welcome_tagline: &'static str,
	pub open_vault: &'static str,
	pub open_vault_prompt: &'static str,
	pub new_note: &'static str,
	pub new_drawing: &'static str,
	pub new_folder: &'static str,
	pub rename: &'static str,
	pub delete: &'static str,
	pub reveal: &'static str,
	pub copy_path: &'static str,
	pub search: &'static str,
	pub settings: &'static str,
	pub delete_title: &'static str,
	pub delete_description: &'static str,
	pub cancel: &'static str,
	pub untitled: &'static str,
	pub new_folder_name: &'static str,
	pub err_invalid_name: &'static str,
	pub err_already_exists: &'static str,
	pub err_into_itself: &'static str,
	pub err_disk: &'static str,
	pub draw_stroke: &'static str,
	pub draw_background: &'static str,
	pub draw_width: &'static str,
	pub draw_thin: &'static str,
	pub draw_medium: &'static str,
	pub draw_bold: &'static str,
	pub draw_style: &'static str,
	pub draw_sharp: &'static str,
	pub draw_hand: &'static str,
	pub draw_sketch: &'static str,
	pub draw_opacity: &'static str,
	pub elements: &'static str,
	pub embed_expand: &'static str,
	pub embed_edit: &'static str,
	pub drawing_not_found: &'static str,
	pub no_note_open: &'static str,
	pub cannot_open: &'static str,
	pub cannot_read: &'static str,
	pub save_failed: &'static str,
	pub conflict_tag: &'static str,
	pub conflict_saved: &'static str,
	pub read_only_banner: &'static str,
	pub mode_source: &'static str,
	pub mode_live: &'static str,
	pub quick_open_placeholder: &'static str,
	pub palette_placeholder: &'static str,
	pub search_notes_placeholder: &'static str,
	pub no_results: &'static str,
	pub results_count: &'static str,
	pub results_truncated: &'static str,
	pub find_placeholder: &'static str,
	pub replace_placeholder: &'static str,
	pub find_previous: &'static str,
	pub find_next: &'static str,
	pub replace_one: &'static str,
	pub replace_all: &'static str,
	pub links_updated: &'static str,
	pub setting_font_size: &'static str,
	pub setting_max_width: &'static str,
	pub setting_default_mode: &'static str,
	pub setting_attachments: &'static str,
	pub copy: &'static str,
	pub copied: &'static str,
	pub image_not_found: &'static str,
	pub words: &'static str,
	pub position: &'static str,
	pub outline_title: &'static str,
	pub no_headings: &'static str,
	pub language: &'static str,
	pub system_language: &'static str,
	pub unsaved_title: &'static str,
	pub unsaved_description: &'static str,
	pub close_anyway: &'static str,
}

impl Strings {
	pub fn fs_error(&self, e: &FsError) -> String {
		match e {
			FsError::InvalidName => self.err_invalid_name.to_string(),
			FsError::AlreadyExists => self.err_already_exists.to_string(),
			FsError::IntoItself => self.err_into_itself.to_string(),
			FsError::Io(io) => fmt(self.err_disk, &[io]),
		}
	}
}

pub static EN: Strings = Strings {
	welcome_tagline: "Choose the folder that contains your notes.",
	open_vault: "Open a vault",
	open_vault_prompt: "Open this folder as a vault",
	new_note: "New note",
	new_drawing: "New drawing",
	new_folder: "New folder",
	rename: "Rename",
	delete: "Delete",
	reveal: "Show in Explorer",
	copy_path: "Copy path",
	search: "Search",
	settings: "Settings",
	delete_title: "Delete \"{}\"?",
	delete_description: "The item will be moved to the Recycle Bin.",
	cancel: "Cancel",
	untitled: "Untitled",
	new_folder_name: "New folder",
	err_invalid_name: "Invalid name",
	err_already_exists: "A file with this name already exists",
	err_into_itself: "Cannot move a folder into itself",
	err_disk: "Disk error: {}",
	draw_stroke: "Stroke",
	draw_background: "Background",
	draw_width: "Width",
	draw_thin: "thin",
	draw_medium: "medium",
	draw_bold: "bold",
	draw_style: "Style",
	draw_sharp: "sharp",
	draw_hand: "hand",
	draw_sketch: "sketch",
	draw_opacity: "Opacity",
	elements: "{} elements",
	embed_expand: "Expand",
	embed_edit: "Edit",
	drawing_not_found: "Drawing not found: {}",
	no_note_open: "No note open",
	cannot_open: "Cannot open {}: {}",
	cannot_read: "Cannot read {}: {}",
	save_failed: "Save failed: {}",
	conflict_tag: "conflict",
	conflict_saved: "Conflict: the version on disk was saved as {}",
	read_only_banner: "Not a UTF-8 file, opened read-only",
	mode_source: "Source",
	mode_live: "Live preview",
	quick_open_placeholder: "Go to file (Shift+Enter creates the note)",
	palette_placeholder: "Run a command",
	search_notes_placeholder: "Search in all notes",
	no_results: "No results",
	results_count: "{} results in {} notes",
	results_truncated: "First {} results",
	find_placeholder: "Find",
	replace_placeholder: "Replace with",
	find_previous: "Previous",
	find_next: "Next",
	replace_one: "Replace",
	replace_all: "Replace all",
	links_updated: "{} links updated",
	setting_font_size: "Text size",
	setting_max_width: "Text width",
	setting_default_mode: "Notes open in",
	setting_attachments: "Folder for pasted images",
	copy: "Copy",
	copied: "Copied",
	image_not_found: "Image not found: {}",
	words: "{} words",
	position: "Ln {}, Col {}",
	outline_title: "OUTLINE",
	no_headings: "No headings",
	language: "Language",
	system_language: "System ({})",
	unsaved_title: "Some notes could not be saved",
	unsaved_description: "{} still has unsaved changes. Closing now loses them.",
	close_anyway: "Close without saving",
};

pub static FR: Strings = Strings {
	welcome_tagline: "Choisis le dossier qui contient tes notes.",
	open_vault: "Ouvrir un vault",
	open_vault_prompt: "Ouvrir ce dossier comme vault",
	new_note: "Nouvelle note",
	new_drawing: "Nouveau dessin",
	new_folder: "Nouveau dossier",
	rename: "Renommer",
	delete: "Supprimer",
	reveal: "Afficher dans l'Explorateur",
	copy_path: "Copier le chemin",
	search: "Rechercher",
	settings: "Paramètres",
	delete_title: "Supprimer \"{}\" ?",
	delete_description: "L'élément sera déplacé dans la corbeille.",
	cancel: "Annuler",
	untitled: "Sans titre",
	new_folder_name: "Nouveau dossier",
	err_invalid_name: "Nom invalide",
	err_already_exists: "Un fichier porte déjà ce nom",
	err_into_itself: "Impossible de déplacer un dossier dans lui-même",
	err_disk: "Erreur disque : {}",
	draw_stroke: "Trait",
	draw_background: "Fond",
	draw_width: "Épaisseur",
	draw_thin: "fin",
	draw_medium: "moyen",
	draw_bold: "gros",
	draw_style: "Style",
	draw_sharp: "net",
	draw_hand: "main",
	draw_sketch: "brouillon",
	draw_opacity: "Opacité",
	elements: "{} éléments",
	embed_expand: "Agrandir",
	embed_edit: "Éditer",
	drawing_not_found: "Dessin introuvable : {}",
	no_note_open: "Aucune note ouverte",
	cannot_open: "Impossible d'ouvrir {} : {}",
	cannot_read: "Lecture impossible de {} : {}",
	save_failed: "Échec de l'enregistrement : {}",
	conflict_tag: "conflit",
	conflict_saved: "Conflit : la version du disque a été enregistrée dans {}",
	read_only_banner: "Fichier non UTF-8, ouvert en lecture seule",
	mode_source: "Source",
	mode_live: "Aperçu",
	quick_open_placeholder: "Aller au fichier (Maj+Entrée crée la note)",
	palette_placeholder: "Lancer une commande",
	search_notes_placeholder: "Rechercher dans les notes",
	no_results: "Aucun résultat",
	results_count: "{} résultats dans {} notes",
	results_truncated: "{} premiers résultats",
	find_placeholder: "Rechercher",
	replace_placeholder: "Remplacer par",
	find_previous: "Précédent",
	find_next: "Suivant",
	replace_one: "Remplacer",
	replace_all: "Tout remplacer",
	links_updated: "{} liens mis à jour",
	setting_font_size: "Taille du texte",
	setting_max_width: "Largeur du texte",
	setting_default_mode: "Ouvrir les notes en",
	setting_attachments: "Dossier des images collées",
	copy: "Copier",
	copied: "Copié",
	image_not_found: "Image introuvable : {}",
	words: "{} mots",
	position: "Ln {}, Col {}",
	outline_title: "PLAN",
	no_headings: "Aucun titre",
	language: "Langue",
	system_language: "Système ({})",
	unsaved_title: "Des notes n'ont pas pu être enregistrées",
	unsaved_description: "{} a encore des modifications non enregistrées. Fermer maintenant les perd.",
	close_anyway: "Fermer sans enregistrer",
};

#[cfg(test)]
mod tests {
	use super::{Language, fmt, resolve};

	#[test]
	fn locale_mapping() {
		assert_eq!(Language::from_locale("fr-FR"), Some(Language::Fr));
		assert_eq!(Language::from_locale("fr_CA"), Some(Language::Fr));
		assert_eq!(Language::from_locale("FR"), Some(Language::Fr));
		assert_eq!(Language::from_locale("en-US"), Some(Language::En));
		assert_eq!(Language::from_locale("de-DE"), None);
		assert_eq!(Language::from_locale(""), None);
	}

	#[test]
	fn setting_beats_system_and_english_is_the_fallback() {
		assert_eq!(resolve(Some(Language::En), Some("fr-FR")), Language::En);
		assert_eq!(resolve(None, Some("fr-FR")), Language::Fr);
		assert_eq!(resolve(None, Some("de-DE")), Language::En);
		assert_eq!(resolve(None, None), Language::En);
	}

	#[test]
	fn codes_round_trip() {
		for lang in Language::ALL {
			assert_eq!(Language::from_code(lang.code()), Some(lang));
		}
		assert_eq!(Language::from_code("xx"), None);
	}

	#[test]
	fn fmt_fills_placeholders_in_order() {
		assert_eq!(fmt("Ln {}, Col {}", &[&3, &14]), "Ln 3, Col 14");
		assert_eq!(fmt("Supprimer \"{}\" ?", &[&"Été"]), "Supprimer \"Été\" ?");
	}

	#[test]
	fn languages_differ() {
		assert_ne!(
			Language::En.strings().new_note,
			Language::Fr.strings().new_note
		);
	}
}
