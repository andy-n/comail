#!/usr/bin/env bash
# Re-review checklist before merging upstream into the fork (fork-only; see FORK.md).
#
#   git fetch upstream
#   scripts/fork/sync-review.sh <upstream-ref> [<base>]
#
# <base> defaults to merge-base(HEAD, <upstream-ref>): the last upstream commit
# the fork already contains. Upstream code is never executed: the script reads
# git objects, asks GitHub for PR review state (gh), and runs the lockfile
# scanners (cargo-deny, cargo-audit, osv-scanner, when installed) on a
# temp dir holding HEAD merged with <upstream-ref>.
# Exit status: 0 no red flags, 1 red flags (listed at the end), 2 usage error.
set -euo pipefail

# shellcheck source=scripts/fork/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() { sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

case ${1:-} in
  "" | -h | --help)
    usage
    [[ -n ${1:-} ]] && exit 0 || exit 2
    ;;
esac

cd "$FORK_ROOT"
fork_add_rust_paths

ref=$1
git rev-parse --verify --quiet "$ref^{commit}" >/dev/null || {
  warn "unknown ref '$ref' (did you git fetch upstream?)"
  exit 2
}
base=${2:-$(git merge-base HEAD "$ref")}
git rev-parse --verify --quiet "$base^{commit}" >/dev/null || {
  warn "unknown base '$base'"
  exit 2
}
range="$base..$ref"
lock_excludes=(':(exclude)src-tauri/Cargo.lock' ':(exclude)pnpm-lock.yaml')

flags=()
flag() {
  flags+=("$*")
  printf '    %sRED FLAG%s %s\n' "$C_RED" "$C_OFF" "$*"
}
note() { printf '    %sreview%s %s\n' "$C_YEL" "$C_OFF" "$*"; }
show_diff() { git --no-pager diff --stat --patch "$range" -- "$@" | sed 's/^/    | /'; }

slug=${UPSTREAM_REPO:-$(git remote get-url upstream 2>/dev/null | sed -E 's#^(https://github\.com/|git@github\.com:)##; s#\.git$##')}

step "Range $(git rev-parse --short "$base")..$(git rev-parse --short "$ref") ($ref)"
count=$(git rev-list --count --no-merges "$range")
if ((count == 0)); then
  ok "nothing new upstream"
  exit 0
fi
info "$count commits (G = good signature, N = unsigned):"
git --no-pager log --no-merges --format='%h %G? %<(16,trunc)%an %s' "$range" | sed 's/^/    /'

# --- 1. Who merged what, and was it reviewed? ----------------------------------
step "1. Pull requests and review state ($slug)"
if [[ -z $slug ]]; then
  flag "no 'upstream' remote and UPSTREAM_REPO unset: PR review state unchecked"
elif ! have gh || ! gh auth status >/dev/null 2>&1; then
  flag "gh missing or not logged in: PR review state unchecked (brew install gh && gh auth login)"
else
  prs=()
  for sha in $(git rev-list --no-merges "$range"); do
    nums=$(gh api "repos/$slug/commits/$sha/pulls" --jq '.[] | select(.merged_at != null) | .number')
    if [[ -z $nums ]]; then
      flag "pushed straight to the branch, no merged PR: $(git log -1 --format='%h %an: %s' "$sha")"
    else
      for n in $nums; do prs+=("$n"); done
    fi
  done
  for n in $(printf '%s\n' ${prs[@]+"${prs[@]}"} | sort -nu); do
    IFS=$'\t' read -r author merger head title < <(gh api "repos/$slug/pulls/$n" \
      --jq '[.user.login, (.merged_by.login // "-"), (.head.repo.full_name // "-"), .title] | @tsv')
    approvers=$(gh api --paginate "repos/$slug/pulls/$n/reviews" --jq '.[] | select(.state == "APPROVED") | .user.login' |
      grep -vxF "$author" | sort -u | paste -sd, - || true)
    info "#$n by $author from $head, merged by $merger, approved by ${approvers:-nobody}: $title"
    [[ -n $approvers ]] || flag "PR #$n merged without an approving review (author $author, merged by $merger)"
  done
