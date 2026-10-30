//! Fully offline documentation and published-contract checks.
mod document_links;
mod document_scan;
use data_serialization::Serialize;
use json_codec::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug)]
struct ContextError {
    context: String,
    source: Box<dyn std::error::Error>,
}
impl std::fmt::Display for ContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.context, self.source)
    }
}
impl std::error::Error for ContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}
fn require(condition: bool, message: impl Into<String>) -> Result {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}
fn array(value: &Value) -> Result<&Vec<Value>> {
    value.as_array().ok_or_else(|| "expected array".into())
}
fn object(value: &Value) -> Result<&json_codec::Map<String, Value>> {
    value.as_object().ok_or_else(|| "expected object".into())
}
fn text(value: &Value) -> Result<&str> {
    value.as_str().ok_or_else(|| "expected string".into())
}
fn uint(value: &Value) -> Result<u64> {
    Ok(storage_contract::parse_u64_decimal(text(value)?)?)
}
fn small(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .ok_or_else(|| "expected unsigned small integer".into())
}
fn read(path: &Path) -> Result<Value> {
    Ok(storage_contract::strict_json(&fs::read(path)?)?)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn unhex(value: &str) -> Result<Vec<u8>> {
    require(
        value.len().is_multiple_of(2) && value.is_ascii(),
        "invalid hex length",
    )?;
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(Into::into))
        .collect()
}

fn validate_result(result: &Value) -> Result {
    let mut seen = BTreeSet::new();
    let mut healthy = true;
    for version in array(&result["versions"])? {
        require(
            seen.insert(text(&version["version_id"])?),
            "duplicate version",
        )?;
        let route = array(&version["required_slots"])?;
        let slots = array(&version["slots"])?;
        require(
            route.len() as u64 == small(&version["target_copies"])? && route.len() == slots.len(),
            "route count",
        )?;
        let aliases = route
            .iter()
            .map(|slot| Ok(text(slot)?.to_ascii_lowercase()))
            .collect::<Result<BTreeSet<_>>>()?;
        require(aliases.len() == route.len(), "slot aliases")?;
        require(
            route.first() == Some(&version["primary_slot_id"]),
            "primary route mismatch",
        )?;
        let mut disks = BTreeSet::new();
        for (index, slot) in slots.iter().enumerate() {
            require(slot["slot_id"] == route[index], "slot order mismatch")?;
            require(
                small(&slot["copy_level"])? == index as u64 + 1,
                "nonadjacent levels",
            )?;
            require(disks.insert(text(&slot["disk_id"])?), "same physical disk")?;
            healthy &= slot["state"] == "COMPLETE_VERIFIED";
        }
    }
    require(
        healthy == (result["outcome"] == "COMMITTED_AND_PROTECTED"),
        "protection result mismatch",
    )
}

fn validate_content_view(value: &Value) -> Result {
    let inherited = value["inherited"]
        .as_bool()
        .ok_or("inherited must be boolean")?;
    require(
        value["materialized"] == !inherited,
        "content ownership and inheritance disagree",
    )?;
    if inherited {
        require(
            value["own_version_id"].is_null(),
            "inherited content is not owned",
        )?;
        require(
            value["version_id"] == value["source_version_id"],
            "inherited visible version differs from frozen parent",
        )?;
        require(
            value["content_state"] == "PENDING",
            "inherited ownership state",
        )?;
        require(
            value["visible_content_state"] != "PENDING",
            "visible parent state",
        )?;
    } else {
        require(
            value["source_version_id"].is_null(),
            "owned content has no inherited parent",
        )?;
        require(
            value["version_id"] == value["own_version_id"],
            "owned visible version differs from own version",
        )?;
        require(
            value["content_state"] == value["visible_content_state"],
            "owned visible state differs from content state",
        )?;
    }
    Ok(())
}

