#!/usr/bin/env bash
# Sobe um servidor SSH (o bastion) e, numa rede que só ele alcança, um
# PostgreSQL e um SQL Server; depois roda os testes do túnel SSH.
#
#   scripts/ssh-tunnel-test.sh          sobe tudo e roda os testes
#   scripts/ssh-tunnel-test.sh down     remove os contêineres
#
# Precisa de docker e do ssh-keygen. As chaves de teste ficam em
# target/ssh-tunnel-test (nunca no repositório).
set -euo pipefail

cd "$(dirname "$0")/.."
dir="$PWD/target/ssh-tunnel-test"
net=postpilot-tunnel
ssh_password='Bastion!2026'
sa_password='PostPilot!2026'

down() {
  docker rm -f postpilot-bastion postpilot-tunnel-pg postpilot-tunnel-sql >/dev/null 2>&1 || true
  docker network rm "$net" >/dev/null 2>&1 || true
}

if [ "${1:-}" = down ]; then
  down
  exit 0
fi

mkdir -p "$dir"
if [ ! -f "$dir/id_plain" ]; then
  ssh-keygen -q -t ed25519 -N '' -f "$dir/id_plain" -C postpilot-test
  ssh-keygen -q -t ed25519 -N 'frase secreta' -f "$dir/id_locked" -C postpilot-test
  ssh-keygen -q -t rsa -b 3072 -N '' -f "$dir/id_rsa" -C postpilot-test
  # Uma chave que o servidor não conhece.
  ssh-keygen -q -t ed25519 -N '' -f "$dir/id_stranger" -C postpilot-test
fi

down
docker network create "$net" >/dev/null
docker run -d --name postpilot-tunnel-pg --network "$net" --network-alias pgdb \
  -e POSTGRES_PASSWORD=secret postgres:16-alpine >/dev/null
docker run -d --name postpilot-tunnel-sql --network "$net" --network-alias sqldb \
  -e ACCEPT_EULA=Y -e "MSSQL_SA_PASSWORD=$sa_password" -e MSSQL_PID=Developer \
  mcr.microsoft.com/mssql/server:2022-latest >/dev/null
docker run -d --name postpilot-bastion --network "$net" -p 2222:2222 \
  -e USER_NAME=tunnel -e "USER_PASSWORD=$ssh_password" -e PASSWORD_ACCESS=true \
  -e "PUBLIC_KEY=$(cat "$dir/id_plain.pub" "$dir/id_locked.pub" "$dir/id_rsa.pub")" \
  linuxserver/openssh-server >/dev/null
# A imagem vem com o encaminhamento de portas desligado.
for _ in $(seq 30); do
  docker exec postpilot-bastion test -f /config/sshd/sshd_config 2>/dev/null && break
  sleep 1
done
docker exec postpilot-bastion \
  sed -i 's/^AllowTcpForwarding.*/AllowTcpForwarding yes/' /config/sshd/sshd_config
docker restart postpilot-bastion >/dev/null
echo "esperando o SQL Server..."
for _ in $(seq 60); do
  docker logs postpilot-tunnel-sql 2>&1 | grep -q 'Recovery is complete' && break
  sleep 2
done

export SSH_TEST_HOST=127.0.0.1 SSH_TEST_PORT=2222 SSH_TEST_USER=tunnel
export SSH_TEST_PASSWORD="$ssh_password" SSH_TEST_KEYS="$dir"
export SSH_TEST_KEY_PASSPHRASE='frase secreta'
export SSH_TEST_PG=pgdb:5432 SSH_TEST_PG_PASSWORD=secret
export SSH_TEST_MSSQL=sqldb:1433 SSH_TEST_MSSQL_PASSWORD="$sa_password"

cargo test -p pgstudio-ssh
cargo test -p pgstudio-pg-client --test tunnel
cargo test -p pgstudio-mssql --test tunnel
cargo test -p pgstudio-ui-shell --test ssh_tunnel
# Sozinho, porque reinicia o servidor SSH: o túnel aberto tem de voltar.
SSH_TEST_BASTION_CONTAINER=postpilot-bastion \
  cargo test -p pgstudio-ssh --test tunnel tunnel_comes_back
