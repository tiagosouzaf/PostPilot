# Medições

Números do `pgstudio` contra as metas do projeto. Gerados com
`cargo xtask bench`, que compila o app em release, abre e fecha o app várias
vezes e mede dez cenários
(`--scenario startup|grid|intellisense|diagram|diagram2000|designer|plan|activity|edit|export|all`,
que pode se repetir para escolher vários):

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
- **diagram:** com `PGSTUDIO_BENCH=diagram`, o app abre um Database Diagram
  com 500 tabelas sintéticas (4 a 11 colunas cada, cerca de 740 FKs) e passa
  10 s passeando do canto de cima à esquerda ao de baixo à direita, com o
  zoom indo de 120% a 8% e voltando duas vezes. Mede o tempo de montar o
  diagrama (layout automático), o intervalo entre frames e a CPU da
  interface em cada frame.
- **diagram2000:** o mesmo passeio com 2.000 tabelas (cerca de 3.000 FKs),
  quatro vezes a meta, para ver quanto sobra.
- **designer:** com `PGSTUDIO_BENCH=designer`, o app abre o Table Designer
  numa tabela de um banco sintético de 2.000 tabelas, acrescenta 300 colunas,
  uma por frame, e gera o change script no fim. Mede o tempo até o designer
  abrir, o intervalo entre frames, a CPU de cada edição (incluindo o undo) e
  o tempo do script.
- **plan:** com `PGSTUDIO_BENCH=plan`, o app pede o plano estimado (Ctrl+L)
  de uma junção de duas tabelas particionadas a uma sessão sintética, que
  devolve o `EXPLAIN (FORMAT JSON)` de 2.000 nós (duas `Append` com 997
  partições cada). O plano passeia do topo ao fim por 10 s com o zoom indo
  de 100% a 25% e voltando duas vezes. Mede o tempo até o plano chegar e
  ser montado, o intervalo entre frames e a CPU da interface.
- **activity:** com `PGSTUDIO_BENCH=activity`, o app abre o Activity Monitor
  num servidor sintético com 5.000 sessões, 10.000 locks e 20 bancos (um
  décimo das sessões bloqueadas, então a seção Locks abre com 500 cadeias),
  atualizado a cada segundo por 10 s. Mede a primeira leitura, o intervalo
  entre frames, a CPU da interface e quanto cada atualização leva do pedido
  até estar na tela.
- **edit:** com `PGSTUDIO_BENCH=edit`, o app abre o Edit Top 200 Rows de
  uma tabela sintética de 12 colunas com chave primária e passa 10 s
  editando: muda uma célula e desce para a próxima linha, o que grava a
  anterior com um `UPDATE ... RETURNING` (a sessão sintética responde em
  2 ms). Mede o tempo até as linhas abrirem, o intervalo entre frames, a CPU
  da interface e quanto cada linha leva da mudança até voltar gravada.
- **export:** com `PGSTUDIO_BENCH=export`, o app executa uma consulta
  sintética de 1 milhão de linhas em modo Results to File, para um CSV
  temporário. Mede o tempo até o arquivo terminar, o intervalo entre frames
  enquanto ele é escrito e a memória (as linhas não ficam no app), e confere
  que o arquivo tem todas as linhas.

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
| Diagrama: 500 tabelas com zoom e pan | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Diagrama: 2.000 tabelas com zoom e pan | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Table Designer: editar num banco de 2.000 tabelas | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Plano de 2.000 nós com zoom e rolagem | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Activity Monitor: 5.000 sessões, atualizando a cada 1 s | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Edição na grade: gravando uma linha por vez | 60 FPS: intervalo entre frames (p95) < 20 ms |
| Exportação: 1 milhão de linhas para arquivo | 60 FPS (p95 < 20 ms) e < 32 MB a mais que o repouso |

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
  Desde o marco 6.6 o app também traz o glow: `PGSTUDIO_RENDERER=glow cargo
  xtask bench` mede com ele (o bench repassa o ambiente ao app).

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

### Marco 3.4 (diagrama do banco)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + llvmpipe,
sem GPU), 3 execuções de `cargo xtask bench --scenario diagram`:

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Diagrama: 500 tabelas montadas (layout) | — | 2.4 ms | 2.8 ms | — |
| Diagrama: frame com zoom e pan (p95) | < 20 ms (60 FPS) | 18.5 ms | 19.7 ms | ✅ dentro |
| Diagrama: CPU da interface por frame (p95) | — | 0.7 ms | 0.8 ms | — |
| Diagrama: RAM com 500 tabelas | — | 136.4 MB | 136.9 MB | — |

