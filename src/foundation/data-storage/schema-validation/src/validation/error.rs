//! # Error Handling
//!
//! ## Masking Sensitive Data
//!
//! When working with sensitive data, you might want to hide actual values from error messages.
//! The `ValidationError` type provides methods to mask instance values while preserving the error context:
//!
//! ```rust
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use json_codec::json;
//!
//! let schema = json!({"maxLength": 5});
//! let instance = json!("sensitive data");
//! let validator = schema_validation::validator_for(&schema)?;
//!
//! if let Err(error) = validator.validate(&instance) {
//!     // Use default masking (replaces values with "value")
//!     println!("Masked error: {}", error.masked());
//!     // Or provide custom placeholder
//!     println!("Custom masked: {}", error.masked_with("[REDACTED]"));
//! }
//! # Ok(())
//! # }
//! ```
//!
//! The masked error messages will replace instance values with placeholders while maintaining
//! schema-related information like property names, limits, and types.
//!
//! Original error:
//! ```text
//! "sensitive data" is longer than 5 characters
//! ```
//!
//! Masked error:
//! ```text
//! value is longer than 5 characters
//! ```
use crate::reference::Uri;
use crate::validation::{
    paths::{LazyLocation, Location},
    types::{JsonType, JsonTypeSet},
    validator::LazyEvaluationPath,
    LazyInstance,
};
use json_codec::{Number, Value};
use std::{
    borrow::Cow,
    error,
    fmt::{self, Formatter, Write},
    slice,
    string::FromUtf8Error,
    sync::Arc,
    vec,
};

/// An error that can occur during validation.
#[derive(Debug)]
pub struct ValidationError<'a> {
    repr: Box<ValidationErrorRepr<'a>>,
}

struct ValidationErrorRepr<'a> {
    instance: LazyInstance<'a>,
    kind: ValidationErrorKind,
    instance_path: Location,
    /// Canonical schema location without $ref traversals (JSON Schema "keywordLocation")
    schema_path: Location,
    /// Dynamic path including $ref traversals.
    tracker: LazyEvaluationPath,
    /// Absolute keyword location: full URI including JSON Pointer fragment.
    /// E.g. `https://example.com/schema.json#/properties/name/type`.
    /// Set when the schema has a real base URI (via `$id` or `with_base_uri()`).
    /// `None` for schemas using the internal `json-schema:///` default.
    absolute_keyword_location: Option<Arc<Uri<String>>>,
}

impl fmt::Debug for ValidationErrorRepr<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationErrorRepr")
            .field("instance", &self.instance)
            .field("kind", &self.kind)
            .field("instance_path", &self.instance_path)
            .field("schema_path", &self.schema_path)
            .field("absolute_keyword_location", &self.absolute_keyword_location)
            .finish_non_exhaustive()
    }
}

/// An iterator over instances of [`ValidationError`] that represent validation error for the
/// input instance.
///
/// # Examples
///
/// ```rust
/// use json_codec::json;
///
/// let schema = json!({"maxLength": 5});
/// let instance = json!("foo");
/// if let Ok(validator) = schema_validation::validator_for(&schema) {
///     let errors = validator.iter_errors(&instance);
///     for error in errors {
///         println!("Validation error: {}", error)
///     }
/// }
/// ```
#[doc(hidden)]
pub trait ValidationErrorIterator<'a>: Iterator<Item = ValidationError<'a>> + Send + Sync {}

impl<'a, T> ValidationErrorIterator<'a> for T where
    T: Iterator<Item = ValidationError<'a>> + Send + Sync
{
}

/// A lazily-evaluated iterator over validation errors.
///
/// Use [`into_errors()`](Self::into_errors) to convert into [`ValidationErrors`],
/// which implements [`std::error::Error`] for integration with error handling libraries.
pub struct ErrorIterator<'a> {
    iter: Box<dyn ValidationErrorIterator<'a> + 'a>,
}

impl<'a> ErrorIterator<'a> {
    #[inline]
    pub(crate) fn from_iterator<T>(iterator: T) -> Self
    where
        T: ValidationErrorIterator<'a> + 'a,
    {
        Self {
            iter: Box::new(iterator),
        }
    }

    /// Collects all errors into [`ValidationErrors`], which implements [`std::error::Error`].
    #[inline]
    #[must_use]
    pub fn into_errors(self) -> ValidationErrors<'a> {
        ValidationErrors {
            errors: self.collect(),
        }
    }
}

/// An owned collection of validation errors that implements [`std::error::Error`].
///
/// Obtain this by calling [`ErrorIterator::into_errors()`].
pub struct ValidationErrors<'a> {
    errors: Vec<ValidationError<'a>>,
}

impl<'a> ValidationErrors<'a> {
    #[inline]
    #[must_use]
    pub fn len(&self) -> usize {
        self.errors.len()
    }

    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns the errors as a slice.
    #[inline]
    #[must_use]
    pub fn as_slice(&self) -> &[ValidationError<'a>] {
        &self.errors
    }

    #[inline]
    pub fn iter(&self) -> slice::Iter<'_, ValidationError<'a>> {
        self.errors.iter()
    }

    #[inline]
    pub fn iter_mut(&mut self) -> slice::IterMut<'_, ValidationError<'a>> {
        self.errors.iter_mut()
    }
}

