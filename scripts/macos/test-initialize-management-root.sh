#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/initialize-management-root.sh"

if [[ $(id -u) == 0 ]]; then
  username=${SUDO_USER:-}
else
  username=$(id -un)
fi
if [[ -z $username || $username == root ]]; then
  echo 'Run this test as a non-root user, or through sudo from one.' >&2
  exit 1
fi
uid=$(id -u "$username")
gid=$(id -g "$username")

run_as_owner() {
  if [[ $(id -u) == 0 ]]; then
    sudo -u "$username" "$@"
  else
    "$@"
  fi
}

test_root=$(run_as_owner mktemp -d -t composenest)
trap 'rm -rf "$test_root"' EXIT
root="$test_root/日本語 path/ComposeNest"
run_as_owner mkdir -p "$(dirname "$root")"

initialize_root "$root" "$username"
initialize_root "$root" "$username"
[[ $(stat -f '%u:%g:%Lp' "$root/state") == "$uid:$gid:700" ]]

# The GUI user can write a protected state file without elevated privileges.
run_as_owner sh -c 'umask 077; printf %s secret > "$1"' sh "$root/state/test.sqlite"
[[ $(stat -f '%u:%g:%Lp' "$root/state/test.sqlite") == "$uid:$gid:600" ]]
[[ $(ls -le "$root/state/test.sqlite") != *$'\n'* ]]

# A container may own a bind data child without gaining access to state.
mkdir -m 0700 "$root/data/image-owned"
if [[ $(id -u) == 0 ]]; then
  chown nobody:nobody "$root/data/image-owned"
else
  chmod 000 "$root/data/image-owned"
fi
[[ $(stat -f '%u:%Lp' "$root/data") == "$uid:700" ]]
run_as_owner test -w "$root/data"
run_as_owner test -r "$root/state/test.sqlite"
if run_as_owner test -r "$root/data/image-owned"; then
  echo 'Expected access to image-owned data to be denied.' >&2
  exit 1
fi
if [[ $(id -u) != 0 ]]; then
  chmod 0700 "$root/data/image-owned"
fi

# Refuse an existing foreign directory or a symlink without changing it.
if [[ $(id -u) == 0 ]]; then
  chown nobody:nobody "$root/staging"
else
  chmod 0755 "$root/staging"
fi
if initialize_root "$root" "$username"; then
  echo 'Expected mismatched ownership or mode to be rejected.' >&2
  exit 1
fi
if [[ $(id -u) == 0 ]]; then
  [[ $(stat -f '%u' "$root/staging") == "$(id -u nobody)" ]]
  chown "$uid:$gid" "$root/staging"
else
  [[ $(stat -f '%Lp' "$root/staging") == 755 ]]
  chmod 0700 "$root/staging"
fi
chmod +a 'everyone allow read' "$root/staging"
if initialize_root "$root" "$username"; then
  echo 'Expected an ACL entry to be rejected.' >&2
  exit 1
fi
chmod -a# 0 "$root/staging"
rmdir "$root/diagnostics"
ln -s "$root/state" "$root/diagnostics"
if initialize_root "$root" "$username"; then
  echo 'Expected symlink to be rejected.' >&2
  exit 1
fi
echo 'macOS root initialization checks passed.'