- **Só o visível é desenhado:** com o zoom em 120%, 4 a 5 tabelas vão para
  a tela; afastado, todas as 500.
- **Afastado, menos detalhe:** abaixo de 50% de zoom as colunas não
  aparecem (o texto teria menos de 7 px) e caixas e linhas viram uma malha
  só de retângulos, sem cantos arredondados nem suavização; abaixo de 20%,
  nem o nome da tabela. A primeira versão, que desenhava tudo com
  suavização, ficou em 24,7 ms de p95 aqui.
- **Fontes em pixels inteiros:** com o zoom mudando a cada frame, tamanhos
  quebrados faziam o egui rasterizar glifos novos quase todo frame; arredondar
  o tamanho baixou a RAM de 212 MB para 136 MB e a CPU da interface de 2,3 ms
  para 0,7 ms.
- A folga para a meta é pequena porque o renderizador por software gasta uns
  14 ms por frame só para desenhar a janela (a interface gasta menos de 1
  ms). Com GPU, o intervalo deve ficar preso ao vsync. Os números de Windows
  ficaram de fora (veja o marco 3.7).

### Marco 3.7 (fim da Fase 3)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + llvmpipe,
sem GPU), 3 execuções de cada cenário (`cargo xtask bench --scenario
diagram`, `diagram2000` e `designer`):

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Diagrama: 500 tabelas montadas (layout) | — | 3.8 ms | 4.1 ms | — |
| Diagrama: frame com zoom e pan (p95) | < 20 ms (60 FPS) | 18.2 ms | 18.6 ms | ✅ dentro |
| Diagrama: CPU da interface por frame (p95) | — | 0.7 ms | 0.7 ms | — |
| Diagrama: RAM com 500 tabelas | — | 138.2 MB | 138.5 MB | — |
| Diagrama: 2.000 tabelas montadas (layout) | — | 16.2 ms | 19.5 ms | — |
| Diagrama 2.000: frame com zoom e pan (p95) | < 20 ms (60 FPS) | 21.6 ms | 21.8 ms | ❌ acima |
| Diagrama 2.000: CPU da interface por frame (p95) | — | 0.8 ms | 0.9 ms | — |
| Diagrama: RAM com 2.000 tabelas | — | 154.2 MB | 154.4 MB | — |
| Table Designer: abre num banco de 2.000 tabelas | — | 33.3 ms | 40.4 ms | — |
| Table Designer: frame com uma coluna nova (p95) | < 20 ms (60 FPS) | 17.0 ms | 17.1 ms | ✅ dentro |
| Table Designer: CPU da interface por edição (p95) | — | 0.7 ms | 0.7 ms | — |
| Table Designer: script de 300 colunas novas | — | 0.4 ms | 0.5 ms | — |

- **Windows não foi medido.** Não há máquina Windows disponível e os
  minutos do GitHub Actions acabaram, então o job de Windows não rodou. Os
  cenários estão prontos: `cargo xtask bench --scenario diagram`,
  `diagram2000` e `designer` numa máquina Windows completam a tabela.
- **Diagrama com 2.000 tabelas:** fica 1,6 ms acima da meta, mas a interface
  gasta menos de 1 ms por frame; o resto é o renderizador por software
  pintando até 1.650 caixas e 3.000 linhas na tela. A meta do plano é a de
  500 tabelas, que passa. Pintar cada pixel uma vez só (borda em tiras,
  corpo abaixo do cabeçalho) baixou o p95 de 23,3 ms para 21,6 ms; tirar os
  nomes das tabelas não mudou nada, então o próximo passo seria agrupar
  tabelas quando afastado, o que só vale medir com GPU.
- **Table Designer:** o cenário acrescenta 300 colunas, uma por frame, a
  uma tabela num banco de 2.000 tabelas. Na primeira versão cada edição
  custava 8 ms e o script 27 ms, porque o modelo inteiro era copiado para o
  histórico de undo e comparado tabela por tabela. Com as tabelas
  compartilhadas (`Arc`, copiadas só quando mudam), a edição caiu para
  0,4 ms e o script para 0,4 ms. A grade desenha só as linhas visíveis, o que
  baixou a CPU da interface de 3,3 ms para 0,7 ms com 300 colunas.

