#!/usr/bin/env bash
# Give the live-test database servers a TLS certificate from a throwaway CA.
#
#   scripts/live-sources-tls.sh <postgres> <mysql> <mariadb> <mssql>
#
# Each argument is a running Docker container started from the matching
# image (postgres:16, mysql:8.4, mariadb:11.4, mssql/server:2022), or '-' to
# leave that server alone. The script generates a CA and one server
# certificate for localhost, 127.0.0.1 and the service names postgres, mysql,
# mariadb and mssql; copies them into each container; and switches TLS on,
# reloading where the server can and restarting the container where it
# cannot. Cleartext connections keep working: the live tests use both.
#
# The CA ends up inside every container, where the live tests read it back
# over their administrator connection (OTS_TEST_<DIALECT>_TLS_CA):
#
#   postgres  /var/lib/postgresql/ots-tls/ca.pem
#   mysql     /var/lib/mysql-files/ots-tls/ca.pem   (inside secure_file_priv)
#   mariadb   /var/lib/mysql-files/ots-tls/ca.pem
#   mssql     /var/opt/mssql/ots-tls/ca.pem
#
# The CI live-sources job runs this against its service containers; the same
# command works against containers started by hand (see each plugin's
# tests/live.rs). Throwaway material for throwaway servers: nothing here is
# a secret worth keeping.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  sed -n '2,25p' "$0" >&2
  exit 2
fi
pg=$1 my=$2 maria=$3 ms=$4

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

# A CA and a server certificate signed by it, RSA because SQL Server wants
# RSA, valid for two days because CI runs are short.
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj /CN=ots-live-test-ca \
  -keyout "$work/ca.key" -out "$work/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj /CN=ots-live-test-server \
  -keyout "$work/server.key" -out "$work/server.csr" 2>/dev/null
printf '%s\n' \
  'subjectAltName=DNS:localhost,IP:127.0.0.1,DNS:postgres,DNS:mysql,DNS:mariadb,DNS:mssql' \
  'basicConstraints=critical,CA:FALSE' \
  'keyUsage=critical,digitalSignature,keyEncipherment' \
  'extendedKeyUsage=serverAuth' >"$work/server.ext"
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.pem" -CAkey "$work/ca.key" \
  -CAcreateserial -days 2 -extfile "$work/server.ext" -out "$work/server.pem" 2>/dev/null

# restart <container>: a restart that times out on a slow host can leave the
# container stopped with its new configuration in place; start it again.
restart() {
  docker restart -t 60 "$1" >/dev/null || docker start "$1" >/dev/null
}

# install <container> <dir> <owner>: the CA, the certificate and its key,
# owned by the server's account, the key readable by it alone.
install() {
  docker exec -u 0 "$1" mkdir -p "$2"
  for f in ca.pem server.pem server.key; do
    docker cp -q "$work/$f" "$1:$2/$f"
  done
  docker exec -u 0 "$1" sh -c "chown -R $3 '$2' && chmod 600 '$2/server.key'"
}

if [ "$pg" != - ]; then
  d=/var/lib/postgresql/ots-tls
  install "$pg" "$d" postgres:postgres
  # TLS settings take effect on a reload.
  docker exec -u postgres "$pg" psql -q -v ON_ERROR_STOP=1 \
    -c "ALTER SYSTEM SET ssl_cert_file = '$d/server.pem'" \
    -c "ALTER SYSTEM SET ssl_key_file = '$d/server.key'" \
    -c "ALTER SYSTEM SET ssl = on" \
    -c "SELECT pg_reload_conf()" >/dev/null
  echo "postgres: TLS on ($pg)"
fi

if [ "$my" != - ]; then
  d=/var/lib/mysql-files/ots-tls
  install "$my" "$d" mysql:mysql
  # MySQL 8 swaps the TLS context of a running server.
  docker exec "$my" sh -c "MYSQL_PWD=\"\$MYSQL_ROOT_PASSWORD\" mysql -uroot -h127.0.0.1 -e \"
    SET PERSIST ssl_ca = '$d/ca.pem', ssl_cert = '$d/server.pem', ssl_key = '$d/server.key';
    ALTER INSTANCE RELOAD TLS;\""
  echo "mysql: TLS on ($my)"
fi

if [ "$maria" != - ]; then
  d=/var/lib/mysql-files/ots-tls
  install "$maria" "$d" mysql:mysql
  docker exec -u 0 "$maria" sh -c "printf '%s\n' '[mariadbd]' \
    'ssl_ca = $d/ca.pem' 'ssl_cert = $d/server.pem' 'ssl_key = $d/server.key' \
    > /etc/mysql/conf.d/ots-tls.cnf"
  restart "$maria"
  echo "mariadb: TLS on, restarted ($maria)"
fi

if [ "$ms" != - ]; then
  d=/var/opt/mssql/ots-tls
  install "$ms" "$d" mssql
  # SQL Server reads its certificate at start only. Encryption stays
  # optional, so the cleartext test still connects.
  docker exec -u 0 "$ms" sh -c "printf '%s\n' '[network]' \
    'tlscert = $d/server.pem' 'tlskey = $d/server.key' 'forceencryption = 0' \
    > /var/opt/mssql/mssql.conf && chown mssql /var/opt/mssql/mssql.conf"
  restart "$ms"
  echo "mssql: TLS on, restarted ($ms)"
fi
