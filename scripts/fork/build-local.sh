#!/usr/bin/env bash
# Build Comail.app from this checkout on an Apple Silicon Mac with Xcode
# (fork-only; see FORK.md). Never downloads or runs installers itself: missing
# tools are reported with an install hint.
#
#   scripts/fork/build-local.sh            # checks + build, prints the .app path
#   scripts/fork/build-local.sh --install  # also back up and replace /Applications/Comail.app
set -euo pipefail

# shellcheck source=scripts/fork/lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

usage() { sed -n '2,7p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; }

install=0
for arg in "$@"; do
  case $arg in
    --install) install=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) die "unknown argument: $arg (see --help)" ;;
  esac
done

cd "$FORK_ROOT"
fork_add_rust_paths

APP_ID=com.deanoss.comail
INSTALLED=/Applications/Comail.app
BUNDLE="$FORK_ROOT/src-tauri/target/release/bundle/macos/Comail.app"

step "Checking prerequisites"
[[ $(uname -s) == Darwin && $(uname -m) == arm64 ]] || die "this script targets Apple Silicon macOS"

devdir=$(xcode-select -p 2>/dev/null || true)
[[ $devdir == *Xcode*.app/* ]] ||
  die "full Xcode is required (xcode-select -p: '${devdir:-none}'). Install Xcode from the App Store, then: sudo xcode-select -s /Applications/Xcode.app && sudo xcodebuild -license accept"
ok "$(xcodebuild -version | head -1) at $devdir"

have rustup || die "rustup not found. Install: brew install rustup (then open a new shell, or add /opt/homebrew/opt/rustup/bin to PATH)"
info "making sure the toolchain pinned in rust-toolchain.toml is installed (first run downloads it)"
rustup toolchain install >/dev/null 2>&1 || die "rustup toolchain install failed; run it by hand to see why"
want_rust=$(sed -nE 's/^channel = "([^"]+)".*/\1/p' rust-toolchain.toml)
[[ $(rustc --version) == "rustc $want_rust "* ]] ||
  die "rustc is '$(rustc --version)', expected $want_rust from rust-toolchain.toml (is a non-rustup cargo/rustc earlier on PATH?)"
ok "$(rustc --version)"

have node || die "node not found. Install: brew install node (or your version manager, e.g. mise use -g node@24)"
node_ok=$(node -p 'const [a, b] = process.versions.node.split(".").map(Number); a > 22 || (a === 22 && b >= 12) || (a === 20 && b >= 19)')
[[ $node_ok == true ]] || die "node $(node --version) is too old for vite 8 (needs 20.19+ or 22.12+). Install: brew install node"
ok "node $(node --version)"

want_pnpm=$(node -p 'require("./package.json").packageManager.split("@")[1]')
have pnpm || die "pnpm not found. Enable the pinned version: corepack enable pnpm (node 25+: npm install -g corepack first, or brew install pnpm)"
[[ $(pnpm --version) == "$want_pnpm" ]] ||
  die "pnpm is $(pnpm --version), package.json pins $want_pnpm. Run: corepack enable pnpm"
ok "pnpm $want_pnpm"

have cargo-deny || die "cargo-deny not found. Install: brew install cargo-deny (or cargo install --locked cargo-deny)"
ok "$(cargo deny --version)"

# Ad-hoc signing comes from tauri.conf.json (signingIdentity "-"). Keep any
# signing/notarization credentials from the shell out of the build, so they
# can neither override that nor reach dependency build scripts.
unset APPLE_SIGNING_IDENTITY APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD \
  APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH \
  TAURI_SIGNING_PRIVATE_KEY TAURI_SIGNING_PRIVATE_KEY_PASSWORD

step "Installing frontend dependencies (lockfile only, no lifecycle scripts)"
pnpm install --frozen-lockfile --ignore-scripts

step "Supply-chain checks"
(cd src-tauri && cargo deny --log-level error --locked check)
ok "cargo deny"
if have cargo-audit; then
  fork_cargo_audit "$FORK_ROOT" || die "cargo audit found a vulnerability in the macOS graph (see above)"
  ok "cargo audit"
else
  info "cargo-audit not installed; skipped (optional: brew install cargo-audit)"
fi

step "Building Comail.app"
pnpm tauri build --bundles app -- --locked
[[ -d $BUNDLE ]] || die "build finished but $BUNDLE is missing"

step "Result"
codesign --verify --deep --strict "$BUNDLE" || die "codesign --verify failed for $BUNDLE"
sig=$(codesign -dv --verbose=2 "$BUNDLE" 2>&1)
grep -E '^(Identifier|Format|CodeDirectory|Signature|TeamIdentifier|Runtime Version)=' <<<"$sig" | sed 's/^/    /' || true
grep -qx 'Signature=adhoc' <<<"$sig" || warn "expected an ad-hoc signature (tauri.conf.json bundle.macOS.signingIdentity = \"-\")"
info "app: $BUNDLE"
info "version: $(defaults read "$BUNDLE/Contents/Info" CFBundleShortVersionString), commit $(git rev-parse --short HEAD)$(git diff --quiet HEAD -- || echo ' (dirty worktree)')"

((install)) || {
  info "install with: $0 --install"
  exit 0
}

step "Installing to $INSTALLED"
if [[ $(osascript -e "application id \"$APP_ID\" is running" 2>/dev/null) == true ]]; then
  info "quitting Comail"
  osascript -e "tell application id \"$APP_ID\" to quit" >/dev/null
  for _ in $(seq 1 30); do
    [[ $(osascript -e "application id \"$APP_ID\" is running") == false ]] && break
    sleep 1
  done
  [[ $(osascript -e "application id \"$APP_ID\" is running") == false ]] ||
    die "Comail is still running after 30s; quit it yourself and re-run with --install"
fi

if [[ -d $INSTALLED ]]; then
  mkdir -p "$HOME/Backups"
  backup="$HOME/Backups/Comail-$(date +%Y%m%d-%H%M%S).zip"
  ditto -c -k --sequesterRsrc --keepParent "$INSTALLED" "$backup"
  ok "backed up the previous app to $backup"
fi

staging="/Applications/.Comail.app.new.$$"
trap 'rm -rf "$staging"' EXIT
ditto "$BUNDLE" "$staging"
rm -rf "$INSTALLED"
mv "$staging" "$INSTALLED"
codesign --verify --deep --strict "$INSTALLED"
ok "installed $INSTALLED"
info "Ad-hoc signature: on first launch macOS asks again for access to the saved"
info "Comail Keychain items. Check that the prompt names Comail, then click Always Allow."
