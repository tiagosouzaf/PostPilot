# ADR 0002: auto-hide de painéis (pin/unpin)

- **Status:** proposta (aguardando o Tiago)
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

## Decisão proposta

Opção 1, depois do marco 1.8, para não atrasar a Fase 1. O `Workspace` já
isola o `egui_dock`, então o auto-hide entra sem mudar a casca. Até lá, o
botão de fechar e o menu View fazem o papel do alfinete.
