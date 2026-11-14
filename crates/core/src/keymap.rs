//! Atalhos de teclado: acordes, keymap padrão do SSMS e arquivo de keymap.
//!
//! Um atalho tem um ou dois acordes, como `F5` ou `Ctrl+K, Ctrl+C`. O
//! [`ChordMatcher`] recebe as teclas uma a uma e diz quando um comando foi
//! completado. Esta crate não lê arquivos: quem chama passa o texto.

use std::fmt;

use crate::command::CommandId;

/// Tecla sem modificadores. Cobre o que os atalhos do SSMS usam.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Letter(char),
    Digit(u8),
    F(u8),
    Enter,
    Escape,
    Tab,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Right,
    Up,
    Down,
    Break,
}

impl Key {
    fn parse(s: &str) -> Option<Key> {
        let upper = s.to_ascii_uppercase();
        let mut chars = upper.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if c.is_ascii_uppercase() {
                return Some(Key::Letter(c));
            }
            if let Some(d) = c.to_digit(10) {
                return Some(Key::Digit(d as u8));
            }
        }
        if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u8>().ok()) {
            return (1..=24).contains(&n).then_some(Key::F(n));
        }
        Some(match upper.as_str() {
            "ENTER" | "RETURN" => Key::Enter,
            "ESC" | "ESCAPE" => Key::Escape,
            "TAB" => Key::Tab,
            "SPACE" => Key::Space,
            "BACKSPACE" | "BKSP" => Key::Backspace,
            "DEL" | "DELETE" => Key::Delete,
            "INS" | "INSERT" => Key::Insert,
            "HOME" => Key::Home,
            "END" => Key::End,
            "PGUP" | "PAGEUP" => Key::PageUp,
            "PGDN" | "PAGEDOWN" => Key::PageDown,
            "LEFT" => Key::Left,
            "RIGHT" => Key::Right,
            "UP" => Key::Up,
            "DOWN" => Key::Down,
            "BREAK" | "PAUSE" => Key::Break,
            _ => return None,
        })
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Key::Letter(c) => write!(f, "{c}"),
            Key::Digit(d) => write!(f, "{d}"),
            Key::F(n) => write!(f, "F{n}"),
            Key::Enter => f.write_str("Enter"),
            Key::Escape => f.write_str("Esc"),
            Key::Tab => f.write_str("Tab"),
            Key::Space => f.write_str("Space"),
            Key::Backspace => f.write_str("Backspace"),
            Key::Delete => f.write_str("Del"),
            Key::Insert => f.write_str("Ins"),
            Key::Home => f.write_str("Home"),
            Key::End => f.write_str("End"),
            Key::PageUp => f.write_str("PgUp"),
            Key::PageDown => f.write_str("PgDn"),
            Key::Left => f.write_str("Left"),
            Key::Right => f.write_str("Right"),
            Key::Up => f.write_str("Up"),
            Key::Down => f.write_str("Down"),
            Key::Break => f.write_str("Break"),
        }
    }
}

/// Modificadores de um acorde.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Uma tecla com modificadores, como `Ctrl+Shift+S`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Chord {
    pub mods: Modifiers,
    pub key: Key,
}

impl Chord {
    /// Acorde sem modificadores.
    pub fn plain(key: Key) -> Self {
        Chord {
            mods: Modifiers::default(),
            key,
        }
    }

    /// Acorde com Ctrl.
    pub fn ctrl(key: Key) -> Self {
        Chord {
            mods: Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
            key,
        }
    }

