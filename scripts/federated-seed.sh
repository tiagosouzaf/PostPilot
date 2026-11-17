#!/usr/bin/env bash
# Dados dos testes da consulta entre servidores (crates/ui-shell/tests/federated.rs,
# FEDERATED_TEST=1). Espera os servidores de teste nas portas de sempre:
#   PostgreSQL 54329 (senha em PGPASSWORD), e os contêineres pp-mssql (14330),
#   pp-mongo (27117), pp-redis (16379) e pp-es (19200, sem segurança).
# Uso: PGPASSWORD=... MSSQL_PASSWORD=... NOSQL_PASSWORD=... scripts/federated-seed.sh
set -euo pipefail

psql -h 127.0.0.1 -p "${PGPORT:-54329}" -U postgres -q -c "drop database if exists loja" -c "create database loja"
psql -h 127.0.0.1 -p "${PGPORT:-54329}" -U postgres -d loja -q <<'SQL'
create table clientes (id int primary key, nome text not null, cidade text, cpf text,
                       ativo boolean default true, criado timestamp default '2026-01-15 10:30:00');
insert into clientes (id, nome, cidade, cpf, ativo) values
  (1, 'Ana Souza', 'São Paulo', '01234567890', true),
  (2, 'Bruno Lima', 'Rio de Janeiro', '98765432100', true),
  (3, 'Carla Dias', 'Curitiba', '11122233344', false),
  (4, 'Diego Alves', 'Recife', null, true);
create table grande as select g as id, 'linha ' || g as texto from generate_series(1, 100000) g;
SQL

# Dois bancos com tabelas de milhões de linhas: o que a consulta não pode
# baixar inteiro.
psql -h 127.0.0.1 -p "${PGPORT:-54329}" -U postgres -q \
  -c "drop database if exists tiibcontrol" -c "create database tiibcontrol" \
  -c "drop database if exists tiibsaas" -c "create database tiibsaas"
psql -h 127.0.0.1 -p "${PGPORT:-54329}" -U postgres -d tiibcontrol -q <<'SQL'
create table documento (id bigint primary key, caixa_id int, chave_processamento_tiib text,
                        criado timestamp not null, status text, conteudo text);
insert into documento
  select g, g % 5000, 'chave-' || (g % 2500000), timestamp '2020-01-01' + (g || ' seconds')::interval,
         case when g % 10 = 0 then 'ERRO' else 'OK' end, repeat('x', 100)
  from generate_series(1, 3000000) g;
create index on documento (criado);
analyze documento;
SQL
psql -h 127.0.0.1 -p "${PGPORT:-54329}" -U postgres -d tiibsaas -q <<'SQL'
create table lotes (id bigint primary key, chave_de_processamento text, nome text, enchimento text);
insert into lotes select g, 'chave-' || g, 'lote ' || g, repeat('y', 100)
  from generate_series(1, 3000000) g where g % 3 <> 0;
create index on lotes (chave_de_processamento);
analyze lotes;
SQL

sqlcmd() { docker exec pp-mssql /opt/mssql-tools18/bin/sqlcmd -C -S localhost -U sa -P "$MSSQL_PASSWORD" "$@"; }
sqlcmd -Q "if db_id('erp') is not null drop database erp; create database erp"
sqlcmd -d erp -Q "create table dbo.pedidos (id int primary key, cliente_id int, total decimal(10,2), feito datetime2 default '2026-02-01 08:00:00', pago bit);
insert into dbo.pedidos (id, cliente_id, total, pago) values (1,1,150.50,1),(2,1,49.50,0),(3,2,1000.00,1),(4,9,5.00,1);
create table dbo.movimentos (id int primary key, pedido_id int, valor decimal(10,2));
with n as (select top (300000) row_number() over (order by (select null)) as i from sys.all_objects a cross join sys.all_objects b)
insert into dbo.movimentos select i, i % 4 + 1, i * 0.01 from n;"

docker exec pp-mongo mongosh -u root -p "$NOSQL_PASSWORD" --quiet --eval '
  db = db.getSiblingDB("app"); db.sessoes.drop();
  db.sessoes.insertMany([
    {usuario: 1, dispositivo: "celular", perfil: {plano: "pro", tags: ["a", "b"]}},
    {usuario: 2, dispositivo: "web", perfil: {plano: "free"}},
    {usuario: 2, dispositivo: "tablet"}]);'

redis() { docker exec pp-redis redis-cli -a "$NOSQL_PASSWORD" --no-auth-warning "$@" > /dev/null; }
redis FLUSHDB
redis MSET carrinho:1 3 carrinho:2 0
redis HSET usuario:1 nome Ana pontos 120
redis HSET usuario:2 nome Bruno pontos 15
redis RPUSH fila:1 a b

curl -s -XDELETE 'localhost:19200/avaliacoes' > /dev/null || true
curl -s -XPOST 'localhost:19200/avaliacoes/_bulk?refresh=true' -H 'Content-Type: application/x-ndjson' --data-binary $'{"index":{"_id":"1"}}\n{"cliente_id":1,"nota":5,"texto":"ótimo"}\n{"index":{"_id":"2"}}\n{"cliente_id":2,"nota":3,"texto":"ok"}\n{"index":{"_id":"3"}}\n{"cliente_id":1,"nota":4,"texto":"bom"}\n' > /dev/null
echo "ok"
