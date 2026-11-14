# Workspace num repositório

Um workspace é uma pasta, normalmente um repositório git. Ela guarda os
servidores, as consultas e as senhas criptografadas de um projeto. É a
"Solution" do SSMS, mas feita para ir para o GitHub. Quem clona o repositório
abre o mesmo workspace e conecta nos mesmos servidores. A única coisa que essa
pessoa precisa digitar é a senha mestra.

## O que vai na pasta

```
vendas/
  postpilot-workspace.json   nome e versão do formato
  servers.json               servidores registrados, sem nenhuma senha
  secrets.json               senhas, criptografadas com a senha mestra
  queries/                   uma consulta por arquivo .sql, em subpastas
  .gitignore                 arquivos temporários desta máquina
```

Todos os arquivos são texto, para o diff e o merge do git funcionarem:

- `servers.json` tem o mesmo formato do export dos Registered Servers.
- `secrets.json` tem uma linha por senha, de modo que duas pessoas salvando
  senhas diferentes não brigam no merge.

A primeira linha de uma consulta pode ligá-la a um servidor do workspace:

```sql
-- postpilot: server=Produção; database=vendas
select count(*) from pedidos;
```

Abrir essa consulta conecta no servidor "Produção", no banco `vendas`. Ao
salvar uma consulta nova conectada a um servidor do workspace, essa linha é
escrita sozinha.

## Começar

- **File > New > Workspace...** escolhe uma pasta e cria o workspace nela.
- **File > Open > Workspace...** abre um workspace que já existe.
- **Clone Workspace...** fica no Workspace Explorer (View > Workspace
  Explorer, Ctrl+Alt+L). Você cola a URL do repositório
  (`git@github.com:voce/sql.git` ou https) e escolhe a pasta. O git usa suas
  chaves SSH e o credential helper de sempre. Se o repositório estiver vazio,
  ele vira um workspace.

O PostPilot reabre o último workspace ao iniciar.

## Servidores

A árvore "Servers" do Workspace Explorer funciona como a dos Registered
Servers: tem grupos, "New Server Registration..." e Connect. As mudanças vão
para `servers.json`.

- **Connection string:** em New Server Registration, cole uma connection
  string no campo de cima e ele preenche servidor, login, banco, SSL e senha.
  Valem as duas formas da libpq:
  - `postgresql://usuario:senha@host:5432/banco?sslmode=require`
  - `host=db port=5432 dbname=vendas user=app password='...'`

  A connection string não é guardada. A senha vai para o cofre do workspace
  (veja abaixo) e nunca para `servers.json`.
- **"Import from this computer" (Importar deste computador):** este botão, ao lado de "Servers", traz os
  Registered Servers desta máquina para o workspace. Grupos de mesmo nome se
  juntam e um servidor de mesmo nome é substituído. Com as senhas do workspace
  destrancadas, as senhas lembradas desses servidores são copiadas para o
  cofre. O keyring do sistema continua com a cópia dele.

## Senhas do workspace

Sem a senha mestra, as senhas ficam só no keyring do sistema, como nos
servidores fora do workspace. Com ela, o "Remember password" dos servidores do
workspace salva a senha em `secrets.json`. Esse arquivo vai para o repositório
criptografado:

1. Clique em **Save in workspace...** (Salvar no workspace...) na linha de
   senhas do Workspace Explorer.
2. Escolha uma senha mestra de pelo menos 10 caracteres.
3. Marque **Remember on this computer** para guardar a senha mestra no keyring
   desta máquina. Assim o workspace destranca sozinho ao abrir.
4. Faça Commit e Push de `secrets.json`.

Em outra máquina, depois do clone ou do pull, o PostPilot pede a senha mestra
uma vez.

Outros comandos:

- **Lock** tranca as senhas na sessão.
- **Change Master Password...** criptografa tudo de novo com a senha nova.
  Faça Commit e Push de `secrets.json`. As outras máquinas pedem a senha nova
  depois do próximo pull.

### Como as senhas são protegidas

- A chave vem da senha mestra com Argon2id (64 MiB de memória, 3 passadas).
  Cada workspace tem o seu salt aleatório.
- Cada senha é criptografada com XChaCha20-Poly1305 (criptografia
  autenticada, do RustCrypto), com um nonce aleatório. O nome da entrada
  (`usuario@host:porta`) entra como dado autenticado. Por isso trocar linhas
  de lugar ou mexer no arquivo é detectado, e a senha não abre.
- A senha mestra e a chave nunca são gravadas na pasta. A chave fica só na
  memória enquanto o workspace está destrancado e é apagada ao trancar.
- Os testes do projeto procuram a senha em todos os arquivos do workspace e no
  histórico do git. Eles falham se a senha aparecer em texto puro, em base64
  ou com %-escape.

### O que alguém com acesso ao repositório vê

Essa pessoa vê os nomes dos servidores, hosts, portas, bancos e logins
(`servers.json`) e as chaves `usuario@host:porta` de `secrets.json`. As
senhas ela não vê. Se isso já é demais, o repositório precisa ser privado. A
segurança das senhas depende de a senha mestra ser forte e não estar no
repositório.

### Esqueci a senha mestra

Não há recuperação, e esse é o objetivo. Quem tem outra máquina ainda
destrancada pode trocar a senha mestra (Change Master Password...) sem saber
a antiga. Se ninguém tiver:

1. Apague `secrets.json` e faça Commit.
2. Ligue as senhas de novo com **Save in workspace...** e uma senha mestra
   nova.
3. Conecte de novo em cada servidor com "Remember password". Ou use
   "Importar deste computador" para trazer as senhas que ainda estão no
   keyring de alguém.

Se a senha mestra vazou, troque-a. Mas quem copiou o `secrets.json` antigo
continua conseguindo abri-lo com a senha antiga. Por isso, troque também as
senhas dos bancos.

## Git

A faixa "Source Control" do Workspace Explorer mostra o branch, os commits a
enviar e a trazer (↑/↓) e as alterações:

- **Pull** traz o remoto com merge, nunca com rebase.
- **Commit...** faz o commit de tudo o que mudou, com a opção "Commit and
  Push".
- **Push** envia os commits. Na primeira vez, cria o branch em `origin`.
- **Remote...** define a URL de `origin`.
- **Initialize Git** aparece quando a pasta ainda não é um repositório.

O PostPilot usa o `git` instalado no sistema (no Windows, o Git for Windows).
Um conflito no Pull lista os arquivos. Corrija os trechos entre `<<<<<<<` e
`>>>>>>>` e faça o Commit. Em `secrets.json` cada senha é uma linha, então um
conflito ali normalmente se resolve mantendo as duas linhas.
