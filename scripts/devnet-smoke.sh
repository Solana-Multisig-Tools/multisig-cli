#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
real_home="${HOME}"

keypair="${MSIG_DEVNET_KEYPAIR:-${SOLANA_KEYPAIR:-${real_home}/.config/solana/id.json}}"
rpc="${MSIG_DEVNET_RPC:-https://api.devnet.solana.com}"
priority_fee="${MSIG_DEVNET_PRIORITY_FEE:-0}"
memo_program="${MSIG_DEVNET_MEMO_PROGRAM:-MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr}"
system_program="11111111111111111111111111111111"
token_program="TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
dev_usdc_mint="${MSIG_DEVNET_USDC_MINT:-4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU}"
dev_usdc_amount="${MSIG_DEVNET_USDC_AMOUNT:-0.000001}"
vault_sol_fund="${MSIG_DEVNET_VAULT_SOL_FUND:-0.001}"
smoke_delay="${MSIG_DEVNET_SMOKE_DELAY:-1}"
bin="${repo_root}/target/debug/msig"

if [[ "${MSIG_DEVNET_NO_DELAY:-0}" == "1" ]]; then
  smoke_delay="0"
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --delay)
      smoke_delay="${2:?--delay requires seconds}"
      shift 2
      ;;
    --no-delay)
      smoke_delay="0"
      shift
      ;;
    *)
      echo "unknown smoke flag: $1" >&2
      exit 1
      ;;
  esac
done

echo "Devnet smoke creates on-chain devnet artifacts and spends devnet rent; it does not clean them up."

if [[ "${smoke_delay}" == "0" || "${smoke_delay}" == "0.0" || "${smoke_delay}" == "0.00" ]]; then
  echo "Devnet smoke step delay: disabled"
else
  echo "Devnet smoke step delay: ${smoke_delay}s (use --no-delay or MSIG_DEVNET_NO_DELAY=1 to disable)"
fi

if [[ ! -f "${keypair}" ]]; then
  echo "devnet smoke keypair not found: ${keypair}" >&2
  echo "Set MSIG_DEVNET_KEYPAIR or create the default Solana keypair at ~/.config/solana/id.json." >&2
  exit 1
fi

if [[ -n "${MSIG_DEVNET_MEMBER:-}" ]]; then
  member="${MSIG_DEVNET_MEMBER}"
else
  command -v solana-keygen >/dev/null 2>&1 || {
    echo "MSIG_DEVNET_MEMBER is not set and solana-keygen is not available to derive it from ${keypair}" >&2
    exit 1
  }
  member="$(solana-keygen pubkey "${keypair}")"
fi

command -v solana >/dev/null 2>&1 || {
  echo "devnet smoke requires solana CLI to fund the vault for executable system-program coverage" >&2
  exit 1
}

command -v jq >/dev/null 2>&1 || {
  echo "devnet smoke requires jq" >&2
  exit 1
}

command -v python3 >/dev/null 2>&1 || {
  echo "devnet smoke requires python3" >&2
  exit 1
}

cmd_color=""
reset_color=""
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
  cmd_color=$'\033[36m'
  reset_color=$'\033[0m'
fi

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

