use super::*;

const SHARE: &str = "11111111111111111111111111111111";
const DIR: &str = "22222222222222222222222222222222";
const CURSOR: &str = "33333333333333333333333333333333";

#[test]
fn typed_route_round_trip_and_wire_size() {
    let hash = format!("{DIRECTORY}{SHARE}/{DIR}/{CURSOR}");
    let Location::Directory(folder) = Location::parse(&hash) else { panic!("directory") };
    assert_eq!(folder.fragment(), hash);
    assert_eq!(Location::Directory(folder).selected(), Some(Panel::Folders));
    assert_eq!(Panel::Folders.id(), "zxs-files");
    assert_eq!(folder.request_path(), format!("/ui/v1/d/{SHARE}/{DIR}/{CURSOR}"));
    assert_eq!(folder.request_path().len(), 107);
    assert_eq!(folder.destination(None).fragment(), format!("{DIRECTORY}{SHARE}/-/-"));
}

#[test]
fn no_guessed_ids_paths_or_percent_decoding() {
    assert_eq!(Location::parse("#zxs-versions"), Location::Panel(Panel::Versions));
    assert_eq!(Location::parse("#not-a-panel"), Location::Other);
    assert_eq!(Location::parse(""), Location::Landing);
    for bad in [
        format!("{DIRECTORY}{SHARE}/../-"),
        format!("{DIRECTORY}{SHARE}/%2d/-"),
        format!("{DIRECTORY}{SHARE}/{DIR}/-/-"),
        format!("{DIRECTORY}{SHARE}/{DIR}/?after={CURSOR}"),
        format!("{DIRECTORY}00000000000000000000000000000000/-/-"),
        format!("{DIRECTORY}Aa111111111111111111111111111111/-/-"),
        format!("{DIRECTORY}/{DIR}/-"),
    ] {
        assert_eq!(Location::parse(&bad), Location::InvalidDirectory, "{bad}");
    }
}
