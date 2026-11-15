//! Atualização do app pelos Releases do GitHub.
//!
//! A consulta é um `GET` em `api.github.com/repos/<repo>/releases/latest`,
//! sem nada além do `User-Agent` com o nome e a versão do app (a API do
//! GitHub recusa pedidos sem ele). Nenhum identificador da máquina, do
//! usuário ou das conexões vai junto.
//!
//! O que fazer com uma versão nova depende de como o app foi instalado
//! ([`InstallKind`]):
//!
//! - **AppImage** e **Windows** (`.msi` ou `.zip`): o app baixa o arquivo do
//!   Release, confere o SHA-256 contra o `SHA256SUMS` do mesmo Release e se
//!   atualiza ([`install`]).
//! - **Pacote do sistema** (pacman/AUR, `.deb`): os arquivos são do
//!   gerenciador de pacotes, então o app não se sobrescreve; ele só avisa e
//!   mostra o comando ([`InstallKind::update_command`]): o
//!   `yay` para o pacote do AUR, o instalador ([`INSTALL_COMMAND`]) para o
//!   pacote do Release e o `.deb`.
//! - **Outros** (`cargo build`, `cargo install`): só avisa, com o link.
//!
//! O `SHA256SUMS` protege contra download corrompido ou trocado no caminho,
//! não contra quem conseguir publicar um Release no repositório: os pacotes
//! ainda não são assinados.

mod install;
mod net;
mod release;
mod settings;

pub use install::{
    AUR_PACKAGE, INSTALL_COMMAND, InstallKind, Installed, aur_helper, cleanup_old_binary, install,
};
pub use net::{Progress, download_verified, fetch_latest};
pub use release::{Asset, Release, ReleaseError, parse_release, parse_sha256sums};
pub use settings::UpdateSettings;

/// Repositório consultado, no formato `dono/nome`.
pub const REPOSITORY: &str = "tiagosouzaf/postpilot";

/// Página dos Releases, para abrir no navegador.
pub fn releases_page() -> String {
    format!("https://github.com/{REPOSITORY}/releases")
}