fi

# --- 2. Build and execution surface -------------------------------------------
step "2. Build/execution surface"
red_paths=(.githooks .npmrc pnpm-workspace.yaml src-tauri/.cargo rust-toolchain.toml ':(glob)**/build.rs')
review_paths=(.github package.json vite.config.ts vitest.config.ts ':(glob)tsconfig*.json' scripts
  src-tauri/scripts src-tauri/swift ':(glob)**/Cargo.toml')
changed=$(git diff --name-only "$range" -- "${red_paths[@]}")
if [[ -n $changed ]]; then
  flag "runs on this Mac at install/build time, read every line: $(echo $changed)"
  show_diff "${red_paths[@]}"
fi
scripts_diff=$(git diff "$range" -- package.json | grep -E '^[-+][[:space:]]*"(pre|post)?(install|prepare|prepack|postpack)"' || true)
[[ -z $scripts_diff ]] || flag "package.json lifecycle scripts changed: $(echo $scripts_diff)"
changed=$(git diff --name-only "$range" -- "${review_paths[@]}")
if [[ -n $changed ]]; then
  note "changed: $(echo $changed)"
  show_diff "${review_paths[@]}"
fi

# --- 3. Lockfiles --------------------------------------------------------------
step "3. Lockfiles"
cargo_pkgs() { # name version source, one per [[package]]
  git show "$1:src-tauri/Cargo.lock" | awk '
    /^\[\[package\]\]/ { if (n != "") print n, v, s; n = v = s = "" }
    /^name = /    { n = $3 } /^version = / { v = $3 } /^source = / { s = $3 }
    END { if (n != "") print n, v, s }' | tr -d '"' | sort -u
}
npm_pkgs() { # name version, from the packages: section
  git show "$1:pnpm-lock.yaml" | awk '/^packages:/ { p = 1; next } /^[a-z]/ { p = 0 } p && /^  [^ ]/' |
    sed -E "s/^ +'?//; s/'?:\$//" | sed -E 's/^(.+)@([^@]+)$/\1 \2/' | sort -u
}
new_names() { comm -13 <(cut -d' ' -f1 <<<"$1" | sort -u) <(cut -d' ' -f1 <<<"$2" | sort -u); }

cb=$(cargo_pkgs "$base") cr=$(cargo_pkgs "$ref")
for name in $(new_names "$cb" "$cr"); do
  flag "new crate: $(grep "^$name " <<<"$cr" | cut -d' ' -f1-2 | paste -sd, -) (check age, owner, downloads, build.rs)"
done
bumps=$(comm -13 <(cut -d' ' -f1-2 <<<"$cb") <(cut -d' ' -f1-2 <<<"$cr") | paste -sd, - | sed 's/,/, /g')
[[ -z $bumps ]] || note "crate versions added/changed: $bumps"
odd=$(awk '$3 != "" && $3 != "registry+https://github.com/rust-lang/crates.io-index"' <<<"$cr")
[[ -z $odd ]] || flag "Cargo.lock has non-crates.io sources: $(echo $odd)"

nb=$(npm_pkgs "$base") nr=$(npm_pkgs "$ref")
for name in $(new_names "$nb" "$nr"); do
  flag "new npm package: $(grep "^$name " <<<"$nr" | paste -sd, -)"
done
bumps=$(comm -13 <(echo "$nb") <(echo "$nr") | paste -sd, - | sed 's/,/, /g')
[[ -z $bumps ]] || note "npm versions added/changed: $bumps"
if git show "$ref:pnpm-lock.yaml" | grep -qE 'tarball:|resolution: \{(repo|type: git|directory)'; then
  flag "pnpm-lock.yaml resolves packages outside the npm registry"
fi

