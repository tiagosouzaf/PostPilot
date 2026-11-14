//! Como o app foi instalado e como ele se atualiza em cada caso.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::net::{Progress, download_verified, with_suffix};
use crate::release::{Asset, Release};

/// Como o app em execução foi instalado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallKind {
    /// AppImage: o próprio arquivo `.AppImage` (variável `APPIMAGE`).
    AppImage(PathBuf),
    /// Windows, instalado pelo `.msi` (em Program Files).
    WindowsMsi,
    /// Windows, o `.zip` extraído em qualquer pasta: o caminho do `.exe`.
    WindowsPortable(PathBuf),
    /// Linux, instalado pelo pacman (pacote Arch ou AUR): o nome do pacote.
    Pacman(String),
    /// Linux, instalado pelo `.deb`.
    Dpkg,
    /// Compilado ou copiado à mão (`cargo build`, `cargo install`, outra
    /// distribuição): o app só avisa.
    Other,
}

/// Nome do pacote no AUR, quando o pacman não diz de quem é o binário.
pub const AUR_PACKAGE: &str = "postpilot-bin";

impl InstallKind {
    /// Descobre como o app em execução foi instalado.
    pub fn detect() -> InstallKind {
        let exe = std::env::current_exe().ok();
        let exe = exe.and_then(|e| e.canonicalize().ok().or(Some(e)));
        if cfg!(windows) {
            return match exe {
                Some(exe) if under_program_files(&exe) => InstallKind::WindowsMsi,
                Some(exe) => InstallKind::WindowsPortable(exe),
                None => InstallKind::Other,
            };
        }
        if let Some(image) = std::env::var_os("APPIMAGE").map(PathBuf::from)
            && image.is_file()
        {
            return InstallKind::AppImage(image);
        }
        let Some(exe) = exe else {
            return InstallKind::Other;
        };
        if !(exe.starts_with("/usr") || exe.starts_with("/opt")) {
            return InstallKind::Other;
        }
        if Path::new("/var/lib/pacman/local").is_dir() {
            return InstallKind::Pacman(pacman_owner(&exe).unwrap_or_else(|| AUR_PACKAGE.into()));
        }
        if Path::new("/var/lib/dpkg/status").is_file() {
            return InstallKind::Dpkg;
        }
        InstallKind::Other
    }

    /// Se o app pode baixar e instalar a versão nova sozinho.
    pub fn can_self_update(&self) -> bool {
        matches!(
            self,
            InstallKind::AppImage(_) | InstallKind::WindowsMsi | InstallKind::WindowsPortable(_)
        )
    }

    /// Descrição curta para a janela de atualização.
    pub fn describe(&self) -> &'static str {
        match self {
            InstallKind::AppImage(_) => "AppImage",
            InstallKind::WindowsMsi => "Windows installer (.msi)",
            InstallKind::WindowsPortable(_) => "Windows portable (.zip)",
            InstallKind::Pacman(_) => "Arch Linux package (pacman)",
            InstallKind::Dpkg => "Debian package (.deb)",
            InstallKind::Other => "manual build",
        }
    }

    /// Comando para o usuário atualizar pelo gerenciador de pacotes.
    /// `helper` é o ajudante do AUR instalado (`yay`, `paru`), se houver.
    pub fn update_command(&self, release: &Release, helper: Option<&str>) -> Option<String> {
        match self {
            InstallKind::Pacman(pkg) => Some(match helper {
                Some(h) => format!("{h} -Syu {pkg}"),
                None => format!("sudo pacman -Syu {pkg}"),
            }),
            InstallKind::Dpkg => release
                .asset_ending_with("_amd64.deb")
                .map(|deb| format!("sudo apt install ./{}", deb.name)),
            _ => None,
        }
    }

    /// O arquivo do Release que esta instalação baixa.
    pub fn asset<'r>(&self, release: &'r Release) -> Option<&'r Asset> {
        match self {
            InstallKind::AppImage(_) => release.asset_ending_with("-x86_64.AppImage"),
            InstallKind::WindowsMsi => release.asset_ending_with("-x86_64.msi"),
            InstallKind::WindowsPortable(_) => release.asset_ending_with("-windows-x86_64.zip"),
            InstallKind::Pacman(_) | InstallKind::Dpkg | InstallKind::Other => None,
        }
    }
}

/// O primeiro ajudante do AUR encontrado no `PATH`.
pub fn aur_helper() -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    ["yay", "paru"]
        .into_iter()
        .find(|h| std::env::split_paths(&path).any(|dir| dir.join(h).is_file()))
}

fn pacman_owner(exe: &Path) -> Option<String> {
    let out = Command::new("pacman").arg("-Qqo").arg(exe).output().ok()?;
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.lines().next()?.trim();
    (out.status.success() && !name.is_empty()).then(|| name.to_string())
}

