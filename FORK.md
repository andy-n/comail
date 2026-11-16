# Personal fork of Comail

This is a personal fork of [NextOSP/comail](https://github.com/NextOSP/comail),
kept at [andy-n/comail](https://github.com/andy-n/comail). Its owner uses it
every day on macOS with a real mailbox. Nothing in this file is upstreamed. Fixes
that help everyone go upstream as separate PRs. The fork-only changes are
listed below.

## Why the fork exists

A security audit of upstream `d2384cd` (Nov 2026) found that the main risk is
**trusting whatever upstream ships**, more than any bug in the app. The audit
notes stay outside this repository (`~/security-audit-skill/comail/`):

| Note | What it covers |
|---|---|
| `own-audit/supply-chain.md` | Dependencies, CI, release signing, provenance. The hardening plan is in §6. |
| `own-audit/local-surface.md` | Parsing of mail input (invites, RRULE, attachments) and the MCP server. |
| `own-audit/egress-privacy.md` | Logs, remote images, AI providers, quick replies. |
| `run-1/REPORT.md`, `run-1/FINDINGS-DETAIL.md` | IMAP sync stall and raw headers written to logs. |

Main findings and how this fork handles them:

- **Signed auto-updates (SC-2, SC-3).** One upstream maintainer account can
  push a signed, notarized auto-update without review. The release signing
  secrets are also exposed to every build script.
  *Fork:* the updater is gone, and the app is built locally from a reviewed
  commit with no signing secrets in the environment.
- **Floating toolchain, no dependency policy (SC-10).** Upstream builds with
  whatever toolchain and dependency versions resolve that day.
  *Fork:* the toolchain is pinned in `rust-toolchain.toml`, and every build
  runs the `src-tauri/deny.toml` policy first: crates.io only, an allowlist of
  crates that may run a build script, and a written reason for every ignored
  advisory. pnpm refuses packages released less than 7 days ago and packages
  whose provenance gets weaker than in earlier versions
  (`pnpm-workspace.yaml`).
- **Crash, privacy and sync bugs.** These are fixed on their own branches and
  sent upstream as PRs.

## What differs from upstream (fork-only)

- No auto-updater: no `tauri-plugin-updater` or `tauri-plugin-process`, no
  update check at startup, no "Check for updates" button. You update by
  rebuilding (see below).
- `tauri.conf.json` uses ad-hoc signing (`signingIdentity: "-"`) and leaves
  `icons/Icon.icon` out. Xcode 26.6's `actool` crashes on that icon; the app
  keeps `icon.icns`.
- `rust-toolchain.toml`, `src-tauri/deny.toml`, `src-tauri/osv-scanner.toml`,
  and `publish = false` in both Cargo manifests.
- pnpm policy keys in `pnpm-workspace.yaml`.
- `scripts/fork/`: `build-local.sh` and `sync-review.sh`.
- The upstream workflows (`.github/workflows`) and the updater section of
  `docs/DEVELOPMENT.md` are still in the tree but don't apply to the fork,
  which has GitHub Actions turned off.

## Build on a Mac

You need Apple Silicon and Xcode from the App Store (opened once, license
accepted). Then:

```sh
brew install rustup node cargo-deny       # required
brew install cargo-audit osv-scanner gh   # optional; sync-review.sh uses them
corepack enable pnpm                      # gives you the pnpm version pinned in package.json
git clone https://github.com/andy-n/comail && cd comail
scripts/fork/build-local.sh               # or: scripts/fork/build-local.sh --install
```

`build-local.sh` does the following:

1. Checks the tools. If one is missing, it prints how to install it. It never
   downloads or runs an installer.
2. Runs `pnpm install --frozen-lockfile --ignore-scripts`. This skips every
   lifecycle script, including the root `prepare` hook. esbuild works without
   its postinstall.
3. Runs `cargo deny --locked check`, then `cargo audit` if it's installed.
   `cargo audit` fails only on a vulnerable crate that the macOS build actually
   uses and that `deny.toml` doesn't ignore.
4. Removes any `APPLE_*` and `TAURI_SIGNING_*` variables from the build
   environment, then runs `pnpm tauri build --bundles app -- --locked`.
5. Prints the path to the `.app` and a summary of its signature.

With `--install`, it also quits Comail, zips the current
`/Applications/Comail.app` to `~/Backups/Comail-<timestamp>.zip`, and copies
the new build into place. To roll back, unzip the backup into `/Applications`.

The first build takes about 10 minutes. Build on the Mac where you'll run the
app. A `.app` you download or copy from elsewhere gets quarantined, and an
ad-hoc signature can't pass Gatekeeper.

## Keychain and signing

Comail keeps OAuth tokens, passwords and API keys in the login Keychain. Each
item's access list names the apps that can read it without asking, by code
signature.

- **Ad-hoc signing is deliberate.** A stable self-signed or Developer ID
  identity would avoid Keychain prompts, but its private key would sit on this
  Mac. Any local process that could use that key could sign a binary that
  passes as Comail and reads the Keychain items without a prompt. With ad-hoc
  signing, there is no key to steal.
- **The cost: a new prompt after every rebuild.** Each build has a new
  signature, so macOS asks again for each item. Check that the prompt names
  Comail and that you just installed a build, then click **Always Allow**.
- Don't use `src-tauri/scripts/macos-dev-cert.sh` for this fork. It creates a
  persistent signing identity whose key any app can use.
- Switching from the upstream binary: remove the upstream app from each
  `comail` Keychain item's access list in Keychain Access, or delete the items
  and sign in again (supply-chain.md §6.10). The bundle identifier and
  `~/Library/Application Support/comail` stay the same, so your data carries
  over.

## Syncing with upstream

```sh
git remote add upstream https://github.com/NextOSP/comail.git   # once
git fetch upstream
scripts/fork/sync-review.sh upstream/master   # optional second argument: <base>
```

`sync-review.sh` never runs upstream code. It covers:

- **Commits and review.** Lists each upstream commit with its PR and that PR's
  GitHub review state (via `gh`). An unreviewed PR or a direct push is a red
  flag.
- **Diffs to read line by line.** Hooks, build scripts, `pnpm-workspace.yaml`,
  `.cargo`, `package.json`, workflows, `Cargo.toml`, capabilities, the CSP and
  `tauri.conf.json`.
- **Lockfiles.** New crate or npm package names, and sources outside the
  registry, are red flags. Version bumps are listed for review.
- **Network.** New hosts in the diff, changes to TLS verification, and new HTTP
  clients or sockets.
- **Merge preview.** Merges the upstream ref into HEAD in a temp dir
  (`git merge-tree`; your branch isn't touched). It checks that the fork
  patches still hold, then runs `cargo-deny`, `cargo-audit` and `osv-scanner`,
  whichever are installed.

The script exits with status 1 on any red flag. Look into every flag; read the
"review" diffs either way. Then:

1. Merge: `git merge --no-ff upstream/master`. Re-apply any fork patch the
   script reported as lost.
2. If `cargo deny` reports a crate that isn't allowed to run a build script,
   read that crate's `build.rs` first. Only then add the crate to
   `allow-build-scripts` (the comment there shows how to regenerate the list).
3. Remove ignores in `deny.toml` and `osv-scanner.toml` that no longer match
   anything. Delete the **TEMPORARY** block in `deny.toml` once
   `chore/security-bumps` is merged.
4. Run `scripts/fork/build-local.sh --install`. Write down the commit you
   reviewed (for example as a signed tag: `git tag -s andy-v<version>-<n>`).

Bump the Rust toolchain in its own commit, after reading the release notes.
