use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use bahamut_launcher::release::{
    Channel, Product, ReleaseError, ReleaseScope, Target, accept_offer, accepted_state_summary,
    bootstrap_accepted_state, parse_and_validate_metadata, read_delivery_manifest_file,
    read_metadata_file, read_public_key_file, read_seed_file, read_signature_file,
    record_installed_release, sign_metadata, verify_artifact_file, verify_metadata,
    write_signature_file,
};
use zeroize::Zeroize;
use zeroize::Zeroizing;

const USAGE: &str = "Usage:\n  release-metadata sign --metadata FILE --signature-out FILE (--key-file FILE | --key-stdin) --artifact FILE [--delivery-manifest FILE]\n  release-metadata verify --metadata FILE --signature FILE --public-key FILE --product PRODUCT --channel stable --target TARGET --artifact FILE [--delivery-manifest FILE]\n  release-metadata trust --metadata FILE --signature FILE --public-key FILE --product PRODUCT --channel stable --target TARGET --state FILE --starting-version VERSION --artifact FILE [--delivery-manifest FILE]\n  release-metadata accept --metadata FILE --signature FILE --public-key FILE --product PRODUCT --channel stable --target TARGET --state FILE --artifact FILE [--delivery-manifest FILE]\n  release-metadata record-installed --metadata FILE --signature FILE --public-key FILE --product PRODUCT --channel stable --target TARGET --state FILE --artifact FILE [--delivery-manifest FILE]\n\nProducts: game, launcher\nTargets: platform-independent, windows-x86_64, linux-x86_64, macos-universal\nKey files and key stdin use raw bytes: 32-byte Ed25519 seed/public key, 64-byte signature.\nMetadata is signed and verified as its exact file bytes. Game commands require --delivery-manifest.";

#[derive(Default)]
struct Options {
    values: HashMap<String, OsString>,
    flags: HashSet<String>,
}

fn main() -> ExitCode {
    match run(std::env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<(), ReleaseError> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(ReleaseError::Invalid(USAGE.to_owned()));
    };
    let command = command
        .to_str()
        .ok_or_else(|| ReleaseError::Invalid("Command name must be UTF-8.".into()))?;
    if command == "--help" || command == "-h" || command == "help" {
        println!("{USAGE}");
        return Ok(());
    }
    let options = parse_options(args)?;
    match command {
        "sign" => sign_command(options),
        "verify" => verify_command(options),
        "trust" => trust_command(options),
        "accept" => accept_command(options),
        "record-installed" => record_installed_command(options),
        _ => Err(ReleaseError::Invalid(USAGE.to_owned())),
    }
}

fn parse_options(args: impl Iterator<Item = OsString>) -> Result<Options, ReleaseError> {
    let mut args = args;
    let mut options = Options::default();
    while let Some(argument) = args.next() {
        let flag = argument
            .to_str()
            .ok_or_else(|| ReleaseError::Invalid("Option name must be UTF-8.".into()))?;
        if flag == "--key-stdin" {
            if !options.flags.insert(flag.to_owned()) {
                return Err(ReleaseError::Invalid("Duplicate command option.".into()));
            }
            continue;
        }
        if !flag.starts_with("--") {
            return Err(ReleaseError::Invalid(USAGE.to_owned()));
        }
        let value = args
            .next()
            .ok_or_else(|| ReleaseError::Invalid("Command option is missing its value.".into()))?;
        if options.values.insert(flag.to_owned(), value).is_some() {
            return Err(ReleaseError::Invalid("Duplicate command option.".into()));
        }
    }
    Ok(options)
}

fn sign_command(mut options: Options) -> Result<(), ReleaseError> {
    require_only(
        &mut options,
        &[
            "--metadata",
            "--signature-out",
            "--key-file",
            "--artifact",
            "--delivery-manifest",
        ],
        &["--key-stdin"],
    )?;
    let metadata_path = take_path(&mut options, "--metadata")?;
    let signature_path = take_path(&mut options, "--signature-out")?;
    let artifact_path = take_path(&mut options, "--artifact")?;
    let delivery_path = take_optional_path(&mut options, "--delivery-manifest");
    let key_file = take_optional_path(&mut options, "--key-file");
    let key_stdin = options.flags.remove("--key-stdin");
    if key_file.is_some() == key_stdin {
        return Err(ReleaseError::Invalid(
            "Choose exactly one of --key-file or --key-stdin.".into(),
        ));
    }

    let metadata_bytes = read_metadata_file(&metadata_path)?;
    let delivery_bytes = read_optional_delivery_manifest(delivery_path.as_ref())?;
    let metadata = parse_and_validate_metadata(&metadata_bytes, delivery_bytes.as_deref())?;
    verify_artifact_file(&metadata, &artifact_path)?;
    let mut seed = Zeroizing::new(match key_file {
        Some(path) => read_seed_file(&path)?,
        None => read_seed_stdin()?,
    });
    let signature = sign_metadata(&metadata_bytes, &seed, delivery_bytes.as_deref())?;
    seed.zeroize();
    write_signature_file(&signature_path, &signature)?;
    println!(
        "signed {} {} {}",
        product_name(metadata.product),
        target_name(metadata.target),
        metadata.version
    );
    Ok(())
}

