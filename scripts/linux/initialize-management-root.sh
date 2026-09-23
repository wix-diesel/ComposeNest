#!/usr/bin/env bash
# Initialize the Ubuntu management root for one explicitly selected user.
set -euo pipefail

readonly MANAGEMENT_ROOT=/var/lib/composenest
readonly DIRECTORIES=(locks state templates templates/local instances data ownership staging diagnostics)

usage() {
  echo "Usage: sudo $0 <existing non-root username>" >&2
  exit 2
}

check_directory() {
  local path=$1 uid=$2 gid=$3
  if [[ -L $path || ! -d $path ]]; then
    echo "Refusing a link or non-directory: $path" >&2
    return 1
  fi
  local actual_uid actual_gid actual_mode
  read -r actual_uid actual_gid actual_mode < <(stat -c '%u %g %a' -- "$path") || return 1
  if [[ $actual_uid != "$uid" || $actual_gid != "$gid" || $actual_mode != 700 ]]; then
    echo "Refusing existing directory with different owner or mode: $path" >&2
    return 1
  fi
}

create_directory() {
  local path=$1 uid=$2 gid=$3
  if [[ -e $path || -L $path ]]; then
    check_directory "$path" "$uid" "$gid"
  else
    install -d -m 0700 -o "$uid" -g "$gid" -- "$path"
  fi
}

initialize_root() {
  local root=$1 username=$2
  local uid gid directory
  uid=$(id -u "$username") || return 1
  gid=$(id -g "$username") || return 1
  if [[ $uid == 0 ]]; then
    echo 'The management user must not be root.' >&2
    return 1
  fi

  # Check the existing tree before creating anything; never repair it recursively.
  for directory in "$root"; do
    if [[ -e $directory || -L $directory ]]; then
      check_directory "$directory" "$uid" "$gid" || return 1
    fi
  done
  for directory in "${DIRECTORIES[@]}"; do
    if [[ -e $root/$directory || -L $root/$directory ]]; then
      check_directory "$root/$directory" "$uid" "$gid" || return 1
    fi
  done

  umask 077
  create_directory "$root" "$uid" "$gid" || return 1
  for directory in "${DIRECTORIES[@]}"; do
    create_directory "$root/$directory" "$uid" "$gid" || return 1
  done
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
  [[ $# == 1 ]] || usage
  [[ $EUID == 0 ]] || { echo 'Run initial setup as root.' >&2; exit 1; }
  initialize_root "$MANAGEMENT_ROOT" "$1"
fi
