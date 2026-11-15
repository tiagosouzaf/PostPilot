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
//! - **Pacote do Release** (o `postpilot` do pacman e o `.deb`): o app baixa
//!   o pacote, confere o SHA-256 como o usuário e só a instalação roda como
//!   administrador, com a senha pedida pelo `pkexec` ([`find_pkexec`]). Sem
//!   `pkexec`, ele mostra o comando do instalador ([`INSTALL_COMMAND`]).
//! - **Pacote do AUR**: os arquivos são do `yay`/`paru`, então o app só avisa
//!   e mostra o comando ([`InstallKind::update_command`]).
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
    AUR_PACKAGE, CANCELLED, INSTALL_COMMAND, InstallKind, Installed, NOT_AUTHORIZED, aur_helper,
    cleanup_old_binary, find_pkexec, install,
};
pub use net::{Progress, download_verified, fetch_latest, fetch_latest_from_sums};
pub use release::{
    Asset, Release, ReleaseError, parse_release, parse_sha256sums, release_from_sums,
};
pub use settings::{CHECK_INTERVAL, UpdateSettings};

/// Repositório consultado, no formato `dono/nome`.
pub const REPOSITORY: &str = "tiagosouzaf/postpilot";

/// Página dos Releases, para abrir no navegador.
pub fn releases_page() -> String {
    format!("https://github.com/{REPOSITORY}/releases")
}
