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
  snippets/                  seus trechos do Template Explorer, um por arquivo .sql
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

Os trechos de `snippets/` aparecem no Template Explorer (Exibir > Template
Explorer, Ctrl+Alt+T), junto dos modelos que vêm com o PostPilot para
PostgreSQL e SQL Server. Salve um trecho pelo botão "Novo Trecho" do painel
ou por "Salvar como Trecho..." no menu de contexto do editor; as subpastas
de `snippets/` são as pastas do painel. Sem workspace aberto, os trechos
ficam só neste computador, na pasta de configuração do PostPilot.

## Começar

O Workspace Explorer (View > Workspace Explorer, Ctrl+Alt+L) e o botão com a
maleta na barra de ferramentas têm **Adicionar**, com duas formas:

- **Clonar um repositório**: você cola o endereço
  (`git@github.com:voce/sql.git` ou https) e confere a pasta onde ele fica
  (`~/PostPilot` por padrão). O git usa suas chaves SSH e o credential helper
  de sempre. Se o repositório estiver vazio, ele vira um workspace.
- **Usar uma pasta deste computador**: abre o workspace que está na pasta ou,
  se ela ainda não é um, cria um nela.

**File > New > Workspace...**, **File > Open > Workspace...** e **File > Open >
Clone Workspace...** continuam no menu.

## Vários workspaces

Dá para ter vários workspaces abertos ao mesmo tempo, por exemplo um por
cliente ou por time. Cada um continua sendo uma pasta com o seu próprio git e a
sua própria senha mestra; abrir outro não fecha os que já estão abertos.

- O Workspace Explorer mostra um cartão por workspace, com o estado do
  repositório em uma frase: **Em dia**, **3 alterações · 1 para enviar**,
  **2 para receber**, **Só neste computador** (sem git ou sem remoto) ou
  **Conflito para resolver**. Um cadeado avisa que as senhas daquele workspace
  estão trancadas.
- Clicar num cartão torna aquele workspace o **ativo**: os servidores e as
  consultas dele aparecem embaixo, e é nele que entra o que você registra ou
  importa. O botão da maleta na barra de ferramentas e a lista **Workspace:**
  do Object Explorer fazem a mesma troca.
- O cartão do ativo tem **Salvar alterações...** (Commit) e **Sincronizar**
  (Pull e depois Push). Pull, Push, Remote... e **Fechar Workspace** ficam no
  menu `...` de cada cartão. Fechar só tira o workspace da lista; os arquivos
  continuam na pasta.
- As conexões de todos os workspaces abertos aparecem no **Connect to
  Server**, separadas pelo nome do workspace, e como destinos de "executar em
  vários servidores". Os trechos de todos aparecem no Template Explorer
  ("Trechos de <workspace>").
- Uma consulta salva num workspace é ligada ao servidor daquele workspace,
  mesmo que ele não seja o ativo. A senha lembrada de um servidor vai para o
  cofre do workspace em que ele está.

Ao iniciar, o PostPilot reabre todos os workspaces que estavam abertos e o
mesmo ativo (a lista fica em `<config>/PostPilot/workspace.txt`).

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
- **Um workspace novo nasce sem conexões.** As que você já usava ficam nesta
  máquina (Registered Servers, conexões recentes e a conexão ativa) e só vão
  para o repositório depois de importadas. Por isso, ao criar o workspace, o
  PostPilot pergunta **"Trazer suas conexões?"**. Se você responder "Agora
  não", o painel continua mostrando o botão **Importar as N conexões deste
  computador** enquanto o workspace estiver vazio.
- **"Import from this computer" (Importar deste computador):** traz para o
  workspace os Registered Servers desta máquina, as conexões recentes do
  "Connect to Server" e a conexão ativa. Grupos de mesmo nome se juntam e um
  servidor de mesmo nome é substituído. As senhas lembradas (e a senha em uso
  na conexão ativa) são copiadas para o cofre do workspace: se ele ainda não
  existe ou está trancado, a senha mestra é pedida e a cópia acontece em
  seguida. O keyring do sistema continua com a cópia dele. Sem nada para
  importar, o botão abre o New Server Registration do workspace.
- **"Add to workspace" (Adicionar ao workspace):** aparece quando você está
  conectado a um servidor que não está no workspace, e o registra nele com a
  senha em uso.
- **"Connect to Server" lista as conexões salvas:** com um workspace aberto,
  o diálogo mostra no alto as conexões dele (etiqueta do ambiente, nome e
  pasta) e, abaixo, os Registered Servers desta máquina que não estão no
  workspace. Um clique preenche os campos e, se a senha está no cofre,
  conecta direto; sem senha guardada, o diálogo fica aberto esperando a
  senha. Os campos continuam ali para uma conexão digitada à mão.