impl fmt::Display for ValidationErrors<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.errors.is_empty() {
            f.write_str("Validation succeeded")
        } else {
            writeln!(f, "Validation errors:")?;
            for (idx, error) in self.errors.iter().enumerate() {
                if error.instance_path().is_empty() {
                    writeln!(f, "{:02}: {error}", idx + 1)?;
                } else {
                    writeln!(f, "{:02}: {}: {error}", idx + 1, error.instance_path())?;
                }
            }
            Ok(())
        }
    }
}

impl fmt::Debug for ValidationErrors<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValidationErrors")
            .field("errors", &self.errors)
            .finish()
    }
}

impl error::Error for ValidationErrors<'_> {}

impl<'a> Iterator for ErrorIterator<'a> {
    type Item = ValidationError<'a>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.as_mut().next()
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> IntoIterator for ValidationErrors<'a> {
    type Item = ValidationError<'a>;
    type IntoIter = vec::IntoIter<ValidationError<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.into_iter()
    }
}

impl<'a, 'b> IntoIterator for &'b ValidationErrors<'a> {
    type Item = &'b ValidationError<'a>;
    type IntoIter = slice::Iter<'b, ValidationError<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter()
    }
}

impl<'a, 'b> IntoIterator for &'b mut ValidationErrors<'a> {
    type Item = &'b mut ValidationError<'a>;
    type IntoIter = slice::IterMut<'b, ValidationError<'a>>;

    fn into_iter(self) -> Self::IntoIter {
        self.errors.iter_mut()
    }
}

/// Kinds of errors that may happen during validation
#[derive(Debug)]
#[allow(missing_docs)]
pub enum ValidationErrorKind {
    /// The input array contain more items than expected.
    AdditionalItems { limit: usize },
    /// Unexpected properties.
    AdditionalProperties { unexpected: Vec<String> },
    /// The input value is not valid under any of the schemas listed in the 'anyOf' keyword.
    AnyOf {
        context: Vec<Vec<ValidationError<'static>>>,
    },
    /// Results from a [`fancy_regex::RuntimeError::BacktrackLimitExceeded`] variant when matching
    BacktrackLimitExceeded { error: fancy_regex::Error },
    /// The regex engine failed to evaluate a `pattern` (e.g. recovered panic from
    /// [`regex-automata`](https://github.com/rust-lang/regex/issues/1344)).
    RegexEngineFailure { message: String },
    /// The input value doesn't match expected constant.
    Constant { expected_value: Value },
    /// The input array doesn't contain items conforming to the specified schema.
    Contains,
    /// The input value does not respect the defined contentEncoding
    ContentEncoding { content_encoding: String },
    /// The input value does not respect the defined contentMediaType
    ContentMediaType { content_media_type: String },
    /// Custom error message for user-defined validation.
    Custom { keyword: String, message: String },
    /// The input value doesn't match any of specified options.
    Enum { options: Value },
    /// Value is too large.
    ExclusiveMaximum { limit: Value },
    /// Value is too small.
    ExclusiveMinimum { limit: Value },
    /// Everything is invalid for `false` schema.
    FalseSchema,
    /// When the input doesn't match to the specified format.
    Format { format: String },
    /// May happen in `contentEncoding` validation if `base64` encoded data is invalid.
    FromUtf8 { error: FromUtf8Error },
    /// Too many items in an array.
    MaxItems { limit: u64 },
    /// Value is too large.
    Maximum { limit: Value },
    /// String is too long.
    MaxLength { limit: u64 },
    /// Too many properties in an object.
    MaxProperties { limit: u64 },
    /// Too few items in an array.
    MinItems { limit: u64 },
    /// Value is too small.
    Minimum { limit: Value },
    /// String is too short.
    MinLength { limit: u64 },
    /// Not enough properties in an object.
    MinProperties { limit: u64 },
    /// When some number is not a multiple of another number.
    MultipleOf {
        #[cfg(feature = "arbitrary-precision")]
        multiple_of: Value,
        #[cfg(not(feature = "arbitrary-precision"))]
        multiple_of: f64,
    },
    /// Negated schema failed validation.
    Not { schema: Value },
    /// The given schema is valid under more than one of the schemas listed in the 'oneOf' keyword.
    OneOfMultipleValid {
        context: Vec<Vec<ValidationError<'static>>>,
    },
    /// The given schema is not valid under any of the schemas listed in the 'oneOf' keyword.
    OneOfNotValid {
        context: Vec<Vec<ValidationError<'static>>>,
    },
    /// When the input doesn't match to a pattern.
    Pattern { pattern: String },
    /// Object property names are invalid.
    PropertyNames {
        error: Box<ValidationError<'static>>,
    },
    /// When a required property is missing.
    Required { property: Value },
    /// When the input value doesn't match one or multiple required types.
    Type { kind: TypeKind },
    /// Unexpected items.
    UnevaluatedItems { unexpected: Vec<String> },
    /// Unexpected properties.
    UnevaluatedProperties { unexpected: Vec<String> },
    /// When the input array has non-unique elements.
    UniqueItems,
    /// Error during schema ref resolution.
    Referencing(crate::reference::Error),
}