display_cmd() {
  local args=("$@")
  if [[ ${#args[@]} -eq 0 ]]; then
    return
  fi

  if [[ -z "${bin:-}" || "${args[0]}" != "${bin}" ]]; then
    quote_cmd "${args[@]}"
    return
  fi

  args=("${args[@]:1}")
  local visible=()
  local command=()
  local i=0

  while (( i < ${#args[@]} )); do
    local arg="${args[i]}"
    if [[ "${arg}" != -* ]]; then
      command=("${args[@]:i}")
      break
    fi

    case "${arg}" in
      --cluster|--keypair|--commitment|--priority-fee)
        (( i += 2 ))
        ;;
      --yes)
        (( i += 1 ))
        ;;
      --output|--multisig|--vault-index|--program-id)
        visible+=("${arg}")
        if (( i + 1 < ${#args[@]} )); then
          visible+=("${args[i + 1]}")
          (( i += 2 ))
        else
          (( i += 1 ))
        fi
        ;;
      *)
        visible+=("${arg}")
        (( i += 1 ))
        ;;
    esac
  done

  local rendered=()
  if (( ${#command[@]} > 0 )); then
    rendered+=("${command[@]}")
  fi
  if (( ${#visible[@]} > 0 )); then
    rendered+=("${visible[@]}")
  fi
  quote_cmd "${rendered[@]}"
}

sleep_after_step() {
  case "${smoke_delay}" in
    ""|0|0.0|0.00)
      return
      ;;
  esac
  sleep "${smoke_delay}"
}

print_run_header() {
  local rendered
  rendered="$(display_cmd "$@")"
  printf "\nRunning command: %s%s%s\n" "${cmd_color}" "${rendered}" "${reset_color}"
}

extract_json_object() {
  python3 -c '
import json
import sys

text = sys.stdin.read()
decoder = json.JSONDecoder()
best = None
best_end = -1

for idx, char in enumerate(text):
    if char != "{":
        continue
    try:
        value, end = decoder.raw_decode(text[idx:])
    except json.JSONDecodeError:
        continue
    if not isinstance(value, dict):
        continue
    absolute_end = idx + end
    if text[absolute_end:].strip() == "":
        print(json.dumps(value))
        sys.exit(0)
    if absolute_end > best_end:
        best = value
        best_end = absolute_end

if best is None:
    print("no JSON object found in command output", file=sys.stderr)
    sys.exit(1)

print(json.dumps(best))
'
}

run() {
  print_run_header "$@"
  "$@"
  sleep_after_step
}

run_optional_quiet() {
  local output

  print_run_header "$@"
  if ! output="$("$@" 2>&1)"; then
    sleep_after_step
    return 1
  fi
  printf "%s\n" "${output}"
  sleep_after_step
}

capture_optional_plain() {
  local __var="$1"
  shift
  local output

  print_run_header "$@"
  if ! output="$("$@" 2>&1)"; then
    sleep_after_step
    return 1
  fi
  printf "%s\n" "${output}"
  printf -v "${__var}" "%s" "${output}"
  sleep_after_step
}

capture() {
  local __var="$1"
  shift
  local output
  local json_output

  print_run_header "$@"
  if ! output="$("$@" 2>&1)"; then
    printf "%s\n" "${output}"
    exit 1
  fi
  printf "%s\n" "${output}"
  if ! json_output="$(printf "%s" "${output}" | extract_json_object)"; then
    exit 1
  fi
  printf -v "${__var}" "%s" "${json_output}"
  sleep_after_step
}

token_account_exists() {
  spl-token --url "${rpc}" account-info --address "$1" >/dev/null 2>&1
}

skip_dev_usdc_smoke() {
  echo "USDC token smoke: not available"
}

token_balance_covers_smoke_amount() {
  python3 - "$1" "${dev_usdc_amount}" <<'PY'
from decimal import Decimal, InvalidOperation
import re
import sys

balance_text = sys.argv[1]
required_text = sys.argv[2]
match = re.search(r"[-+]?\d+(?:\.\d+)?", balance_text)
if not match:
    sys.exit(1)

try:
    balance = Decimal(match.group(0))
    required = Decimal(required_text)
except InvalidOperation:
    sys.exit(1)

sys.exit(0 if balance >= required else 1)
PY
}

create_normal_token_account() {
  local __var="$1"
  local owner="$2"
  local name="$3"
  local account_keypair="${tmp_dir}/${name}.json"
  local account_address

  if ! run_optional_quiet solana-keygen new \
    --no-bip39-passphrase \
    --silent \
    --outfile "${account_keypair}"; then
    return 1
  fi

  account_address="$(solana-keygen pubkey "${account_keypair}")" || return 1

  if ! run_optional_quiet spl-token \
    --url "${rpc}" \
    create-account "${dev_usdc_mint}" "${account_keypair}" \
    --owner "${owner}" \
    --fee-payer "${keypair}"; then
    return 1
  fi

  printf -v "${__var}" "%s" "${account_address}"
}

setup_dev_usdc_smoke() {
  local vault_address="$1"
  local source_balance

  dev_usdc_vault_account=""
  dev_usdc_member_account=""

  if ! command -v spl-token >/dev/null 2>&1; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! capture_optional_plain source_balance spl-token \
    --url "${rpc}" \
    balance "${dev_usdc_mint}" \
    --owner "${keypair}"; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! token_balance_covers_smoke_amount "${source_balance}"; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! create_normal_token_account dev_usdc_member_account "${member}" "dev-usdc-member-token-account"; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! create_normal_token_account dev_usdc_vault_account "${vault_address}" "dev-usdc-vault-token-account"; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! run_optional_quiet spl-token \
    --url "${rpc}" \
    --fee-payer "${keypair}" \
    transfer "${dev_usdc_mint}" "${dev_usdc_amount}" "${dev_usdc_vault_account}" \
    --owner "${keypair}"; then
    skip_dev_usdc_smoke
    return 1
  fi

  if ! token_account_exists "${dev_usdc_vault_account}" || ! token_account_exists "${dev_usdc_member_account}"; then
    skip_dev_usdc_smoke
    return 1
  fi

  return 0
}

if [[ "${MSIG_DEVNET_SKIP_REBUILD:-0}" != "1" ]]; then
  rm -f "${bin}" "${bin}.d"
fi

run cargo build --manifest-path "${repo_root}/Cargo.toml" --locked

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT
mkdir -p "${tmp_dir}/home" "${tmp_dir}/work"
export HOME="${tmp_dir}/home"
cd "${tmp_dir}/work"

common=(
  "${bin}"
  --cluster "${rpc}"
  --keypair "${keypair}"
  --commitment confirmed
  --priority-fee "${priority_fee}"
  --yes
)

json_common=("${common[@]}" --output json)
table_common=("${common[@]}" --output table)

run "${bin}" --version
run "${bin}" --help
run "${common[@]}" config set default.cluster "${rpc}"
run "${common[@]}" config set default.keypair "${keypair}"
run "${common[@]}" config set default.commitment confirmed
run "${common[@]}" config set default.vault_index 0
run "${common[@]}" config set tokens.DEV_USDC "${dev_usdc_mint}"
run "${table_common[@]}" config show
run "${json_common[@]}" config show
run "${table_common[@]}" config doctor
run "${json_common[@]}" config preflight

capture create_json "${json_common[@]}" multisig create \
  --threshold 1 \
  --members "${member}" \
  --rent-collector "${member}"
multisig="$(jq -r '.multisig_address' <<<"${create_json}")"
if [[ -z "${multisig}" || "${multisig}" == "null" ]]; then
  echo "failed to parse multisig_address from create output" >&2
  exit 1
fi

run "${common[@]}" config set default.multisig "${multisig}"
run "${common[@]}" config set "labels.${member}" smoke-member
run "${table_common[@]}" --multisig "${multisig}" multisig info
run "${json_common[@]}" --multisig "${multisig}" multisig info
capture vault_json "${json_common[@]}" --multisig "${multisig}" --vault-index 0 vault balance
vault_address="$(jq -r '.vault_address' <<<"${vault_json}")"
if [[ -z "${vault_address}" || "${vault_address}" == "null" ]]; then
  echo "failed to parse vault_address from vault balance output" >&2
  exit 1
fi

run solana \
  --url "${rpc}" \
  --keypair "${keypair}" \
  transfer "${vault_address}" "${vault_sol_fund}" \
  --allow-unfunded-recipient

run "${table_common[@]}" --multisig "${multisig}" --vault-index 0 vault balance
run "${json_common[@]}" --multisig "${multisig}" --vault-index 0 vault balance
run "${table_common[@]}" --multisig "${multisig}" member list
run "${json_common[@]}" --multisig "${multisig}" member list

cat > memo-template.toml <<'TOML'
id = "devnet.smoke.memo"
version = "1"
description = "Devnet smoke memo template"

[inputs.memo]
type = "bytes"
description = "Memo bytes"

[accounts.memo_program]
const = "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr"

[[instructions]]
program = "memo_program"
data = [
  { input = "memo" },
]
TOML

run "${table_common[@]}" template inspect memo-template.toml
run "${json_common[@]}" template inspect memo-template.toml
run "${table_common[@]}" --multisig "${multisig}" template validate memo-template.toml \
  --input memo=utf8:validated

capture template_json "${json_common[@]}" --multisig "${multisig}" template run memo-template.toml \
  --vault-index 0 \
  --input memo=utf8:template-proposal \
  --description "Devnet smoke template proposal"
template_index="$(jq -r '.transaction_index' <<<"${template_json}")"

run "${table_common[@]}" --multisig "${multisig}" proposal list --limit 10
run "${json_common[@]}" --multisig "${multisig}" proposal list --limit 10 --status Active
run "${table_common[@]}" --multisig "${multisig}" proposal pending --limit 10
run "${table_common[@]}" --multisig "${multisig}" proposal needs-me --limit 10
run "${table_common[@]}" --multisig "${multisig}" proposal show "${template_index}"
run "${table_common[@]}" --multisig "${multisig}" proposal show "${template_index}" --verbose
run "${json_common[@]}" --multisig "${multisig}" proposal show "${template_index}"
run "${table_common[@]}" --multisig "${multisig}" tx show "${template_index}"
run "${table_common[@]}" --multisig "${multisig}" tx list
run "${json_common[@]}" --multisig "${multisig}" tx list
run "${common[@]}" --multisig "${multisig}" tx export "${template_index}" \
  --action approve \
  --file template-approve.sqds
run "${common[@]}" tx status template-approve.sqds

capture tx_json "${json_common[@]}" --multisig "${multisig}" tx create \
  --vault-index 0 \
  --program "${memo_program}" \
  --data "utf8:tx-create-proposal" \
  --description "Devnet smoke tx create proposal"
tx_index="$(jq -r '.transaction_index' <<<"${tx_json}")"
tx_proposal="$(jq -r '.proposal' <<<"${tx_json}")"
if [[ -z "${tx_index}" || "${tx_index}" == "null" ]]; then
  echo "failed to parse transaction_index from tx create output" >&2
  exit 1
fi

run "${table_common[@]}" --multisig "${multisig}" proposal show "${tx_proposal}" --verbose
run "${table_common[@]}" --multisig "${multisig}" proposal approve "${tx_proposal}"
run "${table_common[@]}" --multisig "${multisig}" proposal executable --limit 10
run "${table_common[@]}" --multisig "${multisig}" proposal simulate "${tx_index}"
run "${table_common[@]}" --multisig "${multisig}" proposal simulate "${tx_proposal}" --verbose
run "${json_common[@]}" --multisig "${multisig}" proposal simulate "${tx_index}"
run "${table_common[@]}" --multisig "${multisig}" proposal execute "${tx_proposal}"

capture system_json "${json_common[@]}" --multisig "${multisig}" tx create \
  --vault-index 0 \
  --program "${system_program}" \
  --account "vault:writable:signer" \
  --account "${member}:writable" \
  --data "020000000100000000000000" \
  --description "Devnet smoke system transfer proposal"
system_index="$(jq -r '.transaction_index' <<<"${system_json}")"
system_proposal="$(jq -r '.proposal' <<<"${system_json}")"
if [[ -z "${system_index}" || "${system_index}" == "null" ]]; then
  echo "failed to parse transaction_index from system tx create output" >&2
  exit 1
fi

run "${table_common[@]}" --multisig "${multisig}" proposal show "${system_index}"
run "${table_common[@]}" --multisig "${multisig}" proposal approve "${system_proposal}"
run "${table_common[@]}" --multisig "${multisig}" proposal simulate "${system_index}"
run "${json_common[@]}" --multisig "${multisig}" proposal simulate "${system_proposal}"
run "${table_common[@]}" --multisig "${multisig}" proposal execute "${system_proposal}"

if setup_dev_usdc_smoke "${vault_address}"; then
  capture token_json "${json_common[@]}" --multisig "${multisig}" tx create \
    --vault-index 0 \
    --program "${token_program}" \
    --account "${dev_usdc_vault_account}:writable" \
    --account "${dev_usdc_member_account}:writable" \
    --account "vault:signer" \
    --data "030100000000000000" \
    --description "Devnet smoke token transfer proposal"
  token_index="$(jq -r '.transaction_index' <<<"${token_json}")"
  token_proposal="$(jq -r '.proposal' <<<"${token_json}")"
  if [[ -z "${token_index}" || "${token_index}" == "null" ]]; then
    echo "failed to parse transaction_index from token tx create output" >&2
    exit 1
  fi

  run "${table_common[@]}" --multisig "${multisig}" proposal show "${token_index}"
  run "${table_common[@]}" --multisig "${multisig}" proposal show "${token_index}" --verbose
  run "${table_common[@]}" --multisig "${multisig}" proposal approve "${token_proposal}"
  run "${table_common[@]}" --multisig "${multisig}" proposal simulate "${token_index}"
  run "${json_common[@]}" --multisig "${multisig}" proposal simulate "${token_proposal}"
  run "${table_common[@]}" --multisig "${multisig}" proposal execute "${token_proposal}"
  run "${table_common[@]}" --multisig "${multisig}" --vault-index 0 vault balance
fi

run "${table_common[@]}" --multisig "${multisig}" transfer sol 0 "${member}"
run "${table_common[@]}" --multisig "${multisig}" multisig set-threshold 1
run "${table_common[@]}" --multisig "${multisig}" multisig set-timelock 0
run "${table_common[@]}" --multisig "${multisig}" multisig add-spending-limit \
  --vault-index 0 \
  --mint native \
  --amount 1 \
  --period day \
  --members "${member}" \
  --destinations "${member}"
run "${table_common[@]}" --multisig "${multisig}" rent set-collector "${member}"

echo
echo "Devnet smoke passed"
echo "  multisig: ${multisig}"
echo "  executed memo proposal: #${tx_index} (${tx_proposal})"
echo "  executed system proposal: #${system_index} (${system_proposal})"
if [[ -n "${token_index:-}" && -n "${token_proposal:-}" ]]; then
  echo "  executed token proposal: #${token_index} (${token_proposal})"
fi
