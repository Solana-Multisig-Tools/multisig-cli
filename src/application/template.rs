use std::collections::{BTreeMap, HashMap};

use base64::Engine;
use serde::Deserialize;
use solana_pubkey::Pubkey;

use crate::error::MsigError;
use crate::infra::config::labels;
use crate::infra::instruction::{AccountMeta, Instruction};

pub type RawTemplateInputs = BTreeMap<String, Vec<String>>;

pub struct TemplateContext<'a> {
    pub multisig: Pubkey,
    pub vault: Pubkey,
    pub squads_program_id: Pubkey,
    pub labels: &'a HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CompiledTemplate {
    pub id: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub instructions: Vec<Instruction>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemplateManifest {
    pub id: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub inputs: Vec<TemplateInputInfo>,
    pub instruction_count: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TemplateInputInfo {
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub default: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateFile {
    id: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    inputs: BTreeMap<String, TemplateInput>,
    #[serde(default)]
    accounts: BTreeMap<String, AccountValue>,
    instructions: Vec<InstructionSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateInput {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountValue {
    #[serde(default, rename = "const")]
    const_value: Option<String>,
    #[serde(default)]
    context: Option<String>,
    #[serde(default)]
    input: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstructionSpec {
    program: String,
    #[serde(default)]
    accounts: Vec<InstructionAccountSpec>,
    #[serde(default)]
    data: Vec<DataPart>,
    #[serde(default)]
    for_each: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstructionAccountSpec {
    pubkey: String,
    #[serde(default)]
    writable: bool,
    #[serde(default)]
    signer: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DataPart {
    #[serde(default)]
    const_hex: Option<String>,
    #[serde(default)]
    input: Option<String>,
    #[serde(default)]
    encoding: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    Pubkey,
    PubkeyList,
    Bytes,
    String,
    U8,
    U16,
    U32,
    U64,
    I64,
    Bool,
}

#[derive(Debug, Clone)]
enum InputValue {
    Pubkey(Pubkey),
    PubkeyList(Vec<Pubkey>),
    Bytes(Vec<u8>),
    String(String),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    I64(i64),
    Bool(bool),
}

pub fn load_template_file(
    path: &std::path::Path,
    raw_inputs: &RawTemplateInputs,
    context: &TemplateContext<'_>,
) -> Result<CompiledTemplate, MsigError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        MsigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read template {}: {e}", path.display()),
        ))
    })?;
    compile_template_str(&contents, raw_inputs, context)
}

pub fn inspect_template_file(path: &std::path::Path) -> Result<TemplateManifest, MsigError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        MsigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read template {}: {e}", path.display()),
        ))
    })?;
    inspect_template_str(&contents)
}

pub fn inspect_template_str(contents: &str) -> Result<TemplateManifest, MsigError> {
    let template: TemplateFile = toml::from_str(contents)
        .map_err(|e| MsigError::Usage(format!("invalid template TOML: {e}")))?;
    if template.id.trim().is_empty() {
        return Err(MsigError::Usage("template id cannot be empty".into()));
    }
    if template.instructions.is_empty() {
        return Err(MsigError::Usage(
            "template must contain at least one instruction".into(),
        ));
    }
    let mut inputs = Vec::with_capacity(template.inputs.len());
    for (name, input) in template.inputs {
        parse_kind(&input.kind)?;
        inputs.push(TemplateInputInfo {
            name,
            kind: input.kind,
            required: input.default.is_none(),
            default: input.default,
            description: input.description,
        });
    }
    Ok(TemplateManifest {
        id: template.id,
        version: template.version,
        description: template.description,
        inputs,
        instruction_count: template.instructions.len(),
    })
}