# --- 4. Permissions: capabilities, CSP, tauri.conf -----------------------------
step "4. Capabilities, CSP and tauri.conf.json"
if [[ -n $(git diff --name-only "$range" -- src-tauri/capabilities) ]]; then
  flag "capabilities changed (what the webview may call)"
  show_diff src-tauri/capabilities
fi
csp() { git show "$1:src-tauri/tauri.conf.json" | jq -r '.app.security.csp // "<none>"'; }
if [[ $(csp "$base") != "$(csp "$ref")" ]]; then
  flag "CSP changed"
  info "before: $(csp "$base")"
  info "after:  $(csp "$ref")"
fi
if [[ -n $(git diff --name-only "$range" -- src-tauri/tauri.conf.json) ]]; then
  note "tauri.conf.json changed"
  show_diff src-tauri/tauri.conf.json
fi
commands=$(git diff "$range" -- src-tauri | grep -cE '^\+.*#\[tauri::command' || true)
((commands == 0)) || note "$commands new #[tauri::command] function(s): review what they expose to the webview"
git --no-pager diff --stat "$range" -- src-tauri/src/lib.rs src-tauri/crates/comail-core/src/mcp \
  src-tauri/crates/comail-core/src/ai | sed 's/^/    /'

# --- 5. Egress: new hosts, TLS ------------------------------------------------
step "5. Network egress"
host_re='https?://[A-Za-z0-9.-]+'
added_hosts=$(git diff "$range" -- . "${lock_excludes[@]}" | grep -E '^\+[^+]' | grep -oE "$host_re" | sed -E 's#^https?://##' | sort -u || true)
known_hosts=$(git grep -hoE "$host_re" "$base" -- . "${lock_excludes[@]}" | sed -E 's#^https?://##' | sort -u || true)
for host in $(comm -23 <(echo "$added_hosts") <(echo "$known_hosts")); do
  flag "new host: $host"
  git diff -U0 "$range" -- . "${lock_excludes[@]}" | grep -E "^\+.*https?://$host" | head -3 | sed 's/^/        /'
done
tls=$(git diff "$range" -- src-tauri "${lock_excludes[@]}" | grep -E '^\+.*(dangerous\(|danger_|NoCertificateVerification|accept_invalid_(certs|hostnames)|ServerCertVerifier|webpki)' || true)
[[ -z $tls ]] && ok "no TLS verification changes" || {
  flag "TLS verification code changed"
  sed 's/^/        /' <<<"$tls"
}
clients=$(git diff "$range" -- src-tauri "${lock_excludes[@]}" | grep -E '^\+.*(reqwest::Client|ClientBuilder|TcpStream::connect|UdpSocket::bind)' || true)
[[ -z $clients ]] || {
  note "new network clients/sockets:"
  sed 's/^/        /' <<<"$clients"
}

# --- 6. Merge preview: fork patches + scanners --------------------------------
step "6. Merge preview of $ref into HEAD"
# merge-tree computes the merge in the object store only: no worktree, index
# or ref changes. The merged tree is then unpacked into a temp dir.
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
wt="$tmp/merge"
if ! merged=$(git merge-tree --write-tree --name-only --no-messages HEAD "$ref"); then
  flag "merge conflicts (resolve, then re-run the scanners): $(tail -n +2 <<<"$merged" | paste -sd' ' -)"
