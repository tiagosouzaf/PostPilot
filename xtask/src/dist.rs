//! `cargo xtask dist`: pacotes de instalação do PostPilot.
//!
//! - `.deb` (Debian, Ubuntu e derivados), montado com `dpkg-deb`;
//! - AppImage (qualquer Linux x86-64), o runtime tipo 2 do projeto AppImage
//!   seguido de um squashfs feito com `mksquashfs`;
//! - `.pkg.tar.zst` (Arch Linux e derivados), montado com o `bsdtar` como o
//!   `makepkg` faz; o `packaging/arch/PKGBUILD` gera o mesmo a partir do
//!   código;
//! - `.zip` para Windows, compilado de forma cruzada com o MinGW
//!   (`x86_64-pc-windows-gnu`) quando o host não é Windows;
//! - `.msi` para Windows, com o mesmo `postpilot.exe`, montado pelo `wixl` (do
//!   msitools) a partir de `packaging/windows/postpilot.wxs`.
//!
//! Tudo sai em `target/packages/`, com um `SHA256SUMS`. O binário é
//! compilado no perfil `dist` (o `release` sem símbolos).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const USAGE: &str = "\
Uso: cargo xtask dist [opções]

Gera os pacotes de instalação em target/packages/:

  deb       postpilot_<versão>_amd64.deb (precisa do dpkg-deb)
  appimage  PostPilot-<versão>-x86_64.AppImage (precisa do mksquashfs, do
            pacote squashfs-tools; baixa o runtime do AppImage na primeira
            vez, ou use APPIMAGE_RUNTIME=caminho)
  arch      postpilot-<versão>-1-x86_64.pkg.tar.zst (precisa do bsdtar, do
            pacote libarchive-tools no Ubuntu; no Arch já vem). Instale com
            sudo pacman -U; ou compile com o makepkg em packaging/arch/
  tar       postpilot-<versão>-linux-x86_64.tar.gz, a árvore usr/ dos
            pacotes (binário, .desktop, ícones, licenças), que o pacote
            postpilot-bin do AUR baixa do Release (precisa do bsdtar)
  zip       PostPilot-<versão>-windows-x86_64.zip (no Linux, compila para
            x86_64-pc-windows-gnu: rustup target add x86_64-pc-windows-gnu
            e o pacote mingw-w64)
  msi       PostPilot-<versão>-x86_64.msi, instalador com atalho no menu
            Iniciar e desinstalação pelo Windows (o mesmo exe do zip; precisa
            do wixl, do pacote wixl no Ubuntu ou msitools no Arch)

No Linux, o padrão são os seis; no Windows, só o zip (o msi também sai lá
com --only msi se o wixl estiver no PATH).

O binário Linux é ligado contra a glibc 2.31 (Ubuntu 20.04, Debian 11 e
mais novos) com o cargo-zigbuild (cargo install cargo-zigbuild e
pip install ziglang). Sem ele, ou com --native, sai um build comum, que só
roda com a glibc da máquina que compilou ou mais nova.

Opções:
  --only F    só estes formatos, separados por vírgula
              (deb,appimage,arch,tar,zip,msi)
  --no-build  não recompila; usa os binários do perfil dist que já existem
  --native    compila o binário Linux sem o cargo-zigbuild";

/// Alvo do binário Linux com o cargo-zigbuild: x86-64 ligado contra a glibc
/// 2.31 (Ubuntu 20.04, Debian 11), para rodar também em distros mais velhas
/// que a máquina que compila.
const LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";
const LINUX_GLIBC: &str = "2.31";
/// Alvo da compilação cruzada para Windows.
const WINDOWS_TARGET: &str = "x86_64-pc-windows-gnu";
/// Runtime tipo 2 do AppImage (estático, não depende da libfuse2).
const APPIMAGE_RUNTIME_URL: &str =
    "https://github.com/AppImage/type2-runtime/releases/download/continuous/runtime-x86_64";
/// Bibliotecas que o app abre em tempo de execução (o winit e o wgpu usam
/// `dlopen`, então não aparecem como dependências do ELF).
const DEB_DEPENDS: &[&str] = &[
    "libgcc-s1",
    "libxkbcommon0",
    "libxkbcommon-x11-0",
    "libx11-6",
    "libx11-xcb1",
    "libxcursor1",
    "libxrandr2",
    "libxi6",
    "libwayland-client0",
    "libwayland-cursor0",
    "libwayland-egl1",
    "libvulkan1 | libegl1",
];
/// Drivers de vídeo (Vulkan ou OpenGL ES do Mesa) e o portal dos diálogos de
/// arquivo, com um backend que tenha o seletor de arquivos (o `-wlr` do sway
/// e o do Hyprland não têm). Um desktop já tem; um container mínimo, não.
const DEB_RECOMMENDS: &[&str] = &[
    "mesa-vulkan-drivers | libegl-mesa0",
    "xdg-desktop-portal",
    "xdg-desktop-portal-gtk | xdg-desktop-portal-gnome | xdg-desktop-portal-kde",
];

/// As mesmas dependências, com os nomes do Arch (e do `PKGBUILD`).
const ARCH_DEPENDS: &[&str] = &[
    "glibc",
    "gcc-libs",
    "hicolor-icon-theme",
    "libxkbcommon",
    "libxkbcommon-x11",
    "libx11",
    "libxcursor",
    "libxrandr",
    "libxi",
    "wayland",
    "libglvnd",
    "vulkan-icd-loader",
];
const ARCH_OPTDEPENDS: &[&str] = &[
    "vulkan-driver: renderização por Vulkan (sem ele, usa OpenGL ES)",
    "xdg-desktop-portal: diálogos de abrir e salvar arquivo",
    "xdg-desktop-portal-gtk: seletor de arquivos no Hyprland e no sway (os portais deles não têm)",
];
/// Revisão do pacote Arch (o `pkgrel`).
const ARCH_PKGREL: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Deb,
    AppImage,
    Arch,
    Tar,
    Zip,
    Msi,
}

