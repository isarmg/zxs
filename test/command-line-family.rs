#[cfg(test)]
mod tests {
    use command_line::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
    #[derive(Debug, Clone, PartialEq, ValueEnum)]
    enum Mode { Fast, Safe }
    #[derive(Debug, Args)]
    struct Shared {
        #[arg(short, long, action = command_line::ArgAction::Count)]
        verbose: u8,
    }
    #[derive(Debug, Subcommand)]
    enum Action { Run { #[arg(long)] count: u32 } }
    #[derive(Debug, Parser)]
    #[command(name="fixture", version="1.2.3", about="Stable CLI contract")]
    struct Cli {
        #[command(flatten)] shared: Shared,
        #[arg(long, value_enum, default_value="safe")] mode: Mode,
        #[command(subcommand)] action: Action,
    }
    #[derive(Debug, Parser)]
    #[command(name="functional-helper")]
    struct Helper { #[arg(long)] name: String }
    #[test]
    fn derives_parse_flatten_subcommands_and_values() {
        let cli=Cli::try_parse_from(["fixture", "-vv", "--mode", "fast", "run", "--count", "3"]).unwrap();
        assert_eq!(cli.shared.verbose,2); assert_eq!(cli.mode,Mode::Fast);
        assert!(matches!(cli.action,Action::Run{count:3}));
        assert_eq!(Helper::try_parse_from(["functional-helper","--name","ok"]).unwrap().name,"ok");
    }
    #[test]
    fn help_version_and_error_exit_contracts() {
        let help=Cli::try_parse_from(["fixture","--help"]).unwrap_err();
        assert_eq!(help.kind(),command_line::error::ErrorKind::DisplayHelp); assert_eq!(help.exit_code(),0);
        let text=help.to_string(); assert!(text.contains("Usage: fixture [OPTIONS] <COMMAND>")); assert!(text.contains("--mode <MODE>"));
        let version=Cli::try_parse_from(["fixture","--version"]).unwrap_err();assert_eq!(version.exit_code(),0);assert_eq!(version.to_string(),"fixture 1.2.3\n");
        let bad=Cli::try_parse_from(["fixture","run","--count","no"]).unwrap_err();assert_eq!(bad.exit_code(),2);assert_eq!(bad.kind(),command_line::error::ErrorKind::ValueValidation);
        let unknown=Cli::try_parse_from(["fixture","--unknown"]).unwrap_err();assert_eq!(unknown.exit_code(),2);assert_eq!(unknown.kind(),command_line::error::ErrorKind::UnknownArgument);
        Cli::command().debug_assert();
    }
    #[test]
    fn builder_and_lexer_public_interfaces() {
        let matches=command_line::Command::new("builder").arg(command_line::Arg::new("value").long("value")).try_get_matches_from(["builder","--value","ok"]).unwrap();
        assert_eq!(matches.get_one::<String>("value").unwrap(),"ok");
        let raw=crate::lexer::RawArgs::new(["--flag=value"]);let mut cursor=raw.cursor();let arg=raw.next(&mut cursor).unwrap();let (flag,value)=arg.to_long().unwrap();assert_eq!(flag.unwrap(),"flag");assert_eq!(value.unwrap(),"value");
    }
}
#[cfg(test)]
mod contracts {
    use command_line::{Arg, ArgAction, Command, Parser, ValueEnum, error::ErrorKind};
    #[test]
    fn conflicts_requires_and_required_arguments() {
        let cmd=Command::new("relations")
            .arg(Arg::new("input").long("input").required(true))
            .arg(Arg::new("read").long("read").action(ArgAction::SetTrue).conflicts_with("write"))
            .arg(Arg::new("write").long("write").action(ArgAction::SetTrue))
            .arg(Arg::new("confirm").long("confirm").action(ArgAction::SetTrue).requires("write"));
        assert_eq!(cmd.clone().try_get_matches_from(["relations"]).unwrap_err().kind(),ErrorKind::MissingRequiredArgument);
        assert_eq!(cmd.clone().try_get_matches_from(["relations","--input","x","--read","--write"]).unwrap_err().kind(),ErrorKind::ArgumentConflict);
        assert_eq!(cmd.try_get_matches_from(["relations","--input","x","--confirm"]).unwrap_err().kind(),ErrorKind::MissingRequiredArgument);
    }
    #[test]
    fn defaults_delimiters_and_trailing_values() {
        let cmd=Command::new("values").arg(Arg::new("level").long("level").default_value("2").value_parser(command_line::value_parser!(u32)))
          .arg(Arg::new("tags").long("tags").value_delimiter(',').action(ArgAction::Append))
          .arg(Arg::new("tail").num_args(0..).allow_hyphen_values(true));
        let parsed=cmd.try_get_matches_from(["values","--tags","a,b","--","-x","y"]).unwrap();
        assert_eq!(parsed.get_one::<u32>("level"),Some(&2));
        assert_eq!(parsed.get_many::<String>("tags").unwrap().map(String::as_str).collect::<Vec<_>>(),["a","b"]);
        assert_eq!(parsed.get_many::<String>("tail").unwrap().map(String::as_str).collect::<Vec<_>>(),["-x","y"]);
    }
    #[test]
    fn derive_updates_existing_values() {
        #[derive(Parser)] struct Settings { #[arg(long,default_value_t=1)] count:u32, #[arg(long)] label:Option<String> }
        let mut settings=Settings::try_parse_from(["settings","--label","keep"]).unwrap();
        settings.try_update_from(["settings","--count","4"]).unwrap();
        assert_eq!(settings.count,4);assert_eq!(settings.label.as_deref(),Some("keep"));
    }
    #[test]
    fn display_name_template_and_width_setters() {
        let mut cmd=Command::new("binary").display_name("Public Name").help_template("{name}\n{usage}\n{all-args}").term_width(80).max_term_width(100);
        let text=cmd.render_help().to_string();assert!(text.starts_with("Public Name\n"));assert!(text.contains("Usage:")||text.contains("binary"));
    }
    #[test]
    fn value_enum_uses_complete_documentation() {
        #[derive(Clone,ValueEnum)] enum Mode {
            /// Short summary.
            ///
            /// Full explanation is retained.
            Safe,
        }
        let value=Mode::Safe.to_possible_value().unwrap();
        assert!(value.get_help().unwrap().to_string().contains("Full explanation is retained."));
    }
    #[test]
    fn deferred_subcommands_keep_validation_and_help() {
        #[derive(Parser)] enum Actions { Run { #[arg(long)] count:u32 }, Stop }
        assert!(matches!(Actions::try_parse_from(["actions","run","--count","5"]).unwrap(),Actions::Run{count:5}));
        assert_eq!(Actions::try_parse_from(["actions","run"]).err().unwrap().kind(),ErrorKind::MissingRequiredArgument);
        let help=Actions::try_parse_from(["actions","run","--help"]).err().unwrap();assert_eq!(help.exit_code(),0);assert!(help.to_string().contains("--count <COUNT>"));
    }
}

mod command_line_error_source_regressions {
    use command_line::error::{Error, ErrorFormatter, ErrorKind, KindFormatter};
    use std::error::Error as StdError;
    use std::fmt;
    use std::io;