    /// Lê um acorde como `Ctrl+Shift+S`.
    pub fn parse(s: &str) -> Result<Chord, KeymapError> {
        let s = s.trim();
        let err = || KeymapError::InvalidShortcut(s.to_string());
        // O último "+" separa a tecla, o que permite tecla "+" no futuro.
        let mut parts: Vec<&str> = s.split('+').map(str::trim).collect();
        let key = parts.pop().filter(|k| !k.is_empty()).ok_or_else(err)?;
        let mut mods = Modifiers::default();
        for m in parts {
            let flag = match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => &mut mods.ctrl,
                "shift" => &mut mods.shift,
                "alt" => &mut mods.alt,
                _ => return Err(err()),
            };
            if *flag {
                return Err(err());
            }
            *flag = true;
        }
        Ok(Chord {
            mods,
            key: Key::parse(key).ok_or_else(err)?,
        })
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.mods.shift {
            f.write_str("Shift+")?;
        }
        if self.mods.alt {
            f.write_str("Alt+")?;
        }
        write!(f, "{}", self.key)
    }
}

/// Um atalho: um acorde, ou dois em sequência.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    first: Chord,
    second: Option<Chord>,
}

impl Shortcut {
    /// Atalho de um acorde.
    pub fn single(chord: Chord) -> Self {
        Shortcut {
            first: chord,
            second: None,
        }
    }

    /// Atalho de dois acordes, como `Ctrl+K, Ctrl+C`.
    pub fn sequence(first: Chord, second: Chord) -> Self {
        Shortcut {
            first,
            second: Some(second),
        }
    }

    /// Lê `F5`, `Ctrl+Shift+S` ou `Ctrl+K, Ctrl+C`.
    pub fn parse(s: &str) -> Result<Shortcut, KeymapError> {
        let chords: Vec<&str> = s.split(',').collect();
        match chords.as_slice() {
            [one] => Ok(Shortcut::single(Chord::parse(one)?)),
            [a, b] => Ok(Shortcut::sequence(Chord::parse(a)?, Chord::parse(b)?)),
            _ => Err(KeymapError::InvalidShortcut(s.trim().to_string())),
        }
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.first)?;
        if let Some(second) = self.second {
            write!(f, ", {second}")?;
        }
        Ok(())
    }
}

/// Erro ao ler um atalho ou um arquivo de keymap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeymapError {
    InvalidShortcut(String),
    UnknownCommand(String),
    /// Linha sem `=`; guarda o número da linha (a partir de 1).
    MissingEquals(usize),
}

impl fmt::Display for KeymapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeymapError::InvalidShortcut(s) => write!(f, "atalho inválido: {s:?}"),
            KeymapError::UnknownCommand(s) => write!(f, "comando desconhecido: {s:?}"),
            KeymapError::MissingEquals(n) => write!(f, "linha {n}: falta o \"=\""),
        }
    }
}

impl std::error::Error for KeymapError {}

/// Associação entre atalhos e comandos.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Keymap {
    bindings: Vec<(Shortcut, CommandId)>,
}