pub fn compile_template_str(
    contents: &str,
    raw_inputs: &RawTemplateInputs,
    context: &TemplateContext<'_>,
) -> Result<CompiledTemplate, MsigError> {
    let template: TemplateFile = toml::from_str(contents)
        .map_err(|e| MsigError::Usage(format!("invalid template TOML: {e}")))?;

    if template.id.trim().is_empty() {
        return Err(MsigError::Usage("template id cannot be empty".into()));
    }
    if template.instructions.is_empty() {
        return Err(MsigError::Usage(
            "template must contain at least one instruction".into(),
        ));
    }

    for name in raw_inputs.keys() {
        if !template.inputs.contains_key(name) {
            return Err(MsigError::Usage(format!(
                "input '{name}' is not declared by template '{}'",
                template.id
            )));
        }
    }

    for name in template.accounts.keys() {
        if template.inputs.contains_key(name) {
            return Err(MsigError::Usage(format!(
                "template '{}' uses '{name}' as both an account alias and an input",
                template.id
            )));
        }
    }

    let inputs = parse_inputs(&template, raw_inputs, context.labels)?;
    let mut instructions = Vec::new();

    for spec in &template.instructions {
        if let Some(loop_input) = spec.for_each.as_deref() {
            let items = match inputs.get(loop_input) {
                Some(InputValue::PubkeyList(items)) => items,
                Some(_) => {
                    return Err(MsigError::Usage(format!(
                        "instruction for_each='{loop_input}' must reference a pubkey[] input"
                    )));
                }
                None => {
                    return Err(MsigError::Usage(format!(
                        "instruction for_each references unknown input '{loop_input}'"
                    )));
                }
            };
            if items.is_empty() {
                return Err(MsigError::Usage(format!(
                    "input '{loop_input}' must contain at least one pubkey"
                )));
            }
            for item in items {
                instructions.push(compile_instruction(
                    spec,
                    &template.accounts,
                    &inputs,
                    context,
                    Some(*item),
                )?);
            }
        } else {
            instructions.push(compile_instruction(
                spec,
                &template.accounts,
                &inputs,
                context,
                None,
            )?);
        }
    }

    for (ix_idx, ix) in instructions.iter().enumerate() {
        for account in &ix.accounts {
            if account.is_signer && account.pubkey != context.vault {
                return Err(MsigError::Usage(format!(
                    "template instruction #{} marks {} as signer, but only the active Squads vault can sign template instructions",
                    ix_idx + 1,
                    account.pubkey
                )));
            }
        }
    }

    Ok(CompiledTemplate {
        id: template.id,
        version: template.version,
        description: template.description,
        instructions,
    })
}

fn parse_inputs(
    template: &TemplateFile,
    raw_inputs: &RawTemplateInputs,
    label_map: &HashMap<String, String>,
) -> Result<BTreeMap<String, InputValue>, MsigError> {
    let mut parsed = BTreeMap::new();
    for (name, spec) in &template.inputs {
        let kind = parse_kind(&spec.kind)?;
        let values = match raw_inputs.get(name) {
            Some(values) => values.clone(),
            None => match &spec.default {
                Some(default) => vec![default.clone()],
                None => {
                    let hint = spec
                        .description
                        .as_deref()
                        .map(|description| format!(" ({description})"))
                        .unwrap_or_default();
                    return Err(MsigError::Usage(format!(
                        "missing required template input '{name}'{hint}"
                    )));
                }
            },
        };
        parsed.insert(name.clone(), parse_value(name, kind, &values, label_map)?);
    }
    Ok(parsed)
}

fn parse_kind(kind: &str) -> Result<InputKind, MsigError> {
    match kind.trim().to_ascii_lowercase().as_str() {
        "pubkey" => Ok(InputKind::Pubkey),
        "pubkey[]" | "pubkeys" | "pubkey_array" => Ok(InputKind::PubkeyList),
        "bytes" | "data" => Ok(InputKind::Bytes),
        "string" | "utf8" => Ok(InputKind::String),
        "u8" => Ok(InputKind::U8),
        "u16" => Ok(InputKind::U16),
        "u32" => Ok(InputKind::U32),
        "u64" => Ok(InputKind::U64),
        "i64" => Ok(InputKind::I64),
        "bool" | "boolean" => Ok(InputKind::Bool),
        other => Err(MsigError::Usage(format!(
            "unsupported template input type '{other}'"
        ))),
    }
}

