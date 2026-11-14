# Medições

Números do `pgstudio` contra as metas do projeto. Gerados com
`cargo xtask bench`, que compila o app em release, abre e fecha o app várias
vezes e mede três cenários (`--scenario startup|grid|intellisense|all`):

- **startup, até o primeiro frame:** do início do processo até o primeiro
  frame exibido (o app avisa pela saída padrão no modo `PGSTUDIO_BENCH=1`).
  Inclui carregar o binário, criar a janela, iniciar a GPU e desenhar a casca
  inteira.
- **startup, RAM em repouso:** RSS no Linux e working set no Windows, lidos de
  fora do processo depois de 30 s parado, com o Object Explorer de
  demonstração conectado.
- **grid:** com `PGSTUDIO_BENCH=grid`, o app abre uma janela de consulta e
  executa uma consulta sintética de 1 milhão de linhas (6 colunas, parecidas
  com uma tabela de clientes) pelo mesmo caminho de uma consulta real: as
  linhas vão de uma thread para o buffer do resultado, e a grade mostra o que
  chegou. Depois a grade rola do começo ao fim em 10 s (cerca de 600 frames a 60 FPS). Mede o tempo
  até as linhas chegarem, o intervalo entre frames (o que a tela mostra), o
  tempo de CPU da interface em cada frame e a memória com o resultado
  carregado.
- **intellisense:** com `PGSTUDIO_BENCH=intellisense`, o app abre uma janela
  de consulta no banco `stress` do servidor de demonstração (10 mil tabelas,
  o dobro da meta), espera o catálogo e digita quatro consultas, uma tecla
  por frame, com a lista de sugestões aberta depois de `FROM`, `JOIN` e
  `t.`. Mede o tempo de cada frame com tecla, que inclui recalcular as
  sugestões, refazer o realce e desenhar a lista.

Os cenários rodam no app de verdade, com a janela, a GPU (ou o renderizador
por software) e o servidor de demonstração, sem PostgreSQL. Ao terminar, o
app escreve uma linha `pgstudio-bench: result chave=valor ...`, e o xtask lê
a memória do processo nesse momento.

O CI roda a medição em Linux e Windows em todo PR (job "Medição"); o resultado
aparece no resumo do job e como artefato `bench-<sistema>`. Por enquanto as
metas não reprovam o CI: os runners não têm GPU e variam de uma execução para
outra. `cargo xtask bench --check` sai com erro se alguma meta não for
atingida.

```sh
cargo xtask bench                    # todos os cenários, 5 execuções cada
cargo xtask bench --runs 3 --idle 10 # mais rápido
cargo xtask bench --scenario grid    # só a grade
xvfb-run -a cargo xtask bench        # Linux sem tela
```

## Metas

| Medida | Meta |
| --- | --- |
| Inicialização a frio | < 1 s |
| RAM em repouso | < 100 MB |
| Grade: rolando 1 milhão de linhas | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Grade: memória | limitada pelo buffer: < 96 MB a mais que o repouso |
| IntelliSense: 5 mil tabelas | sugestões em < 50 ms (pior tecla) |

- **60 FPS como p95 < 20 ms:** um frame a 60 Hz dura 16,7 ms. Com vsync, o
  intervalo medido oscila em volta disso mesmo sem nenhum frame perdido, e um
  frame perdido dobra o intervalo (33 ms). Os 20 ms separam as duas coisas.
- **Memória da grade:** cada resultado guarda até 64 MB em memória e manda o
  resto para um arquivo temporário. A meta é esse buffer mais 32 MB de folga
  para as linhas visíveis, os textos desenhados e as estruturas da grade,
  medida contra a RAM em repouso do mesmo `cargo xtask bench`.

## Resultados

### Marco 1.8 (fim da Fase 1)

