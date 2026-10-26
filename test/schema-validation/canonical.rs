#![allow(clippy::needless_pass_by_value)]

use data_serialization::Deserialize;
use json_codec::Value;
use schema_validation::{
    canonical::{
        self, CanonicalSchema, CanonicalizationError, CanonicalizeOptions, Satisfiability,
    },
    Draft, Validator,
};

fn run_case(case: CanonicalCase) {
    let inputs = case.inputs();

    if let Some(expected_kind) = &case.error {
        assert!(
            case.expected.is_none()
                && case.expected_arbitrary_precision.is_none()
                && case.tests.is_empty()
                && case.satisfiable.is_none()
                && case.kind.is_none(),
            "case `{}`: `error` cases cannot also set `expected`/`tests`/`satisfiable`/`kind`",
            case.description
        );
        for input in &inputs {
            match case.options().canonicalize(input) {
                Err(error) if error_kind(&error) == expected_kind => {}
                other => panic!(
                    "case `{}`: expected {expected_kind} for {input}, got {other:?}",
                    case.description
                ),
            }
        }
        return;
    }

    // Every pointer reported empty must answer `No` when selected on its own.
    for input in &inputs {
        let Ok(prepared) = case.options().prepare(input) else {
            continue;
        };
        for pointer in prepared.unsatisfiable().expect("reports").keys() {
            assert_eq!(
                prepared
                    .canonicalize_at(pointer)
                    .expect("canonicalizes")
                    .satisfiability(),
                Satisfiability::No,
                "case `{}`: `{pointer}` reported empty for {input}",
                case.description
            );
        }
    }

    let raw_draft = case
        .draft()
        .unwrap_or_else(|| Draft::default().detect(inputs[0]));

    // Idempotency and convergence across inputs, at both the emitted-JSON and the IR level.
    let mut form: Option<Value> = None;
    let mut form_draft = raw_draft;
    let mut canonical: Option<CanonicalSchema> = None;
    for input in &inputs {
        let canon = case.canonicalize(input);
        let emitted = canon.to_json_schema();
        let recanonicalized = case.canonicalize(&emitted);
        let reemitted = recanonicalized.to_json_schema();
        assert_eq!(
            emitted, reemitted,
            "case `{}`: not idempotent\n  input = {input}\n  once  = {emitted}\n  twice = {reemitted}",
            case.description
        );
        assert_eq!(
            canon, recanonicalized,
            "case `{}`: emitted form is stable but the IR is not\n  input = {input}\n  emitted = {emitted}",
            case.description
        );
        match &canonical {
            None => {
                form_draft = canon.draft();
                form = Some(emitted);
                canonical = Some(canon);
            }
            Some(first) => {
                let prev = form.as_ref().expect("set together with `canonical`");
                assert_eq!(
                    prev, &emitted,
                    "case `{}`: inputs do not converge\n  a = {prev}\n  b = {emitted}",
                    case.description
                );
                assert_eq!(
                    first, &canon,
                    "case `{}`: inputs converge on emitted JSON but not on IR\n  input = {input}\n  form = {prev}",
                    case.description
                );
            }
        }
    }
    let form = form.expect("at least one input");
    let canonical = canonical.expect("at least one input");

    // `satisfiable` asks only whether the schema was proven empty; `satisfiability` pins the whole
    // answer, witness included.
    let satisfiable = canonical.satisfiability() != Satisfiability::No;
    if let Some(expected) = &case.satisfiability {
        assert_eq!(
            canonical.satisfiability().as_str(),
            expected,
            "case `{}`: satisfiability mismatch\n  form = {form}",
            case.description
        );
    }
    if let Some(expected) = &case.kind {
        assert_eq!(
            canonical.kind().as_str(),
            expected,
            "case `{}`: canonical kind mismatch\n  form = {form}",
            case.description
        );
    }
    if let Some(expected) = case.satisfiable {
        assert_eq!(
            satisfiable,
            expected,
            "case `{}`: satisfiability() = {:?}, expected satisfiable = {expected}\n  form = {form}",
            case.description,
            canonical.satisfiability(),
        );
    }
    if case.tests.iter().any(|test| test.valid == Some(true)) {
        assert!(
            satisfiable,
            "case `{}`: admits a valid value but satisfiability() is No\n  form = {form}",
            case.description
        );
    }

    let expected = case.expected_for_build().unwrap_or_else(|| {
        panic!(
            "case `{}`: every non-`error` case must set `expected`\n  actual = {form}",
            case.description
        )
    });
    assert_eq!(
        &form, expected,
        "case `{}`: canonical form mismatch\n  expected = {expected}\n  actual   = {form}",
        case.description
    );

    // Raw and canonical validators must agree on every test value.
    if !case.tests.is_empty() {
        let canonical_validator = case.build(&form, form_draft);
        let raw_validators: Vec<_> = inputs
            .iter()
            .map(|input| (input, case.build(input, raw_draft)))
            .collect();
        for test in &case.tests {
            let canon_valid = canonical_validator.is_valid(&test.data);
            for (input, raw_validator) in &raw_validators {
                let raw_valid = raw_validator.is_valid(&test.data);
                assert_eq!(
                    raw_valid, canon_valid,
                    "case `{}`: parity disagrees on {}\n  raw   ({input}) = {raw_valid}\n  canon ({form}) = {canon_valid}",
                    case.description, test.data
                );
                if let Some(expected_valid) = test.valid {
                    assert_eq!(
                        raw_valid, expected_valid,
                        "case `{}`: raw verdict wrong on {}: expected {expected_valid}, got {raw_valid}",
                        case.description, test.data
                    );
                }
            }
        }
    }
}

