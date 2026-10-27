#!/usr/bin/env bash
# Print the cgroup v2 directory of a running container, so its `cpu.stat`
# can be bind-mounted into the goose container and sampled
# (tests/perf/README.md, NFR-0030). Exits 1 (printing nothing) when the
# directory cannot be found, e.g. on cgroup v1 or a remote Docker daemon;
# callers then run without CPU numbers.
#
# Usage: tests/perf/cgroup-path.sh <container name or id>
# Environment: CGROUP_ROOT (default /sys/fs/cgroup)
set -euo pipefail

container=${1:?usage: $0 <container>}
root=${CGROUP_ROOT:-/sys/fs/cgroup}
id=$(docker inspect -f '{{.Id}}' "$container")

# cgroupfs driver (docker/<id>) and systemd driver (system.slice/docker-<id>.scope).
for dir in "$root/docker/$id" "$root/system.slice/docker-$id.scope"; do
  if [ -r "$dir/cpu.stat" ]; then
    echo "$dir"
    exit 0
  fi
done
exit 1
