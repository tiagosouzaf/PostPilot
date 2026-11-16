//! Conversão de eventos de teclado do egui para acordes do keymap.

use eframe::egui;
use pgstudio_core::keymap::{Chord, Key, Modifiers};

/// Converte um evento do egui num acorde, se for uma tecla pressionada.
///
/// O egui transforma Ctrl+C, Ctrl+X e Ctrl+V em `Copy`, `Cut` e `Paste`, sem
/// evento de tecla; por isso eles voltam a ser acordes aqui. O `Paste` só
/// chega quando a área de transferência tem texto.
pub fn chord(event: &egui::Event) -> Option<Chord> {
    match event {
        egui::Event::Key {
            key,
            pressed: true,
            repeat: false,
            modifiers,
            ..
        } => {
            let key = key_of(*key)?;
            Some(Chord {
                mods: Modifiers {
                    ctrl: modifiers.ctrl || modifiers.mac_cmd,
                    // Em vários teclados o "+" é Shift+"=": o Shift faz
                    // parte da tecla, não do acorde.
                    shift: modifiers.shift && !matches!(key, Key::Plus | Key::Minus),
                    alt: modifiers.alt,
                },
                key,
            })
        }
        egui::Event::Copy => Some(Chord::ctrl(Key::Letter('C'))),
        egui::Event::Cut => Some(Chord::ctrl(Key::Letter('X'))),
        egui::Event::Paste(_) => Some(Chord::ctrl(Key::Letter('V'))),
        _ => None,
    }
}

/// Se o acorde é uma tecla de edição de texto, que pertence ao campo em
/// foco e não ao keymap: teclas sem Ctrl e Alt (exceto F1–F12 e Esc), e
/// Ctrl com letras de edição, setas, Home, End, Backspace e Delete.
pub fn is_text_editing(chord: Chord) -> bool {
    if chord.mods.alt || matches!(chord.key, Key::F(_) | Key::Escape) {
        return false;
    }
    if !chord.mods.ctrl {
        return true;
    }
    // Ctrl+D no editor é do VS Code: seleciona a próxima ocorrência.
    if chord.key == Key::Letter('D') && !chord.mods.shift {
        return true;
    }
    matches!(
        chord.key,
        Key::Letter('A' | 'C' | 'V' | 'X' | 'Y' | 'Z')
            | Key::Left
            | Key::Right
            | Key::Up
            | Key::Down
            | Key::Home
            | Key::End
            | Key::Backspace
            | Key::Delete
    )
}

fn key_of(key: egui::Key) -> Option<Key> {
    use egui::Key as E;
    let name = key.name();
    if name.len() == 1 {
        let c = name.chars().next()?;
        if c.is_ascii_alphabetic() {
            return Some(Key::Letter(c.to_ascii_uppercase()));
        }
        if let Some(d) = c.to_digit(10) {
            return Some(Key::Digit(d as u8));
        }
    }
    Some(match key {
        E::F1 => Key::F(1),
        E::F2 => Key::F(2),
        E::F3 => Key::F(3),
        E::F4 => Key::F(4),
        E::F5 => Key::F(5),
        E::F6 => Key::F(6),
        E::F7 => Key::F(7),
        E::F8 => Key::F(8),
        E::F9 => Key::F(9),
        E::F10 => Key::F(10),
        E::F11 => Key::F(11),
        E::F12 => Key::F(12),
        E::Enter => Key::Enter,
        E::Escape => Key::Escape,
        E::Tab => Key::Tab,
        E::Space => Key::Space,
        E::Backspace => Key::Backspace,
        E::Delete => Key::Delete,
        E::Insert => Key::Insert,
        E::Home => Key::Home,
        E::End => Key::End,
        E::PageUp => Key::PageUp,
        E::PageDown => Key::PageDown,
        E::ArrowLeft => Key::Left,
        E::ArrowRight => Key::Right,
        E::ArrowUp => Key::Up,
        E::ArrowDown => Key::Down,
        E::Plus | E::Equals => Key::Plus,
        E::Minus => Key::Minus,
        E::Period => Key::Period,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn converts_keys_and_modifiers() {
        let c = chord(&press(
            egui::Key::S,
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
        ))
        .unwrap();
        assert_eq!(c.to_string(), "Ctrl+Shift+S");
        let c = chord(&press(egui::Key::F5, egui::Modifiers::NONE)).unwrap();
        assert_eq!(c.to_string(), "F5");
        let c = chord(&press(egui::Key::Num1, egui::Modifiers::ALT)).unwrap();
        assert_eq!(c.to_string(), "Alt+1");
    }

    #[test]
    fn ignores_releases_and_repeats() {
        let mut e = press(egui::Key::A, egui::Modifiers::CTRL);
        if let egui::Event::Key { pressed, .. } = &mut e {
            *pressed = false;
        }
        assert_eq!(chord(&e), None);
    }

    #[test]
    fn editing_keys_belong_to_the_text_field() {
        let p = |s| Chord::parse(s).unwrap();
        for s in [
            "A",
            "Shift+A",
            "Enter",
            "Tab",
            "Ctrl+C",
            "Ctrl+Z",
            "Ctrl+Left",
        ] {
            assert!(is_text_editing(p(s)), "{s}");
        }
        for s in ["F5", "Ctrl+S", "Ctrl+N", "Alt+X", "Esc", "Ctrl+Shift+S"] {
            assert!(!is_text_editing(p(s)), "{s}");
        }
    }

    #[test]
    fn clipboard_events_become_chords() {
        assert_eq!(chord(&egui::Event::Copy).unwrap().to_string(), "Ctrl+C");
        assert_eq!(chord(&egui::Event::Cut).unwrap().to_string(), "Ctrl+X");
    }
}