fn under_program_files(exe: &Path) -> bool {
    ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .into_iter()
        .filter_map(std::env::var_os)
        .any(|dir| starts_with_ignore_case(exe, Path::new(&dir)))
}

fn starts_with_ignore_case(path: &Path, prefix: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase().replace('/', "\\");
    let p = p.strip_prefix(r"\\?\").unwrap_or(&p).to_string();
    let pre = prefix.to_string_lossy().to_lowercase().replace('/', "\\");
    let pre = pre.trim_end_matches('\\');
    !pre.is_empty() && (p == pre || p.starts_with(&format!("{pre}\\")))
}

/// O que fazer depois de [`install`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    /// A versão nova já está no lugar; abrir este programa a usa.
    Restart(PathBuf),
    /// O instalador do Windows foi aberto; o app precisa fechar para ele
    /// trocar os arquivos.
    InstallerStarted,
}

/// Baixa, confere e instala a versão nova. Bloqueia: rode numa thread.
pub fn install(
    kind: &InstallKind,
    release: &Release,
    user_agent: &str,
    progress: &Progress,
) -> Result<Installed, String> {
    let asset = kind
        .asset(release)
        .ok_or("This release has no package for this installation.")?;
    match kind {
        InstallKind::AppImage(image) => {
            let dir = image.parent().ok_or("the AppImage has no folder")?;
            let tmp = dir.join(format!(".{}.download", asset.name));
            let file = download_verified(release, asset, &tmp, user_agent, progress)?;
            replace_appimage(&file, image).inspect_err(|_| {
                let _ = std::fs::remove_file(&file);
            })?;
            Ok(Installed::Restart(image.clone()))
        }
        InstallKind::WindowsMsi => {
            let dir = std::env::temp_dir().join("postpilot-update");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let msi = download_verified(release, asset, &dir.join(&asset.name), user_agent, progress)?;
            // /passive: só a barra de progresso; o UAC ainda pede permissão.
            // O MajorUpgrade do .msi remove a versão antiga.
            Command::new("msiexec")
                .arg("/i")
                .arg(&msi)
                .arg("/passive")
                .spawn()
                .map_err(|e| format!("could not start the Windows installer: {e}"))?;
            Ok(Installed::InstallerStarted)
        }
        InstallKind::WindowsPortable(exe) => {
            let dir = std::env::temp_dir().join("postpilot-update");
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let zip = download_verified(release, asset, &dir.join(&asset.name), user_agent, progress)?;
            let result = replace_from_zip(&zip, exe);
            let _ = std::fs::remove_file(&zip);
            result.map(|()| Installed::Restart(exe.clone()))
        }
        _ => Err("This installation is updated by its package manager.".into()),
    }
}