### Marco 4.9 (fim da Fase 4)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + llvmpipe,
sem GPU), 3 execuções de cada cenário (`cargo xtask bench --scenario plan`,
`activity` e, para comparar com o marco 3.7, `diagram`):

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Plano: 2.000 nós chegam e são desenhados (Ctrl+L) | — | 53.9 ms | 55.5 ms | — |
| Plano: frame com zoom e rolagem (p95) | < 20 ms (60 FPS) | 22.1 ms | 22.7 ms | ❌ acima |
| Plano: CPU da interface por frame (p95) | — | 4.0 ms | 4.4 ms | — |
| Plano: RAM com o plano de 2.000 nós | — | 141.7 MB | 141.7 MB | — |
| Activity Monitor: primeira leitura de 5.000 sessões | — | 39.0 ms | 43.1 ms | — |
| Activity Monitor: frame atualizando a cada 1 s (p95) | < 20 ms (60 FPS) | 18.3 ms | 19.8 ms | ✅ dentro |
| Activity Monitor: CPU da interface por frame (p95) | — | 1.1 ms | 1.1 ms | — |
| Activity Monitor: atualização pronta (p95) | — | 27.1 ms | 31.0 ms | — |
| Activity Monitor: RAM com 5.000 sessões | — | 162.2 MB | 162.4 MB | — |
| Diagrama: frame com zoom e pan (p95), para comparar | < 20 ms (60 FPS) | 21.4 ms | 23.0 ms | ❌ acima |

- **O container estava mais lento que no marco 3.7:** o mesmo cenário
  `diagram`, sem nenhuma mudança no código, deu 21,4 ms de p95 (18,2 ms no
  marco 3.7). O plano fica no mesmo patamar (22,1 ms) com a interface
  gastando 4 ms; o resto é o renderizador por software. Fica como "precisa
  de medição com GPU", como o diagrama de 2.000 tabelas.
- **Plano:** a primeira versão desenhava uma seta inteira por filho, e as
  mil setas de uma `Append` dividiam o mesmo tronco vertical, todas
  desenhadas sempre que o pai ou qualquer filho estava na tela. Com o tronco
  e a ponta desenhados uma vez por pai e cortados ao visível, o p95 caiu de
  32,2 ms para 22,1 ms.
- **Activity Monitor:** a primeira versão copiava as 5.000 sessões, filtrava
  e ordenava a cada frame, e a seção Locks procurava cada sessão das 500
  cadeias de bloqueio na lista inteira e desenhava todas. Com o filtro, a
  ordem e as cadeias refeitos só quando chega uma leitura ou o filtro e a
  ordem mudam, e as cadeias numa lista virtualizada, a CPU da interface caiu
  de 11,6 ms para 1,1 ms por frame e o p95 de 30,6 ms para 18,3 ms.
- **Windows não foi medido**, pelo mesmo motivo do marco 3.7: os cenários
  `plan` e `activity` numa máquina Windows completam a tabela.

### Marco 5.6 (plano grande)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + llvmpipe,
sem GPU), 5 execuções de cada cenário, em duas rodadas alternando `plan` e
`diagram` (o cenário de referência, sem mudança no código):

| Cenário | Meta | Rodada 1: mediana | Rodada 1: pior | Rodada 2: mediana | Rodada 2: pior |
| --- | --- | --- | --- | --- | --- |
| Plano: 2.000 nós chegam e são desenhados (Ctrl+L) | — | 69.1 ms | 70.1 ms | 59.2 ms | 62.4 ms |
| Plano: frame com zoom e rolagem (p95) | < 20 ms (60 FPS) | 20.3 ms ❌ | 20.7 ms ❌ | 19.8 ms ✅ | 24.7 ms ❌ |
| Plano: CPU da interface por frame (p95) | — | 0.9 ms | 0.9 ms | 0.9 ms | 1.2 ms |
| Plano: RAM com o plano de 2.000 nós | — | 140.9 MB | 141.3 MB | 140.8 MB | 140.9 MB |
| Diagrama: frame com zoom e pan (p95), para comparar | < 20 ms (60 FPS) | 25.0 ms ❌ | 26.0 ms ❌ | 23.0 ms ❌ | 25.7 ms ❌ |

