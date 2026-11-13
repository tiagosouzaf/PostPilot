# Medições

Números do `pgstudio` contra as metas do projeto. Gerados com
`cargo xtask bench`, que compila o app em release, abre e fecha o app várias
vezes e mede:

- **Até o primeiro frame:** do início do processo até o primeiro frame exibido
  (o app avisa pela saída padrão no modo `PGSTUDIO_BENCH=1`). Inclui carregar
  o binário, criar a janela, iniciar a GPU e desenhar a casca inteira.
- **RAM em repouso:** RSS no Linux e working set no Windows, lidos de fora do
  processo depois de 30 s parado, com o Object Explorer de demonstração
  conectado.

O CI roda a medição em Linux e Windows em todo PR (job "Medição"); o resultado
aparece no resumo do job e como artefato `bench-<sistema>`. Por enquanto as
metas não reprovam o CI: os runners não têm GPU e variam de uma execução para
outra. `cargo xtask bench --check` sai com erro se alguma meta não for
atingida.

```sh
cargo xtask bench                    # 5 execuções, 30 s em repouso
cargo xtask bench --runs 3 --idle 10 # mais rápido
xvfb-run -a cargo xtask bench        # Linux sem tela
```

## Metas

| Medida | Meta |
| --- | --- |
| Inicialização a frio | < 1 s |
| RAM em repouso | < 100 MB |

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

## Como ler

- O pior caso conta para a meta, não a mediana.
- A primeira execução é a mais parecida com uma partida a frio; as seguintes
  aproveitam o cache de disco do sistema.
- Sem GPU, o Linux usa renderização por software (lavapipe ou llvmpipe) e o
  Windows usa o WARP, que gastam mais memória e CPU do que um driver de
  verdade. Os números de uma máquina com GPU tendem a ser menores.
