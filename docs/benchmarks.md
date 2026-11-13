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

PENDENTE_CI

## Como ler

- O pior caso conta para a meta, não a mediana.
- A primeira execução é a mais parecida com uma partida a frio; as seguintes
  aproveitam o cache de disco do sistema.
- Sem GPU, o Linux usa renderização por software (lavapipe ou llvmpipe) e o
  Windows usa o WARP, que gastam mais memória e CPU do que um driver de
  verdade. Os números de uma máquina com GPU tendem a ser menores.
