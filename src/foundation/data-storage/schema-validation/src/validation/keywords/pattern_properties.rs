use crate::validation::regex::property_match_error;
use crate::validation::LazyInstance;
use std::{borrow::Cow, sync::Arc};

use crate::validation::{
    compiler,
    error::ValidationError,
    evaluation::{Annotations, ChildList, ErrorDescription},
    keywords::CompilationResult,
    node::SchemaNode,
    options::PatternEngineOptions,
    paths::{LazyEvaluationPath, LazyLocation, Location, RefTracker},
    regex::{analyze_pattern, LiteralMatcher, PatternOptimization, RegexEngine},
    types::JsonType,
    validator::{EvaluationResult, Validate, ValidationContext},
    Json, JsonValueBackend, Node, Object,
};
use json_codec::{Map, Value};

/// Validator for multiple patterns using compiled regex.
pub(crate) struct PatternPropertiesValidator<R, F: Json = JsonValueBackend> {
    patterns: Vec<(Arc<R>, SchemaNode<F>)>,
}

impl<F: Json, R: RegexEngine> Validate<F> for PatternPropertiesValidator<R, F> {
    fn is_valid(&self, instance: &F::Node<'_>, ctx: &mut ValidationContext) -> bool {
        if let Some(object) = instance.as_object() {
            for (re, node) in &self.patterns {
                for (key, value) in object.members() {
                    if match re.is_match(key.as_ref()) {
                        Ok(matched) => matched,
                        Err(_) => return false,
                    } && !node.is_valid(&value, ctx)
                    {
                        return false;
                    }
                }
            }
            true
        } else {
            true
        }
    }