#[derive(Debug, Deserialize)]
#[serialization(deny_unknown_fields)]
struct CanonicalCase {
    description: String,
    #[serialization(default)]
    schema: Option<Value>,
    #[serialization(default)]
    schemas: Vec<Value>,
    #[serialization(default)]
    draft: Option<String>,
    #[serialization(default)]
    expected: Option<Value>,
    /// Overrides `expected` under `arbitrary-precision`, where wide literals survive as written
    /// instead of going through `f64`.
    #[serialization(default)]
    expected_arbitrary_precision: Option<Value>,
    #[serialization(default)]
    tests: Vec<SchemaTest>,
    #[serialization(default)]
    satisfiable: Option<bool>,
    /// Pins the exact three-valued answer, where `satisfiable` only asks whether it is `no`.
    #[serialization(default)]
    satisfiability: Option<String>,
    /// Pins the public structural kind label.
    #[serialization(default)]
    kind: Option<String>,
    /// Force `should_validate_formats`; `None` keeps the draft default.
    #[serialization(default)]
    validate_formats: Option<bool>,
    /// Every input must be rejected with this exact `CanonicalizationError` variant.
    #[serialization(default)]
    error: Option<String>,
}

fn error_kind(error: &CanonicalizationError) -> &'static str {
    match error {
        CanonicalizationError::InvalidSchemaType(_) => "InvalidSchemaType",
        CanonicalizationError::ReferenceResolution(_) => "ReferenceResolution",
        CanonicalizationError::ValidationError(_) => "ValidationError",
        CanonicalizationError::InvalidPattern { .. } => "InvalidPattern",
        _ => "Unknown",
    }
}

/// Plain JSON value (parity-only) or `{"data": ..., "valid": bool}` (also checks verdict).
#[derive(Debug)]
struct SchemaTest {
    data: Value,
    valid: Option<bool>,
}

impl<'de> Deserialize<'de> for SchemaTest {
    fn deserialize<D: data_serialization::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serialization(deny_unknown_fields)]
        struct Tagged {
            data: Value,
            #[serialization(default)]
            valid: Option<bool>,
        }
        let value = Value::deserialize(d)?;
        match &value {
            Value::Object(object) if object.contains_key("data") => {
                let Tagged { data, valid } =
                    json_codec::from_value(value).map_err(data_serialization::de::Error::custom)?;
                Ok(SchemaTest { data, valid })
            }
            _ => Ok(SchemaTest {
                data: value,
                valid: None,
            }),
        }
    }
}

