#!/bin/bash
# Runs run.sh for each scenario named, then shows its answers:
#     tools/ace/backup/runmany.sh <scenario>... [-- show.py options]
here=$(cd "$(dirname "$0")" && pwd)
names=()
while [ $# -gt 0 ] && [ "$1" != -- ]; do names+=("$1"); shift; done
[ "$1" = -- ] && shift
for s in "${names[@]}"; do
    "$here/run.sh" "$s"
    echo "##### $s"
    python3 "$here/show.py" "$s" "$@"
done