fn semantics(name: &str, value: &Value) -> Result {
    match name {
        "config" => {
            let metadata = &value["metadata"];
            let members = small(&metadata["replica_members"])?;
            let majority = members / 2 + 1;
            require(
                members > 0
                    && small(&metadata["decision_members"])? == majority
                    && small(&metadata["recovery_evidence_members"])? == majority,
                "metadata quorum must match fixed configured membership",
            )?;
            let r = &value["resource_limits"];
            let o = &value["observability"];
            for protocol in ["http", "smb"] {
                let account = uint(&r[format!("{protocol}_spool_bytes_per_account")])?;
                let total = uint(&r[format!("{protocol}_spool_bytes_total")])?;
                require(
                    account >= 65536 && account <= total,
                    format!("{protocol} spool owner/total limit"),
                )?;
                require(
                    uint(&r[format!("{protocol}_spool_recovery_reserve_bytes")])? > 0,
                    format!("{protocol} spool recovery reserve"),
                )?;
            }
            require(
                small(&r["default_page_items"])? <= small(&r["maximum_page_items"])?,
                "page limit",
            )?;
            require(uint(&r["event_retention_bytes"])? > 0, "event budget")?;
            require(
                small(&o["decision_warning_seconds"])? < small(&o["decision_critical_seconds"])?,
                "alert order",
            )?;
        }
        "RequestEnvelope" => require(
            value["path_parameters"]["workspace"] == value["body"]["workspace_id"],
            "workspace mismatch",
        )?,
        "Branch" | "FileBranchView" | "Commit" => {
            if name == "Commit" {
                for entry in array(&value["entries"])? {
                    if entry["kind"] == "file" {
                        storage_contract::validate_id(text(&entry["file_id"])?, "file")?;
                        storage_contract::validate_id(text(&entry["version_id"])?, "ver")?;
                    }
                }
            }
            if let Some(pending) = value.get("pending_file_versions") {
                for (file, version) in object(pending)? {
                    storage_contract::validate_id(file, "file")?;
                    storage_contract::validate_id(text(version)?, "ver")?;
                    if value["branch_kind"] == "file" {
                        require(
                            value["file_id"] == *file,
                            "pending file escapes branch scope",
                        )?;
                    }
                    if name == "Commit" {
                        require(
                            array(&value["entries"])?.iter().any(|entry| {
                                entry["kind"] == "file"
                                    && entry["file_id"] == *file
                                    && entry["version_id"] == *version
                            }),
                            "snapshot pending source differs from its file entry",
                        )?;
                    }
                }
            }
            if name == "FileBranchView" {
                validate_content_view(value)?;
                let file = text(&value["file_id"])?;
                let source = value
                    .get("pending_file_versions")
                    .and_then(|pending| pending.get(file));
                require(
                    (value["materialized"] == false) == source.is_some(),
                    "branch content state differs from pending record",
                )?;
                if let Some(source) = source {
                    require(
                        value["source_version_id"] == *source,
                        "pending projection changes its lineage source",
                    )?;
                }
            }
        }
        "CommitRequest" => {
            let mut body = object(value)?.clone();
            body.remove("request_digest");
            let envelope = json!({"method":"POST","route":"/api/workspaces/{workspace}/commit","path_parameters":{"workspace":body["workspace_id"]},"query":{},"body":body});
            require(
                storage_contract::object_hash("request", &envelope)?
                    == text(&value["request_digest"])?,
                "request digest mismatch",
            )?;
        }
        "SaveResult" => validate_result(value)?,
        "SaveOperation" | "SaveOperationObservation" if !value["result"].is_null() => {
            validate_result(&value["result"])?;
            require(
                (value["result"]["outcome"] == "COMMITTED_DEGRADED")
                    == (value["commit_state"] == "PUBLISHED_DEGRADED"),
                "operation terminal mismatch",
            )?;
        }
        "Publication" => {
            validate_result(&value["result"])?;
            let members = array(&value["finalize_receipts"])?
                .iter()
                .map(|receipt| text(&receipt["disk_id"]))
                .collect::<Result<Vec<_>>>()?;
            require(
                !members.is_empty()
                    && members
                        .iter()
                        .copied()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>()
                        == members,
                "member identities/order",
            )?;
            require(
                value["decision_slot"] == value["result"]["publication_sequence"],
                "publication sequence",
            )?;
            require(
                value["membership_epoch"] == value["result"]["membership_epoch"],
                "publication epoch",
            )?;
            let mut expected = BTreeMap::new();
            for version in array(&value["result"]["versions"])? {
                for slot in array(&version["slots"])? {
                    if slot["state"] == "COMPLETE_VERIFIED" {
                        expected.insert(
                            (text(&version["version_id"])?, text(&slot["slot_id"])?),
                            (text(&slot["disk_id"])?, text(&slot["binding_generation"])?),
                        );
                    }
                }
            }
            let mut observed = BTreeMap::new();
            for proof in array(&value["content_proofs"])? {
                let key = (text(&proof["version_id"])?, text(&proof["slot_id"])?);
                require(
                    observed
                        .insert(
                            key,
                            (
                                text(&proof["disk_id"])?,
                                text(&proof["binding_generation"])?,
                            ),
                        )
                        .is_none(),
                    "duplicate content proof",
                )?;
            }
            require(expected == observed, "content proof coverage")?;
        }
        "UploadStatus" => {
            let size = uint(&value["logical_size"])?;
            let mut end = 0u64;
            let mut complete = true;
            for range in array(&value["durable_ranges"])? {
                let start = uint(&range["offset"])?;
                let length = uint(&range["length"])?;
                let next = start.checked_add(length).ok_or("range overflow")?;
                require(
                    length > 0 && end <= start && next <= size,
                    "range overlap/overflow",
                )?;
                complete &= start == end;
                end = next;
            }
            if value["state"] == "COMPLETE"
                || value["state"] == "SEALED"
                || value["state"] == "CHECKSUM_MISMATCH"
            {
                require(complete && end == size, "missing upload bytes")?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn checksum_manifest(bytes: &[u8], content: &[u8], expected_hash: &str) -> Result {
    require(
        blake3::hash(bytes).to_hex().as_str() == expected_hash,
        "manifest hash",
    )?;
    let manifest = storage_contract::ChecksumManifest::decode(bytes)?;
    require(
        manifest.logical_size == content.len() as u64,
        "checksum dimensions",
    )?;
    for (part, expected) in content
        .chunks(manifest.chunk_bytes as usize)
        .zip(&manifest.chunks)
    {
        require(blake3::hash(part).as_bytes() == expected, "chunk checksum")?;
    }
    Ok(())
}

fn bundle(value: &Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        if key == "$ref" {
                            Value::String(
                                value
                                    .as_str()
                                    .expect("invariant: validated $ref is a string")
                                    .replace("#/$defs/", "#/components/schemas/"),
                            )
                        } else {
                            bundle(value)
                        },
                    )
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(bundle).collect()),
        _ => value.clone(),
    }
}
fn validator(schema: &Value) -> Result<schema_validation::Validator> {
    schema_validation::options()
        .with_draft(schema_validation::Draft::Draft202012)
        .should_validate_formats(true)
        .build(schema)
        .map_err(|error| error.to_owned().into())
}
fn schema_valid(schema: &Value, value: &Value) -> Result {
    validator(schema)?
        .validate(value)
        .map_err(|error| error.to_owned().into())
}

fn openapi(api: &Value) -> Result {
    let schema =
        storage_contract::strict_json(include_bytes!("../schema/openapi-3.1-2022-10-07.json"))?;
    schema_valid(&schema, api)?;
    fn references(root: &Value, value: &Value) -> Result {
        match value {
            Value::Object(map) => {
                if let Some(reference) = map.get("$ref") {
                    let reference = text(reference)?;
                    require(
                        reference.starts_with("#/"),
                        "external OpenAPI references are forbidden in offline bundle",
                    )?;
                    require(
                        root.pointer(&reference[1..]).is_some(),
                        format!("unresolved OpenAPI reference {reference}"),
                    )?;
                }
                for (key, child) in map {
                    if key == "schema" {
                        schema_validation::meta::validate(child)
                            .map_err(|error| error.to_owned())?;
                    }
                    references(root, child)?;
                }
            }
            Value::Array(values) => {
                for child in values {
                    references(root, child)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    references(api, api)?;
    let path_names = regex::Regex::new(r"\{([^{}]+)\}")?;
    let mut operation_ids = BTreeSet::new();
    for (path, item) in object(&api["paths"])? {
        for (method, operation) in object(item)? {
            if ![
                "get", "put", "post", "delete", "options", "head", "patch", "trace",
            ]
            .contains(&method.as_str())
            {
                continue;
            }
            if let Some(id) = operation.get("operationId") {
                require(
                    operation_ids.insert(text(id)?),
                    "duplicate OpenAPI operationId",
                )?;
            }
            let mut parameters = BTreeMap::new();
            for owner in [item, operation] {
                let mut unique = BTreeSet::new();
                if let Some(list) = owner.get("parameters") {
                    for parameter in array(list)? {
                        let parameter = if let Some(reference) = parameter.get("$ref") {
                            api.pointer(&text(reference)?[1..])
                                .ok_or("unresolved parameter")?
                        } else {
                            parameter
                        };
                        let key = (text(&parameter["in"])?, text(&parameter["name"])?);
                        require(unique.insert(key), "duplicate operation parameter")?;
                        parameters.insert(key, parameter);
                    }
                }
            }
            let placeholders = path_names
                .captures_iter(path)
                .map(|capture| capture[1].to_owned())
                .collect::<BTreeSet<_>>();
            let declared = parameters
                .iter()
                .filter(|((location, _), _)| *location == "path")
                .map(|((_, name), parameter)| {
                    require(
                        parameter["required"] == true,
                        "path parameter must be required",
                    )?;
                    Ok((*name).to_owned())
                })
                .collect::<Result<BTreeSet<_>>>()?;
            require(
                placeholders == declared,
                format!("path parameter mismatch: {path}"),
            )?;
            for owner in [api, operation] {
                if let Some(requirements) = owner.get("security") {
                    for requirement in array(requirements)? {
                        for name in object(requirement)?.keys() {
                            require(
                                api["components"]["securitySchemes"].get(name).is_some(),
                                "unknown security scheme",
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct Report {
    schema_files: usize,
    openapi_paths: usize,
    schema_and_semantic_vectors: usize,
    canonical_vectors: usize,
    checksum_vectors: usize,
    strict_json_rejections: usize,
    local_links_checked: usize,
    local_generated_artifact_links: usize,
    missing_local_generated_artifacts: usize,
    result: &'static str,
    scope: &'static str,
}
fn configuration_projection(schema: &Value, configuration: &Value) -> Result<Value> {
    let mut projection = json_codec::Map::new();
    for (section, shape) in object(&schema["properties"])? {
        let source = configuration
            .get(section)
            .ok_or_else(|| format!("configuration projection missing {section}"))?;
        let mut value = json_codec::Map::new();
        for key in object(&shape["properties"])?.keys() {
            value.insert(
                key.clone(),
                source
                    .get(key)
                    .ok_or_else(|| format!("configuration projection missing {section}.{key}"))?
                    .clone(),
            );
        }
        // All resource fields are covered. Do not silently discard obsolete
        // account upload pools or a fictional application queue from the TOML.
        // Other sections still deliberately project only part of the target.
        projection.insert(
            section.clone(),
            if section == "resource_limits" {
                source.clone()
            } else {
                Value::Object(value)
            },
        );
    }
    Ok(Value::Object(projection))
}

fn validate_configuration_projection(schema: &Value, projection: &Value) -> Result {
    schema_valid(schema, projection).map_err(|source| ContextError {
        context: "docs/21-configuration.md TOML projection against docs/contracts/config-constraints.schema.json".into(),
        source,
    })?;
    semantics("config", projection).map_err(|source| {
        ContextError {
            context: "docs/21-configuration.md TOML projection semantics".into(),
            source,
        }
        .into()
    })
}
fn run(root: &Path) -> Result<Report> {
    let root = root.canonicalize()?;
    let directory = root.join("docs/contracts");
    let core = read(&directory.join("core.schema.json"))?;
    let config = read(&directory.join("config-constraints.schema.json"))?;
    let api = read(&directory.join("openapi.json"))?;
    let vectors = read(&directory.join("vectors.json"))?;
    for schema in [&core, &config] {
        schema_validation::meta::validate(schema).map_err(|error| error.to_owned())?;
    }
    openapi(&api)?;
    require(
        api["components"]["schemas"] == bundle(&core["$defs"]),
        "OpenAPI/schema drift",
    )?;
    let mut validators = BTreeMap::new();
    let mut cases = 0;
    for group in ["positive", "negative"] {
        for case in array(&vectors[group])? {
            let name = text(&case["schema"])?;
            let label = text(&case["name"])?;
            let value = &case["value"];
            if !validators.contains_key(name) {
                let mut schema = if name == "config" {
                    config.clone()
                } else {
                    core.clone()
                };
                if name != "config" {
                    schema["$ref"] = format!("#/$defs/{name}").into();
                }
                validators.insert(name.to_owned(), validator(&schema)?);
            }
            let valid = validators[name].is_valid(value);
            if group == "positive" {
                require(valid, format!("positive vector failed schema: {label}"))?;
                semantics(name, value).map_err(|error| format!("{label}: {error}"))?;
            } else if case["layer"] == "schema" {
                require(!valid, format!("schema accepted {label}"))?;
            } else {
                require(
                    valid,
                    format!("semantic vector failed schema first: {label}"),
                )?;
                require(
                    semantics(name, value).is_err(),
                    format!("invalid input accepted: {label}"),
                )?;
            }
            cases += 1;
        }
    }
    for vector in array(&vectors["canonical"])? {
        let bytes = storage_contract::canonical_json(&vector["value"])?;
        require(
            hex(&bytes) == text(&vector["canonical_utf8_hex"])?,
            "canonical bytes",
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(&unhex(text(&vector["prefix_hex"])?)?);
        hash.update(&bytes);
        require(
            hash.finalize().to_hex().as_str() == text(&vector["blake3"])?,
            "canonical hash",
        )?;
        let reversed = Value::Object(
            object(&vector["value"])?
                .iter()
                .rev()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        require(
            storage_contract::canonical_json(&reversed)? == bytes,
            "field order affects hash",
        )?;
    }
    for vector in array(&vectors["strict_json_negative"])? {
        require(
            storage_contract::strict_json(text(&vector["text"])?.as_bytes()).is_err(),
            format!("invalid JSON accepted: {}", text(&vector["name"])?),
        )?;
    }
    for vector in array(&vectors["binary"])? {
        let content = unhex(text(&vector["content_hex"])?)?;
        let manifest = unhex(text(&vector["manifest_hex"])?)?;
        require(
            blake3::hash(&content).to_hex().as_str() == text(&vector["content_hash"])?,
            "content hash",
        )?;
        checksum_manifest(&manifest, &content, text(&vector["manifest_hash"])?)?;
        let truncated = &manifest[..manifest.len() - 1];
        require(
            checksum_manifest(
                truncated,
                &content,
                blake3::hash(truncated).to_hex().as_str(),
            )
            .is_err(),
            "truncated checksum manifest accepted",
        )?;
        let mut corrupt = manifest.clone();
        *corrupt.last_mut().ok_or("empty manifest")? ^= 1;
        require(
            checksum_manifest(&corrupt, &content, blake3::hash(&corrupt).to_hex().as_str())
                .is_err(),
            "corrupted checksum manifest accepted",
        )?;
    }
    let fences = regex::Regex::new(r"(?ms)^```([^\n]*)\n(.*?)^```")?;
    let configuration = fs::read_to_string(root.join("docs/21-configuration.md"))?;
    let source = fences
        .captures_iter(&configuration)
        .find(|capture| &capture[1] == "toml")
        .ok_or("configuration TOML missing")?;
    let configuration: toml::Value = source[2].parse()?;
    let configuration = json_codec::to_value(configuration)?;
    let projection = configuration_projection(&config, &configuration)?;
    validate_configuration_projection(&config, &projection)?;
    let expected = array(&vectors["positive"])?
        .iter()
        .find(|case| case["schema"] == "config")
        .ok_or("configuration vector missing")?;
    require(
        projection == expected["value"],
        "configuration sample/vector drift",
    )?;
    let anchors = regex::Regex::new(r#"<a\s+id="([^"]+)""#)?;
    let links = regex::Regex::new(r"\[[^\]\n]*\]\(([^)\s]+)\)")?;
    let scheme = regex::Regex::new(r"^[A-Za-z]+:")?;
    let mut paths = vec![root.join("README.md")];
    paths.extend(document_scan::scan(
        &root.join("docs"),
        &root.join("docs/upstream"),
    )?);
    paths.sort();
    let mut checked = 0;
    let mut generated = 0;
    let mut missing_generated = 0;
    for path in paths {
        let document = fs::read_to_string(&path)?;
        let mut seen = BTreeSet::new();
        for capture in anchors.captures_iter(&document) {
            require(
                seen.insert(capture[1].to_owned()),
                format!("duplicate anchor in {}", path.display()),
            )?;
        }
        for link in links.captures_iter(&document) {
            let href = &link[1];
            if scheme.is_match(href) {
                continue;
            }
            let decoded = url_encoding::percent::percent_decode_str(href).decode_utf8()?;
            let (relative, anchor) = decoded
                .split_once('#')
                .map_or((decoded.as_ref(), None), |(relative, anchor)| {
                    (relative, Some(anchor))
                });
            let resolved = document_links::resolve(&root, &path, relative)?;
            if resolved.generated {
                generated += 1;
            }
            let Some(target) = resolved.path else {
                missing_generated += 1;
                continue;
            };
            if let Some(anchor) = anchor.filter(|value| !value.is_empty()) {
                let target = fs::read_to_string(&target)?;
                require(
                    anchors
                        .captures_iter(&target)
                        .any(|capture| &capture[1] == anchor),
                    format!("missing anchor: {href}"),
                )?;
            }
            checked += 1;
        }
        for capture in fences.captures_iter(&document) {
            match &capture[1] {
                "json" => {
                    storage_contract::strict_json(capture[2].as_bytes()).map_err(|error| {
                        format!("invalid JSON example in {}: {error}", path.display())
                    })?;
                }
                "toml" => {
                    capture[2].parse::<toml::Value>().map_err(|error| {
                        format!("invalid TOML example in {}: {error}", path.display())
                    })?;
                }
                _ => {}
            }
        }
    }
    Ok(Report {
        schema_files: 2,
        openapi_paths: object(&api["paths"])?.len(),
        schema_and_semantic_vectors: cases,
        canonical_vectors: array(&vectors["canonical"])?.len(),
        checksum_vectors: array(&vectors["binary"])?.len(),
        strict_json_rejections: array(&vectors["strict_json_negative"])?.len(),
        local_links_checked: checked,
        local_generated_artifact_links: generated,
        missing_local_generated_artifacts: missing_generated,
        result: "PASS",
        scope: "documentation contracts only; no storage/model/platform validation",
    })
}
fn main() {
    let result = (|| -> Result {
        let arguments = std::env::args().skip(1).collect::<Vec<_>>();
        let root = match arguments.as_slice() {
            [] => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."),
            [flag, path] if flag == "--root" => PathBuf::from(path),
            _ => return Err("usage: contract-check [--root PATH]".into()),
        };
        println!("{}", json_codec::to_string_pretty(&run(&root)?)?);
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("contract check failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[path = "../../../test/tools/contract-check/unit/main_tests.rs"]
mod tests;