Antes da mudança, no mesmo container e no mesmo dia (3 execuções): plano
com p95 de 26,9 ms (pior 27,1 ms) e CPU da interface de 5,9 ms; diagrama com
27,0 ms (pior 28,6 ms).

- **O container estava ainda mais lento que no marco 4.9:** o diagrama de
  referência deu de 23 a 27 ms de p95 (18,2 ms no marco 3.7 e 21,4 ms no
  4.9). Contra ele, o plano passou de "um pouco pior que o diagrama" (22,1
  contra 21,4 ms no marco 4.9; 26,9 contra 27,0 ms antes desta mudança) para
  3 a 5 ms melhor. Mas o pior caso ainda fica acima de 20 ms aqui, então a
  meta conta como **não atingida** neste ambiente.
- **O que sobra é o renderizador por software:** com o plano montado e o
  passeio igual, mas sem desenhar nenhum nó nem seta, o mesmo cenário deu
  19,4 ms de p95 (pior 19,6 ms, 5 execuções) contra 20,0 ms (pior 21,2 ms)
  desenhando. O plano custa cerca de 0,6 ms do frame; o resto é o llvmpipe
  pintando a janela de 1280×800. Fica como "precisa de medição com GPU",
  como o diagrama de 2.000 tabelas.
- **A maior parte da CPU não estava no desenho:** a aba do plano copiava o
  plano inteiro (2.000 nós com todas as propriedades) a cada frame só para
  calcular o custo relativo ao lote. Sem a cópia, a CPU da interface caiu de
  5,9 ms para 1,1 ms e o p95 de 26,9 ms para 22,6 ms.
- **Textos e formas guardados:** os textos de cada nó (título, tabela,
  custo, linhas, o rótulo para leitores de tela), a fração de custo e a
  espessura das setas são montados uma vez por plano (antes, cada nó na
  tela somava o custo do plano inteiro duas vezes por frame). Os galleys dos
  nós na tela ficam guardados enquanto o tamanho da fonte não muda, sem cor
  (a cor entra na hora de desenhar). Os nós e os filhos de cada `Append` são
  achados por busca binária na altura visível, em vez de percorrer os 2.000.
  Com isso a CPU foi de 1,1 ms para 0,9 ms e o p95 para 20 ms.
- **Windows não foi medido**, pelo mesmo motivo dos marcos anteriores.

### Marco 5.8 (Fase 5)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + llvmpipe,
sem GPU), 3 execuções de cada cenário (`cargo xtask bench --runs 3 --idle 10
--scenario startup --scenario edit --scenario export`):

| Cenário | Meta | Mediana | Pior | Situação |
| --- | --- | --- | --- | --- |
| Até o primeiro frame | < 1 s | 108 ms | 109 ms | ✅ dentro |
| RAM em repouso | < 100 MB | 129.4 MB | 129.7 MB | ❌ acima |
| Edição na grade: 200 linhas de 12 colunas abertas | — | 35.3 ms | 35.3 ms | — |
| Edição na grade: frame editando e gravando (p95) | < 20 ms (60 FPS) | 15.8 ms | 23.9 ms | ❌ acima |
| Edição na grade: CPU da interface por frame (p95) | — | 0.7 ms | 0.8 ms | — |
| Edição na grade: linha gravada, ida e volta (p95) | — | 15.8 ms | 23.8 ms | — |
| Edição na grade: linhas gravadas em 10 s | — | 771 | 795 | — |
| Exportação: 1 milhão de linhas para CSV (Results to File) | — | 501 ms | 517 ms | — |
| Exportação: frame enquanto o arquivo é escrito (p95) | < 20 ms (60 FPS) | 22.5 ms | 22.6 ms | ❌ acima |
| Exportação: CPU da interface por frame (p95) | — | 0.7 ms | 0.7 ms | — |
| Exportação: RAM a mais que o repouso | < 32 MB | 1.7 MB | 2.1 MB | ✅ dentro |