fn verify_command(mut options: Options) -> Result<(), ReleaseError> {
    require_verified_options(&mut options, false, false)?;
    let (release, artifact_path) = verified_release(&mut options)?;
    verify_artifact_file(&release.metadata, &artifact_path)?;
    println!(
        "verified {} {} {}",
        product_name(release.metadata.product),
        target_name(release.metadata.target),
        release.metadata.version
    );
    println!("metadata_sha256 {}", release.metadata_sha256);
    Ok(())
}

fn trust_command(mut options: Options) -> Result<(), ReleaseError> {
    require_verified_options(&mut options, true, true)?;
    let state_path = take_path(&mut options, "--state")?;
    let starting_version = take_text(&mut options, "--starting-version")?;
    let (release, artifact_path) = verified_release(&mut options)?;
    verify_artifact_file(&release.metadata, &artifact_path)?;
    bootstrap_accepted_state(&state_path, &release, &starting_version)?;
    let (highest, _) = accepted_state_summary(&state_path, release.metadata.scope())?;
    println!("trusted starting version {highest}");
    Ok(())
}

fn accept_command(mut options: Options) -> Result<(), ReleaseError> {
    require_verified_options(&mut options, true, false)?;
    let state_path = take_path(&mut options, "--state")?;
    let (release, artifact_path) = verified_release(&mut options)?;
    verify_artifact_file(&release.metadata, &artifact_path)?;
    match accept_offer(&state_path, &release)? {
        bahamut_launcher::release::OfferDecision::Accepted => {
            println!("accepted remote version {}", release.metadata.version)
        }
        bahamut_launcher::release::OfferDecision::ReuseIdentical => {
            println!(
                "reused identical remote version {}",
                release.metadata.version
            )
        }
    }
    Ok(())
}

fn record_installed_command(mut options: Options) -> Result<(), ReleaseError> {
    require_verified_options(&mut options, true, false)?;
    let state_path = take_path(&mut options, "--state")?;
    let (release, artifact_path) = verified_release(&mut options)?;
    verify_artifact_file(&release.metadata, &artifact_path)?;
    match record_installed_release(&state_path, &release)? {
        bahamut_launcher::release::InstalledDecision::Recorded => {
            println!("recorded installed version {}", release.metadata.version)
        }
        bahamut_launcher::release::InstalledDecision::AlreadyCurrent => {
            println!(
                "installed version {} is already recorded",
                release.metadata.version
            )
        }
    }
    let (highest, installed) = accepted_state_summary(&state_path, release.metadata.scope())?;
    println!("highest_remote {highest}");
    if let Some(installed) = installed {
        println!("installed {installed}");
    }
    Ok(())
}

fn require_verified_options(
    options: &mut Options,
    with_state: bool,
    with_starting_version: bool,
) -> Result<(), ReleaseError> {
    let mut allowed = vec![
        "--metadata",
        "--signature",
        "--public-key",
        "--product",
        "--channel",
        "--target",
        "--artifact",
        "--delivery-manifest",
    ];
    if with_state {
        allowed.push("--state");
    }
    if with_starting_version {
        allowed.push("--starting-version");
    }
    if options.flags.is_empty()
        && options
            .values
            .keys()
            .all(|key| allowed.contains(&key.as_str()))
    {
        Ok(())
    } else {
        Err(ReleaseError::Invalid(USAGE.to_owned()))
    }
}

fn verified_release(
    options: &mut Options,
) -> Result<(bahamut_launcher::release::VerifiedRelease, PathBuf), ReleaseError> {
    let metadata_path = take_path(options, "--metadata")?;
    let signature_path = take_path(options, "--signature")?;
    let public_key_path = take_path(options, "--public-key")?;
    let product = parse_product(&take_text(options, "--product")?)?;
    let channel = parse_channel(&take_text(options, "--channel")?)?;
    let target = parse_target(&take_text(options, "--target")?)?;
    let artifact_path = take_path(options, "--artifact")?;
    let delivery_path = take_optional_path(options, "--delivery-manifest");

    let metadata_bytes = read_metadata_file(&metadata_path)?;
    let signature = read_signature_file(&signature_path)?;
    let public_key = read_public_key_file(&public_key_path)?;
    let delivery_bytes = read_optional_delivery_manifest(delivery_path.as_ref())?;
    let release = verify_metadata(
        &metadata_bytes,
        &signature,
        &public_key,
        ReleaseScope {
            product,
            channel,
            target,
        },
        delivery_bytes.as_deref(),
    )?;
    Ok((release, artifact_path))
}

