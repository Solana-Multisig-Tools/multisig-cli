use super::{load_config_only, next_verb, parse_value, GlobalOpts};
use crate::error::{MsigError, OutputMode};

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "config")?;
    match verb.as_str() {
        "show" => cmd_show(globals),
        "set" => cmd_set(globals, &mut parser),
        "use" => cmd_use(&mut parser),
        "doctor" | "preflight" => cmd_doctor(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown config command: '{other}'"
        ))),
    }
}

fn cmd_show(globals: GlobalOpts) -> Result<(), MsigError> {
    let cfg = load_config_only(&globals)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());

    match output_mode {
        OutputMode::Json => {
            let json = serde_json::json!({
                "cluster": cfg.cluster,
                "keypair": cfg.keypair,
                "multisig": cfg.multisig,
                "commitment": cfg.commitment,
                "vault_index": cfg.vault_index,
                "priority_fee": cfg.priority_fee,
                "program_id": cfg.program_id.to_string(),
                "labels": cfg.labels,
                "tokens": cfg.tokens,
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&json).unwrap_or_default()
            );
        }
        OutputMode::Text => {
            println!("Resolved configuration:");
            println!("  cluster:      {}", cfg.cluster);
            println!(
                "  keypair:      {}",
                cfg.keypair.as_deref().unwrap_or("(none)")
            );
            println!(
                "  multisig:     {}",
                cfg.multisig.as_deref().unwrap_or("(none)")
            );
            println!("  commitment:   {}", cfg.commitment);
            println!("  vault_index:  {}", cfg.vault_index);
            println!("  priority_fee: {}", cfg.priority_fee);
            println!("  program_id:   {}", cfg.program_id);
            if !cfg.labels.is_empty() {
                println!("  labels:");
                for (k, v) in &cfg.labels {
                    println!("    {v} = {k}");
                }
            }
            if !cfg.tokens.is_empty() {
                println!("  tokens:");
                for (k, v) in &cfg.tokens {
                    println!("    {k} = {v}");
                }
            }
        }
    }
    Ok(())
}

fn cmd_set(_globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    let key = parse_value(parser, "key")?;
    let value = parse_value(parser, "value")?;
    crate::infra::config::file::save_config_value(&key, &value)?;
    println!("Saved: {key} = {value}");
    Ok(())
}

fn cmd_use(parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    let profile = parse_value(parser, "profile")?;
    crate::infra::config::file::validate_profile_exists(&profile)?;
    crate::infra::config::file::save_active_profile(&profile)?;
    println!("Switched to profile: {profile}");
    Ok(())
}

#[derive(Debug, serde::Serialize)]
struct DoctorReport {
    summary: DoctorSummary,
    config: DoctorConfig,
    project_config: ProjectConfigReport,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug, serde::Serialize)]
struct DoctorSummary {
    ready: bool,
    errors: usize,
    warnings: usize,
}

#[derive(Debug, serde::Serialize)]
struct DoctorConfig {
    cluster: String,
    cluster_kind: String,
    commitment: String,
    keypair: Option<String>,
    ledger: bool,
    multisig: Option<String>,
    vault_index: u8,
    priority_fee: u64,
    program_id: String,
}

#[derive(Debug, serde::Serialize)]
struct ProjectConfigReport {
    present: bool,
    path: Option<String>,
    trusted: bool,
}

#[derive(Debug, serde::Serialize)]
struct DoctorCheck {
    status: &'static str,
    name: &'static str,
    detail: String,
}

