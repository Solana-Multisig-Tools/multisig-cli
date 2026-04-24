#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tag=""
tag_provided=0
cargo_audit_version="${CARGO_AUDIT_VERSION:-0.22.1}"
cargo_deny_version="${CARGO_DENY_VERSION:-0.19.4}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      tag="${2:?--tag requires vMAJOR.MINOR.PATCH}"
      tag_provided=1
      shift 2
      ;;
    *)
      echo "unknown release dry-run flag: $1" >&2
      exit 1
      ;;
  esac
done

cd "${repo_root}"

quote_cmd() {
  local first=1
  for arg in "$@"; do
    if (( first )); then
      first=0
    else
      printf " "
    fi
    printf "%q" "${arg}"
  done
}

run() {
  echo
  echo "Running command: $(quote_cmd "$@")"
  "$@"
}

if [[ -n "${tag}" ]]; then
  if [[ ! "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "release tag must look like vMAJOR.MINOR.PATCH" >&2
    exit 1
  fi
  if [[ "$(git rev-list -n 1 "${tag}")" != "$(git rev-parse HEAD)" ]]; then
    echo "tag ${tag} does not point at HEAD" >&2
    exit 1
  fi
else
  tag="dry-run-$(git rev-parse --short HEAD)"
fi

package_flags=()
if [[ -n "$(git status --porcelain)" ]]; then
  if (( tag_provided )); then
    echo "working tree must be clean when --tag is provided" >&2
    exit 1
  fi
  echo "Working tree is dirty; using cargo package --allow-dirty for this untagged dry run."
  package_flags=(--allow-dirty)
fi

dist_dir="${repo_root}/target/release-dry-run"

run rustup toolchain install
host_target="$(rustc -vV | awk '/host:/ {print $2}')"
run rustup target add "${host_target}"

run cargo fetch --locked
run cargo install cargo-audit --version "${cargo_audit_version}" --locked
run cargo install cargo-deny --version "${cargo_deny_version}" --locked
run cargo audit --deny warnings
run cargo deny check advisories bans licenses sources
run cargo fmt --all --check
run cargo test --locked --all-features
run cargo clippy --locked --all-targets --all-features -- -D warnings
if (( ${#package_flags[@]} > 0 )); then
  run cargo package --locked "${package_flags[@]}"
else
  run cargo package --locked
fi
run cargo build --release --locked --no-default-features --features ledger --target "${host_target}"

rm -rf "${dist_dir}"
mkdir -p "${dist_dir}"

crate_path="$(find "${repo_root}/target/package" -maxdepth 1 -name 'msig-[0-9]*.crate' -print | sort | tail -n 1)"
if [[ -z "${crate_path}" ]]; then
  echo "crate artifact not found under target/package" >&2
  exit 1
fi

binary_name="msig-${tag}-${host_target}"
cp "${crate_path}" "${dist_dir}/"
cp "${repo_root}/target/${host_target}/release/msig" "${dist_dir}/${binary_name}"
(
  cd "${dist_dir}"
  shasum -a 256 * > SHA256SUMS
)

echo
echo "Release dry-run artifacts:"
ls -lh "${dist_dir}"
echo
cat "${dist_dir}/SHA256SUMS"
