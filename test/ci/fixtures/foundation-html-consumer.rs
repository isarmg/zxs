#[test]
fn attribute_encoding_keeps_untrusted_quotes_and_markup_inside_the_value() {
    let input = "\" onmouseover=\"alert(1)\" <tag>&版本";
    let escaped = html_escape::encode_double_quoted_attribute(input);
    assert_eq!(
        escaped,
        "&quot; onmouseover=&quot;alert(1)&quot; &lt;tag&gt;&amp;版本"
    );
    assert_eq!(html_escape::decode_html_entities(&escaped), input);
}

#[test]
fn entity_decoding_handles_named_numeric_and_unrecognized_entities() {
    let input = "A&amp;B &#x7248;&#26412; &unknown;";
    assert_eq!(
        html_escape::decode_html_entities(input),
        "A&B 版本 &unknown;"
    );
    assert!(matches!(
        html_escape::decode_html_entities("plain 版本"),
        std::borrow::Cow::Borrowed(_)
    ));
}
