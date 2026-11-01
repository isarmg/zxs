use super::*;
use rust_syntax::parse::Parser;

fn parse_prop(source: &str) -> Field {
    Field::parse_named.parse_str(source).unwrap()
}

use generate_prop_docs as prop_docs;

#[test]
fn docs_classify_props_like_the_builder() {
    for (option, optional) in [
        ("", false),
        ("#[prop(optional)]", true),
        ("#[prop(optional_no_strip)]", true),
        ("#[prop(attrs)]", true),
        ("#[prop(default = 7)]", true),
        ("#[prop(optional = false)]", false),
        ("#[prop(attrs = false)]", false),
    ] {
        let field = parse_prop(&format!("{option} value: u8"));
        let prop = Prop::new(field).unwrap();
        let builder = TypedBuilderOpts::from_opts(&prop.prop_opts, &prop.ty)
            .expand()
            .unwrap()
            .to_string();
        assert_eq!(builder.contains("default"), optional, "{option}");
        let docs = prop_docs(&[prop]).unwrap().to_string();
        assert_eq!(docs.contains("# Optional Props"), optional, "{option}");
        assert_eq!(docs.contains("# Required Props"), !optional, "{option}");
    }
}
