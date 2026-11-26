//! Every command of the command palette, with its label in each language and its shortcut.

use gpui_kit::{Action, App};

use crate::editor::actions::*;
use crate::i18n::{I18n, Language};
use crate::workspace::*;

pub struct Command {
	en: &'static str,
	fr: &'static str,
	pub shortcut: &'static str,
	pub action: fn() -> Box<dyn Action>,
}

impl Command {
	pub fn label(&self, cx: &App) -> &'static str {
		match cx.global::<I18n>().language {
			Language::En => self.en,
			Language::Fr => self.fr,
		}
	}
}

macro_rules! command {
	($en:literal, $fr:literal, $shortcut:literal, $action:expr) => {
		Command {
			en: $en,
			fr: $fr,
			shortcut: $shortcut,
			action: || Box::new($action),
		}
	};
}

pub fn all() -> Vec<Command> {
	vec![
		command!("Go to file", "Aller au fichier", "Ctrl+P", QuickOpen),
		command!(
			"Search in all notes",
			"Rechercher dans toutes les notes",
			"Ctrl+Shift+F",
			SearchVault
		),
		command!("Find in note", "Rechercher dans la note", "Ctrl+F", Find),
		command!(
			"Replace in note",
			"Remplacer dans la note",
			"Ctrl+H",
			Replace
		),
		command!("New note", "Nouvelle note", "Ctrl+N", NewNote),
		command!("Open vault", "Ouvrir un vault", "", OpenVault),
		command!("Settings", "Param\u{e8}tres", "Ctrl+,", OpenSettings),
		command!(
			"Toggle source mode",
			"Basculer le mode source",
			"Ctrl+E",
			ToggleLivePreview
		),
		command!(
			"Toggle file tree",
			"Afficher ou masquer les fichiers",
			"Ctrl+Shift+E",
			ToggleLeftPanel
		),
		command!(
			"Toggle outline",
			"Afficher ou masquer le plan",
			"Ctrl+Alt+B",
			ToggleRightPanel
		),
		command!("Close tab", "Fermer l'onglet", "Ctrl+W", CloseTab),
		command!(
			"Reopen closed tab",
			"Rouvrir l'onglet ferm\u{e9}",
			"Ctrl+Shift+T",
			ReopenClosedTab
		),
		command!("Next tab", "Onglet suivant", "Ctrl+Tab", NextTab),
		command!(
			"Previous tab",
			"Onglet pr\u{e9}c\u{e9}dent",
			"Ctrl+Shift+Tab",
			PrevTab
		),
		command!("Go back", "Retour", "Alt+Left", GoBack),
		command!("Go forward", "Suivant", "Alt+Right", GoForward),
		command!("Bold", "Gras", "Ctrl+B", Bold),
		command!("Italic", "Italique", "Ctrl+I", Italic),
		command!("Insert link", "Ins\u{e9}rer un lien", "Ctrl+K", InsertLink),
		command!(
			"Move line up",
			"D\u{e9}placer la ligne vers le haut",
			"Alt+Up",
			MoveLinesUp
		),
		command!(
			"Move line down",
			"D\u{e9}placer la ligne vers le bas",
			"Alt+Down",
			MoveLinesDown
		),
		command!(
			"Copy line up",
			"Dupliquer la ligne vers le haut",
			"Shift+Alt+Up",
			CopyLinesUp
		),
		command!(
			"Copy line down",
			"Dupliquer la ligne vers le bas",
			"Shift+Alt+Down",
			CopyLinesDown
		),
		command!(
			"Delete line",
			"Supprimer la ligne",
			"Ctrl+Shift+K",
			DeleteLines
		),
		command!(
			"Add next occurrence",
			"Ajouter l'occurrence suivante",
			"Ctrl+D",
			AddNextOccurrence
		),
		command!(
			"Select all occurrences",
			"S\u{e9}lectionner toutes les occurrences",
			"Ctrl+Shift+L",
			SelectAllOccurrences
		),
		command!("Undo", "Annuler", "Ctrl+Z", Undo),
		command!("Redo", "R\u{e9}tablir", "Ctrl+Y", Redo),
	]
}