fn read_optional_delivery_manifest(
    path: Option<&PathBuf>,
) -> Result<Option<Vec<u8>>, ReleaseError> {
    path.map(|path| read_delivery_manifest_file(path))
        .transpose()
}

fn read_seed_stdin() -> Result<Vec<u8>, ReleaseError> {
    let stdin = io::stdin().lock();
    let mut seed = Vec::with_capacity(33);
    stdin
        .take(33)
        .read_to_end(&mut seed)
        .map_err(|_| ReleaseError::Invalid("Could not read signing seed from stdin.".into()))?;
    if seed.len() != 32 {
        return Err(ReleaseError::Invalid(
            "Signing seed from stdin must be exactly 32 raw bytes.".into(),
        ));
    }
    Ok(seed)
}

fn parse_product(value: &str) -> Result<Product, ReleaseError> {
    match value {
        "game" => Ok(Product::Game),
        "launcher" => Ok(Product::Launcher),
        _ => Err(ReleaseError::Invalid("Unknown release product.".into())),
    }
}

fn parse_channel(value: &str) -> Result<Channel, ReleaseError> {
    match value {
        "stable" => Ok(Channel::Stable),
        _ => Err(ReleaseError::Invalid(
            "Only the stable channel is supported.".into(),
        )),
    }
}

fn parse_target(value: &str) -> Result<Target, ReleaseError> {
    match value {
        "platform-independent" => Ok(Target::PlatformIndependent),
        "windows-x86_64" => Ok(Target::WindowsX86_64),
        "linux-x86_64" => Ok(Target::LinuxX86_64),
        "macos-universal" => Ok(Target::MacosUniversal),
        _ => Err(ReleaseError::Invalid("Unknown release target.".into())),
    }
}

fn take_path(options: &mut Options, name: &str) -> Result<PathBuf, ReleaseError> {
    options
        .values
        .remove(name)
        .map(PathBuf::from)
        .ok_or_else(|| ReleaseError::Invalid(format!("Missing required option {name}.")))
}

fn take_optional_path(options: &mut Options, name: &str) -> Option<PathBuf> {
    options.values.remove(name).map(PathBuf::from)
}

fn take_text(options: &mut Options, name: &str) -> Result<String, ReleaseError> {
    options
        .values
        .remove(name)
        .and_then(|value| value.into_string().ok())
        .ok_or_else(|| ReleaseError::Invalid(format!("Missing or invalid option {name}.")))
}

fn require_only(
    options: &mut Options,
    allowed_values: &[&str],
    allowed_flags: &[&str],
) -> Result<(), ReleaseError> {
    if options
        .values
        .keys()
        .all(|key| allowed_values.contains(&key.as_str()))
        && options
            .flags
            .iter()
            .all(|key| allowed_flags.contains(&key.as_str()))
    {
        Ok(())
    } else {
        Err(ReleaseError::Invalid(USAGE.to_owned()))
    }
}

fn product_name(product: Product) -> &'static str {
    match product {
        Product::Game => "game",
        Product::Launcher => "launcher",
    }
}

fn target_name(target: Target) -> &'static str {
    match target {
        Target::PlatformIndependent => "platform-independent",
        Target::WindowsX86_64 => "windows-x86_64",
        Target::LinuxX86_64 => "linux-x86_64",
        Target::MacosUniversal => "macos-universal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_cli_targets_remain_explicit() {
        assert_eq!(parse_product("launcher").unwrap(), Product::Launcher);
        assert_eq!(parse_channel("stable").unwrap(), Channel::Stable);
        assert_eq!(
            parse_target("windows-x86_64").unwrap(),
            Target::WindowsX86_64
        );
        assert_eq!(
            parse_target("macos-universal").unwrap(),
            Target::MacosUniversal
        );
        assert!(parse_target("windows-aarch64").is_err());
        assert!(parse_target("macos-x86_64").is_err());
        assert!(parse_channel("beta").is_err());
    }

    #[test]
    fn option_parser_rejects_duplicate_and_unknown_arguments() {
        let duplicate = vec!["--metadata", "a", "--metadata", "b"]
            .into_iter()
            .map(OsString::from);
        assert!(parse_options(duplicate).is_err());
        let unknown = vec!["--not-an-option", "value"]
            .into_iter()
            .map(OsString::from);
        let mut parsed = parse_options(unknown).unwrap();
        assert!(require_only(&mut parsed, &["--metadata"], &[]).is_err());
    }
}