fn parse_value(
    name: &str,
    kind: InputKind,
    values: &[String],
    label_map: &HashMap<String, String>,
) -> Result<InputValue, MsigError> {
    match kind {
        InputKind::Pubkey => {
            let raw = single_value(name, values)?;
            Ok(InputValue::Pubkey(parse_pubkey_input(
                name, raw, label_map,
            )?))
        }
        InputKind::PubkeyList => {
            let mut pubkeys = Vec::new();
            for value in values {
                for part in value.split(',') {
                    let trimmed = part.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    pubkeys.push(parse_pubkey_input(name, trimmed, label_map)?);
                }
            }
            Ok(InputValue::PubkeyList(pubkeys))
        }
        InputKind::Bytes => Ok(InputValue::Bytes(parse_bytes(single_value(name, values)?)?)),
        InputKind::String => Ok(InputValue::String(single_value(name, values)?.to_string())),
        InputKind::U8 => Ok(InputValue::U8(parse_unsigned(name, values)?)),
        InputKind::U16 => Ok(InputValue::U16(parse_unsigned(name, values)?)),
        InputKind::U32 => Ok(InputValue::U32(parse_unsigned(name, values)?)),
        InputKind::U64 => Ok(InputValue::U64(parse_unsigned(name, values)?)),
        InputKind::I64 => {
            let raw = single_value(name, values)?;
            Ok(InputValue::I64(raw.parse().map_err(|_| {
                MsigError::Usage(format!("input '{name}' must be an i64"))
            })?))
        }
        InputKind::Bool => {
            let raw = single_value(name, values)?.to_ascii_lowercase();
            let value = match raw.as_str() {
                "true" | "1" | "yes" => true,
                "false" | "0" | "no" => false,
                _ => {
                    return Err(MsigError::Usage(format!(
                        "input '{name}' must be a boolean"
                    )));
                }
            };
            Ok(InputValue::Bool(value))
        }
    }
}

fn single_value<'a>(name: &str, values: &'a [String]) -> Result<&'a str, MsigError> {
    match values {
        [value] => Ok(value.as_str()),
        [] => Err(MsigError::Usage(format!(
            "input '{name}' is missing a value"
        ))),
        _ => Err(MsigError::Usage(format!(
            "input '{name}' only accepts one value"
        ))),
    }
}

fn parse_unsigned<T>(name: &str, values: &[String]) -> Result<T, MsigError>
where
    T: std::str::FromStr,
{
    let raw = single_value(name, values)?;
    raw.parse()
        .map_err(|_| MsigError::Usage(format!("input '{name}' has invalid numeric value '{raw}'")))
}

fn parse_pubkey_input(
    name: &str,
    raw: &str,
    label_map: &HashMap<String, String>,
) -> Result<Pubkey, MsigError> {
    let resolved = labels::resolve_address(raw, label_map).map_err(|_| {
        MsigError::Usage(format!(
            "input '{name}' must be a pubkey or configured label, got '{raw}'"
        ))
    })?;
    resolved
        .parse()
        .map_err(|_| MsigError::Usage(format!("input '{name}' is not a valid pubkey")))
}

fn compile_instruction(
    spec: &InstructionSpec,
    account_aliases: &BTreeMap<String, AccountValue>,
    inputs: &BTreeMap<String, InputValue>,
    context: &TemplateContext<'_>,
    loop_item: Option<Pubkey>,
) -> Result<Instruction, MsigError> {
    let program_id = resolve_pubkey_ref(
        &spec.program,
        account_aliases,
        inputs,
        context,
        loop_item,
        "program",
    )?;
    let mut accounts = Vec::with_capacity(spec.accounts.len());
    for account in &spec.accounts {
        let pubkey = resolve_pubkey_ref(
            &account.pubkey,
            account_aliases,
            inputs,
            context,
            loop_item,
            "account",
        )?;
        accounts.push(if account.writable {
            AccountMeta::new(pubkey, account.signer)
        } else {
            AccountMeta::new_readonly(pubkey, account.signer)
        });
    }

    let mut data = Vec::new();
    for part in &spec.data {
        append_data_part(&mut data, part, inputs)?;
    }

    Ok(Instruction {
        program_id,
        accounts,
        data,
    })
}