    fn validate<'i>(
        &self,
        instance: &F::Node<'i>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
    ) -> Result<(), ValidationError<'i>> {
        if let Some(object) = instance.as_object() {
            for (key, value) in object.members() {
                for (re, node) in &self.patterns {
                    if re.is_match(key.as_ref()).map_err(|error| {
                        property_match_error(
                            error,
                            node.location(),
                            &location.push(key.as_ref()),
                            tracker,
                            key.as_ref(),
                        )
                    })? {
                        node.validate(&value, &location.push(key.as_ref()), tracker, ctx)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn collect_errors<'i>(
        &self,
        instance: &F::Node<'i>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
        errors: &mut Vec<ValidationError<'i>>,
    ) {
        let Some(object) = instance.as_object() else {
            return;
        };
        for (re, node) in &self.patterns {
            for (key, value) in object.members() {
                if match re.is_match(key.as_ref()) {
                    Ok(matched) => matched,
                    Err(error) => {
                        errors.push(property_match_error(
                            error,
                            node.location(),
                            &location.push(key.as_ref()),
                            tracker,
                            key.as_ref(),
                        ));
                        continue;
                    }
                } {
                    node.collect_errors(&value, &location.push(key.as_ref()), tracker, ctx, errors);
                }
            }
        }
    }

    fn evaluate(
        &self,
        instance: &F::Node<'_>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
    ) -> EvaluationResult {
        if let Some(object) = instance.as_object() {
            let mut matched_propnames = Vec::with_capacity(object.len());
            let mut children = ChildList::default();
            for (pattern, node) in &self.patterns {
                for (key, value) in object.members() {
                    if match pattern.is_match(key.as_ref()) {
                        Ok(matched) => matched,
                        Err(error) => {
                            return EvaluationResult::invalid_empty(vec![
                                ErrorDescription::from_validation_error(&property_match_error(
                                    error,
                                    node.location(),
                                    &location.push(key.as_ref()),
                                    tracker,
                                    key.as_ref(),
                                )),
                            ])
                        }
                    } {
                        matched_propnames.push(key.as_ref().to_owned());
                        let child = node.evaluate_instance_below(
                            &value,
                            &location.push(key.as_ref()),
                            tracker,
                            ctx,
                        );
                        children.push(&mut ctx.arena, child);
                    }
                }
            }
            let mut result = EvaluationResult::from_children(children);
            result.annotate(Annotations::new(Value::from(matched_propnames)));
            result
        } else {
            EvaluationResult::valid_empty()
        }
    }
}

pub(crate) struct SingleValuePatternPropertiesValidator<R, F: Json = JsonValueBackend> {
    regex: Arc<R>,
    node: SchemaNode<F>,
}

impl<F: Json, R: RegexEngine> Validate<F> for SingleValuePatternPropertiesValidator<R, F> {
    fn is_valid(&self, instance: &F::Node<'_>, ctx: &mut ValidationContext) -> bool {
        if let Some(object) = instance.as_object() {
            for (key, value) in object.members() {
                if match self.regex.is_match(key.as_ref()) {
                    Ok(matched) => matched,
                    Err(_) => return false,
                } && !self.node.is_valid(&value, ctx)
                {
                    return false;
                }
            }
            true
        } else {
            true
        }
    }

    fn validate<'i>(
        &self,
        instance: &F::Node<'i>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
    ) -> Result<(), ValidationError<'i>> {
        if let Some(object) = instance.as_object() {
            for (key, value) in object.members() {
                if self.regex.is_match(key.as_ref()).map_err(|error| {
                    property_match_error(
                        error,
                        self.node.location(),
                        &location.push(key.as_ref()),
                        tracker,
                        key.as_ref(),
                    )
                })? {
                    self.node
                        .validate(&value, &location.push(key.as_ref()), tracker, ctx)?;
                }
            }
        }
        Ok(())
    }

    fn collect_errors<'i>(
        &self,
        instance: &F::Node<'i>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
        errors: &mut Vec<ValidationError<'i>>,
    ) {
        let Some(object) = instance.as_object() else {
            return;
        };
        for (key, value) in object.members() {
            if match self.regex.is_match(key.as_ref()) {
                Ok(matched) => matched,
                Err(error) => {
                    errors.push(property_match_error(
                        error,
                        self.node.location(),
                        &location.push(key.as_ref()),
                        tracker,
                        key.as_ref(),
                    ));
                    continue;
                }
            } {
                self.node.collect_errors(
                    &value,
                    &location.push(key.as_ref()),
                    tracker,
                    ctx,
                    errors,
                );
            }
        }
    }

    fn evaluate(
        &self,
        instance: &F::Node<'_>,
        location: &LazyLocation,
        tracker: Option<&RefTracker>,
        ctx: &mut ValidationContext,
    ) -> EvaluationResult {
        if let Some(object) = instance.as_object() {
            let mut matched_propnames = Vec::with_capacity(object.len());
            let mut children = ChildList::default();
            for (key, value) in object.members() {
                if match self.regex.is_match(key.as_ref()) {
                    Ok(matched) => matched,
                    Err(error) => {
                        return EvaluationResult::invalid_empty(vec![
                            ErrorDescription::from_validation_error(&property_match_error(
                                error,
                                self.node.location(),
                                &location.push(key.as_ref()),
                                tracker,
                                key.as_ref(),
                            )),
                        ])
                    }
                } {
                    matched_propnames.push(key.as_ref().to_owned());
                    let child = self.node.evaluate_instance_below(
                        &value,
                        &location.push(key.as_ref()),
                        tracker,
                        ctx,
                    );
                    children.push(&mut ctx.arena, child);
                }
            }
            let mut result = EvaluationResult::from_children(children);
            result.annotate(Annotations::new(Value::from(matched_propnames)));
            result
        } else {
            EvaluationResult::valid_empty()
        }
    }
}

#[inline]
pub(crate) fn compile<'a, F: Json>(
    ctx: &compiler::Context<F>,
    parent: &'a Map<String, Value>,
    schema: &'a Value,
) -> Option<CompilationResult<'a, F>> {
    if matches!(
        parent.get("additionalProperties"),
        Some(Value::Bool(false) | Value::Object(_))
    ) && !ctx.is_keyword_overridden("additionalProperties")
    {
        // This type of `additionalProperties` validator handles `patternProperties` logic
        return None;
    }

    let Value::Object(map) = schema else {
        let location = ctx.location().join("patternProperties");
        return Some(Err(ValidationError::single_type_error(
            location.clone(),
            location,
            Location::new(),
            LazyInstance::Ready(Cow::Borrowed(schema)),
            JsonType::Object,
        )));
    };
    let ctx = ctx.new_at_location("patternProperties");

    // Try to compile all patterns as literal matches first (optimized path)
    if let Some(validator) = try_compile_as_literals(&ctx, map) {
        return Some(validator);
    }

    // Fall back to regex compilation
    let result = match ctx.config().pattern_options() {
        PatternEngineOptions::FancyRegex { .. } => {
            compile_pattern_entries(&ctx, map, |pctx, pattern| {
                pctx.get_or_compile_regex(pattern)
                    .map_err(|()| invalid_regex(pctx, pattern))
            })
            .map(|patterns| {
                build_validator_from_entries(patterns, |regex, node| {
                    Box::new(SingleValuePatternPropertiesValidator { regex, node })
                        as Box<dyn Validate<F>>
                })
            })
        }
        PatternEngineOptions::Regex { .. } => {
            compile_pattern_entries(&ctx, map, |pctx, pattern| {
                pctx.get_or_compile_standard_regex(pattern)
                    .map_err(|()| invalid_regex(pctx, pattern))
            })
            .map(|patterns| {
                build_validator_from_entries(patterns, |regex, node| {
                    Box::new(SingleValuePatternPropertiesValidator { regex, node })
                        as Box<dyn Validate<F>>
                })
            })
        }
    };
    Some(result)
}

