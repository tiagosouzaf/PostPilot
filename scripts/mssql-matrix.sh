#!/usr/bin/env bash
# Roda os testes do SQL Server contra cada versão suportada, em containers
# descartáveis (precisa de Docker). Uso: scripts/mssql-matrix.sh [versões]
#   scripts/mssql-matrix.sh            # 2017 2019 2022 2025
#   scripts/mssql-matrix.sh 2019 2022
set -u
cd "$(dirname "$0")/.."
versions=("$@")
[ ${#versions[@]} -eq 0 ] && versions=(2017 2019 2022 2025)
# Senha só do container de teste, que some no fim.
password="${MSSQL_TEST_PASSWORD:-Matriz-$RANDOM-$RANDOM-aA1}"
tests=()
for f in crates/ui-shell/tests/*_mssql.rs; do
    tests+=(--test "$(basename "$f" .rs)")
done
failed=0
summary=""
for v in "${versions[@]}"; do
    name="postpilot-matrix-$v"
    port=$((14400 + v % 100))
    docker rm -f "$name" >/dev/null 2>&1
    docker run -d --name "$name" -e ACCEPT_EULA=Y -e "MSSQL_SA_PASSWORD=$password" \
        -e MSSQL_PID=Developer -p "$port:1433" \
        "mcr.microsoft.com/mssql/server:$v-latest" >/dev/null || { failed=1; summary+="$v: não testada"$'\n'; continue; }
    for _ in $(seq 90); do
        docker logs "$name" 2>&1 | grep -q "Recovery is complete" && break
        sleep 2
    done
    if MSSQL_TEST_HOST=127.0.0.1 MSSQL_TEST_PORT=$port MSSQL_TEST_PASSWORD=$password \
        cargo test -p pgstudio-mssql -p pgstudio-ui-shell --test server --test designer "${tests[@]}"; then
        summary+="$v: ok"$'\n'
    else
        failed=1
        summary+="$v: FALHOU"$'\n'
    fi
    docker rm -f "$name" >/dev/null 2>&1
done
printf '\nSQL Server\n%s' "$summary"
exit $failed