/// Atalhos padrão, iguais aos do SSMS (os prioritários vêm da especificação).
const DEFAULTS: &[(CommandId, &str)] = &[
    (CommandId::NewQuery, "Ctrl+N"),
    (CommandId::OpenFile, "Ctrl+O"),
    (CommandId::Save, "Ctrl+S"),
    (CommandId::SaveAs, "Ctrl+Shift+S"),
    (CommandId::CloseDocument, "Ctrl+F4"),
    (CommandId::Exit, "Alt+F4"),
    (CommandId::Undo, "Ctrl+Z"),
    (CommandId::Redo, "Ctrl+Y"),
    (CommandId::Cut, "Ctrl+X"),
    (CommandId::Copy, "Ctrl+C"),
    (CommandId::Paste, "Ctrl+V"),
    (CommandId::Delete, "Del"),
    (CommandId::SelectAll, "Ctrl+A"),
    (CommandId::QuickFind, "Ctrl+F"),
    (CommandId::QuickReplace, "Ctrl+H"),
    (CommandId::GoTo, "Ctrl+G"),
    (CommandId::ListMembers, "Ctrl+J"),
    (CommandId::ParameterInfo, "Ctrl+Shift+Space"),
    (CommandId::QuickInfo, "Ctrl+K, Ctrl+I"),
    (CommandId::CompleteWord, "Alt+Right"),
    (CommandId::RefreshLocalCache, "Ctrl+Shift+R"),
    (CommandId::CommentSelection, "Ctrl+K, Ctrl+C"),
    (CommandId::UncommentSelection, "Ctrl+K, Ctrl+U"),
    (CommandId::MakeUppercase, "Ctrl+Shift+U"),
    (CommandId::MakeLowercase, "Ctrl+Shift+L"),
    (CommandId::ObjectExplorer, "F8"),
    (CommandId::ObjectExplorerDetails, "F7"),
    (CommandId::RegisteredServers, "Ctrl+Alt+G"),
    (CommandId::TemplateExplorer, "Ctrl+Alt+T"),
    (CommandId::PropertiesWindow, "F4"),
    (CommandId::Output, "Ctrl+Alt+O"),
    (CommandId::FullScreen, "Shift+Alt+Enter"),
    (CommandId::Execute, "F5"),
    (CommandId::Execute, "Ctrl+E"),
    (CommandId::Parse, "Ctrl+F5"),
    (CommandId::DesignQuery, "Ctrl+Shift+Q"),
    (CommandId::CancelQuery, "Alt+Break"),
    (CommandId::EstimatedPlan, "Ctrl+L"),
    (CommandId::ActualPlan, "Ctrl+M"),
    (CommandId::ResultsToText, "Ctrl+T"),
    (CommandId::ResultsToGrid, "Ctrl+D"),
    (CommandId::ResultsToFile, "Ctrl+Shift+F"),
    (CommandId::ObjectInfo, "Alt+F1"),
    (CommandId::ToggleResultsPane, "Ctrl+R"),
    (CommandId::ViewHelp, "F1"),
];

impl Keymap {
    /// Keymap padrão do SSMS.
    pub fn ssms() -> Keymap {
        let bindings = DEFAULTS
            .iter()
            .map(|&(cmd, s)| {
                let shortcut = Shortcut::parse(s).expect("atalho padrão inválido");
                (shortcut, cmd)
            })
            .collect();
        Keymap { bindings }
    }

    /// Aplica um arquivo de keymap sobre este keymap.
    ///
    /// Formato: uma linha por atalho, `Nome.Do.Comando = Atalho`, com `#` para
    /// comentários. A primeira linha de um comando substitui os atalhos
    /// padrão dele; linhas seguintes do mesmo comando acrescentam atalhos;
    /// `Comando =` sem atalho remove todos. Linhas com erro são ignoradas e
    /// devolvidas na lista de erros.
    pub fn apply_overrides(&mut self, text: &str) -> Vec<KeymapError> {
        let mut errors = Vec::new();
        let mut replaced = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, shortcut)) = line.split_once('=') else {
                errors.push(KeymapError::MissingEquals(n + 1));
                continue;
            };
            let Some(cmd) = CommandId::from_name(name.trim()) else {
                errors.push(KeymapError::UnknownCommand(name.trim().to_string()));
                continue;
            };
            let shortcut = shortcut.trim();
            let parsed = if shortcut.is_empty() {
                None
            } else {
                match Shortcut::parse(shortcut) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                }
            };
            if !replaced.contains(&cmd) {
                replaced.push(cmd);
                self.bindings.retain(|(_, c)| *c != cmd);
            }
            if let Some(s) = parsed {
                // O atalho passa a ser só deste comando.
                self.bindings.retain(|(b, _)| *b != s);
                self.bindings.push((s, cmd));
            }
        }
        errors
    }

    /// Primeiro atalho do comando, para exibir em menus.
    pub fn shortcut_for(&self, cmd: CommandId) -> Option<&Shortcut> {
        self.bindings
            .iter()
            .find(|(_, c)| *c == cmd)
            .map(|(s, _)| s)
    }

    /// Comando de um atalho completo.
    pub fn command_for(&self, shortcut: &Shortcut) -> Option<CommandId> {
        self.bindings
            .iter()
            .find(|(s, _)| s == shortcut)
            .map(|(_, c)| *c)
    }

    fn is_prefix(&self, chord: Chord) -> bool {
        self.bindings
            .iter()
            .any(|(s, _)| s.second.is_some() && s.first == chord)
    }
}