/// Try to compile all patterns as literal matches (prefix or exact).
/// Returns `Some` if ALL patterns are optimizable, `None` if any requires a full regex.
fn try_compile_as_literals<'a, F: Json>(
    ctx: &compiler::Context<F>,
    map: &'a Map<String, Value>,
) -> Option<CompilationResult<'a, F>> {
    let mut entries = Vec::with_capacity(map.len());
    for (pattern, subschema) in map {
        let pctx = ctx.new_at_location(pattern.as_str());
        let matcher = match analyze_pattern(pattern)? {
            PatternOptimization::Prefix(literal) => LiteralMatcher::Prefix { literal },
            PatternOptimization::Exact(exact) => LiteralMatcher::Exact { exact },
            PatternOptimization::Alternation(alternatives) => {
                LiteralMatcher::Alternation { alternatives }
            }
            PatternOptimization::NoWhitespace => LiteralMatcher::NoWhitespace,
        };
        let node = match compiler::compile(&pctx, pctx.as_resource_ref(subschema)) {
            Ok(node) => node,
            Err(e) => return Some(Err(e)),
        };
        entries.push((Arc::new(matcher), node));
    }
    Some(Ok(build_validator_from_entries(entries, |regex, node| {
        Box::new(SingleValuePatternPropertiesValidator { regex, node }) as Box<dyn Validate<F>>
    })))
}

/// Build error for a `patternProperties` key that is not a valid regex; `ctx` points at the key.
pub(crate) fn invalid_regex<F: Json>(
    ctx: &compiler::Context<F>,
    pattern: &str,
) -> ValidationError<'static> {
    ValidationError::format(
        ctx.location().clone(),
        LazyEvaluationPath::SameAsSchemaPath,
        Location::new(),
        LazyInstance::Ready(Cow::Owned(Value::String(pattern.to_owned()))),
        "regex",
    )
}

type CompiledPatterns<R, F> = Vec<(Arc<R>, SchemaNode<F>)>;

/// Compile every `(pattern, subschema)` pair into `(regex, node)` tuples.
fn compile_pattern_entries<'a, R, C, F: Json>(
    ctx: &compiler::Context<F>,
    map: &'a Map<String, Value>,
    mut compile_regex: C,
) -> Result<CompiledPatterns<R, F>, ValidationError<'a>>
where
    C: FnMut(&compiler::Context<F>, &str) -> Result<Arc<R>, ValidationError<'a>>,
{
    let mut patterns = Vec::with_capacity(map.len());
    for (pattern, subschema) in map {
        let pctx = ctx.new_at_location(pattern.as_str());
        let regex = compile_regex(&pctx, pattern)?;
        let node = compiler::compile(&pctx, pctx.as_resource_ref(subschema))?;
        patterns.push((regex, node));
    }
    Ok(patterns)
}

/// Pick the optimal validator representation for the compiled pattern entries.
fn build_validator_from_entries<R, F: Json>(
    mut entries: Vec<(Arc<R>, SchemaNode<F>)>,
    single_factory: impl FnOnce(Arc<R>, SchemaNode<F>) -> Box<dyn Validate<F>>,
) -> Box<dyn Validate<F>>
where
    R: RegexEngine + 'static,
{
    if entries.len() == 1 {
        let (regex, node) = entries.pop().expect("len checked");
        single_factory(regex, node)
    } else {
        Box::new(PatternPropertiesValidator { patterns: entries })
    }
}

#[cfg(test)]
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../test/foundation/schema-validation/upstream/jsonschema/unit/src/keywords/pattern_properties/tests_347.rs"
));
