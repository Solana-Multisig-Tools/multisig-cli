use super::{build_context, next_verb, parse_value, GlobalOpts};
use crate::application::program_upgrade;
use crate::error::MsigError;

pub fn run(globals: GlobalOpts, mut parser: lexopt::Parser) -> Result<(), MsigError> {
    let verb = next_verb(&mut parser, "program")?;
    match verb.as_str() {
        "upgrade" => cmd_upgrade(globals, &mut parser),
        other => Err(MsigError::Usage(format!(
            "unknown program command: '{other}'"
        ))),
    }
}

fn cmd_upgrade(globals: GlobalOpts, parser: &mut lexopt::Parser) -> Result<(), MsigError> {
    use lexopt::Arg::*;
    let mut program_addr: Option<String> = None;
    let mut buffer_addr: Option<String> = None;
    let mut spill_addr: Option<String> = None;
    let mut memo: Option<String> = None;

    while let Some(arg) = parser.next().map_err(|e| MsigError::Usage(e.to_string()))? {
        match arg {
            Long("program") => program_addr = Some(parse_value(parser, "--program")?),
            Long("buffer") => buffer_addr = Some(parse_value(parser, "--buffer")?),
            Long("spill") => spill_addr = Some(parse_value(parser, "--spill")?),
            Long("memo") => memo = Some(parse_value(parser, "--memo")?),
            Short('h') | Long("help") => {
                super::help::print_resource_help("program");
                return Ok(());
            }
            _ => return Err(MsigError::Usage("unexpected flag".into())),
        }
    }

    let program_addr =
        program_addr.ok_or_else(|| MsigError::Usage("--program is required".into()))?;
    let buffer_addr = buffer_addr.ok_or_else(|| MsigError::Usage("--buffer is required".into()))?;
    let spill_addr = spill_addr.ok_or_else(|| MsigError::Usage("--spill is required".into()))?;

    let ctx = build_context(&globals)?;
    let multisig = ctx.resolve_multisig(None)?;
    let vault_index = ctx.vault_index(None);
    program_upgrade::create_program_upgrade_proposal(
        ctx.rpc(),
        ctx.signer.as_ref(),
        &multisig,
        &program_addr,
        &buffer_addr,
        &spill_addr,
        vault_index,
        memo.as_deref(),
        &ctx.config,
        globals.dry_run,
        globals.yes,
    )?;
    Ok(())
}
