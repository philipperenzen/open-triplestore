#!/bin/bash
# Cleanup stale Docker containers before docker compose up
# This prevents "container name already in use" conflicts

set -e

echo "Cleaning up stale containers..."

# Stop and remove any container belonging to this project, whatever compose
# named it: the fixed dev names (triplestore, triplestore-minio, …) as well as
# the project-prefixed variants other workspaces/compose runs produce
# (<dir>-triplestore-1, <dir>-minio-1 under a project dir containing
# "triplestore", ots-*). The pattern deliberately requires a project-specific
# component — never a bare service name like "minio" or "postgres" — so
# containers from unrelated projects are left alone.
docker ps -a --format "{{.Names}}" | grep -E '(^|[-_])(triplestore)([-_]|$)|^ots[-_]' | while read -r container; do
  echo "Removing stale container: $container"
  docker stop "$container" 2>/dev/null || true
  docker rm "$container" 2>/dev/null || true
done

echo "Cleanup complete. Ready to run: docker compose up --build"
