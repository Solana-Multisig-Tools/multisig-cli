use sha2::{Digest, Sha256};

use super::{build_context, load_config_only, next_verb, parse_value, GlobalOpts};
use crate::application::{template, vault_tx};
use crate::error::{MsigError, OutputMode};
use crate::output::json;

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "template")?;
    match verb.as_str() {
        "run" => cmd_run(globals, &mut parser),
        "inspect" => cmd_inspect(globals, &mut parser),
        "validate" => cmd_validate(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown template command: '{other}'"
        ))),
    }
}

fn cmd_inspect(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut template_path: Option<String> = None;
    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(value) if template_path.is_none() => {
                template_path = Some(
                    value
                        .into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("template");
                return Ok(());
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for template inspect".into(),
                ));
            }
        }
    }

    let template_path =
        template_path.ok_or_else(|| MsigError::Usage("template file is required".into()))?;
    let path = std::path::Path::new(&template_path);
    let manifest = template::inspect_template_file(path)?;
    let checksum = file_sha256(path)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());

    match output_mode {
        OutputMode::Json => json::print_json(&serde_json::json!({
            "path": template_path,
            "sha256": checksum,
            "template": manifest,
        })),
        OutputMode::Text => {
            render_template_manifest(&manifest, &checksum);
        }
    }
    Ok(())
}

fn cmd_validate(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut template_path: Option<String> = None;
    let mut raw_inputs = template::RawTemplateInputs::new();

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(value) if template_path.is_none() => {
                template_path = Some(
                    value
                        .into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Long("input") => {
                let pair = parse_value(parser, "--input")?;
                add_input_pair(&mut raw_inputs, &pair)?;
            }
            Short('h') | Long("help") => {
                super::help::print_resource_help("template");
                return Ok(());
            }
            Long(name) => {
                let name = name.to_string();
                let flag = format!("--{name}");
                let value = parse_value(parser, &flag)?;
                raw_inputs.entry(name).or_default().push(value);
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for template validate".into(),
                ));
            }
        }
    }

    let template_path =
        template_path.ok_or_else(|| MsigError::Usage("template file is required".into()))?;
    let path = std::path::Path::new(&template_path);
    let (cfg, multisig, vault_index, vault) = template_context_inputs(&globals)?;
    let template_context = template::TemplateContext {
        multisig,
        vault,
        squads_program_id: cfg.program_id,
        labels: &cfg.labels,
    };
    let compiled = template::load_template_file(path, &raw_inputs, &template_context)?;
    let checksum = file_sha256(path)?;
    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());

    match output_mode {
        OutputMode::Json => json::print_json(&serde_json::json!({
            "action": "template_validate",
            "path": template_path,
            "sha256": checksum,
            "template": {
                "id": compiled.id,
                "version": compiled.version,
                "description": compiled.description,
            },
            "vault_index": vault_index,
            "vault": vault.to_string(),
            "instruction_count": compiled.instructions.len(),
        })),
        OutputMode::Text => {
            println!("Template validation: OK");
            println!("  SHA-256: {checksum}");
            preview_template(&compiled, vault_index, &vault, false);
        }
    }
    Ok(())
}

fn cmd_run(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;

    let mut template_path: Option<String> = None;
    let mut raw_inputs = template::RawTemplateInputs::new();
    let mut description_override: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Value(value) if template_path.is_none() => {
                template_path = Some(
                    value
                        .into_string()
                        .map_err(|_| MsigError::Usage("invalid UTF-8".into()))?,
                );
            }
            Long("input") => {
                let pair = parse_value(parser, "--input")?;
                add_input_pair(&mut raw_inputs, &pair)?;
            }
            Long("description") => {
                description_override = Some(parse_value(parser, "--description")?);
            }
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("template");
                return Ok(());
            }
            Long(name) => {
                let name = name.to_string();
                let flag = format!("--{name}");
                let value = parse_value(parser, &flag)?;
                raw_inputs.entry(name).or_default().push(value);
            }
            _ => {
                return Err(MsigError::Usage(
                    "unexpected argument for template run".into(),
                ));
            }
        }
    }

    let template_path =
        template_path.ok_or_else(|| MsigError::Usage("template file is required".into()))?;

    let output_mode = crate::output::detect_output_mode(globals.output.as_deref());
    let is_json = matches!(output_mode, OutputMode::Json);

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(globals.vault_index);
    let (vault, _) = crate::infra::pda::vault_pda(&multisig, vault_index, &ctx.config.program_id);
    let template_context = template::TemplateContext {
        multisig,
        vault,
        squads_program_id: ctx.config.program_id,
        labels: &ctx.config.labels,
    };

    let compiled = template::load_template_file(
        std::path::Path::new(&template_path),
        &raw_inputs,
        &template_context,
    )?;
    preview_template(&compiled, vault_index, &vault, true);

    let description = description_override
        .or_else(|| compiled.description.clone())
        .unwrap_or_else(|| format!("Run template {}", compiled.id));

    let result = vault_tx::create_vault_transaction_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        compiled.instructions.clone(),
        vault_index,
        description,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
        is_json,
    )?;

    if is_json {
        json::print_json(&serde_json::json!({
            "action": "template_run",
            "template": {
                "id": compiled.id,
                "version": compiled.version,
                "path": template_path,
            },
            "transaction_index": result.transaction_index,
            "vault_index": vault_index,
            "vault": result.vault.to_string(),
            "transaction": result.transaction.to_string(),
            "proposal": result.proposal.to_string(),
            "signature": result.signature,
        }));
    }

    Ok(())
}

