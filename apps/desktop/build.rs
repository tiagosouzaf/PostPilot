//! Põe o ícone e a versão no `postpilot.exe` quando o alvo é Windows com o
//! toolchain GNU (a compilação cruzada de `cargo xtask dist`).
//!
//! Usa o `windres` do MinGW (`x86_64-w64-mingw32-windres`, ou o da variável
//! `WINDRES`). Sem ele, ou com o toolchain MSVC, o exe sai sem ícone e o
//! build só avisa: o ícone da janela vem do PNG embutido de qualquer jeito.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os != "windows" {
        return;
    }
    if env != "gnu" {
        println!("cargo:warning=exe sem ícone: o recurso só é gerado com o toolchain GNU");
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let icon = manifest.join("../../packaging/icons/postpilot.ico");
    println!("cargo:rerun-if-changed={}", icon.display());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let numbers: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    let (major, minor, patch) = (
        numbers.first().copied().unwrap_or(0),
        numbers.get(1).copied().unwrap_or(0),
        numbers.get(2).copied().unwrap_or(0),
    );
    // O windres lê o caminho entre aspas; barras normais servem no Windows.
    let icon_path = icon.display().to_string().replace('\\', "/");
    let rc = format!(
        r#"1 ICON "{icon_path}"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "PostPilot"
      VALUE "FileDescription", "PostPilot"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "postpilot"
      VALUE "OriginalFilename", "postpilot.exe"
      VALUE "ProductName", "PostPilot"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
    );
    let rc_path = out.join("postpilot.rc");
    std::fs::write(&rc_path, rc).expect("escrever postpilot.rc");
    let obj = out.join("pgstudio-res.o");
    let windres = std::env::var("WINDRES").ok().map_or_else(
        || {
            ["x86_64-w64-mingw32-windres", "windres"]
                .into_iter()
                .find(|w| Command::new(w).arg("--version").output().is_ok())
                .map(str::to_string)
        },
        Some,
    );
    let Some(windres) = windres else {
        println!("cargo:warning=exe sem ícone: windres não encontrado");
        return;
    };
    let status = Command::new(&windres)
        .arg("--input")
        .arg(&rc_path)
        .arg("--output-format=coff")
        .arg("--output")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg-bins={}", obj.display());
        }
        Ok(s) => println!("cargo:warning=exe sem ícone: {windres} saiu com {s}"),
        Err(e) => println!("cargo:warning=exe sem ícone: {windres}: {e}"),
    }
}
