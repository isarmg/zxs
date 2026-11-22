use super::normalized_share_path;
use std::io;

#[test]
fn normalized_path_uses_exact_components_without_reinterpreting_them(){
    assert_eq!(normalized_share_path(["Pool-É","Sub🚀","File.TXt"].into_iter()).unwrap(),
        r"\Pool-É\Sub🚀\File.TXt");
    assert_eq!(normalized_share_path(["F","a"].into_iter()).unwrap(),r"\F\a");
    for parts in [["F","..","leaf"],["F",r"stack\other","leaf"],["F","","leaf"]]{
        assert_eq!(normalized_share_path(parts.into_iter()).unwrap_err().kind(),io::ErrorKind::InvalidInput);
    }
}

#[test]
fn normalized_path_budget_counts_utf16_units_and_the_leading_separator(){
    let long="A".repeat(255);
    let exact="A".repeat(245)+"🚀";
    let out=normalized_share_path(std::iter::repeat_n(long.as_str(),127).chain([exact.as_str()].into_iter())).unwrap();
    assert_eq!(out.encode_utf16().count(),32_760);
    assert_eq!(out.len(),32_762); // the supplementary scalar uses four UTF-8 bytes
    let over=exact+"A";
    assert_eq!(normalized_share_path(std::iter::repeat_n(long.as_str(),127).chain([over.as_str()].into_iter()))
        .unwrap_err().kind(),io::ErrorKind::Unsupported);
}