- **Edição:** a interface gasta menos de 1 ms por frame e cada linha volta
  gravada em cerca de um frame (a gravação espera o `UPDATE` e a próxima
  pintura). A pior execução foi a primeira: 15,4 e 15,8 ms nas outras duas,
  23,9 ms nela, com um frame de 48 ms logo no começo. Como nos marcos
  anteriores, conta o pior caso, e fica como "medir com GPU".
- **Exportação:** 1 milhão de linhas (83,4 MB de CSV) em cerca de meio
  segundo, sem guardar as linhas: a memória fica a 2 MB do repouso. O
  arquivo é conferido linha a linha no fim de cada execução, e o teste
  `crates/ui-shell/tests/export.rs` confere o conteúdo com `COPY` num
  PostgreSQL de verdade.
- **O frame da exportação passa de 20 ms por disputa de CPU, não pela
  interface:** durante o meio segundo de escrita, a geração das linhas e a
  escrita do arquivo ocupam núcleos que o llvmpipe usa para desenhar, e a
  interface continua em 0,7 ms por frame. São só cerca de 34 frames por
  execução, então o p95 é praticamente o segundo pior frame. Com uma GPU o
  desenho sai da CPU; fica como "precisa de medição com GPU", junto com o
  diagrama de 2.000 tabelas e o plano.
- **RAM em repouso** segue acima da meta neste container (129 MB, como nos
  marcos anteriores com llvmpipe).
- **Windows não foi medido:** os cenários `edit` e `export` numa máquina
  Windows completam a tabela.

### Marco 6.8 (wgpu contra glow, sem GPU)

Medido localmente (Linux x86_64 num container de 4 núcleos, Xvfb + Mesa por
software), 1 execução de cada cenário e 5 s em repouso, com o roteiro de
[`na-sua-maquina.md`](na-sua-maquina.md) (`cargo xtask bench --markdown`,
depois o mesmo com `PGSTUDIO_RENDERER=glow`). Uma execução só serve para
comparar os dois renderizadores no mesmo container, não para fechar metas.

| Cenário | Meta | wgpu (Vulkan, lavapipe) | glow (OpenGL, llvmpipe) |
| --- | --- | --- | --- |
| Até o primeiro frame | < 1 s | 208 ms ✅ | 290 ms ✅ |
| RAM em repouso | < 100 MB | 137.0 MB ❌ | 125.8 MB ❌ |
| Grade: frame rolando (p95) | < 20 ms | 19.0 ms ✅ | 12.8 ms ✅ |
| Diagrama 500: frame (p95) | < 20 ms | 19.5 ms ✅ | 14.4 ms ✅ |
| Diagrama 2.000: frame (p95) | < 20 ms | 24.4 ms ❌ | 19.8 ms ✅ |
| Table Designer: frame (p95) | < 20 ms | 17.8 ms ✅ | 14.1 ms ✅ |
| Plano 2.000 nós: frame (p95) | < 20 ms | 15.4 ms ✅ | 10.3 ms ✅ |
| Activity Monitor: frame (p95) | < 20 ms | 16.1 ms ✅ | 12.1 ms ✅ |
| Edição na grade: frame (p95) | < 20 ms | 17.8 ms ✅ | 12.1 ms ✅ |
| Exportação: frame (p95) | < 20 ms | 24.8 ms ❌ | 13.5 ms ✅ |

- **Sem GPU, o glow é mais rápido em todos os cenários com frame** (3 a 11
  ms a menos de p95) e usa 11 MB a menos em repouso; com ele, todas as metas
  de frame passam neste container. A CPU da interface é a mesma nos dois
  (menos de 1 ms por frame): a diferença é só o driver por software.
- **Isso não decide o padrão.** Numa GPU de verdade os dois devem ficar presos
  ao vsync, e o wgpu é o caminho testado desde a Fase 0. A comparação que
  decide é a do roteiro na máquina com GPU; se lá o glow também gastar menos
  RAM, vale trocar o padrão no Linux.
- **RAM em repouso** continua acima de 100 MB nos dois sem GPU.

## Como ler

- O pior caso conta para a meta, não a mediana.
- A primeira execução é a mais parecida com uma partida a frio; as seguintes
  aproveitam o cache de disco do sistema.
- Sem GPU, o Linux usa renderização por software (lavapipe ou llvmpipe) e o
  Windows usa o WARP, que gastam mais memória e CPU do que um driver de
  verdade. Os números de uma máquina com GPU tendem a ser menores.
