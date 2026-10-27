//! Helper command-line parsing and dispatch.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;

use super::confirm::confirm_command;
use super::recover::recover_command;
use super::replace::replacement_command;
use super::{Result, TransactionMode, UpdateError, invalid};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Apply,
    Repair,
    Confirm,
    Recover,
}

#[derive(Default)]
pub(super) struct Options {
    pub(super) values: BTreeMap<String, OsString>,
    pub(super) flags: BTreeSet<String>,
}

/// Run the explicit helper protocol used by the separately packaged binary.
pub fn run_cli(args: impl IntoIterator<Item = OsString>) -> std::result::Result<(), UpdateError> {
    let mut args = args.into_iter();
    let Some(mode) = args.next() else {
        return Err(invalid(usage()));
    };
    let mode = match mode.to_str() {
        Some("apply") => Mode::Apply,
        Some("repair") => Mode::Repair,
        Some("confirm") => Mode::Confirm,
        Some("recover") => Mode::Recover,
        Some("--help" | "-h" | "help") => {
            println!("{}", usage());
            return Ok(());
        }
        _ => return Err(invalid(usage())),
    };
    let mut options = parse_options(args)?;
    match mode {
        Mode::Apply => replacement_command(&mut options, TransactionMode::Update),
        Mode::Repair => replacement_command(&mut options, TransactionMode::Repair),
        Mode::Confirm => confirm_command(options),
        Mode::Recover => recover_command(options),
    }
}

fn usage() -> &'static str {
    "Usage:\n  bahamut-update-helper apply --target DIR --stage DIR [--transaction DIR] --metadata FILE --signature FILE --public-key FILE --artifact FILE --previous-metadata FILE --previous-signature FILE --wait-pid PID [--restart]\n  bahamut-update-helper repair --target DIR --stage DIR [--transaction DIR] --metadata FILE --signature FILE --public-key FILE --artifact FILE --previous-metadata FILE --previous-signature FILE --wait-pid PID [--restart]\n  bahamut-update-helper confirm --target DIR [--transaction DIR] --public-key FILE\n  bahamut-update-helper recover --target DIR [--transaction DIR] --public-key FILE [--wait-pid PID] [--restart]\n\nThe helper handles only signed Windows x86_64 launcher releases. All keys, metadata, signatures, artifacts, and transaction paths are explicit local inputs."
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Options> {
    let mut args = args;
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        let flag = argument
            .to_str()
            .ok_or_else(|| invalid("Option name must be UTF-8."))?;
        if matches!(flag, "--restart" | "--launcher-handoff") {
            if !options.flags.insert(flag.to_owned()) {
                return Err(invalid("Duplicate command option."));
            }
            continue;
        }
        if !flag.starts_with("--") {
            return Err(invalid(usage()));
        }
        let value = args
            .next()
            .ok_or_else(|| invalid("Command option is missing its value."))?;
        if options.values.insert(flag.to_owned(), value).is_some() {
            return Err(invalid("Duplicate command option."));
        }
    }
    Ok(options)
}

pub(super) fn require_only(options: &Options, values: &[&str], flags: &[&str]) -> Result<()> {
    if options
        .values
        .keys()
        .all(|key| values.contains(&key.as_str()))
        && options
            .flags
            .iter()
            .all(|flag| flags.contains(&flag.as_str()))
    {
        Ok(())
    } else {
        Err(invalid(usage()))
    }
}

pub(super) fn take_path(options: &mut Options, key: &str) -> Result<PathBuf> {
    options
        .values
        .remove(key)
        .map(PathBuf::from)
        .ok_or_else(|| invalid(&format!("Missing required option {key}.")))
}

pub(super) fn take_optional_path(options: &mut Options, key: &str) -> Option<PathBuf> {
    options.values.remove(key).map(PathBuf::from)
}

pub(super) fn take_optional_pid(options: &mut Options, key: &str) -> Result<Option<u32>> {
    let Some(value) = options.values.remove(key) else {
        return Ok(None);
    };
    let value = value
        .into_string()
        .map_err(|_| invalid(&format!("Option {key} must be UTF-8.")))?;
    let pid = value
        .parse::<u32>()
        .map_err(|_| invalid("Parent process ID must be a positive integer."))?;
    if pid == 0 || pid == std::process::id() {
        return Err(invalid("Parent process ID must identify another process."));
    }
    Ok(Some(pid))
}

pub(super) fn take_text(options: &mut Options, key: &str) -> Result<String> {
    options
        .values
        .remove(key)
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| invalid(&format!("Option {key} must be UTF-8.")))
}
