# ADR 0001: toolkit de GUI

- **Status:** proposta (aguardando as medições no Windows e a revisão do Tiago)
- **Data:** 2026-11-13

## Contexto

O PGStudio precisa reproduzir o SSMS com fidelidade: docking com abas e janelas
flutuantes, Object Explorer grande, grade de resultados virtualizada e, com
prioridade máxima, um editor SQL com IntelliSense. Precisa rodar em Windows e
Linux, sem WebView, com inicialização abaixo de 1 s e pouca RAM em repouso.

Os candidatos eram egui, Slint e GPUI. Fizemos o mesmo protótipo nos três
(`spikes/`) e medimos no mesmo ambiente.

## Medições no Linux

Ambiente: contêiner Linux, 4 vCPUs, sem GPU, Xvfb com Mesa (llvmpipe/lavapipe).
Mediana de 3 execuções. Os números servem para comparar os toolkits entre si;
em máquina com GPU todos devem ficar bem abaixo disso.

| Protótipo | Primeiro frame | Intervalo entre frames rolando 1M linhas (p50 / p95) | Memória em repouso | Binário (release) |
| --- | --- | --- | --- | --- |
| egui + wgpu | 106 ms | 16,9 / 20,5 ms | 157 MB | 22 MB |
| egui + glow | 114 ms | 14,1 / 17,4 ms | 143 MB | 22 MB |
| Slint (femtovg) | 220 ms | 16,0 / 18,2 ms | 165 MB | (a medir no CI) |
| GPUI | não mediu | não mediu | não mediu | (a medir no CI) |

A memória inclui o driver de renderização por software do Mesa, que é grande; a
meta de 100 MB precisa ser verificada numa máquina com GPU. Os ~16 ms de
intervalo parecem ser o ritmo de 60 Hz do Xvfb: os dois toolkits sustentam 60
FPS rolando a grade.

Medições no Windows: a preencher com o resultado do workflow
`.github/workflows/spikes.yml`.

## O que cada protótipo mostrou

**egui (egui_dock, egui_extras)**

- Cobre todo o cenário: docking com abas, arrastar e janelas flutuantes
  (`egui_dock`), grade de 1 milhão de linhas (`TableBuilder`), árvore
  virtualizada com `show_rows`.
- Editor: `TextEdit` com realce por `layouter` e popup de autocompletar
  posicionado no cursor funcionaram no protótipo. Com 5 mil linhas o realce
  precisa de cache por conteúdo; um editor de verdade vai exigir buffer próprio.
- Pontos fracos: `egui_dock` 0.21 depende de egui 0.36, uma versão atrás da
  atual (0.37), então o projeto fica preso ao ritmo do `egui_dock`. Glifos fora
  da fonte embutida (ex.: ▸) aparecem como quadrados; ícones precisam de fonte
  ou imagens próprias. Visual "de ferramenta", que exige tema próprio para
  lembrar o SSMS.

**Slint**

- Menus nativos com atalhos (`MenuBar`), lista virtualizada com modelo sob
  demanda e visual mais polido de fábrica.
- Não tem docking: o protótipo usa layout fixo. Docking teria de ser construído
  do zero.
- `TextEdit` não aceita realce de sintaxe nem expõe a posição do cursor em
  pixels, o que bloqueia o popup do IntelliSense sem um editor próprio.
- Licença: GPLv3, royalty-free (com exigência de atribuição) ou comercial.
  Compatível com um app MIT/Apache, mas adiciona obrigações.
- Pegadinha encontrada: `TextEdit` direto dentro de `Tab` não aparece; precisa
  de um layout em volta.

**GPUI**

- A versão do crates.io (0.2.2) não compila sem remendo: depende de `xattr`
  0.2.3, incompatível com a `libc` atual. O spike usa um patch local.
- Não traz campo de texto editável (o exemplo oficial de input tem ~750 linhas),
  nem docking; `set_menus` só vira menu nativo no macOS.
- No Linux sem GPU (Xvfb + lavapipe) desenhou um frame e não apresentou mais
  nenhum, então não foi possível medir. O CI no Windows vai mostrar se roda lá.

## Decisão proposta

Usar **egui + eframe + egui_dock**, com renderizador **wgpu** como padrão e
glow como alternativa. É o único dos três que entrega docking pronto e onde o
popup de IntelliSense funcionou sem construir um editor do zero; desempenho e
memória ficaram equivalentes ou melhores que os do Slint no mesmo ambiente.

## Consequências

- A crate `docking` isola o `egui_dock`, para podermos seguir egui mais rápido
  (fork ou implementação própria) se o atraso de versão incomodar.
- A crate `sql-editor` terá buffer próprio (ex.: `ropey`) e realce incremental
  (ex.: `tree-sitter`), usando o `TextEdit` só como ponto de partida.
- Tema próprio e fonte de ícones entram já no marco 1.2.
- Reavaliar se as medições do Windows contradisserem as do Linux.
