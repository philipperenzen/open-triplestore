#!/bin/bash
# Cleanup stale Docker containers before docker compose up
# This prevents "container name already in use" conflicts

set -e

echo "Cleaning up stale containers..."

# Stop and remove any containers matching patterns from this project
# This catches variants from different workspaces/compose runs
docker ps -a --format "{{.Names}}" | grep -E '(open-triplestore|triplestore|ots-|minio)' | while read container; do
  echo "Removing: $container"
  docker stop "$container" 2>/dev/null || true
  docker rm "$container" 2>/dev/null || true
done

echo "Cleanup complete. Ready to run: docker compose up --build"