/// Resultado de uma tecla dada ao [`ChordMatcher`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Match {
    /// O atalho foi completado.
    Command(CommandId),
    /// Primeiro acorde de uma sequência; esperando o segundo.
    Pending(Chord),
    /// Sequência iniciada que não corresponde a nenhum comando.
    Unbound(Chord, Chord),
    /// A tecla não é atalho; segue para o widget com foco.
    None,
}

/// Acompanha sequências de acordes, como `Ctrl+K, Ctrl+C`.
#[derive(Clone, Debug, Default)]
pub struct ChordMatcher {
    pending: Option<Chord>,
}

impl ChordMatcher {
    /// Primeiro acorde de uma sequência em andamento.
    pub fn pending(&self) -> Option<Chord> {
        self.pending
    }

    /// Cancela a sequência em andamento.
    pub fn reset(&mut self) {
        self.pending = None;
    }

    /// Processa uma tecla pressionada.
    pub fn feed(&mut self, keymap: &Keymap, chord: Chord) -> Match {
        if let Some(first) = self.pending.take() {
            return match keymap.command_for(&Shortcut::sequence(first, chord)) {
                Some(cmd) => Match::Command(cmd),
                None => Match::Unbound(first, chord),
            };
        }
        if keymap.is_prefix(chord) {
            self.pending = Some(chord);
            return Match::Pending(chord);
        }
        match keymap.command_for(&Shortcut::single(chord)) {
            Some(cmd) => Match::Command(cmd),
            None => Match::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    #[test]
    fn parses_chords() {
        assert_eq!(chord("F5"), Chord::plain(Key::F(5)));
        assert_eq!(chord("ctrl+e"), Chord::ctrl(Key::Letter('E')));
        let c = chord("Ctrl+Shift+S");
        assert!(c.mods.ctrl && c.mods.shift && !c.mods.alt);
        assert_eq!(c.key, Key::Letter('S'));
        assert_eq!(chord("Alt+Break").key, Key::Break);
        assert_eq!(
            chord(" Shift + Alt + Enter ").to_string(),
            "Shift+Alt+Enter"
        );
    }

    #[test]
    fn rejects_invalid_chords() {
        for bad in ["", "Ctrl+", "Hyper+A", "Ctrl+Ctrl+A", "F25", "Ctrl+Foo"] {
            assert!(Chord::parse(bad).is_err(), "{bad:?} deveria falhar");
        }
    }

    #[test]
    fn parses_sequences() {
        let s = Shortcut::parse("Ctrl+K, Ctrl+C").unwrap();
        assert_eq!(
            s,
            Shortcut::sequence(Chord::ctrl(Key::Letter('K')), Chord::ctrl(Key::Letter('C')))
        );
        assert_eq!(s.to_string(), "Ctrl+K, Ctrl+C");
        assert!(Shortcut::parse("Ctrl+K, Ctrl+C, Ctrl+D").is_err());
    }

    #[test]
    fn display_round_trips_for_every_default() {
        let keymap = Keymap::ssms();
        for (s, _) in &keymap.bindings {
            assert_eq!(&Shortcut::parse(&s.to_string()).unwrap(), s);
        }
    }

    #[test]
    fn priority_shortcuts_trigger_their_commands() {
        let keymap = Keymap::ssms();
        let cases = [
            ("F5", CommandId::Execute),
            ("Ctrl+E", CommandId::Execute),
            ("Ctrl+N", CommandId::NewQuery),
            ("Ctrl+S", CommandId::Save),
            ("Ctrl+Shift+S", CommandId::SaveAs),
            ("Ctrl+R", CommandId::ToggleResultsPane),
            ("Ctrl+L", CommandId::EstimatedPlan),
            ("Ctrl+M", CommandId::ActualPlan),
            ("Alt+F1", CommandId::ObjectInfo),
            ("Ctrl+Shift+Q", CommandId::DesignQuery),
            ("Ctrl+K, Ctrl+C", CommandId::CommentSelection),
            ("Ctrl+K, Ctrl+U", CommandId::UncommentSelection),
        ];
        for (text, cmd) in cases {
            let mut matcher = ChordMatcher::default();
            let mut result = Match::None;
            for part in text.split(',') {
                result = matcher.feed(&keymap, chord(part));
            }
            assert_eq!(result, Match::Command(cmd), "{text}");
        }
    }

    #[test]
    fn sequence_waits_for_second_chord() {
        let keymap = Keymap::ssms();
        let mut m = ChordMatcher::default();
        let ctrl_k = chord("Ctrl+K");
        assert_eq!(m.feed(&keymap, ctrl_k), Match::Pending(ctrl_k));
        assert_eq!(m.pending(), Some(ctrl_k));
        // Ctrl+C sozinho é Copy; depois de Ctrl+K é Comment Selection.
        assert_eq!(
            m.feed(&keymap, chord("Ctrl+C")),
            Match::Command(CommandId::CommentSelection)
        );
        assert_eq!(
            m.feed(&keymap, chord("Ctrl+C")),
            Match::Command(CommandId::Copy)
        );
    }

    #[test]
    fn unknown_sequence_resets() {
        let keymap = Keymap::ssms();
        let mut m = ChordMatcher::default();
        m.feed(&keymap, chord("Ctrl+K"));
        assert_eq!(
            m.feed(&keymap, chord("Ctrl+Q")),
            Match::Unbound(chord("Ctrl+K"), chord("Ctrl+Q"))
        );
        assert_eq!(m.pending(), None);
        assert_eq!(m.feed(&keymap, chord("Ctrl+Q")), Match::None);
    }

    #[test]
    fn overrides_replace_add_and_remove() {
        let mut keymap = Keymap::ssms();
        let errors = keymap.apply_overrides(
            "# comentário\n\
             Query.Execute = Ctrl+Enter   # troca F5 e Ctrl+E\n\
             Query.Execute = F9\n\
             File.Save =\n\
             Edit.MakeUppercase = Ctrl+E\n",
        );
        assert!(errors.is_empty(), "{errors:?}");
        let cmd = |s: &str| keymap.command_for(&Shortcut::parse(s).unwrap());
        assert_eq!(cmd("Ctrl+Enter"), Some(CommandId::Execute));
        assert_eq!(cmd("F9"), Some(CommandId::Execute));
        assert_eq!(cmd("F5"), None);
        assert_eq!(cmd("Ctrl+S"), None);
        assert_eq!(cmd("Ctrl+E"), Some(CommandId::MakeUppercase));
        assert_eq!(keymap.shortcut_for(CommandId::Save), None);
        assert_eq!(
            keymap.shortcut_for(CommandId::Execute).unwrap().to_string(),
            "Ctrl+Enter"
        );
    }

    #[test]
    fn overrides_report_errors_and_keep_going() {
        let mut keymap = Keymap::ssms();
        let errors = keymap.apply_overrides(
            "sem igual\nNada.Disso = F2\nQuery.Execute = Ctrl+Nope\nQuery.Parse = F6\n",
        );
        assert_eq!(
            errors,
            vec![
                KeymapError::MissingEquals(1),
                KeymapError::UnknownCommand("Nada.Disso".into()),
                KeymapError::InvalidShortcut("Ctrl+Nope".into()),
            ]
        );
        // Erros não apagam o padrão, e linhas válidas são aplicadas.
        assert_eq!(
            keymap.command_for(&Shortcut::parse("F5").unwrap()),
            Some(CommandId::Execute)
        );
        assert_eq!(
            keymap.command_for(&Shortcut::parse("F6").unwrap()),
            Some(CommandId::Parse)
        );
    }
}