impl Format {
    const ALL: [Format; 6] = [
        Format::Deb,
        Format::AppImage,
        Format::Arch,
        Format::Tar,
        Format::Zip,
        Format::Msi,
    ];

    fn parse(name: &str) -> Option<Self> {
        match name {
            "deb" => Some(Format::Deb),
            "appimage" => Some(Format::AppImage),
            "arch" => Some(Format::Arch),
            "tar" => Some(Format::Tar),
            "zip" => Some(Format::Zip),
            "msi" => Some(Format::Msi),
            _ => None,
        }
    }

    fn needs_linux_binary(self) -> bool {
        matches!(
            self,
            Format::Deb | Format::AppImage | Format::Arch | Format::Tar
        )
    }

    fn needs_windows_binary(self) -> bool {
        matches!(self, Format::Zip | Format::Msi)
    }
}

#[derive(Debug, PartialEq)]
pub struct DistArgs {
    pub formats: Vec<Format>,
    pub build: bool,
    pub native: bool,
}

pub fn parse(args: &[String]) -> Result<DistArgs, String> {
    let mut out = DistArgs {
        formats: if cfg!(windows) {
            vec![Format::Zip]
        } else {
            Format::ALL.to_vec()
        },
        build: true,
        native: false,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--only" => {
                let list = it.next().ok_or("--only precisa de formatos")?;
                let mut formats = Vec::new();
                for name in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    let f = Format::parse(name)
                        .ok_or_else(|| format!("formato desconhecido: {name}"))?;
                    if !formats.contains(&f) {
                        formats.push(f);
                    }
                }
                if formats.is_empty() {
                    return Err("--only precisa de formatos".into());
                }
                out.formats = formats;
            }
            "--no-build" => out.build = false,
            "--native" => out.native = true,
            other => return Err(format!("opção desconhecida: {other}\n\n{USAGE}")),
        }
    }
    if cfg!(windows) && out.formats.iter().any(|f| f.needs_linux_binary()) {
        return Err("o .deb, o AppImage e o pacote do Arch só são gerados no Linux".into());
    }
    Ok(out)
}

/// Versão do workspace (o xtask herda a mesma do `[workspace.package]`).
fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn run(args: DistArgs, root: &Path) -> Result<bool, String> {
    let out = root.join("target").join("packages");
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut made: Vec<PathBuf> = Vec::new();

    if args.formats.iter().any(|f| f.needs_linux_binary()) {
        let zig_exe = root
            .join("target")
            .join(LINUX_TARGET)
            .join("dist")
            .join("postpilot");
        let zig = if args.build {
            !args.native && has_zigbuild()
        } else {
            !args.native && zig_exe.exists()
        };
        if args.build {
            if zig {
                cargo_zigbuild(root)?;
            } else {
                if !args.native {
                    eprintln!(
                        "aviso: cargo-zigbuild não encontrado; o binário vai exigir a glibc \
                         desta máquina (veja `cargo xtask dist --help`)"
                    );
                }
                cargo_build(root, None)?;
            }
        }
        let exe = if zig {
            zig_exe
        } else {
            root.join("target").join("dist").join("postpilot")
        };
        if !exe.exists() {
            return Err(format!("{} não existe; rode sem --no-build", exe.display()));
        }
        let glibc = min_glibc(&std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?);
        if let Some(v) = &glibc {
            eprintln!("o binário precisa da glibc {v} ou mais nova");
        }
        if args.formats.contains(&Format::Deb) {
            made.push(build_deb(root, &out, &exe, glibc.as_deref())?);
        }
        if args.formats.contains(&Format::AppImage) {
            made.push(build_appimage(root, &out, &exe)?);
        }
        if args.formats.contains(&Format::Arch) {
            made.push(build_arch(root, &out, &exe)?);
        }
        if args.formats.contains(&Format::Tar) {
            let tar = build_tar(root, &out, &exe)?;
            write_aur_bin(&out, &tar)?;
            made.push(tar);
        }
    }
    if args.formats.iter().any(|f| f.needs_windows_binary()) {
        let target = (!cfg!(windows)).then_some(WINDOWS_TARGET);
        if args.build {
            cargo_build(root, target)?;
        }
        let mut exe = root.join("target");
        if let Some(t) = target {
            exe.push(t);
        }
        exe.push("dist");
        exe.push("postpilot.exe");
        if !exe.exists() {
            return Err(format!("{} não existe; rode sem --no-build", exe.display()));
        }
        if args.formats.contains(&Format::Zip) {
            made.push(build_zip(root, &out, &exe)?);
        }
        if args.formats.contains(&Format::Msi) {
            made.push(build_msi(root, &out, &exe)?);
        }
    }

    write_checksums(&out, &made);
    println!("Pacotes em {}:", out.display());
    for p in &made {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        println!(
            "  {}  ({:.1} MB)",
            p.file_name().unwrap_or_default().to_string_lossy(),
            size as f64 / 1_048_576.0
        );
    }
    Ok(true)
}