impl ValidationErrorKind {
    #[must_use]
    pub fn keyword(&self) -> &str {
        match self {
            ValidationErrorKind::AdditionalItems { .. } => "additionalItems",
            ValidationErrorKind::AdditionalProperties { .. } => "additionalProperties",
            ValidationErrorKind::AnyOf { .. } => "anyOf",
            ValidationErrorKind::BacktrackLimitExceeded { .. }
            | ValidationErrorKind::RegexEngineFailure { .. }
            | ValidationErrorKind::Pattern { .. } => "pattern",
            ValidationErrorKind::Constant { .. } => "const",
            ValidationErrorKind::Contains => "contains",
            ValidationErrorKind::ContentEncoding { .. } | ValidationErrorKind::FromUtf8 { .. } => {
                "contentEncoding"
            }
            ValidationErrorKind::ContentMediaType { .. } => "contentMediaType",
            ValidationErrorKind::Custom { keyword, .. } => keyword,
            ValidationErrorKind::Enum { .. } => "enum",
            ValidationErrorKind::ExclusiveMaximum { .. } => "exclusiveMaximum",
            ValidationErrorKind::ExclusiveMinimum { .. } => "exclusiveMinimum",
            ValidationErrorKind::FalseSchema => "falseSchema",
            ValidationErrorKind::Format { .. } => "format",
            ValidationErrorKind::MaxItems { .. } => "maxItems",
            ValidationErrorKind::Maximum { .. } => "maximum",
            ValidationErrorKind::MaxLength { .. } => "maxLength",
            ValidationErrorKind::MaxProperties { .. } => "maxProperties",
            ValidationErrorKind::MinItems { .. } => "minItems",
            ValidationErrorKind::Minimum { .. } => "minimum",
            ValidationErrorKind::MinLength { .. } => "minLength",
            ValidationErrorKind::MinProperties { .. } => "minProperties",
            ValidationErrorKind::MultipleOf { .. } => "multipleOf",
            ValidationErrorKind::Not { .. } => "not",
            ValidationErrorKind::OneOfMultipleValid { .. }
            | ValidationErrorKind::OneOfNotValid { .. } => "oneOf",
            ValidationErrorKind::PropertyNames { .. } => "propertyNames",
            ValidationErrorKind::Required { .. } => "required",
            ValidationErrorKind::Type { .. } => "type",
            ValidationErrorKind::UnevaluatedItems { .. } => "unevaluatedItems",
            ValidationErrorKind::UnevaluatedProperties { .. } => "unevaluatedProperties",
            ValidationErrorKind::UniqueItems => "uniqueItems",
            ValidationErrorKind::Referencing(_) => "$ref",
        }
    }
}

#[derive(Debug)]
#[allow(missing_docs)]
pub enum TypeKind {
    Single(JsonType),
    Multiple(JsonTypeSet),
}

/// Owned parts returned by [`ValidationError::into_parts`].
#[derive(Debug)]
#[non_exhaustive]
pub struct ValidationErrorParts<'a> {
    pub instance: Cow<'a, Value>,
    pub kind: ValidationErrorKind,
    pub instance_path: Location,
    pub schema_path: Location,
    pub evaluation_path: Location,
    pub absolute_keyword_location: Option<Arc<Uri<String>>>,
}

/// Shortcuts for creation of specific error kinds.
impl<'a> ValidationError<'a> {
    /// Creates a new validation error from parts.
    #[inline]
    #[must_use]
    pub(crate) fn new(
        instance: LazyInstance<'a>,
        kind: ValidationErrorKind,
        instance_path: Location,
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
    ) -> Self {
        Self {
            repr: Box::new(ValidationErrorRepr {
                instance,
                kind,
                instance_path,
                schema_path,
                tracker: tracker.into(),
                absolute_keyword_location: None,
            }),
        }
    }

