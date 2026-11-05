use super::*;

fn css(width: f64, style: &str) -> CssSnapshot {
    let mut result = CssSnapshot::default();
    result
        .values
        .insert("background-color".into(), "rgb(10, 20, 30)".into());
    for side in SIDES {
        result
            .values
            .insert(format!("border-{side}-width"), format!("{width}px"));
        result
            .values
            .insert(format!("border-{side}-color"), "rgb(40, 50, 60)".into());
        result
            .values
            .insert(format!("border-{side}-style"), style.into());
    }
    result
}

#[test]
fn numeric_prefixes_and_css_formatting() {
    for (text, expected) in [
        ("  -.25px", -0.25),
        (".75%", 0.75),
        ("+1.2e2ms", 120.0),
        ("1e-", 1.0),
        ("-0", -0.0),
        ("bad", 0.0),
        ("", 0.0),
    ] {
        assert_eq!(number(text), expected, "{text}");
    }
    assert_eq!(fmt(-0.0000001), "0");
    assert_eq!(fmt(1.2500001), "1.25");
    assert_eq!(fmt(20.0), "20");
    assert_eq!(fmt(0.0), "0");
}

#[test]
fn local_foreground_tracks_the_card_border_box_without_scroll_correction() {
    let mut style = css(1.0, "solid");
    style
        .values
        .insert("border-left-width".into(), "2.5px".into());
    let local = foreground_style(120.5, 180.0, &style, true);
    for expected in [
        "position:absolute!important;",
        "left:-2.5px!important;",
        "top:-1px!important;",
        "width:120.5px!important;",
        "height:180px!important;",
        "pointer-events:none!important;",
    ] {
        assert!(local.contains(expected), "{expected}: {local}");
    }
    assert!(!local.contains("position:fixed"));
    let fixed = foreground_style(120.5, 180.0, &style, false);
    assert!(fixed.contains("position:fixed!important;left:0px!important;top:0px!important;"));
    assert!(!fixed.contains("position:absolute"));
}

#[test]
fn only_owned_property_families_are_removed_from_transitions() {
    for property in [
        "background",
        "background-color",
        "border-left-width",
        "border-radius",
        "box-shadow",
        "mask-image",
        "backdrop-filter",
    ] {
        assert!(owned_paint_property(property));
    }
    for property in [
        "backgrounded",
        "bordered",
        "box-shadow-x",
        "filter",
        "opacity",
        "--zxs-contour-extent",
    ] {
        assert!(!owned_paint_property(property));
    }
}

#[test]
fn splits_css_functions_without_splitting_their_arguments() {
    assert_eq!(
        split(
            "rgb(1, 2, 3) 1px 2px, color(display-p3 1 0 0) -3px 4px",
            Separator::Comma
        ),
        ["rgb(1, 2, 3) 1px 2px", "color(display-p3 1 0 0) -3px 4px"]
    );
    assert_eq!(
        split("  rgb(1, 2, 3)\t1px 2px ", Separator::Space),
        ["rgb(1, 2, 3)", "1px", "2px"]
    );
}

#[test]
fn parses_supported_multi_layer_shadows_and_rejects_inset() {
    let values = shadows("rgba(0, 0, 0, .2) 1px -2px .5px -1px, red 0px 2px").unwrap();
    assert_eq!(values.len(), 2);
    assert_eq!(
        (values[0].x, values[0].y, values[0].blur, values[0].spread),
        (1.0, -2.0, 0.5, -1.0)
    );
    assert_eq!(values[1].color, "red");
    assert!(shadows("none").unwrap().is_empty());
    for value in [
        "inset red 1px 2px",
        "red 1px",
        "red 1px 2px 3px 4px 5px",
        "red blue 1px 2px",
        "red 1em 2px",
        "red +1px 2px",
    ] {
        assert!(shadows(value).is_none(), "{value}");
    }
}

