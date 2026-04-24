pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn print_help() {
    println!("msig {VERSION} — Squads MultisigV4 CLI\n");
    println!("USAGE: msig [OPTIONS] <COMMAND>\n");
    println!("COMMANDS:");
    println!("  multisig  Create and inspect multisigs");
    println!("  vault     Inspect vault balances");
    println!("  member    Manage multisig members");
    println!("  proposal  View and vote on proposals");
    println!("  transfer  Create transfer proposals");
    println!("  template  Run fixed vault-transaction templates");
    println!("  tx        Inspect and export transactions");
    println!("  program   Program upgrade proposals");
    println!("  rent      Rent collector and reclaim");
    println!("  config    Manage configuration");
    println!();
    println!("GLOBAL OPTIONS:");
    println!("  --cluster <URL|MONIKER>   RPC endpoint (devnet, mainnet, or URL)");
    println!("  --keypair <FILE>          Path to keypair JSON");
    println!("  --ledger [N]              Use Ledger hardware wallet");
    println!("  --multisig <ADDR>         Multisig address");
    println!("  --vault-index <N>         Vault index (default: 0)");
    println!("  --output <json|table>     Output format");
    println!("  --commitment <LEVEL>      confirmed, finalized, processed");
    println!("  --priority-fee <MICRO>    Priority fee in microlamports/CU");
    println!("  --dry-run                 Simulate only, don't send");
    println!("  -y, --yes                 Skip confirmation prompts");
    println!("  --no-color                Disable ANSI colors");
    println!("  --version                 Print version");
    println!("  -h, --help                Print help");
}

pub fn print_resource_help(resource: &str) {
    match resource {
        "multisig" => {
            println!("msig multisig — Create and inspect multisigs\n");
            println!("  msig multisig create --threshold <N> --members <A,B,C> [--rent-collector <ADDR>]");
            println!("  msig multisig info                Show multisig config and members");
            println!("  msig multisig set-threshold <N>   Propose a threshold change");
            println!("  msig multisig set-timelock <SEC>  Propose a time-lock change");
            println!("  msig multisig add-spending-limit --mint <MINT|native> --amount <RAW>");
            println!("                 --period <one-time|day|week|month> --members <A,B>");
            println!("                                    Propose a vault spending limit");
            println!("  msig multisig remove-spending-limit <ADDR>");
            println!("                                    Propose removing a spending limit");
        }
        "vault" => {
            println!("msig vault — Inspect vault balances\n");
            println!("  msig vault balance [--vault-index <N>]  Show SOL and token balances");
        }
        "member" => {
            println!("msig member — Manage multisig members\n");
            println!("  msig member list                  List members and permissions");
            println!("  msig member add <ADDR> --permissions <P>  Propose adding a member");
            println!("  msig member remove <ADDR>         Propose removing a member");
        }
        "proposal" => {
            println!("msig proposal — View and vote on proposals\n");
            println!("  msig proposal list [--limit <N>] [--status <STATUS>]");
            println!("                                    List recent proposals");
            println!("                                    STATUS: active, approved, executed, rejected, cancelled");
            println!("  msig proposal pending [--limit <N>]");
            println!(
                "                                    List active/approved/executing proposals"
            );
            println!("  msig proposal executable [--limit <N>]");
            println!(
                "                                    List approved proposals ready to execute"
            );
            println!("  msig proposal needs-me [--limit <N>]");
            println!("                                    List active proposals the signer has not voted on");
            println!("  msig proposal show <INDEX|PROPOSAL_ADDR> [--verbose]");
            println!("                                    Show proposal details and decoded instructions");
            println!("  msig proposal simulate <INDEX|PROPOSAL_ADDR> [--verbose]");
            println!("                                    Simulate execution and show account/token balance diffs");
            println!("  msig proposal approve <INDEX|PROPOSAL_ADDR>");
            println!("                                    Vote to approve");
            println!("  msig proposal reject <INDEX|PROPOSAL_ADDR>");
            println!("                                    Vote to reject");
            println!("  msig proposal cancel <INDEX|PROPOSAL_ADDR>");
            println!("                                    Cancel a proposal");
            println!("  msig proposal execute <INDEX|PROPOSAL_ADDR>");
            println!("                                    Execute an approved proposal");
        }
        "transfer" => {
            println!("msig transfer — Create transfer proposals\n");
            println!("  msig transfer sol <AMOUNT> <RECIPIENT>  Transfer SOL");
            println!("  msig transfer spl <TOKEN> <AMOUNT> <RECIPIENT>  Transfer SPL token");
            println!();
            println!("  Use --vault-index <N> to transfer from a specific vault (default: 0)");
        }
        "template" => {
            println!("msig template — Run fixed vault-transaction templates\n");
            println!("  msig template inspect <FILE>      Show template inputs and SHA-256");
            println!("  msig template validate <FILE> [--input KEY=VALUE] [--KEY VALUE]");
            println!("                                    Compile and preview without creating a proposal");
            println!("  msig template run <FILE> [--input KEY=VALUE] [--KEY VALUE]");
            println!("                                    Create a proposal from a TOML template");
            println!();
            println!("  Templates are explicit files only. They can declare typed inputs, fixed accounts,");
            println!("  fixed instruction data, and for_each over pubkey[] inputs; they cannot run code or call RPC.");
            println!(
                "  Bytes/data inputs accept hex by default, plus base64:<DATA> or utf8:<TEXT>."
            );
        }
        "tx" => {
            println!("msig tx — Inspect and offline-sign transactions\n");
            println!("  msig tx show <INDEX>              Show transaction details");
            println!("  msig tx list                      List recent transactions");
            println!("  msig tx create [--vault-index <N>] --program <ADDR>");
            println!("                 [--account <ADDR[:writable][:signer]>] [--data <HEX|base64:...|utf8:...>]");
            println!("                                    Create a one-off custom vault transaction proposal");
            println!("  msig tx create [--vault-index <N>] --vault-message <HEX|base64:...>");
            println!("                                    Create from pre-serialized vault transaction message bytes");
            println!("  msig tx export <INDEX> [--action approve|reject|cancel|execute]");
            println!("                                    Export a signable .sqds transaction");
            println!("  msig tx status <FILE>             Verify and inspect a .sqds file");
            println!("  msig tx combine --out <FILE> <SIGNED.sqds>...");
            println!(
                "                                    Merge signatures from matching .sqds files"
            );
            println!("  msig tx import <FILE> [--sign] [--push]");
            println!(
                "                                    Sign and/or submit an offline transaction"
            );
        }
        "program" => {
            println!("msig program — Program upgrade proposals\n");
            println!("  msig program upgrade --program <ADDR> --buffer <ADDR> --spill <ADDR>");
        }
        "rent" => {
            println!("msig rent — Rent collector and reclaim\n");
            println!("  msig rent set-collector <ADDR>    Set rent collector address");
            println!("  msig rent reclaim [--last-n <N>]  Reclaim rent from closed accounts");
        }
        "config" => {
            println!("msig config — Manage configuration\n");
            println!("  msig config show                  Display resolved config");
            println!("  msig config doctor                Check local mainnet-readiness and trust settings");
            println!("  msig config preflight             Alias for config doctor");
            println!("  msig config set <KEY> <VALUE>     Set a config value");
            println!("  msig config use <PROFILE>         Switch active profile");
            println!();
            println!("  Auto-loads .msig.toml from cwd with restricted fields.");
            println!("  Set MSIG_TRUST_PROJECT_CONFIG=1 only after reviewing project config.");
        }
        _ => {
            println!("Unknown resource: {resource}. Run 'msig --help'.");
        }
    }
}
