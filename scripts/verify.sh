#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

before=$(git status --porcelain --untracked-files=all)

mise lint
mise test

msrv=$(sed -n 's/^msrv = //p' clippy.toml)
rust_version=$(sed -n 's/^rust-version = //p' Cargo.toml)
if [[ $msrv != "$rust_version" ]]; then
  echo "clippy.toml msrv $msrv is not Cargo.toml rust-version $rust_version" >&2
  exit 1
fi

after=$(git status --porcelain --untracked-files=all)
if [[ $after != "$before" ]]; then
  echo "Verification changed the checkout. Tests write only to a temp dir:" >&2
  diff <(printf '%s\n' "$before") <(printf '%s\n' "$after") >&2 || true
  exit 1
fi
echo "Verified."
