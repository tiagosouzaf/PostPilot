# Túnel SSH

Para o banco que só um servidor SSH (um bastion) alcança. O PostPilot entra
no servidor SSH e, de lá, chega ao banco: PostgreSQL ou SQL Server.

## Como usar

No **Connect to Server** (e no registro de um servidor), marque **Conectar
por um túnel SSH** e preencha:

| Campo | O que é |
|---|---|
| Servidor SSH | `host` ou `host:porta` do bastion (porta 22 se não disser) |
| Usuário SSH | o usuário no bastion |
| Autenticação SSH | arquivo de chave, senha ou agente SSH |
| Arquivo da chave | caminho da chave privada, como `~/.ssh/id_ed25519`; vazio tenta `id_ed25519`, `id_ecdsa` e `id_rsa` de `~/.ssh` |
| Frase secreta / Senha do SSH | vazio usa a que está salva |

O **Server name** continua sendo o endereço do banco **como o bastion o
vê** (por exemplo `10.0.0.5` ou `db.interno`). No SQL Server, pelo túnel
vale a porta: a instância nomeada (`servidor\INSTANCIA`) não é procurada.

Com o agente SSH, o app usa as chaves do `ssh-agent` (`SSH_AUTH_SOCK`) no
Linux e, no Windows, as do agente do OpenSSH ou do Pageant.

## O que fica salvo, e onde

- O túnel (servidor, porta, usuário, modo e **caminho** da chave) vai junto
  com a conexão: conexões recentes, Registered Servers, `servers.json` do
  workspace e a sessão. Por isso chega ao outro computador pelo workspace.
- A chave privada **nunca** é copiada: só o caminho. Escolhida pelo botão,
  ela é gravada com `~` (a pasta do usuário de cada computador), então a
  mesma conexão serve no Linux e no Windows se a chave estiver no mesmo
  lugar relativo.
- A senha do SSH ou a frase secreta da chave, com **Remember password**,
  vai para o cofre: o do workspace (cifrada pela senha mestra, em
  `secrets.json`) para os servidores do workspace, o do sistema para os
  outros. A chave no cofre é `ssh:usuario@host:porta`, uma por bastion:
  todas as conexões pelo mesmo bastion usam o mesmo segredo.
- A senha do banco de uma conexão por túnel fica numa chave que inclui o
  bastion (`app@10.0.0.5:5432 via tiago@bastion:22`): dois clientes podem
  ter o banco no mesmo endereço interno.

## Chave do servidor SSH

Na primeira conexão a um bastion, a chave dele é anotada em
`<config>/PostPilot/ssh_known_hosts`. O `~/.ssh/known_hosts` do usuário
também vale. Se a chave do servidor não for a anotada, a conexão é recusada
e a mensagem diz o arquivo e a linha: confira com quem cuida do servidor e,
se a troca for legítima, apague a linha.

## Como funciona

A crate `pgstudio-ssh` (cliente SSH em Rust puro, `russh` com `ring`) abre
uma porta em `127.0.0.1` e leva cada conexão que chega nela ao banco por
um canal `direct-tcpip`, como o `ssh -L`. Os drivers conectam na porta
local e continuam usando o nome do servidor para o TLS. Conexões para o
mesmo banco pelo mesmo bastion dividem um túnel; ele fecha quando a última
conexão é solta e, se a sessão SSH cair, entra de novo na próxima conexão.
O `pg_dump` e o `pg_restore` do Backup/Restore usam a mesma porta local.

## Testes

`scripts/ssh-tunnel-test.sh` sobe um bastion (sshd) e, numa rede que só ele
alcança, um PostgreSQL e um SQL Server, e roda os testes do túnel: chave
(ed25519 e RSA), chave com frase secreta, senha, chave recusada, túnel
compartilhado e reaberto, consulta, resultado grande e cancelamento pelo
túnel, nos dois bancos. Para o agente: `ssh-add` da chave
`target/ssh-tunnel-test/id_plain` e `SSH_TEST_AGENT=1`.

Para abrir o app direto por um túnel (linha de comando):

```text
PGSTUDIO_CONNECT=pgdb PGUSER=postgres PGPASSWORD=secret \
PGSTUDIO_SSH=tunnel@127.0.0.1:2222 PGSTUDIO_SSH_KEY=target/ssh-tunnel-test/id_plain \
    postpilot
```