fn has_zigbuild() -> bool {
    Command::new("cargo-zigbuild")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

fn cargo_zigbuild(root: &Path) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = Command::new(cargo)
        .args([
            "zigbuild",
            "--profile",
            "dist",
            "--locked",
            "-p",
            "pgstudio-desktop",
            "--target",
            &format!("{LINUX_TARGET}.{LINUX_GLIBC}"),
        ])
        .current_dir(root)
        .status()
        .map_err(|e| format!("não consegui rodar o cargo zigbuild: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("a compilação com o cargo-zigbuild falhou (o zig está instalado?)".into())
    }
}

fn cargo_build(root: &Path, target: Option<&str>) -> Result<(), String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args([
        "build",
        "--profile",
        "dist",
        "--locked",
        "-p",
        "pgstudio-desktop",
    ]);
    if let Some(t) = target {
        cmd.args(["--target", t]);
    }
    let status = cmd
        .current_dir(root)
        .status()
        .map_err(|e| format!("não consegui rodar o cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else if let Some(t) = target {
        Err(format!(
            "a compilação para {t} falhou; confira `rustup target add {t}` e o \
             pacote mingw-w64 (x86_64-w64-mingw32-gcc)"
        ))
    } else {
        Err("a compilação falhou".into())
    }
}

/// Roda um comando e devolve erro com o nome dele se falhar.
fn run_tool(cmd: &mut Command, hint: &str) -> Result<(), String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{name} não encontrado; {hint}")
        } else {
            format!("{name}: {e}")
        }
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{name} saiu com {status}"))
    }
}

fn io<T>(r: std::io::Result<T>, what: &Path) -> Result<T, String> {
    r.map_err(|e| format!("{}: {e}", what.display()))
}

/// Cria `dir` vazio (apaga o que houver).
fn fresh_dir(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        io(std::fs::remove_dir_all(dir), dir)?;
    }
    io(std::fs::create_dir_all(dir), dir)
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        io(std::fs::create_dir_all(parent), parent)?;
    }
    io(std::fs::copy(from, to), from).map(|_| ())
}

fn write(to: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        io(std::fs::create_dir_all(parent), parent)?;
    }
    io(std::fs::write(to, contents), to)
}

/// Maior versão de `GLIBC_2.x` que o ELF pede (as referências de versão dos
/// símbolos ficam como texto no `.gnu.version_r`).
pub fn min_glibc(elf: &[u8]) -> Option<String> {
    const TAG: &[u8] = b"GLIBC_2.";
    let mut best: Option<(u32, u32)> = None;
    let mut i = 0;
    while let Some(pos) = elf[i..].windows(TAG.len()).position(|w| w == TAG) {
        let start = i + pos + TAG.len();
        let digits = |from: usize| {
            let end = elf[from..]
                .iter()
                .position(|b| !b.is_ascii_digit())
                .map_or(elf.len(), |n| from + n);
            let n = std::str::from_utf8(&elf[from..end])
                .ok()
                .and_then(|d| d.parse().ok());
            (n, end)
        };
        let (minor, end): (Option<u32>, usize) = digits(start);
        let patch = if elf.get(end) == Some(&b'.') {
            digits(end + 1).0.unwrap_or(0)
        } else {
            0
        };
        if let Some(minor) = minor {
            let v = (minor, patch);
            if best.is_none_or(|b| v > b) {
                best = Some(v);
            }
        }
        i = start;
    }
    best.map(|(minor, patch)| {
        if patch == 0 {
            format!("2.{minor}")
        } else {
            format!("2.{minor}.{patch}")
        }
    })
}

/// Ícones do tema hicolor: (tamanho, arquivo em packaging/icons).
fn icons(root: &Path) -> Vec<(u32, PathBuf)> {
    [16, 24, 32, 48, 64, 128, 256, 512]
        .into_iter()
        .map(|n| (n, root.join(format!("packaging/icons/postpilot-{n}.png"))))
        .collect()
}

/// Pasta do plugin do Claude Code nos pacotes Linux, sob `usr/`.
pub const LINUX_PLUGIN_DIR: &str = "share/postpilot";

/// O plugin do Claude Code (`claude-plugin/`) e o marketplace que aponta
/// para ele, em `dest`. Com isso, `claude plugin marketplace add <dest>`
/// e `claude plugin install postpilot@postpilot` instalam o plugin.
fn install_claude_plugin(root: &Path, dest: &Path) -> Result<(), String> {
    fn walk(from: &Path, to: &Path) -> Result<(), String> {
        for entry in io(std::fs::read_dir(from), from)? {
            let entry = io(entry, from)?;
            let path = entry.path();
            let target = to.join(entry.file_name());
            if path.is_dir() {
                walk(&path, &target)?;
            } else {
                copy(&path, &target)?;
            }
        }
        Ok(())
    }
    walk(&root.join("claude-plugin"), &dest.join("claude-plugin"))?;
    copy(
        &root.join(".claude-plugin/marketplace.json"),
        &dest.join(".claude-plugin/marketplace.json"),
    )
}

/// Arquivos comuns ao .deb e ao AppImage, sob `usr/`.
fn install_usr(root: &Path, usr: &Path, exe: &Path) -> Result<(), String> {
    let bin = usr.join("bin").join("postpilot");
    copy(exe, &bin)?;
    set_executable(&bin)?;
    copy(
        &root.join("packaging/linux/postpilot.desktop"),
        &usr.join("share/applications/postpilot.desktop"),
    )?;
    for (n, png) in icons(root) {
        copy(
            &png,
            &usr.join(format!("share/icons/hicolor/{n}x{n}/apps/postpilot.png")),
        )?;
    }
    install_claude_plugin(root, &usr.join(LINUX_PLUGIN_DIR))
}

