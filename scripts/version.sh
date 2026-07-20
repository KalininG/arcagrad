#!/usr/bin/env bash
# Keeps the server and Tauri app on one product version (Cargo.toml).
# plugin-sdk versions independently.
# client/package.json is intentionally independent; Tauri uses tauri.conf.json.
#
# Usage:
#   scripts/version.sh get                 # print the current version
#   scripts/version.sh check [<ref>]       # assert server + client agree; if <ref>
#                                          #   (a tag like v0.2.0 or 0.2.0) is given,
#                                          #   also assert they equal it. Exit 1 on mismatch.
#   scripts/version.sh set <X.Y.Z>         # rewrite both files to X.Y.Z + sync Cargo.lock
#
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

CARGO="$ROOT/Cargo.toml"
TAURI="$ROOT/client/src-tauri/tauri.conf.json"

SEMVER_RE='^[0-9]+\.[0-9]+\.[0-9]+([-+.][0-9A-Za-z.-]+)?$'

die() { echo "error: $*" >&2; exit 1; }

cargo_ver() { grep -m1 '^version = "' "$1" | sed -E 's/^version = "([^"]+)".*/\1/'; }
tauri_ver() { grep -m1 '"version"' "$1" | sed -E 's/.*"version"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'; }

set_cargo() { sed -i.bak -E 's/^version = "[^"]*"/version = "'"$1"'"/' "$2" && rm -f "$2.bak"; }
set_tauri() { sed -i.bak -E 's/("version"[[:space:]]*:[[:space:]]*)"[^"]*"/\1"'"$1"'"/' "$2" && rm -f "$2.bak"; }

cmd_get() { cargo_ver "$CARGO"; }

cmd_check() {
  local vc vt
  vc="$(cargo_ver "$CARGO")"
  vt="$(tauri_ver "$TAURI")"

  if [[ "$vc" != "$vt" ]]; then
    echo "version drift — server and client disagree:" >&2
    printf '  %-34s %s\n' "Cargo.toml" "$vc" >&2
    printf '  %-34s %s\n' "client/src-tauri/tauri.conf.json" "$vt" >&2
    echo "run: scripts/version.sh set <X.Y.Z>" >&2
    exit 1
  fi

  if [[ $# -ge 1 && -n "${1:-}" ]]; then
    local want="${1#v}"
    if [[ "$want" != "$vc" ]]; then
      die "tag '$1' does not match committed version '$vc' — bump with scripts/version.sh set $want, commit, then re-tag"
    fi
    echo "ok: tag $1 == version $vc (server + client in sync)"
  else
    echo "ok: version $vc (server + client in sync)"
  fi
}

cmd_set() {
  local new="${1:-}"
  [[ -n "$new" ]] || die "usage: scripts/version.sh set <X.Y.Z>"
  new="${new#v}"
  [[ "$new" =~ $SEMVER_RE ]] || die "'$new' is not a valid semver (X.Y.Z[-pre][+build])"

  set_cargo "$new" "$CARGO"
  set_tauri "$new" "$TAURI"

  local vc vt
  vc="$(cargo_ver "$CARGO")"; vt="$(tauri_ver "$TAURI")"
  [[ "$vc" == "$new" && "$vt" == "$new" ]] \
    || die "bump incomplete (cargo=$vc tauri=$vt) — check the files by hand"

  # Refresh local Cargo.lock entries when Cargo is available.
  if command -v cargo >/dev/null 2>&1; then
    ( cd "$ROOT" && cargo metadata --offline --format-version 1 >/dev/null 2>&1 ) \
      || ( cd "$ROOT" && cargo metadata --format-version 1 >/dev/null 2>&1 ) || true
  fi

  echo "set version -> $new"
  echo "  Cargo.toml, client/src-tauri/tauri.conf.json (+ Cargo.lock)"
  echo "next: commit 'chore(release): v$new', open a PR, then tag the merge commit v$new"
}

main() {
  local sub="${1:-}"
  case "$sub" in
    get)   cmd_get ;;
    check) shift; cmd_check "${1:-}" ;;
    set)   shift; cmd_set "${1:-}" ;;
    *) echo "usage: scripts/version.sh {get|check [<ref>]|set <X.Y.Z>}" >&2; exit 2 ;;
  esac
}

main "$@"
