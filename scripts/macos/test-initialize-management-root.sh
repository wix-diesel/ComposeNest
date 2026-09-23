#!/usr/bin/env bash
set -euo pipefail

[[ $(id -u) == 0 ]] || { echo 'Run this test as root.' >&2; exit 1; }
source "$(dirname "$0")/initialize-management-root.sh"

username=${SUDO_USER:-}
if [[ -z $username || $username == root || $(id -u "$username") == 0 ]]; then
  echo 'Run this test through sudo from a non-root user.' >&2
  exit 1
fi
uid=$(id -u "$username")
gid=$(id -g "$username")
nobody_uid=$(id -u nobody)
nobody_gid=$(id -g nobody)

test_root=$(sudo -u "$username" mktemp -d -t composenest)
trap 'rm -rf "$test_root"' EXIT
root="$test_root/日本語 path/ComposeNest"
sudo -u "$username" mkdir -p "$(dirname "$root")"

initialize_root "$root" "$username"
initialize_root "$root" "$username"
[[ $(stat -f '%u:%g:%Lp' "$root/state") == "$uid:$gid:700" ]]

# The GUI user can write a protected state file without elevated privileges.
sudo -u "$username" sh -c 'umask 077; printf %s secret > "$1"' sh "$root/state/test.sqlite"
[[ $(stat -f '%u:%g:%Lp' "$root/state/test.sqlite") == "$uid:$gid:600" ]]

# A container may own a bind data child without gaining access to state.
mkdir -m 0700 "$root/data/image-owned"
chown "$nobody_uid:$nobody_gid" "$root/data/image-owned"
[[ $(stat -f '%u:%Lp' "$root/data") == "$uid:700" ]]
sudo -u "$username" test -w "$root/data"
sudo -u "$username" test -r "$root/state/test.sqlite"
if sudo -u "$username" test -r "$root/data/image-owned"; then
  echo 'Expected access to image-owned data to be denied.' >&2
  exit 1
fi

# Refuse an existing foreign directory or a symlink without changing it.
chown "$nobody_uid:$nobody_gid" "$root/staging"
if initialize_root "$root" "$username"; then
  echo 'Expected mismatched ownership to be rejected.' >&2
  exit 1
fi
[[ $(stat -f '%u' "$root/staging") == "$nobody_uid" ]]
chown "$uid:$gid" "$root/staging"
rmdir "$root/diagnostics"
ln -s "$root/state" "$root/diagnostics"
if initialize_root "$root" "$username"; then
  echo 'Expected symlink to be rejected.' >&2
  exit 1
fi
echo 'macOS root initialization checks passed.'
