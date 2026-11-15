//! Publish the complete native FormKey SQL generation.
use color_eyre::{Result, eyre::ensure};
use converter::esm::{EsmParser, form_key_merge::LegacyWriterOutcome};
use std::path::PathBuf;

/// Preserve every accepted owner while publishing authenticated contextual native records.
fn main() -> Result<()> {
    color_eyre::install()?;
    let args: Vec<_> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    ensure!(
        matches!(args.len(), 3 | 4),
        "usage: formkey-records <Data> <plugins.txt> <new world database> [Skyrim.ccc]"
    );
    let (converted, writer) = EsmParser::convert_selected_form_key_plugins(
        &args[0],
        &args[1],
        args.get(3).map(|p| p.as_path()),
        None,
        &args[2],
    )?;
    println!(
        "FormKey native conversion: plugins={} winners={} warnings={}",
        converted.order.plugin_order().len(),
        converted.records.len(),
        converted.warnings.len()
    );
    match writer {
        LegacyWriterOutcome::Published => println!("native contextual database published"),
        LegacyWriterOutcome::Unavailable { .. } => {
            color_eyre::eyre::bail!("native contextual database publication is unavailable")
        }
    }
    Ok(())
}