else
  mkdir "$wt"
  git archive "$merged" | tar -x -C "$wt"
  ok "merges cleanly (tree $(git rev-parse --short "$merged"))"
  conf="$wt/src-tauri/tauri.conf.json"
  if grep -rqE 'tauri-plugin-updater|tauri_plugin_updater|plugin-updater|"updater:' \
    "$wt/src-tauri/Cargo.toml" "$wt/src-tauri/src" "$wt/src-tauri/capabilities" "$wt/package.json" "$wt/src" ||
    [[ $(jq '.plugins.updater != null or .bundle.createUpdaterArtifacts != null' "$conf") == true ]]; then
    flag "fork patch lost: the auto-updater is back"
  fi
  [[ $(jq -r '.bundle.macOS.signingIdentity // ""' "$conf") == - ]] || flag "fork patch lost: bundle.macOS.signingIdentity is no longer \"-\""
  [[ $(jq '.bundle.icon | index("icons/Icon.icon")' "$conf") == null ]] || flag "fork patch lost: icons/Icon.icon is back in bundle.icon"
  for m in src-tauri/Cargo.toml src-tauri/crates/comail-core/Cargo.toml; do
    grep -qx 'publish = false' "$wt/$m" || flag "fork patch lost: publish = false missing in $m"
  done

  scanners_run=0
  if have cargo-deny; then
    scanners_run=1
    if (cd "$wt/src-tauri" && cargo deny --log-level error --locked check >"$tmp/deny.log" 2>&1); then
      ok "cargo deny"
    else
      flag "cargo deny failed (a new build-script crate fails bans: read its build.rs first)"
      sed -n '/^error/,/^$/p' "$tmp/deny.log" | sed 's/^/        /'
      tail -1 "$tmp/deny.log" | sed 's/^/        /'
    fi
  else
    warn "cargo-deny not installed: Rust policy unchecked (brew install cargo-deny)"
  fi
  if have cargo-audit; then
    scanners_run=1
    if fork_cargo_audit "$wt"; then ok "cargo audit"; else flag "cargo audit: vulnerable crate in the macOS graph"; fi
  else
    info "cargo-audit not installed; skipped"
  fi
  if have osv-scanner; then
    scanners_run=1
    # Exit status is 1 whenever anything is found; the JSON is what we judge.
    (cd "$wt" && osv-scanner scan source --format json --lockfile src-tauri/Cargo.lock --lockfile pnpm-lock.yaml \
      >"$tmp/osv.json" 2>"$tmp/osv.log") || true
    if ! jq -e '.results' "$tmp/osv.json" >/dev/null 2>&1; then
      flag "osv-scanner produced no report (see $tmp/osv.log)"
    else
      ignores=$(fork_deny_ignores "$wt")
      graph=$(have cargo-deny && fork_deny_graph "$wt" || true)
      osv_bad=0
      # Non-informational findings: npm ones always count; crates count when the
      # crate is in the macOS graph and no id/alias is ignored in deny.toml.
      while IFS=$'\t' read -r eco pkg ids summary; do
        [[ -n $pkg ]] || continue
        if [[ $eco == crates.io ]]; then
          if grep -qxF -f <(tr ',' '\n' <<<"$ids") <<<"$ignores"; then continue; fi
          if [[ -n $graph ]] && ! grep -qxF "$pkg" <<<"$graph"; then
            info "osv lock-only, not in the macOS graph: $pkg ${ids%%,*} $summary"
            continue
          fi
        fi
        flag "osv: $pkg ${ids%%,*} $summary"
        osv_bad=1
      done < <(jq -r '.results[].packages[] | .package as $p | .vulnerabilities[]
        | select([.affected[]?.database_specific?.informational // empty] | length == 0)
        | [$p.ecosystem, $p.name + "@" + $p.version, ([.id] + (.aliases // []) | join(",")), (.summary // "")] | @tsv' "$tmp/osv.json")
      ((osv_bad)) || ok "osv-scanner (src-tauri/osv-scanner.toml ignores applied)"
    fi
  else
    info "osv-scanner not installed; skipped (brew install osv-scanner)"
  fi
  ((scanners_run)) || flag "no lockfile scanner installed: nothing checked the dependency graph"
fi

# --- Summary -------------------------------------------------------------------
step "Summary"
if ((${#flags[@]})); then
  printf '%s%d red flag(s):%s\n' "$C_RED" "${#flags[@]}" "$C_OFF"
  printf '  - %s\n' "${flags[@]}"
  exit 1
fi
ok "no red flags. Still read the 'review' diffs above before merging."
