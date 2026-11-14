//! Barra de menus e toolbar no estilo do SSMS, descritas como dados.
//!
//! Cada item aponta para um [`CommandId`]; o texto vem do registro de
//! comandos e o atalho exibido vem do keymap em uso.

use pgstudio_core::CommandId::{self, *};

/// Uma entrada de menu.
#[derive(Clone, Copy, Debug)]
pub enum Entry {
    /// Item que dispara um comando.
    Item(CommandId),
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

use Entry::{Item as I, Separator as SEP};

const fn sub(label: &'static str, entries: &'static [Entry]) -> Entry {
    Entry::Submenu { label, entries }
}

/// Os menus da barra principal, na ordem do SSMS.
pub const MENUS: &[Menu] = &[
    Menu {
        title: "File",
        entries: &[
            I(ConnectObjectExplorer),
            I(DisconnectObjectExplorer),
            SEP,
            sub("New", &[I(NewQuery), I(NewDatabaseEngineQuery)]),
            sub("Open", &[I(OpenFile)]),
            SEP,
            I(CloseDocument),
            SEP,
            I(Save),
            I(SaveAs),
            I(SaveAll),
            SEP,
            I(Exit),
        ],
    },
    Menu {
        title: "Edit",
        entries: &[
            I(Undo),
            I(Redo),
            SEP,
            I(Cut),
            I(Copy),
            I(Paste),
            I(Delete),
            I(SelectAll),
            SEP,
            sub("Find and Replace", &[I(QuickFind), I(QuickReplace)]),
            I(GoTo),
            SEP,
            sub(
                "IntelliSense",
                &[
                    I(ListMembers),
                    I(ParameterInfo),
                    I(QuickInfo),
                    I(CompleteWord),
                    SEP,
                    I(RefreshLocalCache),
                ],
            ),
            sub(
                "Advanced",
                &[
                    I(CommentSelection),
                    I(UncommentSelection),
                    SEP,
                    I(MakeUppercase),
                    I(MakeLowercase),
                ],
            ),
        ],
    },
    Menu {
        title: "View",
        entries: &[
            I(ObjectExplorer),
            I(ObjectExplorerDetails),
            I(RegisteredServers),
            I(TemplateExplorer),
            I(PropertiesWindow),
            I(Output),
            SEP,
            I(ActivityMonitor),
            SEP,
            sub("Color Theme", &[I(ThemeLight), I(ThemeDark)]),
            SEP,
            I(FullScreen),
        ],
    },
    Menu {
        title: "Query",
        entries: &[
            sub(
                "Connection",
                &[I(Connect), I(Disconnect), I(ChangeConnection)],
            ),
            SEP,
            I(DesignQuery),
            SEP,
            I(Execute),
            I(Parse),
            I(CancelQuery),
            SEP,
            I(EstimatedPlan),
            I(ActualPlan),
            SEP,
            sub(
                "Results To",
                &[I(ResultsToText), I(ResultsToGrid), I(ResultsToFile)],
            ),
            SEP,
            I(QueryOptions),
        ],
    },
    Menu {
        title: "Tools",
        entries: &[I(SchemaCompare), SEP, I(Options)],
    },
    Menu {
        title: "Window",
        entries: &[
            I(NewHorizontalTabGroup),
            I(NewVerticalTabGroup),
            SEP,
            I(ToggleResultsPane),
            I(AutoHide),
            I(AutoHideAll),
            I(ResetWindowLayout),
            SEP,
            I(CloseAllDocuments),
        ],
    },
    Menu {
        title: "Help",
        entries: &[I(ViewHelp), SEP, I(About)],
    },
];

/// Um botão da toolbar padrão.
#[derive(Clone, Copy, Debug)]
pub enum Tool {
    Button {
        icon: &'static str,
        text: &'static str,
        command: CommandId,
    },
    /// Seletor de banco de dados.
    Database,
    Separator,
}

/// Toolbar padrão do SSMS.
pub const TOOLBAR: &[Tool] = {
    use egui_phosphor::regular as icon;
    const fn b(icon: &'static str, text: &'static str, command: CommandId) -> Tool {
        Tool::Button {
            icon,
            text,
            command,
        }
    }
    &[
        b(icon::FILE_PLUS, "New Query", NewQuery),
        Tool::Separator,
        b(icon::FOLDER_OPEN, "", OpenFile),
        b(icon::FLOPPY_DISK, "", Save),
        b(icon::FLOPPY_DISK_BACK, "", SaveAll),
        Tool::Separator,
        Tool::Database,
        b(icon::PLAY, "Execute", Execute),
        b(icon::STOP, "", CancelQuery),
        b(icon::CHECK, "", Parse),
        Tool::Separator,
        b(icon::FLOW_ARROW, "", EstimatedPlan),
        b(icon::TREE_STRUCTURE, "", ActualPlan),
        Tool::Separator,
        b(icon::PULSE, "", ActivityMonitor),
    ]
};

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(entries: &'static [Entry]) -> Vec<CommandId> {
        let mut out = Vec::new();
        for entry in entries {
            match *entry {
                Entry::Item(cmd) => out.push(cmd),
                Entry::Submenu { entries, .. } => out.extend(commands(entries)),
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
            assert!(
                !commands(menu.entries).is_empty(),
                "{} está vazio",
                menu.title
            );
        }
    }

    #[test]
    fn every_command_with_a_menu_appears_once() {
        let all: Vec<_> = MENUS.iter().flat_map(|m| commands(m.entries)).collect();
        for &cmd in CommandId::ALL {
            let n = all.iter().filter(|&&c| c == cmd).count();
            // Object Information (Alt+F1) só existe como atalho, como no SSMS.
            let expected = usize::from(cmd != ObjectInfo);
            assert_eq!(n, expected, "{cmd:?}");
        }
    }
}