#[test]
fn mask_keeps_outside_focus_visible_and_clips_host_content() {
    let geometry = geometry::create(120.0, 80.0, 20.0, 0.02).unwrap();
    let image = mask(&geometry, 4.0);
    assert!(image.contains("width=\"128\" height=\"88\""));
    assert!(image.contains("M0 0H128V88H0ZM4 4H124V84H4Z"));
    assert!(image.contains("fill-rule=\"evenodd\""));
    assert!(image.contains(&geometry.path));
}

#[test]
fn compact_paint_shares_physical_corner_size() {
    let small = geometry::create(100.0, 60.0, 16.0, 0.02).unwrap();
    let large = geometry::create(200.0, 120.0, 16.0, 0.02).unwrap();
    let one = decoration(&small, &css(0.0, "none"), &[], false, false)
        .unwrap()
        .unwrap();
    let two = decoration(&large, &css(0.0, "none"), &[], false, false)
        .unwrap()
        .unwrap();
    assert_eq!(one, two);
    assert_eq!(one.margin, 2.0);
    assert_eq!(one.slice, Some(18.0));
    assert!(one.image.contains("width=\"37\" height=\"37\""));
}

#[test]
fn fractional_compact_slices_keep_the_centre_strip_in_both_axes() {
    for (width, height, extent) in [
        (1768.0, 63.0, 100.0),
        (63.0, 1768.0, 100.0),
        (1768.25, 63.25, 100.0),
        (63.25, 1768.25, 100.0),
        (1768.0, 100.0, 19.75),
    ] {
        let geometry = geometry::create(width, height, extent, 0.02).unwrap();
        let painted = decoration(&geometry, &css(0.0, "none"), &[], false, false)
            .unwrap()
            .unwrap();
        let inset = painted.slice.unwrap();
        let slice_css = painted.border_image_slice();
        assert!(slice_css.ends_with("% fill"), "{slice_css}");
        let fraction = number(&slice_css) / 100.0;
        let source_size = inset * 2.0 + 1.0;
        assert!((source_size * (1.0 - fraction * 2.0) - 1.0).abs() < 1e-10);
        assert!((source_size * fraction - inset).abs() < 1e-10);
        assert_eq!(inset, geometry.extent + painted.margin);
    }
}

#[test]
fn border_shadows_and_foreground_follow_existing_paint_contract() {
    let geometry = geometry::create(120.0, 80.0, 20.0, 0.02).unwrap();
    let layers = shadows("red 3px -4px 2px 1px, blue 1px 2px").unwrap();
    let painted = decoration(&geometry, &css(2.0, "dashed"), &layers, false, false)
        .unwrap()
        .unwrap();
    assert_eq!(painted.margin, 13.0);
    assert!(
        painted.image.find("filter=\"url(#shadow1)\"").unwrap()
            < painted.image.find("filter=\"url(#shadow0)\"").unwrap()
    );
    assert!(painted.image.contains("stroke-dasharray=\"6 4\""));
    assert!(painted.image.contains("rgb(10, 20, 30)"));
    let foreground = decoration(&geometry, &css(2.0, "solid"), &[], true, true)
        .unwrap()
        .unwrap();
    assert!(!foreground.image.contains("stroke="));
    assert!(!foreground.image.contains("rgb(10, 20, 30)"));
    assert!(
        decoration(&geometry, &css(1.0, "double"), &[], false, false)
            .unwrap()
            .is_none()
    );
}

#[test]
fn escapes_css_paint_values_and_cache_keeps_variants_separate() {
    let geometry = geometry::create(120.0, 80.0, 20.0, 0.02).unwrap();
    let mut css = css(1.0, "solid");
    css.values
        .insert("background-color".into(), "a&<b>\"'".into());
    let first = decoration(&geometry, &css, &[], false, false)
        .unwrap()
        .unwrap();
    assert!(first.image.contains("a&amp;&lt;b&gt;&quot;&apos;"));
    css.values.insert("background-color".into(), "red".into());
    let second = decoration(&geometry, &css, &[], false, false)
        .unwrap()
        .unwrap();
    assert_ne!(first.image, second.image);
    assert_eq!(
        second,
        decoration(&geometry, &css, &[], false, false)
            .unwrap()
            .unwrap()
    );
}