    /// Returns a reference to the instance that failed validation.
    #[inline]
    #[must_use]
    pub fn instance(&self) -> &Cow<'a, Value> {
        self.repr.instance.get()
    }

    /// Returns the kind of validation error.
    #[inline]
    #[must_use]
    pub fn kind(&self) -> &ValidationErrorKind {
        &self.repr.kind
    }

    /// Returns the JSON Pointer to the instance location that failed validation.
    #[inline]
    #[must_use]
    pub fn instance_path(&self) -> &Location {
        &self.repr.instance_path
    }

    /// Returns the canonical schema location without `$ref` traversals.
    ///
    /// This corresponds to JSON Schema's "keywordLocation" in output formats.
    /// See JSON Schema 2020-12 Core, Section 12.4.2.
    #[inline]
    #[must_use]
    pub fn schema_path(&self) -> &Location {
        &self.repr.schema_path
    }

    /// Returns the dynamic evaluation path including `$ref` traversals.
    ///
    /// This corresponds to JSON Schema's "evaluationPath" - the actual path taken
    /// through the schema including by-reference applicators (`$ref`, `$dynamicRef`).
    /// See JSON Schema 2020-12 Core, Section 12.4.2.
    #[inline]
    #[must_use]
    pub fn evaluation_path(&self) -> &Location {
        self.repr.tracker.resolve(&self.repr.schema_path)
    }

    /// Returns the absolute keyword location as a full URI including JSON Pointer fragment.
    ///
    /// This corresponds to JSON Schema's "absoluteKeywordLocation" (e.g.
    /// `https://example.com/schema.json#/properties/name/type`).
    ///
    /// Present when the schema has a real base URI set via `$id` or [`schema_validation::options::ValidationOptions::with_base_uri`].
    /// `None` for schemas without an explicit base URI.
    #[inline]
    #[must_use]
    pub fn absolute_keyword_location(&self) -> Option<&Uri<String>> {
        self.repr.absolute_keyword_location.as_deref()
    }

    /// Attach an absolute keyword location if one is not already set.
    ///
    /// Uses "set if not already set" semantics so inner `SchemaNode` calls win over outer ones.
    #[inline]
    pub(crate) fn with_absolute_keyword_location(mut self, uri: Option<Arc<Uri<String>>>) -> Self {
        if self.repr.absolute_keyword_location.is_none() {
            self.repr.absolute_keyword_location = uri;
        }
        self
    }

    /// Place the instance location under `prefix`, for an error raised on a part of a document.
    pub(crate) fn with_instance_path_prefix(mut self, prefix: &Location) -> Self {
        self.repr.instance_path = prefix.join_raw_suffix(self.repr.instance_path.as_str());
        self
    }

    pub(crate) fn set_absolute_keyword_location(&mut self, uri: &Arc<Uri<String>>) {
        if self.repr.absolute_keyword_location.is_none() {
            self.repr.absolute_keyword_location = Some(Arc::clone(uri));
        }
    }

    /// Decomposes the error into its owned parts.
    #[inline]
    #[must_use]
    pub fn into_parts(self) -> ValidationErrorParts<'a> {
        let repr = *self.repr;
        let evaluation_path = repr.tracker.into_owned(repr.schema_path.clone());
        ValidationErrorParts {
            instance: repr.instance.into_cow(),
            kind: repr.kind,
            instance_path: repr.instance_path,
            schema_path: repr.schema_path,
            evaluation_path,
            absolute_keyword_location: repr.absolute_keyword_location,
        }
    }

    #[inline]
    fn borrowed(
        instance: LazyInstance<'a>,
        kind: ValidationErrorKind,
        instance_path: Location,
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
    ) -> Self {
        Self::new(instance, kind, instance_path, schema_path, tracker)
    }

    /// Returns a wrapper that masks instance values in error messages.
    /// Uses "value" as a default placeholder.
    #[must_use]
    pub fn masked<'b>(&'b self) -> MaskedValidationError<'a, 'b, 'static> {
        self.masked_with("value")
    }

    /// Returns a wrapper that masks instance values in error messages with a custom placeholder.
    pub fn masked_with<'b, 'c>(
        &'b self,
        placeholder: impl Into<Cow<'c, str>>,
    ) -> MaskedValidationError<'a, 'b, 'c> {
        MaskedValidationError {
            error: self,
            placeholder: placeholder.into(),
        }
    }
    /// Converts the `ValidationError` into an owned version with `'static` lifetime.
    #[must_use]
    pub fn to_owned(self) -> ValidationError<'static> {
        let parts = self.into_parts();
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(parts.instance.into_owned())),
            parts.kind,
            parts.instance_path,
            parts.schema_path,
            parts.evaluation_path,
        )
        .with_absolute_keyword_location(parts.absolute_keyword_location)
    }

    /// Restates a compile-time failure as a judgement on the schema: the location that failed
    /// to compile is where the problem sits in it.
    pub(crate) fn into_build_error(self) -> ValidationError<'static> {
        let parts = self.into_parts();
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(parts.instance.into_owned())),
            parts.kind,
            parts.schema_path.clone(),
            parts.schema_path,
            parts.evaluation_path,
        )
        .with_absolute_keyword_location(parts.absolute_keyword_location)
    }

    pub(crate) fn additional_items(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: usize,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::AdditionalItems { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn additional_properties(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        unexpected: Vec<String>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::AdditionalProperties { unexpected },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn any_of(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        context: Vec<Vec<ValidationError<'a>>>,
    ) -> ValidationError<'a> {
        let context = context
            .into_iter()
            .map(|errors| errors.into_iter().map(ValidationError::to_owned).collect())
            .collect::<Vec<_>>();

        Self::borrowed(
            instance,
            ValidationErrorKind::AnyOf { context },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn backtrack_limit(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        error: fancy_regex::Error,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::BacktrackLimitExceeded { error },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn regex_engine_failure(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        message: String,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::RegexEngineFailure { message },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_array(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        expected_value: &[Value],
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: Value::Array(expected_value.to_vec()),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_boolean(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        expected_value: bool,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: Value::Bool(expected_value),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_null(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: Value::Null,
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_number(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        expected_value: &Number,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: Value::Number(expected_value.clone()),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_object(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        expected_value: &Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: expected_value.clone(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn constant_string(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        expected_value: &str,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Constant {
                expected_value: Value::String(expected_value.to_string()),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn contains(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Contains,
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn content_encoding(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        encoding: &str,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::ContentEncoding {
                content_encoding: encoding.to_string(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn content_media_type(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        media_type: &str,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::ContentMediaType {
                content_media_type: media_type.to_string(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn enumeration(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        options: &Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Enum {
                options: options.clone(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn exclusive_maximum(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::ExclusiveMaximum { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn exclusive_minimum(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::ExclusiveMinimum { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn false_schema(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::FalseSchema,
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn format(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        format: impl Into<String>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Format {
                format: format.into(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn from_utf8(error: FromUtf8Error) -> ValidationError<'a> {
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(Value::Null)),
            ValidationErrorKind::FromUtf8 { error },
            Location::new(),
            Location::new(),
            Location::new(),
        )
    }
    pub(crate) fn max_items(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MaxItems { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn maximum(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Maximum { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn max_length(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MaxLength { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn max_properties(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MaxProperties { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn min_items(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MinItems { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn minimum(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Minimum { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn min_length(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MinLength { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn min_properties(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        limit: u64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MinProperties { limit },
            instance_path,
            schema_path,
            tracker,
        )
    }
    #[cfg(feature = "arbitrary-precision")]
    pub(crate) fn multiple_of(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        multiple_of: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MultipleOf { multiple_of },
            instance_path,
            schema_path,
            tracker,
        )
    }

    #[cfg(not(feature = "arbitrary-precision"))]
    pub(crate) fn multiple_of(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        multiple_of: f64,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::MultipleOf { multiple_of },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn not(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        schema: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Not { schema },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn one_of_multiple_valid(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        context: Vec<Vec<ValidationError<'a>>>,
    ) -> ValidationError<'a> {
        let context = context
            .into_iter()
            .map(|errors| errors.into_iter().map(ValidationError::to_owned).collect())
            .collect::<Vec<_>>();

        Self::borrowed(
            instance,
            ValidationErrorKind::OneOfMultipleValid { context },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn one_of_not_valid(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        context: Vec<Vec<ValidationError<'a>>>,
    ) -> ValidationError<'a> {
        let context = context
            .into_iter()
            .map(|errors| errors.into_iter().map(ValidationError::to_owned).collect())
            .collect::<Vec<_>>();

        Self::borrowed(
            instance,
            ValidationErrorKind::OneOfNotValid { context },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn pattern(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        pattern: String,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Pattern { pattern },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn property_names(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        error: ValidationError<'a>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::PropertyNames {
                error: Box::new(error.to_owned()),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn required(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        property: Value,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Required { property },
            instance_path,
            schema_path,
            tracker,
        )
    }

    pub(crate) fn single_type_error(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        type_name: JsonType,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Type {
                kind: TypeKind::Single(type_name),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn multiple_type_error(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        types: JsonTypeSet,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Type {
                kind: TypeKind::Multiple(types),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn unevaluated_items(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        unexpected: Vec<String>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::UnevaluatedItems { unexpected },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn unevaluated_properties(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        unexpected: Vec<String>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::UnevaluatedProperties { unexpected },
            instance_path,
            schema_path,
            tracker,
        )
    }
    pub(crate) fn unique_items(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::UniqueItems,
            instance_path,
            schema_path,
            tracker,
        )
    }
    /// Create a custom validation error with just a message.
    ///
    /// Use this in [`Keyword::validate`](schema_validation::Keyword::validate) implementations.
    /// The actual instance, instance path, and schema path are filled in automatically.
    ///
    /// # Example
    ///
    /// ```rust
    /// use schema_validation::ValidationError;
    ///
    /// fn validate_even(n: u64) -> Result<(), ValidationError<'static>> {
    ///     if n % 2 != 0 {
    ///         return Err(ValidationError::custom("number must be even"));
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn custom(message: impl Into<String>) -> ValidationError<'static> {
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(Value::Null)),
            ValidationErrorKind::Custom {
                keyword: String::new(),
                message: message.into(),
            },
            Location::new(),
            Location::new(),
            Location::new(),
        )
    }

    /// Create an error for invalid schema values in keyword factories.
    ///
    /// Use this in custom keyword factory functions when the schema value
    /// is invalid for your custom keyword.
    pub fn schema(message: impl Into<String>) -> ValidationError<'static> {
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(Value::Null)),
            ValidationErrorKind::Custom {
                keyword: String::new(),
                message: message.into(),
            },
            Location::new(),
            Location::new(),
            Location::new(),
        )
    }

    /// Fill in context for a placeholder validation error.
    ///
    /// Used by custom keywords, which never involve `$ref` traversal,
    /// so evaluation path equals schema path.
    pub(crate) fn with_context<'i>(
        self,
        instance: &'i Value,
        instance_path: &LazyLocation,
        schema_path: &Location,
        keyword: &str,
    ) -> ValidationError<'i> {
        let kind = match self.repr.kind {
            ValidationErrorKind::Custom { message, .. } => ValidationErrorKind::Custom {
                keyword: keyword.to_string(),
                message,
            },
            other => other,
        };
        ValidationError::new(
            LazyInstance::Ready(Cow::Borrowed(instance)),
            kind,
            instance_path.into(),
            schema_path.clone(),
            // Custom keywords are never reached via $ref, so evaluation path = schema path
            LazyEvaluationPath::SameAsSchemaPath,
        )
    }

    /// [`Self::with_context`] variant for macro-generated code, which tracks the instance path as an owned [`Location`].

    /// Fill in context for a placeholder schema error (used in keyword factories).
    pub(crate) fn with_schema_context<'s>(
        self,
        schema_value: &'s Value,
        schema_path: Location,
        keyword: &str,
    ) -> ValidationError<'s> {
        let kind = match self.repr.kind {
            ValidationErrorKind::Custom { message, .. } => ValidationErrorKind::Custom {
                keyword: keyword.into(),
                message,
            },
            other => other,
        };
        ValidationError::new(
            LazyInstance::Ready(Cow::Borrowed(schema_value)),
            kind,
            Location::new(),
            schema_path.clone(),
            schema_path,
        )
    }

    pub(crate) fn compile_error(
        schema_path: Location,
        tracker: impl Into<LazyEvaluationPath>,
        instance_path: Location,
        instance: LazyInstance<'a>,
        message: impl Into<String>,
    ) -> ValidationError<'a> {
        Self::borrowed(
            instance,
            ValidationErrorKind::Custom {
                keyword: String::new(),
                message: message.into(),
            },
            instance_path,
            schema_path,
            tracker,
        )
    }
}

impl error::Error for ValidationError<'_> {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self.kind() {
            ValidationErrorKind::BacktrackLimitExceeded { error } => Some(error),
            _ => None,
        }
    }
}
impl From<crate::reference::Error> for ValidationError<'_> {
    #[inline]
    fn from(err: crate::reference::Error) -> Self {
        ValidationError::new(
            LazyInstance::Ready(Cow::Owned(Value::Null)),
            ValidationErrorKind::Referencing(err),
            Location::new(),
            Location::new(),
            Location::new(),
        )
    }
}
impl From<FromUtf8Error> for ValidationError<'_> {
    #[inline]
    fn from(err: FromUtf8Error) -> Self {
        ValidationError::from_utf8(err)
    }
}

fn write_quoted_list(f: &mut Formatter<'_>, items: &[impl fmt::Display]) -> fmt::Result {
    let mut iter = items.iter();
    if let Some(item) = iter.next() {
        f.write_char('\'')?;
        write!(f, "{item}")?;
        f.write_char('\'')?;
    }
    for item in iter {
        f.write_str(", ")?;
        f.write_char('\'')?;
        write!(f, "{item}")?;
        f.write_char('\'')?;
    }
    Ok(())
}

fn write_unexpected_suffix(f: &mut Formatter<'_>, len: usize) -> fmt::Result {
    f.write_str(if len == 1 {
        " was unexpected)"
    } else {
        " were unexpected)"
    })
}

fn write_item_count(f: &mut Formatter<'_>, count: usize) -> fmt::Result {
    write!(f, "{count} item{})", if count == 1 { "" } else { "s" })
}

const MAX_DISPLAYED_ENUM_VARIANTS: usize = 3;

fn write_enum_message(
    f: &mut Formatter<'_>,
    value: impl fmt::Display,
    options: &Value,
) -> fmt::Result {
    let array = options
        .as_array()
        .expect("Enum options must be a JSON array");

    write!(f, "{value} is not one of ")?;

    let total_count = array.len();

    if total_count <= MAX_DISPLAYED_ENUM_VARIANTS {
        // Show all options with proper "a, b or c" formatting
        for (i, option) in array.iter().enumerate() {
            if i == 0 {
                write!(f, "{option}")?;
            } else if i == total_count - 1 {
                write!(f, " or {option}")?;
            } else {
                write!(f, ", {option}")?;
            }
        }
    } else {
        // Show first few, then "or X other candidates"
        let show_count = MAX_DISPLAYED_ENUM_VARIANTS - 1;
        for (i, option) in array.iter().take(show_count).enumerate() {
            if i == 0 {
                write!(f, "{option}")?;
            } else {
                write!(f, ", {option}")?;
            }
        }
        let remaining = total_count - show_count;
        write!(f, " or {remaining} other candidates")?;
    }
    Ok(())
}

/// Textual representation of various validation errors.
impl fmt::Display for ValidationError<'_> {
    #[allow(clippy::too_many_lines)] // The function is long but it does formatting only
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.kind() {
            ValidationErrorKind::Referencing(error) => error.fmt(f),
            ValidationErrorKind::BacktrackLimitExceeded { error } => error.fmt(f),
            ValidationErrorKind::Format { format } => {
                write!(f, r#"{} is not a "{}""#, self.instance(), format)
            }
            ValidationErrorKind::AdditionalItems { limit } => {
                f.write_str("Additional items are not allowed (")?;
                let array = self.instance().as_array().expect("Always valid");
                let mut iter = array.iter().skip(*limit);

                if let Some(item) = iter.next() {
                    write!(f, "{item}")?;
                }
                for item in iter {
                    f.write_str(", ")?;
                    write!(f, "{item}")?;
                }

                write_unexpected_suffix(f, array.len() - limit)
            }
            ValidationErrorKind::AdditionalProperties { unexpected } => {
                f.write_str("Additional properties are not allowed (")?;
                write_quoted_list(f, unexpected)?;
                write_unexpected_suffix(f, unexpected.len())
            }
            ValidationErrorKind::AnyOf { context: _ } => write!(
                f,
                "{} is not valid under any of the schemas listed in the 'anyOf' keyword",
                self.instance()
            ),
            ValidationErrorKind::OneOfNotValid { context: _ } => write!(
                f,
                "{} is not valid under any of the schemas listed in the 'oneOf' keyword",
                self.instance()
            ),
            ValidationErrorKind::Contains => write!(
                f,
                "None of {} are valid under the given schema",
                self.instance()
            ),
            ValidationErrorKind::Constant { expected_value } => {
                write!(f, "{expected_value} was expected")
            }
            ValidationErrorKind::ContentEncoding { content_encoding } => {
                write!(
                    f,
                    r#"{} is not compliant with "{}" content encoding"#,
                    self.instance(),
                    content_encoding
                )
            }
            ValidationErrorKind::ContentMediaType { content_media_type } => {
                write!(
                    f,
                    r#"{} is not compliant with "{}" media type"#,
                    self.instance(),
                    content_media_type
                )
            }
            ValidationErrorKind::FromUtf8 { error } => error.fmt(f),
            ValidationErrorKind::Enum { options } => {
                write_enum_message(f, self.instance(), options)
            }
            ValidationErrorKind::ExclusiveMaximum { limit } => write!(
                f,
                "{} is greater than or equal to the maximum of {}",
                self.instance(),
                limit
            ),
            ValidationErrorKind::ExclusiveMinimum { limit } => write!(
                f,
                "{} is less than or equal to the minimum of {}",
                self.instance(),
                limit
            ),
            ValidationErrorKind::FalseSchema => {
                write!(f, "False schema does not allow {}", self.instance())
            }
            ValidationErrorKind::Maximum { limit } => write!(
                f,
                "{} is greater than the maximum of {}",
                self.instance(),
                limit
            ),
            ValidationErrorKind::Minimum { limit } => {
                write!(
                    f,
                    "{} is less than the minimum of {}",
                    self.instance(),
                    limit
                )
            }
            ValidationErrorKind::MaxLength { limit } => write!(
                f,
                "{} is longer than {} character{}",
                self.instance(),
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MinLength { limit } => write!(
                f,
                "{} is shorter than {} character{}",
                self.instance(),
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MaxItems { limit } => write!(
                f,
                "{} has more than {} item{}",
                self.instance(),
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MinItems { limit } => write!(
                f,
                "{} has less than {} item{}",
                self.instance(),
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MaxProperties { limit } => write!(
                f,
                "{} has more than {} propert{}",
                self.instance(),
                limit,
                if *limit == 1 { "y" } else { "ies" }
            ),
            ValidationErrorKind::MinProperties { limit } => write!(
                f,
                "{} has less than {} propert{}",
                self.instance(),
                limit,
                if *limit == 1 { "y" } else { "ies" }
            ),
            ValidationErrorKind::Not { schema } => {
                write!(f, "{} is not allowed for {}", schema, self.instance())
            }
            ValidationErrorKind::OneOfMultipleValid { .. } => write!(
                f,
                "{} is valid under more than one of the schemas listed in the 'oneOf' keyword",
                self.instance()
            ),
            ValidationErrorKind::Pattern { pattern } => {
                write!(f, r#"{} does not match "{}""#, self.instance(), pattern)
            }
            ValidationErrorKind::PropertyNames { error } => error.fmt(f),
            ValidationErrorKind::Required { property } => {
                write!(f, "{property} is a required property")
            }
            ValidationErrorKind::MultipleOf { multiple_of } => {
                write!(
                    f,
                    "{} is not a multiple of {}",
                    self.instance(),
                    multiple_of
                )
            }
            ValidationErrorKind::UnevaluatedItems { unexpected } => {
                f.write_str("Unevaluated items are not allowed (")?;
                write_quoted_list(f, unexpected)?;
                write_unexpected_suffix(f, unexpected.len())
            }
            ValidationErrorKind::UnevaluatedProperties { unexpected } => {
                f.write_str("Unevaluated properties are not allowed (")?;
                write_quoted_list(f, unexpected)?;
                write_unexpected_suffix(f, unexpected.len())
            }
            ValidationErrorKind::UniqueItems => {
                write!(f, "{} has non-unique elements", self.instance())
            }
            ValidationErrorKind::Type {
                kind: TypeKind::Single(type_),
            } => write!(f, r#"{} is not of type "{}""#, self.instance(), type_),
            ValidationErrorKind::Type {
                kind: TypeKind::Multiple(types),
            } => {
                write!(f, "{} is not of types ", self.instance())?;
                let mut iter = types.iter();
                if let Some(t) = iter.next() {
                    f.write_char('"')?;
                    write!(f, "{t}")?;
                    f.write_char('"')?;
                }
                for t in iter {
                    f.write_str(", ")?;
                    f.write_char('"')?;
                    write!(f, "{t}")?;
                    f.write_char('"')?;
                }
                Ok(())
            }
            ValidationErrorKind::Custom { message, .. }
            | ValidationErrorKind::RegexEngineFailure { message } => f.write_str(message),
        }
    }
}

/// A wrapper that provides a masked display of validation errors.
pub struct MaskedValidationError<'a, 'b, 'c> {
    error: &'b ValidationError<'a>,
    placeholder: Cow<'c, str>,
}

impl fmt::Display for MaskedValidationError<'_, '_, '_> {
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.error.kind() {
            ValidationErrorKind::Referencing(error) => error.fmt(f),
            ValidationErrorKind::BacktrackLimitExceeded { error } => error.fmt(f),
            ValidationErrorKind::Format { format } => {
                write!(f, r#"{} is not a "{format}""#, self.placeholder)
            }
            ValidationErrorKind::AdditionalItems { limit } => {
                // `limit` is how many items the schema allows; report how many exceed it.
                let extra = self
                    .error
                    .instance()
                    .as_array()
                    .map_or(0, |array| array.len().saturating_sub(*limit));
                f.write_str("Additional items are not allowed (")?;
                write_item_count(f, extra)
            }
            ValidationErrorKind::AdditionalProperties { unexpected } => {
                f.write_str("Additional properties are not allowed (")?;
                write_quoted_list(f, unexpected)?;
                write_unexpected_suffix(f, unexpected.len())
            }
            ValidationErrorKind::AnyOf { .. } => write!(
                f,
                "{} is not valid under any of the schemas listed in the 'anyOf' keyword",
                self.placeholder
            ),
            ValidationErrorKind::OneOfNotValid { context: _ } => write!(
                f,
                "{} is not valid under any of the schemas listed in the 'oneOf' keyword",
                self.placeholder
            ),
            ValidationErrorKind::Contains => write!(
                f,
                "None of {} are valid under the given schema",
                self.placeholder
            ),
            ValidationErrorKind::Constant { expected_value } => {
                write!(f, "{expected_value} was expected")
            }
            ValidationErrorKind::ContentEncoding { content_encoding } => {
                write!(
                    f,
                    r#"{} is not compliant with "{}" content encoding"#,
                    self.placeholder, content_encoding
                )
            }
            ValidationErrorKind::ContentMediaType { content_media_type } => {
                write!(
                    f,
                    r#"{} is not compliant with "{}" media type"#,
                    self.placeholder, content_media_type
                )
            }
            ValidationErrorKind::FromUtf8 { error } => error.fmt(f),
            ValidationErrorKind::Enum { options } => {
                write_enum_message(f, &self.placeholder, options)
            }
            ValidationErrorKind::ExclusiveMaximum { limit } => write!(
                f,
                "{} is greater than or equal to the maximum of {}",
                self.placeholder, limit
            ),
            ValidationErrorKind::ExclusiveMinimum { limit } => write!(
                f,
                "{} is less than or equal to the minimum of {}",
                self.placeholder, limit
            ),
            ValidationErrorKind::FalseSchema => {
                write!(f, "False schema does not allow {}", self.placeholder)
            }
            ValidationErrorKind::Maximum { limit } => write!(
                f,
                "{} is greater than the maximum of {}",
                self.placeholder, limit
            ),
            ValidationErrorKind::Minimum { limit } => {
                write!(
                    f,
                    "{} is less than the minimum of {}",
                    self.placeholder, limit
                )
            }
            ValidationErrorKind::MaxLength { limit } => write!(
                f,
                "{} is longer than {} character{}",
                self.placeholder,
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MinLength { limit } => write!(
                f,
                "{} is shorter than {} character{}",
                self.placeholder,
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MaxItems { limit } => write!(
                f,
                "{} has more than {} item{}",
                self.placeholder,
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MinItems { limit } => write!(
                f,
                "{} has less than {} item{}",
                self.placeholder,
                limit,
                if *limit == 1 { "" } else { "s" }
            ),
            ValidationErrorKind::MaxProperties { limit } => write!(
                f,
                "{} has more than {} propert{}",
                self.placeholder,
                limit,
                if *limit == 1 { "y" } else { "ies" }
            ),
            ValidationErrorKind::MinProperties { limit } => write!(
                f,
                "{} has less than {} propert{}",
                self.placeholder,
                limit,
                if *limit == 1 { "y" } else { "ies" }
            ),
            ValidationErrorKind::Not { schema } => {
                write!(f, "{} is not allowed for {}", schema, self.placeholder)
            }
            ValidationErrorKind::OneOfMultipleValid { .. } => write!(
                f,
                "{} is valid under more than one of the schemas listed in the 'oneOf' keyword",
                self.placeholder
            ),
            ValidationErrorKind::Pattern { pattern } => {
                write!(f, r#"{} does not match "{}""#, self.placeholder, pattern)
            }
            ValidationErrorKind::PropertyNames { error } => error.fmt(f),
            ValidationErrorKind::Required { property } => {
                write!(f, "{property} is a required property")
            }
            ValidationErrorKind::MultipleOf { multiple_of } => {
                write!(
                    f,
                    "{} is not a multiple of {}",
                    self.placeholder, multiple_of
                )
            }
            ValidationErrorKind::UnevaluatedItems { unexpected } => {
                f.write_str("Unevaluated items are not allowed (")?;
                write_item_count(f, unexpected.len())
            }
            ValidationErrorKind::UnevaluatedProperties { unexpected } => {
                f.write_str("Unevaluated properties are not allowed (")?;
                write_quoted_list(f, unexpected)?;
                write_unexpected_suffix(f, unexpected.len())
            }
            ValidationErrorKind::UniqueItems => {
                write!(f, "{} has non-unique elements", self.placeholder)
            }
            ValidationErrorKind::Type {
                kind: TypeKind::Single(type_),
            } => write!(f, r#"{} is not of type "{}""#, self.placeholder, type_),
            ValidationErrorKind::Type {
                kind: TypeKind::Multiple(types),
            } => {
                write!(f, "{} is not of types ", self.placeholder)?;
                let mut iter = types.iter();
                if let Some(t) = iter.next() {
                    f.write_char('"')?;
                    write!(f, "{t}")?;
                    f.write_char('"')?;
                }
                for t in iter {
                    f.write_str(", ")?;
                    f.write_char('"')?;
                    write!(f, "{t}")?;
                    f.write_char('"')?;
                }
                Ok(())
            }
            ValidationErrorKind::Custom { message, .. }
            | ValidationErrorKind::RegexEngineFailure { message } => f.write_str(message),
        }
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/schema-validation/upstream/jsonschema/unit/src/error/tests_1859.rs"
));
