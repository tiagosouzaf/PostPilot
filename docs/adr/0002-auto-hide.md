# ADR 0002: auto-hide de painéis (pin/unpin)

- **Status:** aceita (Tiago, 2026-11-13); implementada, completada no marco 6.4
- **Data:** 2026-11-13

## Contexto

No SSMS, o alfinete de um painel (Object Explorer, Properties, Output) o
esconde numa faixa na borda da janela; passar o mouse ou clicar na faixa
mostra o painel por cima dos documentos, e ele some de novo ao perder o foco.
O plano previa investigar isso no marco 1.4.

O `egui_dock` 0.21 não tem auto-hide. O mais próximo é recolher um grupo de
abas até sobrar só a barra de abas (`show_leaf_collapse_buttons`), que não
libera o espaço da coluna nem mostra o painel por cima dos documentos.

## Opções

1. **Implementar na crate `pgstudio-docking`.** Um painel "desafixado" sai do
   `DockState` e vai para uma lista por borda (esquerda, direita, baixo). A
   casca desenha a faixa com um botão vertical por painel e, ao clicar, mostra
   o painel num `egui::Area` deslizando sobre a área central, com largura
   própria e fechamento ao perder o foco. Fixar de novo devolve o painel para a
   área de origem com `Workspace::open`. Estimativa: 2 a 3 dias, incluindo
   persistência no marco 1.7 e testes.
2. **Contribuir no `egui_dock`.** Mais limpo a longo prazo, mas depende do
   ritmo do projeto e mantém o PGStudio esperando.
3. **Não ter auto-hide.** Fechar o painel e reabrir pelo menu View (já
   funciona) cobre o uso básico, com menos fidelidade ao SSMS.

## Decisão

Opção 1, feita depois do marco 1.8, antes da Fase 2.

## Como ficou

- `Workspace` guarda os painéis desafixados fora do `DockState`. Cada um vai
  para a faixa da sua área de origem (esquerda, direita ou baixo); documentos
  não são desafixados.
- A faixa mostra um botão por painel, com o texto na vertical nas laterais.
  Clicar abre o painel por cima dos documentos, encostado na faixa (300 px, ou
  80% do espaço se for menor). Clicar de novo, apertar Esc ou clicar nos
  documentos e nos painéis encaixados esconde o painel. Menus e toolbar não
  escondem, para o Window > Auto Hide funcionar com o painel aberto.
- Desafixar: "Auto Hide" no menu de contexto da aba, Window > Auto Hide (no
  painel com foco) ou Window > Auto Hide All. Fixar de novo: o alfinete no
  cabeçalho do painel aberto ou Window > Auto Hide com ele aberto. View > o
  painel desafixado abre o painel por cima.
- O `layout.json` guarda a lista `hidden`; arquivos sem ela continuam válidos
  (versão 1).

## Complemento (marco 6.4)

O que tinha ficado de fora foi feito no marco 6.4, também na
`pgstudio-docking`:

- **Abrir com o mouse.** Parar o mouse sobre o botão da faixa por 0,3 s abre o
  painel, como no SSMS. Aberto assim, ele some 0,5 s depois que o mouse sai do
  painel e do botão. Clicar dentro dele (ou no botão, ou arrastar a borda)
  faz o painel ficar até perder o foco, como se tivesse sido aberto com
  clique. Depois de fechar com o mouse parado no botão, o painel só reabre
  quando o mouse sai e volta.
- **Deslizar.** O painel desliza a partir da faixa ao abrir e volta para ela
  ao esconder (0,15 s, com desaceleração). Com as animações do tema
  desligadas (`animation_time` zero), aparece e some de uma vez. Fixar ou
  fechar o painel não anima, porque ele já foi para outro lugar.
- **Redimensionar.** A borda que dá para os documentos é uma alça: arrastar
  muda a largura (ou a altura, embaixo), entre 120 px e 90% do espaço. O
  tamanho é de cada painel e fica no `layout.json` em `flyout_sizes`; arquivos
  sem o campo continuam válidos e usam o padrão (300 px, ou 80% do espaço se
  for menor). Tamanhos inválidos no arquivo são ignorados.