fn cmd_doctor(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    ensure_no_args(parser, "config doctor")?;
    let cfg = load_config_only(&globals)?;
    let project_path = crate::infra::config::file::project_config_path();
    let project_present = project_path
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false);
    let project_trusted = trust_project_config_env();
    let cluster_kind = classify_cluster(&cfg.cluster);
    let ledger = globals.ledger.is_some();
    let mut checks = Vec::new();

    match cluster_kind {
        "official-mainnet" => push_check(
            &mut checks,
            "ok",
            "cluster",
            format!("official Solana mainnet RPC ({})", cfg.cluster),
        ),
        "official-devnet" | "official-testnet" => push_check(
            &mut checks,
            "ok",
            "cluster",
            format!("official Solana non-mainnet RPC ({})", cfg.cluster),
        ),
        "localhost" => push_check(
            &mut checks,
            "warn",
            "cluster",
            format!("localhost RPC ({}) is for local testing only", cfg.cluster),
        ),
        _ => push_check(
            &mut checks,
            "warn",
            "cluster",
            format!("custom RPC ({}) must be operator-trusted", cfg.cluster),
        ),
    }

    if cfg.commitment == "processed" {
        push_check(
            &mut checks,
            "warn",
            "commitment",
            "processed commitment is fast but weak for signing review",
        );
    } else {
        push_check(
            &mut checks,
            "ok",
            "commitment",
            format!("{} commitment", cfg.commitment),
        );
    }

    match cfg.keypair.as_deref() {
        Some(path) if std::path::Path::new(path).exists() => push_check(
            &mut checks,
            "ok",
            "keypair",
            format!("configured keypair exists at {path}"),
        ),
        Some(path) => push_check(
            &mut checks,
            "error",
            "keypair",
            format!("configured keypair does not exist at {path}"),
        ),
        None if ledger => push_check(
            &mut checks,
            "ok",
            "signer",
            "Ledger signer selected for this command",
        ),
        None => push_check(
            &mut checks,
            "warn",
            "signer",
            "no keypair configured and --ledger was not passed",
        ),
    }

    if cfg.multisig.is_some() {
        push_check(
            &mut checks,
            "ok",
            "multisig",
            "multisig address is configured",
        );
    } else {
        push_check(
            &mut checks,
            "warn",
            "multisig",
            "no default multisig configured",
        );
    }

    if cfg.program_id == crate::infra::pda::PROGRAM_ID {
        push_check(
            &mut checks,
            "ok",
            "program",
            format!("default Squads v4 program {}", cfg.program_id),
        );
    } else {
        push_check(
            &mut checks,
            "warn",
            "program",
            format!("custom program id {} must be intentional", cfg.program_id),
        );
    }

    if project_present && project_trusted {
        push_check(
            &mut checks,
            "warn",
            "project-config",
            "MSIG_TRUST_PROJECT_CONFIG=1 allows sensitive fields from .msig.toml",
        );
    } else if project_present {
        push_check(
            &mut checks,
            "ok",
            "project-config",
            ".msig.toml present; only restricted safe fields were auto-loaded",
        );
    } else {
        push_check(
            &mut checks,
            "ok",
            "project-config",
            "no current-directory .msig.toml was loaded",
        );
    }

    if cfg.priority_fee == 0 && cluster_kind == "official-mainnet" {
        push_check(
            &mut checks,
            "warn",
            "priority-fee",
            "priority fee is 0 on mainnet; set one explicitly when congestion matters",
        );
    } else {
        push_check(
            &mut checks,
            "ok",
            "priority-fee",
            format!("{} microlamports/CU", cfg.priority_fee),
        );
    }

    let errors = checks
        .iter()
        .filter(|check| check.status == "error")
        .count();
    let warnings = checks.iter().filter(|check| check.status == "warn").count();
    let report = DoctorReport {
        summary: DoctorSummary {
            ready: errors == 0,
            errors,
            warnings,
        },
        config: DoctorConfig {
            cluster: cfg.cluster,
            cluster_kind: cluster_kind.to_string(),
            commitment: cfg.commitment,
            keypair: cfg.keypair,
            ledger,
            multisig: cfg.multisig,
            vault_index: cfg.vault_index,
            priority_fee: cfg.priority_fee,
            program_id: cfg.program_id.to_string(),
        },
        project_config: ProjectConfigReport {
            present: project_present,
            path: project_path.map(|path| path.display().to_string()),
            trusted: project_trusted,
        },
        checks,
    };

    match crate::output::detect_output_mode(globals.output.as_deref()) {
        OutputMode::Json => crate::output::json::print_json(&report),
        OutputMode::Text => print_doctor_text(&report),
    }

    Ok(())
}

fn ensure_no_args(parser: &mut lexopt::Parser, command: &str) -> Result<(), MsigError> {
    match parser.next() {
        Ok(None) => Ok(()),
        Ok(Some(_)) => Err(MsigError::Usage(format!(
            "{command} does not accept extra arguments"
        ))),
        Err(err) => Err(MsigError::Usage(err.to_string())),
    }
}

fn trust_project_config_env() -> bool {
    std::env::var("MSIG_TRUST_PROJECT_CONFIG")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}

fn classify_cluster(cluster: &str) -> &'static str {
    match cluster {
        "https://api.mainnet-beta.solana.com" => "official-mainnet",
        "https://api.devnet.solana.com" => "official-devnet",
        "https://api.testnet.solana.com" => "official-testnet",
        value
            if value.starts_with("http://127.0.0.1")
                || value.starts_with("http://localhost")
                || value.starts_with("https://127.0.0.1")
                || value.starts_with("https://localhost") =>
        {
            "localhost"
        }
        _ => "custom",
    }
}

fn push_check(
    checks: &mut Vec<DoctorCheck>,
    status: &'static str,
    name: &'static str,
    detail: impl Into<String>,
) {
    checks.push(DoctorCheck {
        status,
        name,
        detail: detail.into(),
    });
}

fn print_doctor_text(report: &DoctorReport) {
    println!("Config doctor");
    println!("  cluster:      {}", report.config.cluster);
    println!("  commitment:   {}", report.config.commitment);
    println!(
        "  signer:       {}",
        if report.config.ledger {
            "ledger"
        } else {
            report.config.keypair.as_deref().unwrap_or("(none)")
        }
    );
    println!(
        "  multisig:     {}",
        report.config.multisig.as_deref().unwrap_or("(none)")
    );
    println!("  vault_index:  {}", report.config.vault_index);
    println!("  priority_fee: {}", report.config.priority_fee);
    println!("  program_id:   {}", report.config.program_id);
    println!();

    let rows: Vec<Vec<String>> = report
        .checks
        .iter()
        .map(|check| {
            vec![
                format_status(check.status),
                check.name.to_string(),
                check.detail.clone(),
            ]
        })
        .collect();
    crate::output::table::print_table(&["STATUS", "CHECK", "DETAIL"], &rows);
    println!();
    println!(
        "Summary: {} error(s), {} warning(s)",
        report.summary.errors, report.summary.warnings
    );
}

fn format_status(status: &str) -> String {
    match status {
        "ok" => crate::output::table::colored("ok", "32"),
        "warn" => crate::output::table::colored("warn", "33"),
        "error" => crate::output::table::colored("error", "31"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_official_and_custom_clusters() {
        assert_eq!(
            classify_cluster("https://api.mainnet-beta.solana.com"),
            "official-mainnet"
        );
        assert_eq!(
            classify_cluster("https://api.devnet.solana.com"),
            "official-devnet"
        );
        assert_eq!(classify_cluster("http://127.0.0.1:8899"), "localhost");
        assert_eq!(classify_cluster("https://rpc.example.com"), "custom");
    }
}