fn resolve_pubkey_ref(
    reference: &str,
    account_aliases: &BTreeMap<String, AccountValue>,
    inputs: &BTreeMap<String, InputValue>,
    context: &TemplateContext<'_>,
    loop_item: Option<Pubkey>,
    field: &str,
) -> Result<Pubkey, MsigError> {
    if reference == "$item" {
        return loop_item.ok_or_else(|| {
            MsigError::Usage(format!(
                "{field} references $item outside a for_each instruction"
            ))
        });
    }
    if let Ok(pubkey) = reference.parse::<Pubkey>() {
        return Ok(pubkey);
    }
    if let Some(alias) = account_aliases.get(reference) {
        return resolve_account_alias(reference, alias, inputs, context);
    }
    if let Some(value) = inputs.get(reference) {
        return input_as_pubkey(reference, value);
    }
    Err(MsigError::Usage(format!(
        "{field} references unknown pubkey source '{reference}'"
    )))
}

fn resolve_account_alias(
    name: &str,
    alias: &AccountValue,
    inputs: &BTreeMap<String, InputValue>,
    context: &TemplateContext<'_>,
) -> Result<Pubkey, MsigError> {
    let mut sources = 0u8;
    sources += u8::from(alias.const_value.is_some());
    sources += u8::from(alias.context.is_some());
    sources += u8::from(alias.input.is_some());
    if sources != 1 {
        return Err(MsigError::Usage(format!(
            "account alias '{name}' must set exactly one of const, context, or input"
        )));
    }

    if let Some(raw) = alias.const_value.as_deref() {
        return raw
            .parse()
            .map_err(|_| MsigError::Usage(format!("account alias '{name}' has invalid pubkey")));
    }
    if let Some(context_name) = alias.context.as_deref() {
        return match context_name {
            "vault" => Ok(context.vault),
            "multisig" => Ok(context.multisig),
            "program_id" | "squads_program" | "squads_program_id" => Ok(context.squads_program_id),
            other => Err(MsigError::Usage(format!(
                "account alias '{name}' uses unsupported context '{other}'"
            ))),
        };
    }
    let input_name = alias.input.as_deref().unwrap_or_default();
    let value = inputs.get(input_name).ok_or_else(|| {
        MsigError::Usage(format!(
            "account alias '{name}' references unknown input '{input_name}'"
        ))
    })?;
    input_as_pubkey(input_name, value)
}

fn input_as_pubkey(name: &str, value: &InputValue) -> Result<Pubkey, MsigError> {
    match value {
        InputValue::Pubkey(pubkey) => Ok(*pubkey),
        _ => Err(MsigError::Usage(format!(
            "input '{name}' must be type pubkey when used as an account"
        ))),
    }
}

fn append_data_part(
    out: &mut Vec<u8>,
    part: &DataPart,
    inputs: &BTreeMap<String, InputValue>,
) -> Result<(), MsigError> {
    let mut sources = 0u8;
    sources += u8::from(part.const_hex.is_some());
    sources += u8::from(part.input.is_some());
    if sources != 1 {
        return Err(MsigError::Usage(
            "each data part must set exactly one of const_hex or input".into(),
        ));
    }

    if let Some(hex) = part.const_hex.as_deref() {
        out.extend(parse_hex(hex)?);
        return Ok(());
    }

    let input_name = part.input.as_deref().unwrap_or_default();
    let value = inputs.get(input_name).ok_or_else(|| {
        MsigError::Usage(format!("data part references unknown input '{input_name}'"))
    })?;
    append_encoded_input(out, input_name, value, part.encoding.as_deref())
}

