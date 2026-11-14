//! Registro de comandos.
//!
//! Todo gesto do usuário (menu, toolbar, atalho, menu de contexto) vira um
//! [`CommandId`]. Assim existe uma só implementação por ação e o keymap pode
//! ser configurado por arquivo usando o nome estável de cada comando.

macro_rules! commands {
    ($($variant:ident => $name:literal, $label:literal;)*) => {
        /// Identificador de um comando do PGStudio.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum CommandId {
            $(
                #[doc = $label]
                $variant,
            )*
        }

        impl CommandId {
            /// Todos os comandos, na ordem de declaração.
            pub const ALL: &[CommandId] = &[$(CommandId::$variant),*];

            /// Nome estável, usado no arquivo de keymap (ex.: `Query.Execute`).
            pub fn name(self) -> &'static str {
                match self {
                    $(CommandId::$variant => $name,)*
                }
            }

            /// Texto exibido em menus e dicas, no idioma da interface.
            pub fn label(self) -> &'static str {
                pgstudio_i18n::tr(self.english_label())
            }

            /// Texto em inglês, a chave da tradução.
            pub fn english_label(self) -> &'static str {
                match self {
                    $(CommandId::$variant => $label,)*
                }
            }
        }
    };
}

commands! {
    // File
    ConnectObjectExplorer => "File.ConnectObjectExplorer", "Connect Object Explorer…";
    DisconnectObjectExplorer => "File.DisconnectObjectExplorer", "Disconnect Object Explorer";
    NewQuery => "File.NewQuery", "Query with Current Connection";
    NewDatabaseEngineQuery => "File.NewDatabaseEngineQuery", "Database Engine Query";
    OpenFile => "File.OpenFile", "File…";
    NewWorkspace => "File.NewWorkspace", "Workspace…";
    OpenWorkspace => "File.OpenWorkspace", "Workspace…";
    CloneWorkspace => "File.CloneWorkspace", "Clone Workspace…";
    CloseWorkspace => "File.CloseWorkspace", "Close Workspace";
    CloseDocument => "File.Close", "Close";
    Save => "File.Save", "Save";
    SaveAs => "File.SaveAs", "Save As…";
    SaveAll => "File.SaveAll", "Save All";
    Exit => "File.Exit", "Exit";
    // Edit
    Undo => "Edit.Undo", "Undo";
    Redo => "Edit.Redo", "Redo";
    Cut => "Edit.Cut", "Cut";
    Copy => "Edit.Copy", "Copy";
    Paste => "Edit.Paste", "Paste";
    Delete => "Edit.Delete", "Delete";
    SelectAll => "Edit.SelectAll", "Select All";
    QuickFind => "Edit.QuickFind", "Quick Find";
    QuickReplace => "Edit.QuickReplace", "Quick Replace";
    GoTo => "Edit.GoTo", "Go To…";
    ListMembers => "Edit.ListMembers", "List Members";
    ParameterInfo => "Edit.ParameterInfo", "Parameter Info";
    QuickInfo => "Edit.QuickInfo", "Quick Info";
    CompleteWord => "Edit.CompleteWord", "Complete Word";
    RefreshLocalCache => "Edit.RefreshLocalCache", "Refresh Local Cache";
    CommentSelection => "Edit.CommentSelection", "Comment Selection";
    UncommentSelection => "Edit.UncommentSelection", "Uncomment Selection";
    MakeUppercase => "Edit.MakeUppercase", "Make Uppercase";
    MakeLowercase => "Edit.MakeLowercase", "Make Lowercase";
    FormatDocument => "Edit.FormatDocument", "Format Document";
    // View
    ObjectExplorer => "View.ObjectExplorer", "Object Explorer";
    ObjectExplorerDetails => "View.ObjectExplorerDetails", "Object Explorer Details";
    RegisteredServers => "View.RegisteredServers", "Registered Servers";
    WorkspaceExplorer => "View.WorkspaceExplorer", "Workspace Explorer";
    ClaudePanel => "View.Claude", "Claude";
    TemplateExplorer => "View.TemplateExplorer", "Template Explorer";
    PropertiesWindow => "View.PropertiesWindow", "Properties Window";
    Output => "View.Output", "Output";
    ActivityMonitor => "View.ActivityMonitor", "Activity Monitor";
    ThemeLight => "View.ThemeLight", "Light";
    ThemeDark => "View.ThemeDark", "Dark";
    FullScreen => "View.FullScreen", "Full Screen";
    // Query
    Connect => "Query.Connect", "Connect…";
    Disconnect => "Query.Disconnect", "Disconnect";
    ChangeConnection => "Query.ChangeConnection", "Change Connection…";
    ChangeDatabase => "Query.ChangeDatabase", "Change Database";
    Execute => "Query.Execute", "Execute";
    Parse => "Query.Parse", "Parse";
    DesignQuery => "Query.DesignQuery", "Design Query in Editor…";
    CancelQuery => "Query.Cancel", "Cancel Executing Query";
    EstimatedPlan => "Query.EstimatedPlan", "Display Estimated Execution Plan";
    ActualPlan => "Query.ActualPlan", "Include Actual Execution Plan";
    ResultsToText => "Query.ResultsToText", "Results to Text";
    ResultsToGrid => "Query.ResultsToGrid", "Results to Grid";
    ResultsToFile => "Query.ResultsToFile", "Results to File";
    ObjectInfo => "Query.ObjectInfo", "Object Information";
    QueryOptions => "Query.Options", "Query Options…";
    // Tools
    SchemaCompare => "Tools.SchemaCompare", "Schema Compare…";
    ClaudeCodeAccess => "Tools.ClaudeCodeAccess", "Allow Claude Code (MCP)";
    Options => "Tools.Options", "Options…";
    // Window
    NewHorizontalTabGroup => "Window.NewHorizontalTabGroup", "New Horizontal Tab Group";
    NewVerticalTabGroup => "Window.NewVerticalTabGroup", "New Vertical Tab Group";
    ToggleResultsPane => "Window.ToggleResultsPane", "Show Results Pane";
    AutoHide => "Window.AutoHide", "Auto Hide";
    AutoHideAll => "Window.AutoHideAll", "Auto Hide All";
    ResetWindowLayout => "Window.ResetLayout", "Reset Window Layout";
    CloseAllDocuments => "Window.CloseAllDocuments", "Close All Documents";
    // Help
    ViewHelp => "Help.ViewHelp", "View Help";
    JsonGuide => "Help.JsonGuide", "JSON Query Guide";
    CheckForUpdates => "Help.CheckForUpdates", "Check for Updates…";
    About => "Help.About", "About PostPilot";
}

impl CommandId {
    /// Procura um comando pelo nome estável, sem diferenciar maiúsculas.
    pub fn from_name(name: &str) -> Option<CommandId> {
        CommandId::ALL
            .iter()
            .copied()
            .find(|c| c.name().eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn names_are_unique_and_round_trip() {
        let mut seen = HashSet::new();
        for &cmd in CommandId::ALL {
            assert!(seen.insert(cmd.name()), "nome repetido: {}", cmd.name());
            assert_eq!(CommandId::from_name(cmd.name()), Some(cmd));
        }
    }

    #[test]
    fn every_label_has_a_portuguese_translation() {
        use pgstudio_i18n::{Language, has_translation};
        for &cmd in CommandId::ALL {
            assert!(
                has_translation(Language::PortugueseBrazil, None, cmd.english_label()),
                "sem tradução em locales/pt-BR.po: {:?}",
                cmd.english_label()
            );
        }
    }

    #[test]
    fn from_name_ignores_case() {
        assert_eq!(
            CommandId::from_name("query.execute"),
            Some(CommandId::Execute)
        );
        assert_eq!(CommandId::from_name("Query.Nope"), None);
    }
}