    #[derive(Debug)]
    struct LeafCause;

    impl fmt::Display for LeafCause {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("fixture leaf cause")
        }
    }

    impl StdError for LeafCause {}

    #[derive(Debug)]
    struct IoContext {
        operation_id: u32,
        cause: LeafCause,
    }

    impl fmt::Display for IoContext {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "fixture operation {} failed", self.operation_id)
        }
    }

    impl StdError for IoContext {
        fn source(&self) -> Option<&(dyn StdError + 'static)> {
            Some(&self.cause)
        }
    }

    fn source_messages(mut cause: &(dyn StdError + 'static)) -> Vec<String> {
        let mut messages = Vec::new();
        loop {
            messages.push(cause.to_string());
            match cause.source() {
                Some(next) => cause = next,
                None => return messages,
            }
        }
    }

    fn check_io_conversion<F: ErrorFormatter>() {
        let original = io::Error::new(
            io::ErrorKind::PermissionDenied,
            IoContext {
                operation_id: 17,
                cause: LeafCause,
            },
        );
        let expected_kind = original.kind();
        let expected_message_chain = source_messages(&original);
        // Both conversions already use raw() to own the presentation text.
        // The regression must preserve those exact bytes, not change formatter policy.
        let previous = Error::<F>::raw(ErrorKind::Io, &original);
        let expected_display = previous.to_string();
        let expected_render = previous.render().to_string();

        let converted: Error<F> = original.into();

        assert_eq!(converted.kind(), ErrorKind::Io);
        assert_eq!(converted.to_string(), expected_display);
        assert_eq!(converted.render().to_string(), expected_render);
        let source = converted
            .source()
            .expect("conversion must retain the original io::Error as source");
        let retained = source
            .downcast_ref::<io::Error>()
            .expect("source must preserve the io::Error type");
        assert_eq!(retained.kind(), expected_kind);
        let context = retained
            .get_ref()
            .expect("fixture io::Error owns an inner cause")
            .downcast_ref::<IoContext>()
            .expect("the typed inner cause must remain available");
        assert_eq!(context.operation_id, 17);
        assert_eq!(source_messages(source), expected_message_chain);
    }

    fn check_fmt_conversion<F: ErrorFormatter>() {
        let original = fmt::Error;
        let expected_message_chain = source_messages(&original);
        let previous = Error::<F>::raw(ErrorKind::Format, original);
        let expected_display = previous.to_string();
        let expected_render = previous.render().to_string();

        let converted: Error<F> = original.into();

        assert_eq!(converted.kind(), ErrorKind::Format);
        assert_eq!(converted.to_string(), expected_display);
        assert_eq!(converted.render().to_string(), expected_render);
        let source = converted
            .source()
            .expect("conversion must retain the original fmt::Error as source");
        assert!(source.downcast_ref::<fmt::Error>().is_some());
        assert_eq!(source_messages(source), expected_message_chain);
    }

    #[test]
    fn io_conversion_preserves_default_formatter_source_and_message() {
        check_io_conversion::<command_line::error::DefaultFormatter>();
    }

    #[test]
    fn io_conversion_preserves_kind_formatter_source_and_message() {
        check_io_conversion::<KindFormatter>();
    }

    #[test]
    fn fmt_conversion_preserves_default_formatter_source_and_message() {
        check_fmt_conversion::<command_line::error::DefaultFormatter>();
    }

    #[test]
    fn fmt_conversion_preserves_kind_formatter_source_and_message() {
        check_fmt_conversion::<KindFormatter>();
    }
}