Medido no CI do [PR #8](https://github.com/tiagosouzaf/PostPilot/pull/8)
(runners do GitHub, sem GPU), 5 execuções, 30 s em repouso
([execução do CI](https://github.com/tiagosouzaf/PostPilot/actions/runs/43471960170)).

| Sistema | Até o 1º frame (mediana) | Até o 1º frame (pior) | RAM em repouso (mediana) | RAM em repouso (pior) |
| --- | --- | --- | --- | --- |
| Windows (`windows-latest`, WARP) | 396 ms ✅ | 709 ms ✅ | 54.4 MB ✅ | 56.1 MB ✅ |
| Linux (`ubuntu-latest`, Xvfb + lavapipe) | 108 ms ✅ | 1372 ms ❌ | 116.0 MB ❌ | 124.1 MB ❌ |

- **Windows atinge as duas metas**, mesmo sem GPU. A primeira execução
  (709 ms) é a mais lenta, como esperado de uma partida a frio.
- **Linux, inicialização:** só a primeira execução passa de 1 s (1372 ms); as
  outras ficam em 105 a 109 ms. A diferença é a primeira carga do driver
  Vulkan por software e do disco do runner. Ainda conta como fora da meta até
  ser medido num Linux com GPU.
- **Linux, RAM:** 116 MB em repouso, acima da meta. O driver Vulkan por
  software (lavapipe) fica com boa parte disso: no mesmo ambiente, o protótipo
  egui da Fase 0 ficou entre 141 e 152 MB, e no Windows o mesmo app usa
  54 MB. Precisa ser medido num Linux com GPU antes de decidir se há trabalho
  a fazer (por exemplo, testar o renderizador `glow`).

### Marco 2.8 (fim da Fase 2)

Linux medido no CI do [PR #18](https://github.com/tiagosouzaf/PostPilot/pull/18)
(`ubuntu-latest`, Xvfb + lavapipe, sem GPU), 5 execuções de cada cenário, 30 s
em repouso ([execução do CI](https://github.com/tiagosouzaf/PostPilot/actions/runs/43540649190)):

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Até o primeiro frame | < 1 s | 123 ms | 3311 ms | ❌ acima |
| RAM em repouso | < 100 MB | 117.0 MB | 124.7 MB | ❌ acima |
| Grade: 1 milhão de linhas chegam | — | 652.7 ms | 675.5 ms | — |
| Grade: frame rolando (p95) | < 20 ms (60 FPS) | 28.6 ms | 39.2 ms | ❌ acima |
| Grade: CPU da interface por frame (p95) | — | 0.6 ms | 0.8 ms | — |
| Grade: RAM a mais que o repouso | < 96 MB | 78.1 MB | 78.2 MB | ✅ dentro |
| IntelliSense: catálogo de 10 mil tabelas carregado | — | 292.4 ms | 294.5 ms | — |
| IntelliSense: tecla (p95) | — | 2.5 ms | 3.1 ms | — |
| IntelliSense: pior tecla | < 50 ms | 3.5 ms | 4.6 ms | ✅ dentro |
| IntelliSense: RAM com o catálogo | — | 128.2 MB | 128.5 MB | — |

- **Grade rolando acima da meta no CI:** a interface gasta 0,6 ms por frame;
  os 28 ms são o lavapipe desenhando a janela nos 2 núcleos do runner. Fica
  como "precisa de medição com GPU", como a RAM em repouso no marco 1.8.
- **Primeiro frame:** só a primeira execução (3,3 s, disco frio do runner)
  passou de 1 s; as outras ficaram em 121 a 124 ms.
- **Windows:** a partida e a RAM ficaram como no marco 1.8 (mediana 407 ms e
  54,6 MB). O cenário da grade passou do limite de 180 s do xtask, porque a
  rolagem era de 600 frames fixos e o WARP desenha devagar; agora a rolagem
  dura 10 s em qualquer máquina. Os números de grade e IntelliSense no
  Windows ainda não foram medidos: os minutos do GitHub Actions acabaram, e
  medir exige rodar `cargo xtask bench` numa máquina Windows.

Medido localmente (Linux x86_64 num container, Xvfb + lavapipe, sem GPU),
3 execuções de cada cenário, 10 s em repouso:

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Até o primeiro frame | < 1 s | 100 ms | 113 ms | ✅ dentro |
| RAM em repouso | < 100 MB | 128.2 MB | 128.2 MB | ❌ acima |
| Grade: 1 milhão de linhas chegam | — | 358.4 ms | 375.6 ms | — |
| Grade: frame rolando (p95) | < 20 ms (60 FPS) | 16.1 ms | 17.1 ms | ✅ dentro |
| Grade: CPU da interface por frame (p95) | — | 0.7 ms | 0.7 ms | — |
| Grade: RAM a mais que o repouso | < 96 MB | 77.2 MB | 78.1 MB | ✅ dentro |
| IntelliSense: catálogo de 10 mil tabelas carregado | — | 251.4 ms | 255.3 ms | — |
| IntelliSense: tecla (p95) | — | 2.9 ms | 3.1 ms | — |
| IntelliSense: pior tecla | < 50 ms | 3.7 ms | 4.5 ms | ✅ dentro |
| IntelliSense: RAM com o catálogo | — | 139.1 MB | 139.5 MB | — |

- **Grade:** a interface gasta menos de 1 ms por frame rolando 1 milhão de
  linhas; o resto do intervalo (13 ms na mediana) é o renderizador por
  software desenhando a janela de 1280×800. Com GPU, o intervalo deve ficar
  preso ao vsync.
- **IntelliSense:** a pior tecla leva 4,5 ms com 10 mil tabelas, dez vezes
  abaixo da meta, contando realce, sugestões e desenho.
- **RAM em repouso** segue acima da meta no Linux sem GPU, como no marco 1.8.

## Como ler

- O pior caso conta para a meta, não a mediana.
- A primeira execução é a mais parecida com uma partida a frio; as seguintes
  aproveitam o cache de disco do sistema.
- Sem GPU, o Linux usa renderização por software (lavapipe ou llvmpipe) e o
  Windows usa o WARP, que gastam mais memória e CPU do que um driver de
  verdade. Os números de uma máquina com GPU tendem a ser menores.
