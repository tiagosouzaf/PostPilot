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
create table grande as select g as id, 'linha ' || g as texto from generate_series(1, 200000) g;
SQL

sqlcmd() { docker exec pp-mssql /opt/mssql-tools18/bin/sqlcmd -C -S localhost -U sa -P "$MSSQL_PASSWORD" "$@"; }
sqlcmd -Q "if db_id('erp') is not null drop database erp; create database erp"
sqlcmd -d erp -Q "create table dbo.pedidos (id int primary key, cliente_id int, total decimal(10,2), feito datetime2 default '2026-02-01 08:00:00', pago bit);
insert into dbo.pedidos (id, cliente_id, total, pago) values (1,1,150.50,1),(2,1,49.50,0),(3,2,1000.00,1),(4,9,5.00,1);"

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