impl CanonicalCase {
    fn inputs(&self) -> Vec<&Value> {
        match (&self.schema, self.schemas.as_slice()) {
            (Some(schema), []) => vec![schema],
            (None, list) if !list.is_empty() => list.iter().collect(),
            (Some(_), _) => panic!(
                "case `{}`: set either `schema` or `schemas`, not both",
                self.description
            ),
            (None, _) => panic!(
                "case `{}`: must set `schema` or `schemas`",
                self.description
            ),
        }
    }

    /// The canonical form this build should produce.
    fn expected_for_build(&self) -> Option<&Value> {
        #[cfg(feature = "arbitrary-precision")]
        if self.expected_arbitrary_precision.is_some() {
            return self.expected_arbitrary_precision.as_ref();
        }
        self.expected.as_ref()
    }

    fn draft(&self) -> Option<Draft> {
        self.draft.as_deref().map(|name| match name {
            "draft4" => Draft::Draft4,
            "draft6" => Draft::Draft6,
            "draft7" => Draft::Draft7,
            "draft2019-09" => Draft::Draft201909,
            "draft2020-12" => Draft::Draft202012,
            other => panic!("case `{}`: unknown draft `{other}`", self.description),
        })
    }

    fn options(&self) -> CanonicalizeOptions<'static> {
        let mut options = canonical::options();
        if let Some(draft) = self.draft() {
            options = options.with_draft(draft);
        }
        if let Some(validate) = self.validate_formats {
            options = options.should_validate_formats(validate);
        }
        options
    }

    fn canonicalize(&self, schema: &Value) -> CanonicalSchema {
        self.options().canonicalize(schema).unwrap_or_else(|e| {
            panic!(
                "case `{}`: canonicalize({schema}) failed: {e}",
                self.description
            )
        })
    }

    fn build(&self, schema: &Value, draft: Draft) -> Validator {
        let mut options = schema_validation::options().with_draft(draft);
        if let Some(validate) = self.validate_formats {
            options = options.should_validate_formats(validate);
        }
        options
            .build(schema)
            .unwrap_or_else(|e| panic!("case `{}`: build({schema}) failed: {e}", self.description))
    }
}

#[test]
fn canonical_fixture_suite() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../../../test/foundation/schema-validation/upstream/jsonschema/tests/canonical-suite",
    );
    let mut files: Vec<_> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert_eq!(files.len(), 31);
    let mut count = 0;
    for file in files {
        let cases: Vec<CanonicalCase> =
            json_codec::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        for case in cases {
            count += 1;
            run_case(case);
        }
    }
    assert_eq!(count, 1509);
}

#[test]
fn pattern_property_runtime_errors_fail_closed() {
    use json_codec::json;
    use schema_validation::{error::ValidationErrorKind, PatternOptions};
    use std::error::Error as _;
    for patterns in [
        json!({"(?<=ab)c": false}),
        json!({"(?<=ab)c": false, "^z": true}),
    ] {
        for extra in [None, Some(json!(false)), Some(json!({"type":"integer"}))] {
            let large_properties: json_codec::Map<String, Value> = ('d'..='s')
                .map(|name| (name.to_string(), json!(true)))
                .collect();
            for properties in [
                None,
                Some(json!({"x":true})),
                Some(Value::Object(large_properties)),
            ] {
                let mut schema = json!({"patternProperties":patterns});
                if let Some(extra) = &extra {
                    schema["additionalProperties"] = extra.clone();
                }
                if let Some(properties) = &properties {
                    schema["properties"] = properties.clone();
                }
                let validator =
                    schema_validation::options()
                        .with_pattern_options(
                            PatternOptions::fancy_regex()
                                .backtrack_limit(if properties.is_some() { 2 } else { 1 }),
                        )
                        .build(&schema)
                        .unwrap();
                let key = if properties.is_some() { "zzabc" } else { "abc" };
                let instance = json!({key:1});
                let error = validator
                    .validate(&instance)
                    .expect_err("regex execution must not become a non-match");
                assert!(
                    matches!(
                        error.kind(),
                        ValidationErrorKind::BacktrackLimitExceeded { .. }
                    ),
                    "{schema}: {error}"
                );
                assert!(error.source().unwrap().is::<fancy_regex::Error>());
                assert_eq!(error.instance_path().as_str(), format!("/{key}"));
                assert!(!validator.is_valid(&instance), "{schema}");
                assert!(
                    validator.iter_errors(&instance).any(|error| matches!(
                        error.kind(),
                        ValidationErrorKind::BacktrackLimitExceeded { .. }
                    )),
                    "{schema}"
                );
                let evaluation = validator.evaluate(&instance);
                assert!(!evaluation.is_valid(), "{schema}");
                assert!(
                    json_codec::to_string(&evaluation.list())
                        .unwrap()
                        .contains("backtracking"),
                    "{schema}"
                );
            }
        }
    }
}

