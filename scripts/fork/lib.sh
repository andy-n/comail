# shellcheck shell=bash
# Shared helpers for scripts/fork/*.sh (fork-only). Source it; do not run it.

FORK_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if [[ -t 1 ]]; then
  C_RED=$'\e[31m' C_YEL=$'\e[33m' C_GRN=$'\e[32m' C_BLD=$'\e[1m' C_OFF=$'\e[0m'
else
  C_RED='' C_YEL='' C_GRN='' C_BLD='' C_OFF=''
fi

step() { printf '\n%s==> %s%s\n' "$C_BLD" "$*" "$C_OFF"; }
info() { printf '    %s\n' "$*"; }
ok() { printf '    %sok%s %s\n' "$C_GRN" "$C_OFF" "$*"; }
warn() { printf '%swarning:%s %s\n' "$C_YEL" "$C_OFF" "$*" >&2; }
die() {
  printf '%serror:%s %s\n' "$C_RED" "$C_OFF" "$*" >&2
  exit 1
}
have() { command -v "$1" >/dev/null 2>&1; }

# Homebrew's rustup is keg-only, and `cargo install` puts tools in ~/.cargo/bin.
fork_add_rust_paths() {
  local d
  for d in "$HOME/.cargo/bin" /opt/homebrew/opt/rustup/bin; do
    if [[ -d $d && ":$PATH:" != *":$d:"* ]]; then PATH="$d:$PATH"; fi
  done
  export PATH
}

# RUSTSEC ids that <root>/src-tauri/deny.toml ignores (each with a reason there).
fork_deny_ignores() {
  grep -oE 'id = "RUSTSEC-[0-9]{4}-[0-9]{4}"' "$1/src-tauri/deny.toml" | grep -oE 'RUSTSEC-[0-9-]+' | sort -u
}

# name@version of every crate in cargo-deny's macOS graph for <root>.
fork_deny_graph() {
  (cd "$1/src-tauri" && cargo deny --locked list -l crate) | sed -E 's/ .*//' | sort -u
}

# cargo-audit as a second opinion to cargo-deny. cargo-audit scans every
# Cargo.lock entry for every platform, so it fails only on vulnerabilities in
# crates cargo-deny's macOS graph actually contains that deny.toml does not
# ignore; lock-only hits are printed as notes. Returns 1 on a real finding.
fork_cargo_audit() {
  local root=$1 json graph pkg id title bad=0
  local -a args=()
  for id in $(fork_deny_ignores "$root"); do args+=(--ignore "$id"); done
  # Exit status is 1 whenever anything is found; the JSON is what we judge.
  json=$(cd "$root/src-tauri" && cargo audit --json ${args[@]+"${args[@]}"} 2>/dev/null) || true
  if ! jq -e '.vulnerabilities' >/dev/null 2>&1 <<<"$json"; then
    warn "cargo audit produced no report (advisory-db fetch failed?)"
    return 1
  fi
  graph=$(fork_deny_graph "$root") || {
    warn "could not list the cargo-deny graph"
    return 1
  }
  while IFS=$'\t' read -r pkg id title; do
    [[ -n $pkg ]] || continue
    if grep -qxF "$pkg" <<<"$graph"; then
      printf '    %sVULNERABLE%s %s %s %s\n' "$C_RED" "$C_OFF" "$pkg" "$id" "$title"
      bad=1
    else
      info "lock-only, not in the macOS graph: $pkg $id $title"
    fi
  done < <(jq -r '.vulnerabilities.list[] | [.package.name + "@" + .package.version, .advisory.id, .advisory.title] | @tsv' <<<"$json")
  info "cargo-audit warnings (unmaintained/unsound/yanked; cargo-deny gates these): $(jq '[.warnings[]?[]?] | length' <<<"$json")"
  return "$bad"
}
