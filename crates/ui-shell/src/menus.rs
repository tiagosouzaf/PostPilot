//! Barra de menus no estilo do SSMS, descrita como dados.
//!
//! Todos os textos dos menus ficam aqui, num só lugar, para facilitar a
//! tradução depois. Itens sem [`Action`] aparecem desabilitados: eles ganham
//! comportamento no marco 1.3, quando entra o registro de comandos.

use pgstudio_core::Theme;

/// O que um item de menu faz quando é clicado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Fecha o aplicativo.
    Exit,
    /// Troca o tema da interface.
    SetTheme(Theme),
    /// Abre a janela "About".
    About,
}

/// Uma entrada de menu.
#[derive(Clone, Copy, Debug)]
pub enum Entry {
    /// Item clicável. Sem ação, aparece desabilitado.
    Item {
        label: &'static str,
        shortcut: Option<&'static str>,
        action: Option<Action>,
    },
    /// Submenu com outras entradas.
    Submenu {
        label: &'static str,
        entries: &'static [Entry],
    },
    /// Linha separadora.
    Separator,
}

/// Um menu da barra, como "File".
#[derive(Clone, Copy, Debug)]
pub struct Menu {
    pub title: &'static str,
    pub entries: &'static [Entry],
}

const fn todo(label: &'static str, shortcut: Option<&'static str>) -> Entry {
    Entry::Item {
        label,
        shortcut,
        action: None,
    }
}

const fn item(label: &'static str, shortcut: Option<&'static str>, action: Action) -> Entry {
    Entry::Item {
        label,
        shortcut,
        action: Some(action),
    }
}

const fn sub(label: &'static str, entries: &'static [Entry]) -> Entry {
    Entry::Submenu { label, entries }
}

const SEP: Entry = Entry::Separator;

/// Os menus da barra principal, na ordem do SSMS.
pub const MENUS: &[Menu] = &[
    Menu {
        title: "File",
        entries: &[
            todo("Connect Object Explorer…", None),
            todo("Disconnect Object Explorer", None),
            SEP,
            sub(
                "New",
                &[
                    todo("Query with Current Connection", Some("Ctrl+N")),
                    todo("Database Engine Query", None),
                ],
            ),
            sub("Open", &[todo("File…", Some("Ctrl+O"))]),
            SEP,
            todo("Close", None),
            SEP,
            todo("Save", Some("Ctrl+S")),
            todo("Save As…", None),
            todo("Save All", Some("Ctrl+Shift+S")),
            SEP,
            item("Exit", Some("Alt+F4"), Action::Exit),
        ],
    },
    Menu {
        title: "Edit",
        entries: &[
            todo("Undo", Some("Ctrl+Z")),
            todo("Redo", Some("Ctrl+Y")),
            SEP,
            todo("Cut", Some("Ctrl+X")),
            todo("Copy", Some("Ctrl+C")),
            todo("Paste", Some("Ctrl+V")),
            todo("Delete", Some("Del")),
            todo("Select All", Some("Ctrl+A")),
            SEP,
            sub(
                "Find and Replace",
                &[
                    todo("Quick Find", Some("Ctrl+F")),
                    todo("Quick Replace", Some("Ctrl+H")),
                ],
            ),
            todo("Go To…", Some("Ctrl+G")),
            SEP,
            sub(
                "IntelliSense",
                &[
                    todo("List Members", Some("Ctrl+J")),
                    todo("Parameter Info", Some("Ctrl+Shift+Space")),
                    todo("Quick Info", Some("Ctrl+K, Ctrl+I")),
                    todo("Complete Word", Some("Alt+Right")),
                    SEP,
                    todo("Refresh Local Cache", Some("Ctrl+Shift+R")),
                ],
            ),
            sub(
                "Advanced",
                &[
                    todo("Comment Selection", Some("Ctrl+K, Ctrl+C")),
                    todo("Uncomment Selection", Some("Ctrl+K, Ctrl+U")),
                    SEP,
                    todo("Make Uppercase", Some("Ctrl+Shift+U")),
                    todo("Make Lowercase", Some("Ctrl+Shift+L")),
                ],
            ),
        ],
    },
    Menu {
        title: "View",
        entries: &[
            todo("Object Explorer", Some("F8")),
            todo("Object Explorer Details", Some("F7")),
            todo("Registered Servers", Some("Ctrl+Alt+G")),
            todo("Template Explorer", Some("Ctrl+Alt+T")),
            todo("Properties Window", Some("F4")),
            todo("Output", Some("Ctrl+Alt+O")),
            SEP,
            sub(
                "Color Theme",
                &[
                    item("Light", None, Action::SetTheme(Theme::Light)),
                    item("Dark", None, Action::SetTheme(Theme::Dark)),
                ],
            ),
            SEP,
            todo("Full Screen", Some("Shift+Alt+Enter")),
        ],
    },
    Menu {
        title: "Query",
        entries: &[
            sub(
                "Connection",
                &[
                    todo("Connect…", None),
                    todo("Disconnect", None),
                    todo("Change Connection…", None),
                ],
            ),
            SEP,
            todo("Execute", Some("F5")),
            todo("Parse", Some("Ctrl+F5")),
            todo("Cancel Executing Query", Some("Alt+Break")),
            SEP,
            todo("Display Estimated Execution Plan", Some("Ctrl+L")),
            todo("Include Actual Execution Plan", Some("Ctrl+M")),
            SEP,
            sub(
                "Results To",
                &[
                    todo("Results to Text", Some("Ctrl+T")),
                    todo("Results to Grid", Some("Ctrl+D")),
                    todo("Results to File", Some("Ctrl+Shift+F")),
                ],
            ),
            SEP,
            todo("Query Options…", None),
        ],
    },
    Menu {
        title: "Tools",
        entries: &[todo("Options…", None)],
    },
    Menu {
        title: "Window",
        entries: &[
            todo("New Horizontal Tab Group", None),
            todo("New Vertical Tab Group", None),
            SEP,
            todo("Auto Hide All", None),
            todo("Reset Window Layout", None),
            SEP,
            todo("Close All Documents", None),
        ],
    },
    Menu {
        title: "Help",
        entries: &[
            todo("View Help", Some("F1")),
            SEP,
            item("About PGStudio", None, Action::About),
        ],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn items(entries: &'static [Entry]) -> Vec<(&'static str, Option<Action>)> {
        let mut out = Vec::new();
        for entry in entries {
            match *entry {
                Entry::Item { label, action, .. } => out.push((label, action)),
                Entry::Submenu { entries, .. } => out.extend(items(entries)),
                Entry::Separator => {}
            }
        }
        out
    }

    #[test]
    fn menus_follow_ssms_order() {
        let titles: Vec<_> = MENUS.iter().map(|m| m.title).collect();
        assert_eq!(
            titles,
            ["File", "Edit", "View", "Query", "Tools", "Window", "Help"]
        );
    }

    #[test]
    fn every_menu_lists_items() {
        for menu in MENUS {
            assert!(!items(menu.entries).is_empty(), "{} está vazio", menu.title);
        }
    }

    #[test]
    fn every_theme_has_a_menu_item() {
        let all: Vec<_> = MENUS.iter().flat_map(|m| items(m.entries)).collect();
        for theme in Theme::ALL {
            assert!(
                all.contains(&(theme.label(), Some(Action::SetTheme(theme)))),
                "falta o tema {theme:?}"
            );
        }
    }
}