/// Troca o AppImage em uso pelo novo, no mesmo caminho (atalhos e o
/// AppImageLauncher continuam apontando para ele). O processo em execução
/// segue com o arquivo antigo, que o sistema só apaga quando ele fechar.
#[cfg(unix)]
fn replace_appimage(new: &Path, image: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(image)
        .map(|m| m.permissions().mode())
        .unwrap_or(0o755)
        | 0o111;
    std::fs::set_permissions(new, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", new.display()))?;
    std::fs::rename(new, image).map_err(|e| format!("{}: {e}", image.display()))
}

#[cfg(not(unix))]
fn replace_appimage(_: &Path, _: &Path) -> Result<(), String> {
    Err("AppImages only run on Linux".into())
}

/// Extrai o `.exe` do zip ao lado do atual e troca os dois. O Windows não
/// deixa apagar um `.exe` em execução, mas deixa renomear: o antigo vira
/// `.old` e sai na próxima abertura ([`cleanup_old_binary`]).
fn replace_from_zip(zip: &Path, exe: &Path) -> Result<(), String> {
    let file = std::fs::File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", zip.display()))?;
    let index = (0..archive.len())
        .find(|&i| {
            archive
                .by_index(i)
                .is_ok_and(|f| f.is_file() && f.name().to_ascii_lowercase().ends_with(".exe"))
        })
        .ok_or("the zip has no .exe")?;
    let new = with_suffix(exe, ".new");
    {
        let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(&new).map_err(|e| format!("{}: {e}", new.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("{}: {e}", new.display()))?;
        out.sync_all().map_err(|e| format!("{}: {e}", new.display()))?;
    }
    let old = with_suffix(exe, ".old");
    let _ = std::fs::remove_file(&old);
    let swap = std::fs::rename(exe, &old)
        .map_err(|e| format!("{}: {e}", exe.display()))
        .and_then(|()| {
            std::fs::rename(&new, exe).map_err(|e| {
                // Devolve o antigo ao lugar para o app continuar abrindo.
                let _ = std::fs::rename(&old, exe);
                format!("{}: {e}", exe.display())
            })
        });
    if swap.is_err() {
        let _ = std::fs::remove_file(&new);
    }
    swap
}

/// Apaga o `.exe` antigo deixado por uma atualização da cópia portátil.
/// Chame ao abrir o app.
pub fn cleanup_old_binary() {
    if let Ok(exe) = std::env::current_exe() {
        let old = with_suffix(&exe, ".old");
        if old.is_file() {
            let _ = std::fs::remove_file(old);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use semver::Version;

    fn release(names: &[&str]) -> Release {
        Release {
            tag: "v0.2.0".into(),
            version: Version::new(0, 2, 0),
            page: String::new(),
            notes: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: n.to_string(),
                    url: format!("https://example.test/{n}"),
                    size: 1,
                })
                .collect(),
        }
    }

    #[test]
    fn package_managers_get_a_command_not_a_download() {
        let r = release(&["postpilot_0.2.0_amd64.deb", "PostPilot-0.2.0-x86_64.AppImage"]);
        let pacman = InstallKind::Pacman("postpilot-bin".into());
        assert!(!pacman.can_self_update());
        assert_eq!(pacman.asset(&r), None);
        assert_eq!(
            pacman.update_command(&r, Some("yay")).as_deref(),
            Some("yay -Syu postpilot-bin")
        );
        assert_eq!(
            pacman.update_command(&r, None).as_deref(),
            Some("sudo pacman -Syu postpilot-bin")
        );
        assert_eq!(
            InstallKind::Dpkg.update_command(&r, None).as_deref(),
            Some("sudo apt install ./postpilot_0.2.0_amd64.deb")
        );
        assert_eq!(InstallKind::Other.update_command(&r, None), None);
    }

    #[test]
    fn self_updating_installs_pick_their_asset() {
        let r = release(&[
            "PostPilot-0.2.0-x86_64.AppImage",
            "PostPilot-0.2.0-x86_64.msi",
            "PostPilot-0.2.0-windows-x86_64.zip",
            "SHA256SUMS",
        ]);
        let image = InstallKind::AppImage("/x/a.AppImage".into());
        assert_eq!(image.asset(&r).unwrap().name, "PostPilot-0.2.0-x86_64.AppImage");
        assert_eq!(InstallKind::WindowsMsi.asset(&r).unwrap().name, "PostPilot-0.2.0-x86_64.msi");
        let zip = InstallKind::WindowsPortable("C:/x/postpilot.exe".into());
        assert_eq!(zip.asset(&r).unwrap().name, "PostPilot-0.2.0-windows-x86_64.zip");
        assert!(image.can_self_update() && zip.can_self_update());
    }

    #[test]
    fn program_files_prefix_ignores_case_and_slashes() {
        let pf = Path::new(r"C:\Program Files");
        assert!(starts_with_ignore_case(Path::new(r"c:\program files\PostPilot\postpilot.exe"), pf));
        assert!(starts_with_ignore_case(Path::new(r"\\?\C:\Program Files\PostPilot\postpilot.exe"), pf));
        assert!(!starts_with_ignore_case(Path::new(r"C:\Program Files Extra\p.exe"), pf));
        assert!(!starts_with_ignore_case(Path::new(r"D:\tools\postpilot.exe"), pf));
    }

    #[test]
    fn swaps_the_exe_from_a_zip() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("updater-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("postpilot.exe");
        std::fs::write(&exe, b"old").unwrap();
        let zip_path = dir.join("p.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
            let opts = zip::write::SimpleFileOptions::default();
            w.add_directory("PostPilot-0.2.0-windows-x86_64/", opts).unwrap();
            w.start_file("PostPilot-0.2.0-windows-x86_64/LICENSE-MIT.txt", opts)
                .unwrap();
            w.write_all(b"license").unwrap();
            w.start_file("PostPilot-0.2.0-windows-x86_64/postpilot.exe", opts)
                .unwrap();
            w.write_all(b"new").unwrap();
            w.finish().unwrap();
        }
        replace_from_zip(&zip_path, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(with_suffix(&exe, ".old")).unwrap(), b"old");
        assert!(!with_suffix(&exe, ".new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn replaces_the_appimage_in_place() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("updater-appimage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let image = dir.join("PostPilot-0.1.0-x86_64.AppImage");
        std::fs::write(&image, b"old").unwrap();
        std::fs::set_permissions(&image, std::fs::Permissions::from_mode(0o755)).unwrap();
        let new = dir.join(".download");
        std::fs::write(&new, b"new").unwrap();
        replace_appimage(&new, &image).unwrap();
        assert_eq!(std::fs::read(&image).unwrap(), b"new");
        let mode = std::fs::metadata(&image).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);
        assert!(!new.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