fn add_input_pair(inputs: &mut template::RawTemplateInputs, pair: &str) -> Result<(), MsigError> {
    let (key, value) = pair.split_once('=').ok_or_else(|| {
        MsigError::Usage("--input expects KEY=VALUE, for example --input amount=100".into())
    })?;
    let key = key.trim();
    if key.is_empty() {
        return Err(MsigError::Usage("--input key cannot be empty".into()));
    }
    inputs
        .entry(key.to_string())
        .or_default()
        .push(value.to_string());
    Ok(())
}

fn template_context_inputs(
    globals: &GlobalOpts,
) -> Result<
    (
        crate::infra::config::Config,
        solana_pubkey::Pubkey,
        u8,
        solana_pubkey::Pubkey,
    ),
    MsigError,
> {
    let cfg = load_config_only(globals)?;
    let addr_str = globals
        .multisig
        .as_deref()
        .or(cfg.multisig.as_deref())
        .ok_or_else(|| MsigError::Config("no multisig address".into()))?;
    let resolved = crate::infra::config::labels::resolve_address(addr_str, &cfg.labels)?;
    let multisig = resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("invalid multisig address: '{resolved}'")))?;
    let vault_index = globals.vault_index.unwrap_or(cfg.vault_index);
    let (vault, _) = crate::infra::pda::vault_pda(&multisig, vault_index, &cfg.program_id);
    Ok((cfg, multisig, vault_index, vault))
}

fn file_sha256(path: &std::path::Path) -> Result<String, MsigError> {
    let bytes = std::fs::read(path).map_err(|e| {
        MsigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read template {}: {e}", path.display()),
        ))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn render_template_manifest(manifest: &template::TemplateManifest, checksum: &str) {
    match manifest.version.as_deref() {
        Some(version) => println!("Template: {}@{}", manifest.id, version),
        None => println!("Template: {}", manifest.id),
    }
    if let Some(description) = manifest.description.as_deref() {
        println!("  Description: {description}");
    }
    println!("  SHA-256: {checksum}");
    println!("  Instructions: {}", manifest.instruction_count);
    if manifest.inputs.is_empty() {
        println!("  Inputs: none");
    } else {
        println!("  Inputs:");
        for input in &manifest.inputs {
            let required = if input.required {
                "required"
            } else {
                "optional"
            };
            let default = input
                .default
                .as_deref()
                .map(|value| format!(", default={value}"))
                .unwrap_or_default();
            let description = input
                .description
                .as_deref()
                .map(|value| format!(" - {value}"))
                .unwrap_or_default();
            println!(
                "    {}: {} ({required}{default}){}",
                input.name, input.kind, description
            );
        }
    }
}

fn preview_template(
    compiled: &template::CompiledTemplate,
    vault_index: u8,
    vault: &solana_pubkey::Pubkey,
    stderr: bool,
) {
    let preview = render_template_preview(compiled, vault_index, vault);
    if stderr {
        eprint!("{preview}");
    } else {
        print!("{preview}");
    }
}

fn render_template_preview(
    compiled: &template::CompiledTemplate,
    vault_index: u8,
    vault: &solana_pubkey::Pubkey,
) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let _ = writeln!(out);
    match compiled.version.as_deref() {
        Some(version) => {
            let _ = writeln!(out, "Template: {}@{}", compiled.id, version);
        }
        None => {
            let _ = writeln!(out, "Template: {}", compiled.id);
        }
    }
    let _ = writeln!(out, "  Vault index: {vault_index}");
    let _ = writeln!(out, "  Vault:       {vault}");
    let _ = writeln!(out, "  Inner instructions: {}", compiled.instructions.len());
    for (idx, ix) in compiled.instructions.iter().enumerate() {
        let _ = writeln!(
            out,
            "    #{}: program {}, {} accounts, {} bytes data",
            idx + 1,
            ix.program_id,
            ix.accounts.len(),
            ix.data.len()
        );
        for account in &ix.accounts {
            let mut flags = Vec::new();
            if account.is_signer {
                flags.push("signer");
            }
            if account.is_writable {
                flags.push("writable");
            }
            if flags.is_empty() {
                let _ = writeln!(out, "        {}", account.pubkey);
            } else {
                let _ = writeln!(out, "        {} ({})", account.pubkey, flags.join(", "));
            }
        }
    }
    out
}
