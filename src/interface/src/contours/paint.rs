//! CSS paint interpretation and SVG generation for the contour controller.
//!
//! These functions do not depend on a JavaScript implementation. Keeping the
//! captured CSS values separate from the live declaration also prevents layout
//! reads from being interleaved with paint writes in the controller.

use super::geometry::{self, Geometry};
use std::{cell::RefCell, collections::BTreeMap, collections::VecDeque};

const SIDES: [&str; 4] = ["top", "right", "bottom", "left"];

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CssSnapshot {
    pub values: BTreeMap<String, String>,
}

impl CssSnapshot {
    pub fn get(&self, property: &str) -> &str {
        self.values.get(property).map_or("", String::as_str)
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn snapshot(
    css: &web_sys::CssStyleDeclaration,
) -> Result<CssSnapshot, wasm_bindgen::JsValue> {
    let mut result = CssSnapshot::default();
    for property in [
        "width",
        "height",
        "box-sizing",
        "display",
        "position",
        "overflow-x",
        "overflow-y",
        "border-image-source",
        "box-shadow",
        "background-color",
        "background-image",
        "backdrop-filter",
        "mask-image",
        "outline-style",
        "outline-width",
        "outline-offset",
        "--zxs-contour-extent",
        "--zxs-contour-mode",
        "--zxs-contour-foreground",
    ] {
        result
            .values
            .insert(property.into(), css.get_property_value(property)?);
    }
    for side in SIDES {
        for property in [format!("padding-{side}"), format!("border-{side}-width")] {
            let value = css.get_property_value(&property)?;
            result.values.insert(property, value);
        }
        let visible = number(result.get(&format!("border-{side}-width"))) > 0.0;
        for (suffix, hidden) in [("color", "transparent"), ("style", "none")] {
            let property = format!("border-{side}-{suffix}");
            let value = if visible {
                css.get_property_value(&property)?
            } else {
                hidden.into()
            };
            result.values.insert(property, value);
        }
    }
    if result.get("background-image") != "none" || result.get("backdrop-filter") != "none" {
        for property in [
            "background-position",
            "background-size",
            "background-repeat",
            "background-attachment",
            "background-origin",
            "background-clip",
            "background-blend-mode",
        ] {
            result
                .values
                .insert(property.into(), css.get_property_value(property)?);
        }
    }
    Ok(result)
}

/// Read the numeric prefix of a computed CSS value, matching `parseFloat`.
pub(super) fn number(value: &str) -> f64 {
    let value = value.trim_start();
    let bytes = value.as_bytes();
    let mut cursor = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    if value
        .get(cursor..)
        .is_some_and(|rest| rest.starts_with("Infinity"))
    {
        return if bytes.first() == Some(&b'-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    let start = cursor;
    while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
        cursor += 1;
    }
    let mut digits = cursor - start;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        digits += cursor - start;
    }
    if digits == 0 {
        return 0.0;
    }
    if matches!(bytes.get(cursor), Some(b'e' | b'E')) {
        let exponent = cursor;
        cursor += 1;
        if matches!(bytes.get(cursor), Some(b'+' | b'-')) {
            cursor += 1;
        }
        let start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        if cursor == start {
            cursor = exponent;
        }
    }
    value[..cursor].parse().unwrap_or(0.0)
}

pub(super) fn fmt(value: f64) -> String {
    geometry::fmt(value)
}

pub(super) fn owned_paint_property(name: &str) -> bool {
    name == "background"
        || name.starts_with("background-")
        || name == "border"
        || name.starts_with("border-")
        || name == "box-shadow"
        || name == "mask"
        || name.starts_with("mask-")
        || name == "backdrop-filter"
}

fn xml(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    for character in value.chars() {
        result.push_str(match character {
            '&' => "&amp;",
            '<' => "&lt;",
            '>' => "&gt;",
            '"' => "&quot;",
            '\'' => "&apos;",
            _ => {
                result.push(character);
                continue;
            }
        });
    }
    result
}

#[derive(Clone, Copy)]
pub(super) enum Separator {
    Comma,
    Space,
}

pub(super) fn split(value: &str, separator: Separator) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in value.char_indices() {
        if character == '(' {
            depth += 1;
        } else if character == ')' {
            depth -= 1;
        }
        if depth == 0
            && match separator {
                Separator::Comma => character == ',',
                Separator::Space => character.is_whitespace(),
            }
        {
            let part = value[start..index].trim();
            if !part.is_empty() {
                result.push(part.into());
            }
            start = index + character.len_utf8();
        }
    }
    let part = value[start..].trim();
    if !part.is_empty() {
        result.push(part.into());
    }
    result
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Shadow {
    pub color: String,
    pub x: f64,
    pub y: f64,
    pub blur: f64,
    pub spread: f64,
}

fn pixel_length(token: &str) -> Option<f64> {
    let value = token.strip_suffix("px")?;
    let digits = value.strip_prefix('-').unwrap_or(value);
    let mut dot = false;
    let mut count = 0;
    for byte in digits.bytes() {
        if byte == b'.' && !dot {
            dot = true;
        } else if byte.is_ascii_digit() {
            count += 1;
        } else {
            return None;
        }
    }
    if count == 0 {
        return None;
    }
    value.parse().ok()
}

pub(super) fn shadows(value: &str) -> Option<Vec<Shadow>> {
    if value == "none" {
        return Some(Vec::new());
    }
    let mut result = Vec::new();
    for layer in split(value, Separator::Comma) {
        let tokens = split(&layer, Separator::Space);
        if tokens.iter().any(|token| token == "inset") {
            return None;
        }
        let mut lengths = Vec::new();
        let mut colors = Vec::new();
        for token in tokens {
            if let Some(length) = pixel_length(&token) {
                lengths.push(length);
            } else {
                colors.push(token);
            }
        }
        if !(2..=4).contains(&lengths.len()) || colors.len() != 1 {
            return None;
        }
        result.push(Shadow {
            color: colors.remove(0),
            x: lengths[0],
            y: lengths[1],
            blur: lengths.get(2).copied().unwrap_or(0.0),
            spread: lengths.get(3).copied().unwrap_or(0.0),
        });
    }
    Some(result)
}

pub(super) fn mask(geometry: &Geometry, margin: f64) -> String {
    let width = geometry.width + margin * 2.0;
    let height = geometry.height + margin * 2.0;
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><path d=\"M0 0H{}V{}H0ZM{margin} {margin}H{}V{}H{margin}Z\" fill=\"white\" fill-rule=\"evenodd\"/><path transform=\"translate({margin} {margin})\" d=\"{}\" fill=\"white\"/></svg>",
        fmt(width),
        fmt(height),
        fmt(width),
        fmt(height),
        fmt(geometry.width + margin),
        fmt(geometry.height + margin),
        geometry.path
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Decoration {
    pub image: String,
    pub margin: f64,
    /// Physical inset of a compact nine-slice tile, including its margin.
    pub slice: Option<f64>,
}

impl Decoration {
    pub fn border_image_slice(&self) -> String {
        match self.slice {
            // A compact tile has a 1px centre strip. CSS <number> slices are
            // rounded to image pixels by browsers: a half-pixel inset would
            // consume that strip from both sides. Percentage slices retain
            // its subpixel boundaries while border-image-width stays in
            // physical CSS pixels, so the corner profile is not rescaled.
            Some(inset) => format!("{}% fill", inset / (inset * 2.0 + 1.0) * 100.0),
            None => format!("{} fill", fmt(self.margin)),
        }
    }
}

#[derive(PartialEq)]
struct GeometryKey {
    width: f64,
    height: f64,
    extent: f64,
    radius: f64,
    tolerance: f64,
    error_bound: f64,
    canonical: bool,
    path: String,
    corners: [(f64, f64, f64, f64); 4],
}

#[derive(PartialEq)]
struct PaintKey {
    widths: [f64; 4],
    colors: [String; 4],
    styles: [String; 4],
    shadows: Vec<Shadow>,
    background: String,
    has_background_image: bool,
    foreground: bool,
}

type PaintCache = VecDeque<(GeometryKey, VecDeque<(PaintKey, Option<Decoration>)>)>;
thread_local! { static PAINT_CACHE: RefCell<PaintCache> = const { RefCell::new(VecDeque::new()) }; }

pub(super) fn decoration(
    geometry: &Geometry,
    css: &CssSnapshot,
    layers: &[Shadow],
    has_background_image: bool,
    foreground: bool,
) -> Result<Option<Decoration>, String> {
    let key = GeometryKey {
        width: geometry.width,
        height: geometry.height,
        extent: geometry.extent,
        radius: geometry.radius,
        tolerance: geometry.tolerance,
        error_bound: geometry.error_bound,
        canonical: geometry.canonical,
        path: geometry.path.clone(),
        corners: std::array::from_fn(|index| {
            let corner = &geometry.corners[index];
            (
                corner.extent,
                corner.source.extent,
                corner.source.radius,
                corner.source.offset,
            )
        }),
    };
    let paint_key = PaintKey {
        widths: std::array::from_fn(|index| {
            number(css.get(&format!("border-{}-width", SIDES[index])))
        }),
        colors: std::array::from_fn(|index| {
            css.get(&format!("border-{}-color", SIDES[index])).into()
        }),
        styles: std::array::from_fn(|index| {
            css.get(&format!("border-{}-style", SIDES[index])).into()
        }),
        shadows: layers.to_vec(),
        background: css.get("background-color").into(),
        has_background_image,
        foreground,
    };
    if let Some(value) = PAINT_CACHE.with_borrow(|cache| {
        cache
            .iter()
            .find(|(candidate, _)| candidate == &key)
            .and_then(|(_, values)| values.iter().find(|(candidate, _)| candidate == &paint_key))
            .map(|(_, value)| value.clone())
    }) {
        return Ok(value);
    }
    let result = paint(geometry, &paint_key)?;
    PAINT_CACHE.with_borrow_mut(|cache| {
        let mut values = cache
            .iter()
            .position(|(candidate, _)| candidate == &key)
            .and_then(|index| cache.remove(index))
            .map_or_else(VecDeque::new, |(_, values)| values);
        values.push_back((paint_key, result.clone()));
        while values.len() > 16 {
            values.pop_front();
        }
        cache.push_back((key, values));
        while cache.len() > 96 {
            cache.pop_front();
        }
    });
    Ok(result)
}

fn paint(geometry: &Geometry, css: &PaintKey) -> Result<Option<Decoration>, String> {
    let width = geometry.width;
    let height = geometry.height;
    let extent = geometry.extent;
    let widths = css.widths;
    if css.styles.iter().enumerate().any(|(index, style)| {
        widths[index] > 0.0 && !matches!(style.as_str(), "solid" | "dashed" | "dotted" | "none")
    }) {
        return Ok(None);
    }
    if geometry.canonical
        && !css.has_background_image
        && css.shadows.is_empty()
        && widths.iter().all(|&width| width == 0.0)
    {
        // Only the straight 1px centre strips stretch. Physical corner sizes
        // stay unchanged when otherwise equal tiles have different dimensions.
        let compact = geometry::create(extent * 2.0 + 1.0, extent * 2.0 + 1.0, extent, 0.02)?;
        let margin = 2.0;
        let size = extent * 2.0 + 1.0 + margin * 2.0;
        return Ok(Some(Decoration {
            image: format!(
                "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><path transform=\"translate(2 2)\" d=\"{}\" fill=\"{}\"/></svg>",
                fmt(size),
                fmt(size),
                compact.path,
                xml(&css.background)
            ),
            margin,
            slice: Some(extent + margin),
        }));
    }
    let shadow_extent = css
        .shadows
        .iter()
        .map(|shadow| {
            shadow.x.abs().max(shadow.y.abs()) + shadow.blur * 3.0 + shadow.spread.max(0.0)
        })
        .fold(0.0_f64, f64::max);
    let margin = (shadow_extent + 2.0).ceil().max(1.0);
    let svg_width = width + margin * 2.0;
    let svg_height = height + margin * 2.0;
    let path = &geometry.path;
    let mut defs = format!(
        "<clipPath id=\"inside\"><path d=\"{path}\"/></clipPath><mask id=\"outside\" maskUnits=\"userSpaceOnUse\" x=\"-{margin}\" y=\"-{margin}\" width=\"{svg_width}\" height=\"{svg_height}\"><rect x=\"-{margin}\" y=\"-{margin}\" width=\"{svg_width}\" height=\"{svg_height}\" fill=\"white\"/><path d=\"{path}\" fill=\"black\"/></mask>"
    );
    let mut body = String::new();
    for (index, shadow) in css.shadows.iter().enumerate().rev() {
        let shadow_path = if shadow.spread == 0.0 {
            path.clone()
        } else {
            geometry::offset_path(geometry, shadow.spread)?
        };
        defs.push_str(&format!("<filter id=\"shadow{index}\" filterUnits=\"userSpaceOnUse\" x=\"-{margin}\" y=\"-{margin}\" width=\"{svg_width}\" height=\"{svg_height}\" color-interpolation-filters=\"sRGB\"><feGaussianBlur in=\"SourceAlpha\" stdDeviation=\"{}\"/><feOffset dx=\"{}\" dy=\"{}\" result=\"blurred\"/><feFlood flood-color=\"{}\"/><feComposite operator=\"in\" in2=\"blurred\"/></filter>", shadow.blur / 2.0, shadow.x, shadow.y, xml(&shadow.color)));
        body.push_str(&format!("<path d=\"{shadow_path}\" fill=\"black\" filter=\"url(#shadow{index})\" mask=\"url(#outside)\"/>"));
    }
    if !css.has_background_image {
        body.push_str(&format!(
            "<path d=\"{path}\" fill=\"{}\"/>",
            xml(&css.background)
        ));
    }
    let junction = |a: f64, b: f64, extent: f64| {
        let divisor = a.max(b).max(1e-9);
        (extent * a / divisor, extent * b / divisor)
    };
    let (tlx, tly) = junction(widths[3], widths[0], geometry.corners[3].extent);
    let (trx, try_) = junction(widths[1], widths[0], geometry.corners[0].extent);
    let (brx, bry) = junction(widths[1], widths[2], geometry.corners[1].extent);
    let (blx, bly) = junction(widths[3], widths[2], geometry.corners[2].extent);
    let polygons = [
        format!("0,0 {width},0 {},{try_} {tlx},{tly}", width - trx),
        format!(
            "{width},0 {width},{height} {},{} {},{try_}",
            width - brx,
            height - bry,
            width - trx
        ),
        format!(
            "0,{height} {width},{height} {},{} {blx},{}",
            width - brx,
            height - bry,
            height - bly
        ),
        format!("0,0 {tlx},{tly} {blx},{} 0,{height}", height - bly),
    ];
    for index in 0..SIDES.len() {
        if css.foreground || widths[index] == 0.0 || css.styles[index] == "none" {
            continue;
        }
        defs.push_str(&format!(
            "<clipPath id=\"side{index}\"><polygon points=\"{}\"/></clipPath>",
            polygons[index]
        ));
        let dash = match css.styles[index].as_str() {
            "dashed" => format!(
                " stroke-dasharray=\"{} {}\"",
                widths[index] * 3.0,
                widths[index] * 2.0
            ),
            "dotted" => format!(
                " stroke-dasharray=\"0 {}\" stroke-linecap=\"round\"",
                widths[index] * 2.0
            ),
            _ => String::new(),
        };
        body.push_str(&format!("<g clip-path=\"url(#inside)\"><path d=\"{path}\" fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" clip-path=\"url(#side{index})\"{dash}/></g>", xml(&css.colors[index]), widths[index] * 2.0));
    }
    Ok(Some(Decoration {
        image: format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{}\" height=\"{}\"><defs>{defs}</defs><g transform=\"translate({margin} {margin})\">{body}</g></svg>",
            fmt(svg_width),
            fmt(svg_height)
        ),
        margin,
        slice: None,
    }))
}

#[cfg(test)]
#[path = "../../../../test/interface/unit/contours/paint/tests.rs"]
mod tests;