#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    io(
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)),
        path,
    )
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

/// Tamanho de tudo sob `dir`, em KiB (o `Installed-Size` do .deb).
fn size_kib(dir: &Path) -> u64 {
    fn walk(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| match e.file_type() {
                Ok(t) if t.is_dir() => walk(&e.path()),
                Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                Err(_) => 0,
            })
            .sum()
    }
    walk(dir).div_ceil(1024)
}

/// O `DEBIAN/control`.
pub fn deb_control(version: &str, installed_kib: u64, glibc: Option<&str>) -> String {
    let mut depends = vec![match glibc {
        Some(v) => format!("libc6 (>= {v})"),
        None => "libc6".to_string(),
    }];
    depends.extend(DEB_DEPENDS.iter().map(|d| d.to_string()));
    format!(
        "Package: postpilot\n\
         Version: {version}\n\
         Architecture: amd64\n\
         Maintainer: PostPilot <https://github.com/tiagosouzaf/postpilot>\n\
         Installed-Size: {installed_kib}\n\
         Depends: {}\n\
         Recommends: {}\n\
         Replaces: pgstudio\n\
         Conflicts: pgstudio\n\
         Section: database\n\
         Priority: optional\n\
         Homepage: https://github.com/tiagosouzaf/postpilot\n\
         Description: PostgreSQL management studio inspired by SSMS\n \
         PostPilot is a native desktop client for PostgreSQL written in Rust,\n \
         with an Object Explorer, query windows with IntelliSense, a result\n \
         grid, execution plans, database diagrams and administration tools,\n \
         laid out like SQL Server Management Studio.\n",
        depends.join(", "),
        DEB_RECOMMENDS.join(", "),
    )
}

fn build_deb(root: &Path, out: &Path, exe: &Path, glibc: Option<&str>) -> Result<PathBuf, String> {
    let stage = out.join("deb-root");
    fresh_dir(&stage)?;
    install_usr(root, &stage.join("usr"), exe)?;
    let doc = stage.join("usr/share/doc/postpilot");
    let mut copyright = String::from(
        "Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/\n\
         Upstream-Name: PostPilot\n\
         Source: https://github.com/tiagosouzaf/postpilot\n\n\
         Files: *\n\
         Copyright: PostPilot contributors\n\
         License: MIT or Apache-2.0\n\n",
    );
    for (name, file) in [("MIT", "LICENSE-MIT"), ("Apache-2.0", "LICENSE-APACHE")] {
        let text = io(std::fs::read_to_string(root.join(file)), &root.join(file))?;
        let _ = writeln!(copyright, "License: {name}");
        for line in text.lines() {
            // Linhas vazias viram " ." no formato do Debian.
            if line.trim().is_empty() {
                copyright.push_str(" .\n");
            } else {
                let _ = writeln!(copyright, " {line}");
            }
        }
        copyright.push('\n');
    }
    write(&doc.join("copyright"), copyright)?;
    let installed = size_kib(&stage);
    write(
        &stage.join("DEBIAN/control"),
        deb_control(version(), installed, glibc),
    )?;
    let deb = out.join(format!("postpilot_{}_amd64.deb", version()));
    run_tool(
        Command::new("dpkg-deb")
            .args(["--root-owner-group", "-Zxz", "--build"])
            .arg(&stage)
            .arg(&deb),
        "instale o dpkg (no Debian/Ubuntu já vem)",
    )?;
    io(std::fs::remove_dir_all(&stage), &stage)?;
    Ok(deb)
}

/// Runtime do AppImage: `APPIMAGE_RUNTIME`, ou o baixado antes para
/// `target/packages/tools/`, ou baixa agora com o `curl`.
fn appimage_runtime(out: &Path) -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("APPIMAGE_RUNTIME") {
        return Ok(PathBuf::from(p));
    }
    let cached = out.join("tools").join("runtime-x86_64");
    if !cached.exists() {
        eprintln!("baixando o runtime do AppImage de {APPIMAGE_RUNTIME_URL}");
        io(std::fs::create_dir_all(cached.parent().unwrap_or(out)), out)?;
        let part = cached.with_extension("part");
        run_tool(
            Command::new("curl")
                .args(["-fsSL", "--retry", "3", "-o"])
                .arg(&part)
                .arg(APPIMAGE_RUNTIME_URL),
            "instale o curl ou aponte APPIMAGE_RUNTIME para o runtime",
        )?;
        let head = io(std::fs::read(&part), &part)?;
        if !head.starts_with(b"\x7fELF") {
            return Err(format!("{APPIMAGE_RUNTIME_URL} não devolveu um ELF"));
        }
        io(std::fs::rename(&part, &cached), &cached)?;
    }
    Ok(cached)
}

/// O `.PKGINFO` do pacote Arch.
pub fn arch_pkginfo(version: &str, size: u64, builddate: u64) -> String {
    let mut s = format!(
        "# Gerado por cargo xtask dist\n\
         pkgname = postpilot\n\
         pkgbase = postpilot\n\
         xdata = pkgtype=pkg\n\
         pkgver = {version}-{ARCH_PKGREL}\n\
         pkgdesc = PostgreSQL management studio inspired by SSMS\n\
         url = https://github.com/tiagosouzaf/postpilot\n\
         builddate = {builddate}\n\
         packager = PostPilot <https://github.com/tiagosouzaf/postpilot>\n\
         size = {size}\n\
         arch = x86_64\n\
         license = MIT OR Apache-2.0\n\
         conflict = pgstudio\n\
         replaces = pgstudio\n"
    );
    for d in ARCH_DEPENDS {
        let _ = writeln!(s, "depend = {d}");
    }
    for d in ARCH_OPTDEPENDS {
        let _ = writeln!(s, "optdepend = {d}");
    }
    s
}

