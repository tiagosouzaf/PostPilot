# Na sua máquina

O que não deu para testar sem uma máquina de verdade: medir com GPU, usar no
Hyprland e instalar num Windows. Cada parte tem os comandos prontos e uma
lista curta do que conferir. O resultado vai num arquivo só,
`~/pgstudio-relatorio.md`; mande esse arquivo (ou cole o conteúdo) no projeto,
e os números entram em [`benchmarks.md`](benchmarks.md) e as falhas viram
correções.

A parte de medir foi rodada de ponta a ponta num container Linux sem GPU (Xvfb e
Mesa por software); o que pede Hyprland ou Windows não dá para rodar aqui. Leva cerca de 30 minutos no
Linux e 20 no Windows.

## 1. Linux com GPU (Omarchy)

### Preparar

```sh
sudo pacman -S --needed rustup base-devel git xdg-desktop-portal-gtk wl-clipboard pciutils
rustup default stable
git clone https://github.com/tiagosouzaf/postpilot.git
cd postpilot
```

O `xdg-desktop-portal-gtk` é o que faz File > Open abrir: o portal do Hyprland
não tem seletor de arquivos. Para instalar o PGStudio como pacote (opcional),
`cd packaging/arch && makepkg -si && cd ../..`.

### Medir

```sh
R=~/pgstudio-relatorio.md
{
  echo "# Relatório do PGStudio"
  echo
  echo "- Data: $(date -I)"
  echo "- Sistema: $(uname -sr), $(. /etc/os-release; echo "$PRETTY_NAME")"
  echo "- CPU: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
  echo "- GPU: $(lspci 2>/dev/null | grep -iE 'vga|3d' | cut -d: -f3 | xargs)"
  echo "- Sessão: $XDG_SESSION_TYPE, $XDG_CURRENT_DESKTOP"
  echo
  echo "## wgpu (padrão)"
  echo
  cargo xtask bench --markdown
  echo
  echo "## glow"
  echo
  PGSTUDIO_RENDERER=glow cargo xtask bench --markdown --no-build
} 2>&1 | tee "$R"
```

São os 10 cenários com 5 execuções cada, nos dois renderizadores, com a
janela abrindo e fechando sozinha (não mexa nela enquanto roda). O que mais
interessa, porque no container ficou acima da meta só pelo renderizador por
software:

| Medida | Meta | Sem GPU (container) |
| --- | --- | --- |
| RAM em repouso | < 100 MB | 129 MB |
| Diagrama de 2.000 tabelas, frame p95 | < 20 ms | 21,6 ms |
| Plano de 2.000 nós, frame p95 | < 20 ms | 20,3 a 24,7 ms |
| Exportação, frame p95 | < 20 ms | 22,5 ms |

Se a RAM em repouso continuar acima de 100 MB com GPU, compare o wgpu com o
glow: é o dado que decide se vale trocar o renderizador padrão. Sem GPU, o
glow foi mais rápido em todos os cenários e gastou 11 MB a menos (veja o
marco 6.8 em [`benchmarks.md`](benchmarks.md)).

### Conferir no Hyprland

Abra o app (`cargo run --release -p pgstudio-desktop`, ou `pgstudio` se
instalou o pacote), conecte num PostgreSQL e anote no fim do relatório o que
falhar:

1. Help > About mostra o renderizador e a placa (não `llvmpipe`).
2. Com escala fracionária (`monitor = ,preferred,auto,1.25` no
   `hyprland.conf`), o texto fica nítido e os cliques caem onde o mouse está.
3. Mouse: arrastar um painel para outro lugar, passar o mouse na faixa de um
   painel desafixado (abre em 0,3 s e some ao sair), arrastar uma tabela no
   diagrama e uma coluna até outra no Query Designer.
4. Copiar do editor e colar em outro app, e o contrário (`wl-copy`/`wl-paste`
   funcionam como teste).
5. File > Open e File > Save As abrem o seletor de arquivos.
6. Uma regra de janela do Hyprland pela class `pgstudio` (por exemplo, deixar
   a janela flutuante) pega a janela do PGStudio; `hyprctl clients` mostra
   `class: pgstudio`.
7. `cargo xtask pgmatrix --versions 17` (opcional): roda a suíte no
   PostgreSQL do Arch.

## 2. Windows

Não precisa de Rust para conferir o instalador; só para medir.

### Instalar e conferir

Baixe `PGStudio-0.1.0-x86_64.msi` (em `pgstudio/pacotes` nos arquivos do
projeto, ou gere com `cargo xtask dist --only msi` no Linux) e:

1. Dois cliques no `.msi`. O SmartScreen avisa porque não é assinado: "Mais
   informações" > "Executar assim mesmo". Ele pede elevação e instala sem
   telas.
2. O atalho aparece no menu Iniciar com o ícone (no menu de todos os
   usuários, não só no seu: no Wine ele foi para o do usuário).
3. O app abre pelo atalho, sem janela de console, e Help > About mostra
   Direct3D 12 (ou Vulkan) e a placa.
4. Conecte num PostgreSQL, rode uma consulta, abra um diagrama e o Activity
   Monitor.
5. Botão direito num `.sql` > Abrir com: o PGStudio aparece e abre o arquivo
   numa aba.
6. Configurações > Aplicativos > PGStudio > Desinstalar remove o atalho e a
   pasta `C:\Program Files\PGStudio`.

### Medir

Com o [Rust](https://rustup.rs) instalado (ele pede o Visual Studio Build
Tools), no PowerShell:

```powershell
git clone https://github.com/tiagosouzaf/postpilot.git
cd postpilot
cargo xtask bench --markdown | Tee-Object -FilePath $HOME\pgstudio-relatorio-windows.md
```

Anote no fim do arquivo o que falhou nos passos acima e mande junto com o do
Linux.

## Onde os números entram

Cada relatório vira uma seção nova em [`benchmarks.md`](benchmarks.md)
("Linux com GPU" e "Windows"), com a tabela que o `--markdown` imprime e uma
nota sobre o que passou ou não da meta. O que falhar na lista de conferência
vira uma correção com teste.
