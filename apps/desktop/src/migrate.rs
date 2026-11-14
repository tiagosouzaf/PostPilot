//! Migração da configuração do nome antigo do app (PGStudio) para o novo
//! (PostPilot).
//!
//! Na primeira execução com o nome novo, `<config>/PGStudio` é copiada para
//! `<config>/PostPilot`: conexões recentes, servidores registrados, layout e
//! keymap continuam lá. A pasta antiga fica intacta, então uma versão antiga
//! ainda abre com o que tinha. As senhas lembradas migram sozinhas no cofre
//! (veja `pgstudio_ui_shell::secrets`).

use std::io;
use std::path::Path;

use pgstudio_core::{CONFIG_DIR_NAME, LEGACY_CONFIG_DIR_NAME};

/// Copia a pasta antiga para a nova quando só a antiga existe. Devolve se
/// copiou. A cópia vai para uma pasta temporária e só então ganha o nome
/// final, para que uma cópia interrompida seja refeita na próxima vez.
pub fn legacy_config(config: &Path) -> io::Result<bool> {
    let old = config.join(LEGACY_CONFIG_DIR_NAME);
    let new = config.join(CONFIG_DIR_NAME);
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let staging = config.join(format!("{CONFIG_DIR_NAME}.migrating"));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    copy_dir(&old, &staging)?;
    std::fs::rename(&staging, &new)?;
    Ok(true)
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("postpilot-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copies_the_old_folder_once_and_keeps_it() {
        let config = temp("copy");
        let old = config.join("PGStudio");
        std::fs::create_dir_all(old.join("diagrams")).unwrap();
        std::fs::write(old.join("connections.json"), "[1]").unwrap();
        std::fs::write(old.join("diagrams").join("a.pgsdiagram"), "{}").unwrap();

        assert!(legacy_config(&config).unwrap());
        let new = config.join("PostPilot");
        assert_eq!(
            std::fs::read_to_string(new.join("connections.json")).unwrap(),
            "[1]"
        );
        assert!(new.join("diagrams").join("a.pgsdiagram").exists());
        assert!(old.join("connections.json").exists());
        assert!(!config.join("PostPilot.migrating").exists());

        // A partir daí a pasta nova manda, mesmo que a antiga mude.
        std::fs::write(old.join("connections.json"), "[2]").unwrap();
        assert!(!legacy_config(&config).unwrap());
        assert_eq!(
            std::fs::read_to_string(new.join("connections.json")).unwrap(),
            "[1]"
        );
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn does_nothing_without_the_old_folder() {
        let config = temp("none");
        assert!(!legacy_config(&config).unwrap());
        assert!(!config.join("PostPilot").exists());
        std::fs::remove_dir_all(&config).unwrap();
    }

    #[test]
    fn redoes_an_interrupted_copy() {
        let config = temp("retry");
        std::fs::create_dir_all(config.join("PGStudio")).unwrap();
        std::fs::write(config.join("PGStudio").join("layout.json"), "{}").unwrap();
        std::fs::create_dir_all(config.join("PostPilot.migrating")).unwrap();
        std::fs::write(config.join("PostPilot.migrating").join("half"), "").unwrap();
        assert!(legacy_config(&config).unwrap());
        assert!(config.join("PostPilot").join("layout.json").exists());
        assert!(!config.join("PostPilot").join("half").exists());
        std::fs::remove_dir_all(&config).unwrap();
    }
}
