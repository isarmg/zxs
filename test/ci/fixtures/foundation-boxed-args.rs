// Public consumer regression for Args group metadata through Box.
// The root harness supplies command-line with its default features and derive.
use command_line::{Args, Id, Parser, error::ErrorKind};

#[derive(Debug, Args)]
#[group(id = "details-fields")]
struct Details {
    #[arg(long)]
    label: Option<String>,
    #[arg(long)]
    count: Option<u8>,
}

#[derive(Debug, Parser)]
struct OptionalBoxed {
    #[command(flatten)]
    details: Option<Box<Details>>,
}

#[derive(Debug, Parser)]
struct OptionalUnboxed {
    #[command(flatten)]
    details: Option<Details>,
}

#[derive(Debug, Args)]
#[group(skip)]
struct NoGroup {
    #[arg(long)]
    label: Option<String>,
}

#[derive(Debug, Parser)]
struct InvalidOptionalSkipped {
    #[command(flatten)]
    details: Option<Box<NoGroup>>,
}

#[derive(Debug, Args)]
#[group(id = "required-choice", required = true, multiple = false)]
struct RequiredChoice {
    #[arg(long)]
    left: bool,
    #[arg(long)]
    right: bool,
}

#[derive(Debug, Parser)]
struct OptionalBoxedChoice {
    #[command(flatten)]
    choice: Option<Box<RequiredChoice>>,
}

#[test]
fn boxed_args_forwards_the_underlying_group_id() {
    let original = <Details as Args>::group_id();
    assert_eq!(original, Some(Id::from("details-fields")));
    assert_eq!(<Box<Details> as Args>::group_id(), original);
    assert_eq!(<Box<Box<Details>> as Args>::group_id(), original);
}

#[test]
fn boxed_args_preserves_deliberately_absent_group_ids() {
    assert_eq!(<NoGroup as Args>::group_id(), None);
    assert_eq!(<Box<NoGroup> as Args>::group_id(), None);
    assert_eq!(<() as Args>::group_id(), None);
    assert_eq!(<Box<()> as Args>::group_id(), None);
}

#[test]
fn optional_boxed_flatten_stays_absent_when_no_group_member_is_supplied() {
    let parsed = OptionalBoxed::try_parse_from(["demo"])
        .expect("invariant: this optional fixture has no required group members");
    assert!(parsed.details.is_none());
}

#[test]
fn optional_boxed_flatten_parses_supplied_group_members() {
    let parsed = OptionalBoxed::try_parse_from(["demo", "--label", "value", "--count", "7"])
        .expect("invariant: both supplied fixture values satisfy their argument types");
    let details = parsed
        .details
        .expect("invariant: supplied group members instantiate the optional group");
    assert_eq!(details.label.as_deref(), Some("value"));
    assert_eq!(details.count, Some(7));

    let invalid = OptionalBoxed::try_parse_from(["demo", "--count", "256"])
        .expect_err("invariant: 256 is outside the fixture's u8 argument range");
    assert_eq!(invalid.kind(), ErrorKind::ValueValidation);
    assert_eq!(invalid.exit_code(), 2);
    assert!(invalid.to_string().contains("--count"));
}

#[test]
fn optional_boxed_flatten_update_can_create_a_present_group() {
    let mut parsed = OptionalBoxed::try_parse_from(["demo"])
        .expect("invariant: the initial optional fixture group may be absent");
    parsed
        .try_update_from(["demo", "--count", "7"])
        .expect("invariant: this update supplies a valid member of the same Args group");
    let details = parsed
        .details
        .expect("invariant: the supplied update instantiates the optional group");
    assert_eq!(details.label, None);
    assert_eq!(details.count, Some(7));
}

#[test]
fn unboxed_optional_control_and_boxed_group_constraints_keep_their_semantics() {
    let absent = OptionalUnboxed::try_parse_from(["demo"])
        .expect("invariant: the unboxed optional control has no required members");
    assert!(absent.details.is_none());
    let present = OptionalUnboxed::try_parse_from(["demo", "--label", "value"])
        .expect("invariant: the unboxed control accepts this known valid label");
    assert_eq!(
        present.details.and_then(|details| details.label).as_deref(),
        Some("value")
    );

    let missing = OptionalBoxedChoice::try_parse_from(["demo"])
        .expect_err("invariant: this fixture requires one choice group member");
    assert_eq!(missing.kind(), ErrorKind::MissingRequiredArgument);
    assert_eq!(missing.exit_code(), 2);
    let missing_text = missing.to_string();
    assert!(missing_text.contains("--left"));
    assert!(missing_text.contains("--right"));

    let conflict = OptionalBoxedChoice::try_parse_from(["demo", "--left", "--right"])
        .expect_err("invariant: the fixture group forbids multiple members");
    assert_eq!(conflict.kind(), ErrorKind::ArgumentConflict);
    assert_eq!(conflict.exit_code(), 2);
    let conflict_text = conflict.to_string();
    assert!(conflict_text.contains("--left"));
    assert!(conflict_text.contains("--right"));

    let selected = OptionalBoxedChoice::try_parse_from(["demo", "--left"])
        .expect("invariant: exactly one supplied choice satisfies the fixture group");
    let choice = selected
        .choice
        .expect("invariant: the supplied choice creates the optional boxed group");
    assert!(choice.left);
    assert!(!choice.right);
}

#[test]
#[should_panic(expected = "cannot `#[flatten]` an `Option<Args>` with `#[group(skip)]`")]
fn optional_skipped_group_keeps_the_existing_configuration_rejection() {
    // Preserve this exact existing configuration guard, including its reason.
    let _ = InvalidOptionalSkipped::try_parse_from(["demo"]);
}