fn append_encoded_input(
    out: &mut Vec<u8>,
    name: &str,
    value: &InputValue,
    encoding: Option<&str>,
) -> Result<(), MsigError> {
    let encoding = encoding
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_else(|| default_encoding(value).to_string());

    match encoding.as_str() {
        "bytes" | "data" => match value {
            InputValue::Bytes(bytes) => out.extend(bytes),
            _ => return encoding_mismatch(name, "bytes"),
        },
        "utf8" | "string" => match value {
            InputValue::String(value) => out.extend(value.as_bytes()),
            InputValue::Bytes(bytes) => out.extend(bytes),
            _ => return encoding_mismatch(name, "string or bytes"),
        },
        "pubkey" => match value {
            InputValue::Pubkey(pubkey) => out.extend_from_slice(pubkey.as_ref()),
            _ => return encoding_mismatch(name, "pubkey"),
        },
        "u8" => out.push(
            numeric_u64(name, value)?
                .try_into()
                .map_err(|_| MsigError::Usage(format!("input '{name}' does not fit in u8")))?,
        ),
        "u16_le" => out.extend_from_slice(
            &u16::try_from(numeric_u64(name, value)?)
                .map_err(|_| MsigError::Usage(format!("input '{name}' does not fit in u16")))?
                .to_le_bytes(),
        ),
        "u16_be" => out.extend_from_slice(
            &u16::try_from(numeric_u64(name, value)?)
                .map_err(|_| MsigError::Usage(format!("input '{name}' does not fit in u16")))?
                .to_be_bytes(),
        ),
        "u32_le" => out.extend_from_slice(
            &u32::try_from(numeric_u64(name, value)?)
                .map_err(|_| MsigError::Usage(format!("input '{name}' does not fit in u32")))?
                .to_le_bytes(),
        ),
        "u32_be" => out.extend_from_slice(
            &u32::try_from(numeric_u64(name, value)?)
                .map_err(|_| MsigError::Usage(format!("input '{name}' does not fit in u32")))?
                .to_be_bytes(),
        ),
        "u64_le" => out.extend_from_slice(&numeric_u64(name, value)?.to_le_bytes()),
        "u64_be" => out.extend_from_slice(&numeric_u64(name, value)?.to_be_bytes()),
        "i64_le" => out.extend_from_slice(&numeric_i64(name, value)?.to_le_bytes()),
        "i64_be" => out.extend_from_slice(&numeric_i64(name, value)?.to_be_bytes()),
        "bool_u8" | "bool" => match value {
            InputValue::Bool(value) => out.push(u8::from(*value)),
            _ => return encoding_mismatch(name, "bool"),
        },
        other => {
            return Err(MsigError::Usage(format!(
                "unsupported data encoding '{other}' for input '{name}'"
            )));
        }
    }
    Ok(())
}

fn default_encoding(value: &InputValue) -> &'static str {
    match value {
        InputValue::Pubkey(_) => "pubkey",
        InputValue::PubkeyList(_) => "pubkey",
        InputValue::Bytes(_) => "bytes",
        InputValue::String(_) => "utf8",
        InputValue::U8(_) => "u8",
        InputValue::U16(_) => "u16_le",
        InputValue::U32(_) => "u32_le",
        InputValue::U64(_) => "u64_le",
        InputValue::I64(_) => "i64_le",
        InputValue::Bool(_) => "bool_u8",
    }
}

fn numeric_u64(name: &str, value: &InputValue) -> Result<u64, MsigError> {
    match value {
        InputValue::U8(value) => Ok(u64::from(*value)),
        InputValue::U16(value) => Ok(u64::from(*value)),
        InputValue::U32(value) => Ok(u64::from(*value)),
        InputValue::U64(value) => Ok(*value),
        _ => Err(MsigError::Usage(format!(
            "input '{name}' must be an unsigned integer"
        ))),
    }
}

fn numeric_i64(name: &str, value: &InputValue) -> Result<i64, MsigError> {
    match value {
        InputValue::I64(value) => Ok(*value),
        _ => Err(MsigError::Usage(format!("input '{name}' must be an i64"))),
    }
}

fn encoding_mismatch<T>(name: &str, expected: &str) -> Result<T, MsigError> {
    Err(MsigError::Usage(format!(
        "input '{name}' cannot be encoded this way; expected {expected}"
    )))
}