/// Página do projeto, de onde o pacote `postpilot-bin` do AUR baixa o
/// tarball (do Release `v<versão>`).
const PROJECT_URL: &str = "https://github.com/tiagosouzaf/postpilot";

/// PKGBUILD e .SRCINFO do pacote `postpilot-bin` do AUR, que instala o
/// tarball de `cargo xtask dist --only tar` publicado no Release
/// `v<versão>`.
pub fn aur_bin(version: &str, sha256: &str) -> (String, String) {
    let file = format!("postpilot-{version}-linux-x86_64.tar.gz");
    let quote = |items: &[&str]| -> String {
        items
            .iter()
            .map(|d| format!("  '{d}'\n"))
            .collect::<String>()
    };
    let pkgbuild = format!(
        "# Pacote do AUR gerado por `cargo xtask dist` (xtask/src/dist.rs).\n\
         # Instala o binário publicado no Release v$pkgver do GitHub.\n\
         \n\
         pkgname=postpilot-bin\n\
         pkgver={version}\n\
         pkgrel={ARCH_PKGREL}\n\
         pkgdesc='PostgreSQL management studio inspired by SSMS (binary release)'\n\
         arch=('x86_64')\n\
         url='{PROJECT_URL}'\n\
         license=('MIT OR Apache-2.0')\n\
         depends=(\n{deps})\n\
         optdepends=(\n{opt})\n\
         provides=('postpilot')\n\
         conflicts=('postpilot' 'pgstudio')\n\
         options=('!strip' '!debug')\n\
         source=(\"{file}::$url/releases/download/v$pkgver/{file}\")\n\
         sha256sums=('{sha256}')\n\
         \n\
         package() {{\n\
         \x20 cp -a \"$srcdir/usr\" \"$pkgdir/\"\n\
         }}\n",
        deps = quote(ARCH_DEPENDS),
        opt = quote(ARCH_OPTDEPENDS),
    );
    let mut srcinfo = format!(
        "pkgbase = postpilot-bin\n\
         \tpkgdesc = PostgreSQL management studio inspired by SSMS (binary release)\n\
         \tpkgver = {version}\n\
         \tpkgrel = {ARCH_PKGREL}\n\
         \turl = {PROJECT_URL}\n\
         \tarch = x86_64\n\
         \tlicense = MIT OR Apache-2.0\n"
    );
    for d in ARCH_DEPENDS {
        let _ = writeln!(srcinfo, "\tdepends = {d}");
    }
    for d in ARCH_OPTDEPENDS {
        let _ = writeln!(srcinfo, "\toptdepends = {d}");
    }
    let _ = write!(
        srcinfo,
        "\tprovides = postpilot\n\
         \tconflicts = postpilot\n\
         \tconflicts = pgstudio\n\
         \toptions = !strip\n\
         \toptions = !debug\n\
         \tsource = {file}::{PROJECT_URL}/releases/download/v{version}/{file}\n\
         \tsha256sums = {sha256}\n\
         \n\
         pkgname = postpilot-bin\n"
    );
    (pkgbuild, srcinfo)
}

/// Escreve `target/packages/aur/postpilot-bin/` com o SHA-256 do tarball.
fn write_aur_bin(out: &Path, tar: &Path) -> Result<(), String> {
    let output = Command::new("sha256sum")
        .arg(tar)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let sha = text
        .split_whitespace()
        .next()
        .filter(|h| h.len() == 64)
        .ok_or("sha256sum não devolveu um hash")?;
    let (pkgbuild, srcinfo) = aur_bin(version(), sha);
    let dir = out.join("aur").join("postpilot-bin");
    write(&dir.join("PKGBUILD"), pkgbuild)?;
    write(&dir.join(".SRCINFO"), srcinfo)?;
    eprintln!("pacote do AUR em {}", dir.display());
    Ok(())
}

/// `usr/` com as licenças, para o tarball do AUR.
fn build_tar(root: &Path, out: &Path, exe: &Path) -> Result<PathBuf, String> {
    let stage = out.join("tar-root");
    fresh_dir(&stage)?;
    install_usr(root, &stage.join("usr"), exe)?;
    let licenses = stage.join("usr/share/licenses/postpilot");
    for file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        copy(&root.join(file), &licenses.join(file))?;
    }
    let tar = out.join(format!("postpilot-{}-linux-x86_64.tar.gz", version()));
    run_tool(
        Command::new("bsdtar")
            .env("LANG", "C")
            .args([
                "--uid", "0", "--gid", "0", "--uname", "root", "--gname", "root",
            ])
            .arg("-czf")
            .arg(&tar)
            .arg("usr")
            .current_dir(&stage),
        "instale o bsdtar (libarchive-tools no Ubuntu)",
    )?;
    io(std::fs::remove_dir_all(&stage), &stage)?;
    Ok(tar)
}

