#!/bin/bash
# Cleanup stale Docker containers before docker compose up
# This prevents "container name already in use" conflicts

set -e

echo "Cleaning up stale containers..."

# Stop and remove the specific containers that might conflict
for container in triplestore-minio triplestore triplestore-postgres; do
  if docker ps -a --format "{{.Names}}" | grep -q "^${container}$"; then
    echo "Removing stale container: $container"
    docker stop "$container" 2>/dev/null || true
    docker rm "$container" 2>/dev/null || true
  fi
done

echo "Cleanup complete. Ready to run: docker compose up --build"