#[test]
fn known_property_pattern_precomputation_retains_runtime_failure() {
    use json_codec::json;
    use schema_validation::{error::ValidationErrorKind, PatternOptions};
    use std::error::Error as _;
    for additional in [json!(false), json!({"type":"integer"})] {
        let schema = json!({"properties":{"abc":true}, "patternProperties":{"(?<=ab)c":false}, "additionalProperties":additional});
        let error = schema_validation::options()
            .with_pattern_options(PatternOptions::fancy_regex().backtrack_limit(1))
            .build(&schema)
            .expect_err("failed precomputation cannot become a cached non-match");
        assert!(matches!(
            error.kind(),
            ValidationErrorKind::BacktrackLimitExceeded { .. }
        ));
        assert!(error.source().unwrap().is::<fancy_regex::Error>());
    }
}

#[test]
fn property_patterns_keep_normal_match_and_non_match_semantics() {
    use json_codec::json;
    for pattern in ["^x", "^x[0-9]+$"] {
        for additional in [json!(true), json!(false), json!({"type":"integer"})] {
            for count in [0, 1, 16] {
                let properties: json_codec::Map<String, Value> = (0..count)
                    .map(|index| (format!("safe{index}"), json!(true)))
                    .collect();
                let schema = json!({"patternProperties":{pattern:{"type":"integer"}},
                    "properties":properties, "additionalProperties":additional});
                let validator = schema_validation::validator_for(&schema).unwrap();
                for (instance, expected) in [
                    (json!({"x1":7}), true),
                    (json!({"x1":"wrong"}), false),
                    (json!({"other":7}), additional != json!(false)),
                ] {
                    assert_eq!(
                        validator.is_valid(&instance),
                        expected,
                        "{schema}: {instance}"
                    );
                    assert_eq!(validator.validate(&instance).is_ok(), expected);
                    assert_eq!(validator.iter_errors(&instance).next().is_none(), expected);
                    assert_eq!(validator.evaluate(&instance).is_valid(), expected);
                }
            }
        }
    }
}

#[test]
fn unevaluated_properties_does_not_hide_pattern_execution_failure() {
    use json_codec::json;
    use schema_validation::{error::ValidationErrorKind, PatternOptions};
    for unevaluated in [json!(true), json!(false)] {
        let schema =
            json!({"patternProperties":{"(?<=ab)c":true}, "unevaluatedProperties":unevaluated});
        let validator = schema_validation::options()
            .with_pattern_options(PatternOptions::fancy_regex().backtrack_limit(1))
            .build(&schema)
            .unwrap();
        let instance = json!({"abc":1});
        assert!(!validator.is_valid(&instance));
        assert!(!validator.evaluate(&instance).is_valid());
        assert!(matches!(
            validator.validate(&instance).unwrap_err().kind(),
            ValidationErrorKind::BacktrackLimitExceeded { .. }
        ));
    }
}