- **Register... e Properties no servidor do Object Explorer:** com um
  workspace aberto, abrem o registro desse servidor no workspace (e não nos
  Registered Servers desta máquina), com a senha em uso, o campo **Folder**
  (Pasta) e o **Environment** (Ambiente). Se o servidor já está no workspace,
  abrem o registro dele para editar.
- **Servidor registrado no Object Explorer:** quando a conexão corresponde
  a um servidor registrado (no workspace ou nesta máquina), o nó do servidor
  mostra a etiqueta do ambiente na frente e o nome do registro, por exemplo
  `[Produção] tiib`, com o endereço na dica. A status bar faz o mesmo, e o
  menu troca "Register..." por **Edit Registration...** (Editar Registro...).
- Depois de importar, faça **Commit** e **Push**. Na outra máquina, o clone
  (ou o Pull) mostra os servidores, e as senhas vêm depois da senha mestra.

## Pastas e ambientes

Os grupos funcionam como as pastas do DataGrip, tanto no workspace quanto nos
Registered Servers:

- **Pastas:** "New Server Group..." cria uma pasta, inclusive dentro de outra.
  Para mudar uma conexão ou pasta de lugar, arraste ou use **Move To** (Mover
  Para) no menu dela.
- **Cor da pasta:** em Properties da pasta, escolha uma cor em **Color**. O
  ícone da pasta, das subpastas e das conexões dentro dela fica dessa cor.
- **Ambientes:** todo workspace já vem com Development (Desenvolvimento,
  verde), Staging (Homologação, laranja) e Production (Produção, vermelho).
  Escolha o ambiente em Properties de uma pasta, e todas as conexões dentro
  dela passam a ser daquele ambiente. Uma conexão também pode ter um ambiente
  próprio, que vale mais que o da pasta. **Environments...** (Ambientes...),
  no menu da raiz da árvore, muda nomes e cores e cria outros ambientes, como
  QA.
- **Na janela de consulta:** uma janela aberta numa conexão com ambiente
  ganha uma faixa da cor dele no topo, a faixa de status tingida e a etiqueta
  do ambiente. A status bar também mostra a etiqueta. Sem ambiente, vale a cor
  da pasta. A faixa de status e a status bar mostram a pasta e o nome da
  conexão salva, como `MainDoc / TiibControl@MainDoc (tiibcontrol)`, com o
  endereço do servidor na dica; uma pasta com o mesmo nome do ambiente não é
  repetida, porque a etiqueta já está ao lado.

- **Confirmação em produção:** num ambiente com **Confirm changes**
  (Confirmar alterações) marcado em Environments..., o F5 pergunta antes de
  rodar SQL que pode alterar o banco (INSERT, UPDATE, DELETE, MERGE, DDL,
  GRANT, controle de transação ou SQL que o PostPilot não consegue
  classificar). SELECT roda sem perguntar. Production já vem marcado; os
  outros ambientes, não.

Tudo isso fica em `servers.json` e vai para o git junto com os servidores.
Quem clona o repositório recebe as mesmas pastas, cores e ambientes. Os nomes
dos três ambientes padrão ficam em inglês no arquivo e aparecem traduzidos
na tela, então o mesmo repositório serve para quem usa o PostPilot em outro
idioma.

```json
{
  "version": 1,
  "root": {
    "name": "vendas",
    "groups": [
      {
        "name": "Clientes",
        "color": "#e5534b",
        "environment": "Production",
        "servers": [ { "name": "vendas-prod", "settings": { "...": "..." } } ]
      }
    ]
  }
}
```

A lista `environments` só aparece no arquivo quando alguém muda os
ambientes. Um `servers.json` antigo, sem cores nem ambientes, abre normalmente.

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
- **Alterações**: clique em "N alterações" (ou em ⋯ > **Changes and
  Discard...**) para ver os arquivos alterados. **Discard** (Descartar) volta
  um arquivo ao último commit, ou apaga um arquivo novo. **Discard All...**
  volta todos os arquivos do repositório e deixa os novos. Sempre pede
  confirmação.

O PostPilot usa o `git` instalado no sistema (no Windows, o Git for Windows).
Quando o Pull para porque um arquivo mudou neste computador e também no
repositório, o PostPilot explica quais são os arquivos e abre "Alterações"
para você fazer Commit ou descartar.

Um conflito no Pull lista os arquivos. Corrija os trechos entre `<<<<<<<` e
`>>>>>>>` e faça o Commit. O `secrets.json` é exceção: quando duas máquinas
salvaram senhas, o Pull junta as senhas dos dois lados sozinho (se os dois
mudaram a mesma senha, vale a deste computador). Só fica para você quando
os dois lados têm senhas mestras diferentes. Conectar com a mesma senha já
salva não altera o `secrets.json`.
