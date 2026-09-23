#!/usr/bin/env bash
set -euo pipefail

[[ $EUID == 0 ]] || { echo 'Run this test as root.' >&2; exit 1; }
source "$(dirname "$0")/initialize-management-root.sh"

test_root=$(mktemp -d)
trap 'rm -rf -- "$test_root"' EXIT
root="$test_root/日本語 path/composenest"
mkdir -p -- "$(dirname "$root")"
username=nobody
uid=$(id -u "$username")
gid=$(id -g "$username")

initialize_root "$root" "$username"
initialize_root "$root" "$username"
[[ $(stat -c '%u:%g:%a' "$root/state") == "$uid:$gid:700" ]]

# The GUI user can write a protected state file without elevated privileges.
chmod 755 "$test_root" "$(dirname "$root")"
runuser -u "$username" -- sh -c 'umask 077; printf secret > "$1"' sh "$root/state/test.sqlite"
[[ $(stat -c '%u:%g:%a' "$root/state/test.sqlite") == "$uid:$gid:600" ]]

# A container may own a bind data child without gaining access to state.
mkdir -- "$root/data/image-owned"
chown 12345:12345 -- "$root/data/image-owned"
[[ $(stat -c '%u:%a' "$root/data") == "$uid:700" ]]
runuser -u "$username" -- test -w "$root/data"
runuser -u "$username" -- test -r "$root/state/test.sqlite"
if runuser -u "$username" -- test -r "$root/data/image-owned"; then
  echo 'Expected access to image-owned data to be denied.' >&2
  exit 1
fi

# Refuse an existing foreign directory or a symlink without changing it.
chown 12345:12345 -- "$root/staging"
if initialize_root "$root" "$username"; then
  echo 'Expected mismatched ownership to be rejected.' >&2
  exit 1
fi
[[ $(stat -c '%u' "$root/staging") == 12345 ]]
chown "$uid:$gid" -- "$root/staging"
rm -d -- "$root/diagnostics"
ln -s -- "$root/state" "$root/diagnostics"
if initialize_root "$root" "$username"; then
  echo 'Expected symlink to be rejected.' >&2
  exit 1
fi
echo 'Ubuntu root initialization checks passed.'
