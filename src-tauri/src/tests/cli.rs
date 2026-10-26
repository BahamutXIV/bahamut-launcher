fn cli_args(args: &[&str]) -> impl Iterator<Item = std::ffi::OsString> {
    args.iter()
        .map(std::ffi::OsString::from)
        .collect::<Vec<_>>()
        .into_iter()
}

#[test]
fn cli_request_recognises_the_version_flags() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            crate::cli_request(cli_args(&["bahamut-launcher", flag])),
            Some(crate::CliRequest::Version),
            "{flag}"
        );
    }
}

#[test]
fn cli_request_recognises_the_help_flags() {
    for flag in ["--help", "-h"] {
        assert_eq!(
            crate::cli_request(cli_args(&["bahamut-launcher", flag])),
            Some(crate::CliRequest::Help),
            "{flag}"
        );
    }
}

#[test]
fn cli_request_ignores_a_bare_launch() {
    assert_eq!(crate::cli_request(cli_args(&["bahamut-launcher"])), None);
    assert_eq!(crate::cli_request(cli_args(&[])), None);
}

#[test]
fn cli_request_ignores_an_unrelated_argument() {
    for arg in ["--update-transaction", "--versions", "-v", "version", ""] {
        assert_eq!(
            crate::cli_request(cli_args(&["bahamut-launcher", arg])),
            None,
            "{arg}"
        );
    }
}

#[test]
fn cli_request_ignores_a_flag_with_another_argument() {
    for args in [
        ["bahamut-launcher", "--version", "--help"],
        ["bahamut-launcher", "--version", "extra"],
        ["bahamut-launcher", "--update-recovered", "--version"],
    ] {
        assert_eq!(crate::cli_request(cli_args(&args)), None, "{args:?}");
    }
}
