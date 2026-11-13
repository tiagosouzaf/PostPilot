# Spikes da Fase 0

Protótipos descartáveis para escolher o toolkit de GUI do PGStudio. Não fazem
parte do workspace e não viram base do produto. A decisão fica em
[`docs/adr/0001-gui-toolkit.md`](../docs/adr/0001-gui-toolkit.md).

Os três implementam o mesmo cenário: menus, toolbar, status bar, Object Explorer
com 10 mil nós carregados sob demanda por um provedor falso (120 ms de atraso),
grade virtualizada com 1 milhão de linhas e editor com 5 mil linhas de SQL.

| Diretório | Toolkit |
| --- | --- |
| `egui/` | egui 0.36 + eframe + egui_dock 0.21 |
| `slint/` | Slint 1.18 |
| `gpui/` | GPUI 0.2 (crates.io) |

## Rodar

```sh
cd spikes/egui && cargo run --release
```

## Medir

`SPIKE_BENCH=1` faz o protótipo medir e sair sozinho: tempo até o primeiro frame,
intervalo entre frames rolando a grade por 300 passos e memória (RSS no Linux,
working set no Windows) após `SPIKE_IDLE_SECS` segundos parado (padrão 30).

```sh
./bench-linux.sh 3 10   # 3 execuções de cada, 10 s em repouso, sob Xvfb
```

O workflow `.github/workflows/spikes.yml` roda o mesmo em Linux e Windows no CI.