fn build_arch(root: &Path, out: &Path, exe: &Path) -> Result<PathBuf, String> {
    let stage = out.join("arch-root");
    fresh_dir(&stage)?;
    install_usr(root, &stage.join("usr"), exe)?;
    let licenses = stage.join("usr/share/licenses/postpilot");
    for file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        copy(&root.join(file), &licenses.join(file))?;
    }
    let size = size_kib(&stage) * 1024;
    let builddate = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    write(
        &stage.join(".PKGINFO"),
        arch_pkginfo(version(), size, builddate),
    )?;
    // Como o makepkg: primeiro o .MTREE (o pacman confere os arquivos com
    // ele), depois o pacote, com dono root.
    let owner = [
        "--uid", "0", "--gid", "0", "--uname", "root", "--gname", "root",
    ];
    run_tool(
        Command::new("bsdtar")
            .env("LANG", "C")
            .args(owner)
            .args([
                "-czf",
                ".MTREE",
                "--format=mtree",
                "--options=!all,use-set,type,uid,gid,mode,time,size,sha256,link",
                ".PKGINFO",
                "usr",
            ])
            .current_dir(&stage),
        "instale o bsdtar (libarchive-tools no Ubuntu)",
    )?;
    let pkg = out.join(format!(
        "postpilot-{}-{ARCH_PKGREL}-x86_64.pkg.tar.zst",
        version()
    ));
    run_tool(
        Command::new("bsdtar")
            .env("LANG", "C")
            .args(owner)
            .args(["--zstd", "-cf"])
            .arg(&pkg)
            .args([".MTREE", ".PKGINFO", "usr"])
            .current_dir(&stage),
        "instale o bsdtar (libarchive-tools no Ubuntu)",
    )?;
    io(std::fs::remove_dir_all(&stage), &stage)?;
    Ok(pkg)
}