fn parse_bytes(raw: &str) -> Result<Vec<u8>, MsigError> {
    if let Some(rest) = raw.strip_prefix("base64:") {
        return base64::engine::general_purpose::STANDARD
            .decode(rest)
            .map_err(|e| MsigError::Usage(format!("invalid base64 bytes input: {e}")));
    }
    if let Some(rest) = raw.strip_prefix("utf8:") {
        return Ok(rest.as_bytes().to_vec());
    }
    parse_hex(raw)
}

fn parse_hex(raw: &str) -> Result<Vec<u8>, MsigError> {
    let trimmed = raw
        .trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .unwrap_or(raw.trim());
    let cleaned: String = trimmed
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != '_')
        .collect();

    if !cleaned.len().is_multiple_of(2) {
        return Err(MsigError::Usage(
            "hex data must contain an even number of digits".into(),
        ));
    }

    let mut bytes = Vec::with_capacity(cleaned.len() / 2);
    for idx in (0..cleaned.len()).step_by(2) {
        let byte = u8::from_str_radix(&cleaned[idx..idx + 2], 16)
            .map_err(|_| MsigError::Usage("hex data contains invalid characters".into()))?;
        bytes.push(byte);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> TemplateContext<'static> {
        TemplateContext {
            multisig: Pubkey::from([1u8; 32]),
            vault: Pubkey::from([2u8; 32]),
            squads_program_id: Pubkey::from([3u8; 32]),
            labels: Box::leak(Box::new(HashMap::new())),
        }
    }

    #[test]
    fn bytes_input_is_encoded_into_instruction_data() {
        let template = r#"
id = "memo.raw"
version = "1"

[inputs.payload]
type = "bytes"

[accounts.program]
const = "11111111111111111111111111111111"

[accounts.authority]
context = "vault"

[[instructions]]
program = "program"
accounts = [
  { pubkey = "authority", signer = true },
]
data = [
  { const_hex = "aabb" },
  { input = "payload" },
]
"#;
        let mut inputs = RawTemplateInputs::new();
        inputs.insert("payload".into(), vec!["0x010203".into()]);

        let compiled =
            compile_template_str(template, &inputs, &context()).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(compiled.instructions.len(), 1);
        assert_eq!(compiled.instructions[0].data, vec![0xaa, 0xbb, 1, 2, 3]);
    }

    #[test]
    fn foreach_over_pubkey_array_expands_instructions() {
        let source_a = Pubkey::from([8u8; 32]).to_string();
        let source_b = Pubkey::from([9u8; 32]).to_string();
        let program = Pubkey::from([7u8; 32]).to_string();
        let template = format!(
            r#"
id = "stake.merge"

[inputs.sources]
type = "pubkey[]"

[accounts.program]
const = "{program}"

[accounts.authority]
context = "vault"

[[instructions]]
program = "program"
for_each = "sources"
accounts = [
  {{ pubkey = "$item", writable = true }},
  {{ pubkey = "authority", signer = true }},
]
data = [
  {{ const_hex = "07000000" }},
]
"#
        );
        let mut inputs = RawTemplateInputs::new();
        inputs.insert("sources".into(), vec![format!("{source_a},{source_b}")]);

        let compiled =
            compile_template_str(&template, &inputs, &context()).unwrap_or_else(|e| panic!("{e}"));

        assert_eq!(compiled.instructions.len(), 2);
        assert_eq!(
            compiled.instructions[0].accounts[0].pubkey.to_string(),
            source_a
        );
        assert_eq!(
            compiled.instructions[1].accounts[0].pubkey.to_string(),
            source_b
        );
        assert_eq!(compiled.instructions[0].data, vec![7, 0, 0, 0]);
    }

    #[test]
    fn rejects_non_vault_signers() {
        let bad_signer = Pubkey::from([4u8; 32]).to_string();
        let template = format!(
            r#"
id = "bad"

[accounts.program]
const = "11111111111111111111111111111111"

[[instructions]]
program = "program"
accounts = [
  {{ pubkey = "{bad_signer}", signer = true }},
]
"#
        );
        let inputs = RawTemplateInputs::new();

        assert!(compile_template_str(&template, &inputs, &context()).is_err());
    }
}