fn build_appimage(root: &Path, out: &Path, exe: &Path) -> Result<PathBuf, String> {
    let appdir = out.join("PostPilot.AppDir");
    fresh_dir(&appdir)?;
    install_usr(root, &appdir.join("usr"), exe)?;
    copy(
        &root.join("packaging/linux/postpilot.desktop"),
        &appdir.join("postpilot.desktop"),
    )?;
    copy(
        &root.join("packaging/icons/postpilot-256.png"),
        &appdir.join("postpilot.png"),
    )?;
    symlink("postpilot.png", &appdir.join(".DirIcon"))?;
    symlink("usr/bin/postpilot", &appdir.join("AppRun"))?;

    let squashfs = out.join("PostPilot.squashfs");
    if squashfs.exists() {
        io(std::fs::remove_file(&squashfs), &squashfs)?;
    }
    run_tool(
        Command::new("mksquashfs")
            .arg(&appdir)
            .arg(&squashfs)
            .args([
                "-root-owned",
                "-noappend",
                "-comp",
                "zstd",
                "-no-progress",
                "-quiet",
            ]),
        "instale o squashfs-tools",
    )?;
    let runtime = appimage_runtime(out)?;
    let mut image = io(std::fs::read(&runtime), &runtime)?;
    image.extend(io(std::fs::read(&squashfs), &squashfs)?);
    let path = out.join(format!("PostPilot-{}-x86_64.AppImage", version()));
    write(&path, image)?;
    set_executable(&path)?;
    io(std::fs::remove_file(&squashfs), &squashfs)?;
    io(std::fs::remove_dir_all(&appdir), &appdir)?;
    Ok(path)
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result<(), String> {
    io(std::os::unix::fs::symlink(target, link), link)
}

#[cfg(not(unix))]
fn symlink(_: &str, link: &Path) -> Result<(), String> {
    Err(format!("{}: links simbólicos só no Unix", link.display()))
}

/// Copia o exe e as licenças (em .txt com CRLF) para `dir`: o que o zip e
/// o msi instalam.
fn stage_windows(root: &Path, dir: &Path, exe: &Path) -> Result<(), String> {
    copy(exe, &dir.join("postpilot.exe"))?;
    install_claude_plugin(root, dir)?;
    for file in ["LICENSE-MIT", "LICENSE-APACHE"] {
        let text = io(std::fs::read_to_string(root.join(file)), &root.join(file))?;
        write(&dir.join(format!("{file}.txt")), crlf(&text))?;
    }
    Ok(())
}

fn build_zip(root: &Path, out: &Path, exe: &Path) -> Result<PathBuf, String> {
    let name = format!("PostPilot-{}-windows-x86_64", version());
    let stage = out.join("zip-root");
    fresh_dir(&stage)?;
    let dir = stage.join(&name);
    stage_windows(root, &dir, exe)?;
    let readme = root.join("packaging/windows/LEIA-ME.txt");
    let text = io(std::fs::read_to_string(&readme), &readme)?.replace("{version}", version());
    write(&dir.join("LEIA-ME.txt"), crlf(&text))?;

    let zip = out.join(format!("{name}.zip"));
    if zip.exists() {
        io(std::fs::remove_file(&zip), &zip)?;
    }
    if cfg!(windows) {
        // O tar do Windows 10+ grava zip com -a.
        run_tool(
            Command::new("tar")
                .arg("-a")
                .arg("-cf")
                .arg(&zip)
                .arg(&name)
                .current_dir(&stage),
            "precisa do tar do Windows 10 ou mais novo",
        )?;
    } else {
        run_tool(
            Command::new("zip")
                .args(["-q", "-9", "-r", "-X"])
                .arg(&zip)
                .arg(&name)
                .current_dir(&stage),
            "instale o zip",
        )?;
    }
    io(std::fs::remove_dir_all(&stage), &stage)?;
    Ok(zip)
}

/// Versão no formato do MSI: três números de 0 a 255, 255 e 65535 (o
/// Windows Installer ignora um quarto). O que vier depois de `-` ou `+` (pré-
/// lançamento, metadados de build) fica de fora, porque o MSI só aceita
/// números.
pub fn msi_version(version: &str) -> Result<String, String> {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    let limits = [255u32, 255, 65535];
    if parts.len() != 3 {
        return Err(format!("versão {version}: o MSI precisa de X.Y.Z"));
    }
    for (part, max) in parts.iter().zip(limits) {
        match part.parse::<u32>() {
            Ok(n) if n <= max => {}
            _ => {
                return Err(format!(
                    "versão {version}: {part} não cabe no MSI (máximo {max})"
                ));
            }
        }
    }
    Ok(core.to_string())
}

fn build_msi(root: &Path, out: &Path, exe: &Path) -> Result<PathBuf, String> {
    let product_version = msi_version(version())?;
    let stage = out.join("msi-root");
    fresh_dir(&stage)?;
    stage_windows(root, &stage, exe)?;
    copy(
        &root.join("packaging/icons/postpilot.ico"),
        &stage.join("postpilot.ico"),
    )?;
    let msi = out.join(format!("PostPilot-{}-x86_64.msi", version()));
    run_tool(
        Command::new("wixl")
            .args(["--arch", "x64", "--define"])
            .arg(format!("Version={product_version}"))
            .arg("--define")
            .arg(format!("SourceDir={}", stage.display()))
            .arg("--output")
            .arg(&msi)
            .arg(root.join("packaging/windows/postpilot.wxs")),
        "instale o wixl (pacote wixl no Debian/Ubuntu, msitools no Arch)",
    )?;
    io(std::fs::remove_dir_all(&stage), &stage)?;
    Ok(msi)
}

/// Fim de linha do Windows, para o Bloco de Notas antigo.
pub fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

/// `SHA256SUMS` com o `sha256sum`, quando houver.
fn write_checksums(out: &Path, files: &[PathBuf]) {
    if files.is_empty() {
        return;
    }
    let names: Vec<_> = files.iter().filter_map(|f| f.file_name()).collect();
    match Command::new("sha256sum")
        .args(&names)
        .current_dir(out)
        .output()
    {
        Ok(o) if o.status.success() => {
            let path = out.join("SHA256SUMS");
            if let Err(e) = std::fs::write(&path, &o.stdout) {
                eprintln!("{}: {e}", path.display());
            }
        }
        _ => eprintln!("sha256sum não disponível; sem SHA256SUMS"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Result<DistArgs, String> {
        let v: Vec<String> = s.split_whitespace().map(String::from).collect();
        parse(&v)
    }

    #[test]
    #[cfg(not(windows))]
    fn parses_options() {
        assert_eq!(
            args("").unwrap(),
            DistArgs {
                formats: Format::ALL.to_vec(),
                build: true,
                native: false
            }
        );
        assert_eq!(
            args("--only zip,deb,zip --no-build --native").unwrap(),
            DistArgs {
                formats: vec![Format::Zip, Format::Deb],
                build: false,
                native: true
            }
        );
        assert!(args("--only").is_err());
        assert!(args("--only ,").is_err());
        assert!(args("--only rpm").is_err());
        assert_eq!(args("--only msi").unwrap().formats, vec![Format::Msi]);
        assert_eq!(args("--only arch").unwrap().formats, vec![Format::Arch]);
        assert!(args("--fast").is_err());
    }

    #[test]
    fn finds_the_newest_glibc_version() {
        let elf = b"\0GLIBC_2.2.5\0GLIBC_2.34\0libc.so.6\0GLIBC_2.17\0GLIBC_2.\0";
        assert_eq!(min_glibc(elf).as_deref(), Some("2.34"));
        assert_eq!(
            min_glibc(b"GLIBC_2.3.4\0GLIBC_2.3").as_deref(),
            Some("2.3.4")
        );
        assert_eq!(min_glibc(b"nada aqui"), None);
    }

    #[test]
    fn control_has_versioned_libc_and_folded_description() {
        let c = deb_control("1.2.3", 4096, Some("2.34"));
        assert!(c.contains("Version: 1.2.3\n"));
        assert!(c.contains("Installed-Size: 4096\n"));
        assert!(c.contains("Depends: libc6 (>= 2.34), libgcc-s1,"));
        // Toda linha de continuação da descrição começa com espaço, e o
        // arquivo termina com uma quebra de linha.
        let desc = c.split("Description:").nth(1).unwrap();
        for line in desc.lines().skip(1) {
            assert!(line.starts_with(' '), "{line:?}");
        }
        assert!(c.ends_with('\n'));
        assert!(deb_control("1", 1, None).contains("Depends: libc6, "));
    }

    #[test]
    fn pkginfo_lists_version_size_and_dependencies() {
        let p = arch_pkginfo("1.2.3", 4096, 1_700_000_000);
        assert!(p.contains("\npkgver = 1.2.3-1\n"));
        assert!(p.contains("\nsize = 4096\n"));
        assert!(p.contains("\nbuilddate = 1700000000\n"));
        assert!(p.contains("\ndepend = libxkbcommon-x11\n"));
        assert!(p.contains("\noptdepend = xdg-desktop-portal: "));
        assert!(p.ends_with('\n'));
    }

    /// O `PKGBUILD` e o pacote do xtask têm de concordar na versão e nas
    /// dependências.
    #[test]
    fn claude_plugin_is_staged_with_its_marketplace() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let dest = std::env::temp_dir().join(format!("postpilot-plugin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dest);
        install_claude_plugin(root, &dest).unwrap();
        for file in [
            ".claude-plugin/marketplace.json",
            "claude-plugin/.claude-plugin/plugin.json",
            "claude-plugin/.mcp.json",
            "claude-plugin/skills/postpilot/SKILL.md",
            "claude-plugin/commands/optimize.md",
        ] {
            assert!(dest.join(file).is_file(), "{file}");
        }
        let market = std::fs::read_to_string(dest.join(".claude-plugin/marketplace.json")).unwrap();
        assert!(
            market.contains("\"source\": \"./claude-plugin\""),
            "{market}"
        );
        let pkgbuild = std::fs::read_to_string(root.join("packaging/arch/PKGBUILD")).unwrap();
        assert!(
            pkgbuild.contains(&format!("/usr/{LINUX_PLUGIN_DIR}/")),
            "PKGBUILD sem o plugin"
        );
        let _ = std::fs::remove_dir_all(&dest);
    }

    #[test]
    fn pkgbuild_matches_the_xtask_package() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let pkgbuild = std::fs::read_to_string(root.join("packaging/arch/PKGBUILD")).unwrap();
        assert!(pkgbuild.contains(&format!("\npkgver={}\n", version())));
        assert!(pkgbuild.contains(&format!("\npkgrel={ARCH_PKGREL}\n")));
        let array = |name: &str| -> Vec<String> {
            let start = pkgbuild.find(&format!("\n{name}=(")).unwrap() + name.len() + 3;
            // O array fecha com `)` sozinho numa linha.
            let end = start + pkgbuild[start..].find("\n)").unwrap();
            pkgbuild[start..end]
                .lines()
                .map(|l| l.trim().trim_matches('\'').to_string())
                .filter(|l| !l.is_empty())
                .collect()
        };
        assert_eq!(array("depends"), ARCH_DEPENDS);
        assert_eq!(array("optdepends"), ARCH_OPTDEPENDS);
    }

    #[test]
    fn msi_version_is_three_small_numbers() {
        assert_eq!(msi_version("0.1.0").unwrap(), "0.1.0");
        assert_eq!(msi_version("1.2.3-beta.1+abc").unwrap(), "1.2.3");
        assert_eq!(msi_version("255.255.65535").unwrap(), "255.255.65535");
        assert!(msi_version("256.0.0").is_err());
        assert!(msi_version("1.2").is_err());
        assert!(msi_version("1.2.3.4").is_err());
        assert!(msi_version("1.x.3").is_err());
    }

    /// O .wxs usa as variáveis que o `build_msi` passa, e o UpgradeCode não
    /// pode mudar (sem ele, a versão nova não substitui a velha).
    #[test]
    fn wxs_uses_the_xtask_variables_and_a_fixed_upgrade_code() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let wxs = std::fs::read_to_string(root.join("packaging/windows/postpilot.wxs")).unwrap();
        assert!(wxs.contains("Version=\"$(var.Version)\""));
        for file in [
            "postpilot.exe",
            "postpilot.ico",
            "LICENSE-MIT.txt",
            "LICENSE-APACHE.txt",
        ] {
            assert!(wxs.contains(&format!("$(var.SourceDir)/{file}")), "{file}");
        }
        assert!(wxs.contains("UpgradeCode=\"6B0E9A3C-52F4-4C6D-9B1A-7E2D3F8C4A51\""));
        assert!(wxs.contains("<MajorUpgrade "));
    }

    #[test]
    fn aur_bin_points_at_the_release_and_lists_the_dependencies() {
        let (pkgbuild, srcinfo) = aur_bin("1.2.3", &"a".repeat(64));
        assert!(pkgbuild.contains("\npkgname=postpilot-bin\n"));
        assert!(pkgbuild.contains("\npkgver=1.2.3\n"));
        assert!(pkgbuild.contains(
            "source=(\"postpilot-1.2.3-linux-x86_64.tar.gz::$url/releases/download/v$pkgver/postpilot-1.2.3-linux-x86_64.tar.gz\")"
        ));
        assert!(pkgbuild.contains("\n  'libxkbcommon-x11'\n"));
        assert!(pkgbuild.contains("\n  cp -a \"$srcdir/usr\" \"$pkgdir/\"\n}\n"));
        assert!(srcinfo.starts_with("pkgbase = postpilot-bin\n\tpkgdesc = "));
        assert!(srcinfo.contains(&format!("\tsha256sums = {}\n", "a".repeat(64))));
        assert!(srcinfo.contains(
            "\tsource = postpilot-1.2.3-linux-x86_64.tar.gz::https://github.com/tiagosouzaf/postpilot/releases/download/v1.2.3/postpilot-1.2.3-linux-x86_64.tar.gz\n"
        ));
        assert!(srcinfo.ends_with("\n\npkgname = postpilot-bin\n"));
    }

    #[test]
    fn crlf_does_not_double_carriage_returns() {
        assert_eq!(crlf("a\nb\r\nc"), "a\r\nb\r\nc");
    }

    #[test]
    fn packaging_files_exist() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for (_, png) in icons(root) {
            assert!(png.exists(), "{}", png.display());
        }
        let desktop =
            std::fs::read_to_string(root.join("packaging/linux/postpilot.desktop")).unwrap();
        assert!(desktop.contains("Exec=postpilot %F"));
        assert!(desktop.contains("Icon=postpilot\n"));
        assert!(desktop.contains("StartupWMClass=postpilot\n"));
        assert!(root.join("packaging/windows/LEIA-ME.txt").exists());
        assert!(root.join("packaging/windows/postpilot.wxs").exists());
        assert!(root.join("packaging/icons/postpilot.ico").exists());
    }
}
