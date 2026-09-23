use super::yaml::{Context, Node, TemplateError, Value};

pub(super) fn validate_input(
    node: &Node,
    path: &str,
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    object(
        node,
        path,
        context,
        &["label", "type"],
        &[
            "description",
            "required",
            "default",
            "options",
            "validation",
            "clone",
            "initial",
            "advanced",
        ],
    )?;
    string_length(
        field(node, "label"),
        &format!("{path}.label"),
        context,
        1,
        100,
    )?;
    let input_type = choice(
        field(node, "type"),
        &format!("{path}.type"),
        context,
        &["string", "integer", "boolean", "select", "secret"],
    )?;
    if let Some(value) = optional(node, "description") {
        string(value, &format!("{path}.description"), context)?;
    }
    for key in ["required", "advanced"] {
        if let Some(value) = optional(node, key) {
            boolean(value, &format!("{path}.{key}"), context)?;
        }
    }
    if let Some(value) = optional(node, "default") {
        let valid = matches!(
            (&value.value, input_type),
            (Value::String(_), "string" | "select")
                | (Value::Integer(_), "integer")
                | (Value::Boolean(_), "boolean")
        );
        if !valid {
            return Err(context.error(
                Some(value.position),
                &format!("{path}.default"),
                "default must match input type; secret defaults are forbidden",
            ));
        }
        if input_type == "integer" {
            integer_range(
                value,
                &format!("{path}.default"),
                context,
                -9_007_199_254_740_991,
                9_007_199_254_740_991,
            )?;
        }
    }
    if let Some(value) = optional(node, "options") {
        if input_type != "select" {
            return Err(context.error(
                Some(value.position),
                &format!("{path}.options"),
                "options is only allowed for select inputs",
            ));
        }
        let items = sequence(value, &format!("{path}.options"), context)?;
        length(
            items.len(),
            1,
            128,
            value,
            &format!("{path}.options"),
            context,
        )?;
        for (index, item) in items.iter().enumerate() {
            let item_path = format!("{path}.options[{index}]");
            object(item, &item_path, context, &["value", "label"], &[])?;
            string_length(
                field(item, "value"),
                &format!("{item_path}.value"),
                context,
                1,
                256,
            )?;
            string_length(
                field(item, "label"),
                &format!("{item_path}.label"),
                context,
                1,
                100,
            )?;
        }
    } else if input_type == "select" {
        return Err(context.error(
            Some(node.position),
            &format!("{path}.options"),
            "select inputs require options",
        ));
    }
    if let Some(value) = optional(node, "validation") {
        let allowed = match input_type {
            "string" | "secret" => &["minLength", "maxLength", "pattern"][..],
            "integer" => &["min", "max"][..],
            _ => &[][..],
        };
        object(value, &format!("{path}.validation"), context, &[], allowed)?;
        for (key, item) in map(value, &format!("{path}.validation"), context)? {
            if key == "pattern" {
                let pattern = string(item, &format!("{path}.validation.{key}"), context)?;
                if pattern.chars().count() > 256 {
                    return Err(context.error(
                        Some(item.position),
                        &format!("{path}.validation.{key}"),
                        "pattern exceeds 256 characters",
                    ));
                }
            } else {
                let (minimum, maximum) = match key.as_str() {
                    "minLength" => (0, 4_096),
                    "maxLength" => (1, 4_096),
                    _ => (-9_007_199_254_740_991, 9_007_199_254_740_991),
                };
                integer_range(
                    item,
                    &format!("{path}.validation.{key}"),
                    context,
                    minimum,
                    maximum,
                )?;
            }
        }
    }
    if let Some(value) = optional(node, "clone") {
        let values = if input_type == "secret" {
            &["copy", "regenerate", "clear", "ask"][..]
        } else {
            &["copy", "clear", "ask"][..]
        };
        choice(value, &format!("{path}.clone"), context, values)?;
    }
    if let Some(value) = optional(node, "initial") {
        if input_type != "secret" {
            return Err(context.error(
                Some(value.position),
                &format!("{path}.initial"),
                "initial is only allowed for secret inputs",
            ));
        }
        choice(
            value,
            &format!("{path}.initial"),
            context,
            &["generate", "ask"],
        )?;
    }
    Ok(())
}

pub(super) fn validate_service(
    node: &Node,
    path: &str,
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    object(
        node,
        path,
        context,
        &["healthcheck"],
        &[
            "environment",
            "command",
            "ports",
            "storage",
            "restart",
            "memoryLimitMiB",
        ],
    )?;
    if let Some(value) = optional(node, "environment") {
        named_map(
            value,
            &format!("{path}.environment"),
            context,
            128,
            validate_value,
        )?;
    }
    if let Some(value) = optional(node, "command") {
        validate_command(value, &format!("{path}.command"), context)?;
    }
    for (key, limit, validator) in [
        (
            "ports",
            16,
            validate_port as fn(&Node, &str, &Context<'_>) -> Result<(), TemplateError>,
        ),
        ("storage", 16, validate_storage),
    ] {
        if let Some(value) = optional(node, key) {
            named_map(value, &format!("{path}.{key}"), context, limit, validator)?;
        }
    }
    let health = field(node, "healthcheck");
    let health_path = format!("{path}.healthcheck");
    object(
        health,
        &health_path,
        context,
        &["command"],
        &[
            "intervalSeconds",
            "timeoutSeconds",
            "startPeriodSeconds",
            "retries",
        ],
    )?;
    validate_command(
        field(health, "command"),
        &format!("{health_path}.command"),
        context,
    )?;
    for (key, minimum, maximum) in [
        ("intervalSeconds", 1, 300),
        ("timeoutSeconds", 1, 300),
        ("startPeriodSeconds", 0, 600),
        ("retries", 1, 120),
    ] {
        if let Some(value) = optional(health, key) {
            integer_range(
                value,
                &format!("{health_path}.{key}"),
                context,
                minimum,
                maximum,
            )?;
        }
    }
    if let Some(value) = optional(node, "restart") {
        choice(
            value,
            &format!("{path}.restart"),
            context,
            &["no", "on-failure", "always", "unless-stopped"],
        )?;
    }
    if let Some(value) = optional(node, "memoryLimitMiB") {
        integer_range(
            value,
            &format!("{path}.memoryLimitMiB"),
            context,
            16,
            1_048_576,
        )?;
    }
    Ok(())
}

fn validate_command(node: &Node, path: &str, context: &Context<'_>) -> Result<(), TemplateError> {
    let items = sequence(node, path, context)?;
    length(items.len(), 1, 64, node, path, context)?;
    for (index, item) in items.iter().enumerate() {
        validate_value(item, &format!("{path}[{index}]"), context)?;
    }
    string_length(&items[0], &format!("{path}[0]"), context, 1, 16 * 1024)?;
    Ok(())
}

fn validate_value(node: &Node, path: &str, context: &Context<'_>) -> Result<(), TemplateError> {
    match &node.value {
        Value::String(_) | Value::Integer(_) | Value::Boolean(_) => Ok(()),
        Value::Map(_) => {
            object(node, path, context, &["input"], &["onMissing"])?;
            string(field(node, "input"), &format!("{path}.input"), context)?;
            if let Some(value) = optional(node, "onMissing") {
                if !path.contains(".environment.") {
                    return Err(context.error(
                        Some(value.position),
                        &format!("{path}.onMissing"),
                        "onMissing is only allowed in environment",
                    ));
                }
                choice(value, &format!("{path}.onMissing"), context, &["omit"])?;
            }
            Ok(())
        }
        _ => Err(context.error(
            Some(node.position),
            path,
            "expected a scalar or { input: name } reference",
        )),
    }
}

fn validate_port(node: &Node, path: &str, context: &Context<'_>) -> Result<(), TemplateError> {
    object(
        node,
        path,
        context,
        &["label", "container"],
        &["defaultHost"],
    )?;
    string_length(
        field(node, "label"),
        &format!("{path}.label"),
        context,
        1,
        100,
    )?;
    integer_range(
        field(node, "container"),
        &format!("{path}.container"),
        context,
        1,
        65535,
    )?;
    if let Some(value) = optional(node, "defaultHost") {
        integer_range(value, &format!("{path}.defaultHost"), context, 1024, 65535)?;
    }
    Ok(())
}

fn validate_storage(node: &Node, path: &str, context: &Context<'_>) -> Result<(), TemplateError> {
    object(node, path, context, &["label", "container"], &[])?;
    string_length(
        field(node, "label"),
        &format!("{path}.label"),
        context,
        1,
        100,
    )?;
    string(
        field(node, "container"),
        &format!("{path}.container"),
        context,
    )?;
    Ok(())
}

pub(super) fn validate_connection(
    node: &Node,
    path: &str,
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    object(node, path, context, &["label", "port"], &["inputs"])?;
    string_length(
        field(node, "label"),
        &format!("{path}.label"),
        context,
        1,
        100,
    )?;
    string(field(node, "port"), &format!("{path}.port"), context)?;
    if let Some(value) = optional(node, "inputs") {
        let mut seen = std::collections::HashSet::new();
        for (index, item) in sequence(value, &format!("{path}.inputs"), context)?
            .iter()
            .enumerate()
        {
            let item_path = format!("{path}.inputs[{index}]");
            let key = string(item, &item_path, context)?;
            if !seen.insert(key) {
                return Err(context.error(
                    Some(item.position),
                    &item_path,
                    "duplicate connection input",
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn named_map(
    node: &Node,
    path: &str,
    context: &Context<'_>,
    maximum: usize,
    validate: fn(&Node, &str, &Context<'_>) -> Result<(), TemplateError>,
) -> Result<(), TemplateError> {
    let entries = map(node, path, context)?;
    length(entries.len(), 0, maximum, node, path, context)?;
    for (key, value) in entries {
        let valid_key = if path.ends_with(".environment") {
            key.starts_with(|character: char| character.is_ascii_alphabetic() || character == '_')
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        } else {
            (1..=32).contains(&key.len())
                && key.starts_with(|character: char| character.is_ascii_lowercase())
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if !valid_key {
            return Err(context.error(
                Some(value.position),
                &format!("{path}.{key}"),
                "invalid key; use the key format specified for this map",
            ));
        }
        validate(value, &format!("{path}.{key}"), context)?;
    }
    Ok(())
}

pub(super) fn object<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
    required: &[&str],
    optional_keys: &[&str],
) -> Result<&'a [(String, Node)], TemplateError> {
    let entries = map(node, path, context)?;
    for (key, value) in entries {
        if !required.contains(&key.as_str()) && !optional_keys.contains(&key.as_str()) {
            return Err(context.error(
                Some(value.position),
                &format!("{path}.{key}"),
                format!("unknown key {key}; remove it or correct the spelling"),
            ));
        }
    }
    for key in required {
        if !entries.iter().any(|(existing, _)| existing == key) {
            return Err(context.error(
                Some(node.position),
                &format!("{path}.{key}"),
                format!("required key {key} is missing"),
            ));
        }
    }
    Ok(entries)
}

pub(super) fn field<'a>(node: &'a Node, key: &str) -> &'a Node {
    optional(node, key).expect("required key checked by object")
}
pub(super) fn optional<'a>(node: &'a Node, key: &str) -> Option<&'a Node> {
    let Value::Map(entries) = &node.value else {
        return None;
    };
    entries
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}
pub(super) fn map<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
) -> Result<&'a [(String, Node)], TemplateError> {
    match &node.value {
        Value::Map(entries) => Ok(entries),
        _ => Err(context.error(Some(node.position), path, "expected a mapping")),
    }
}
pub(super) fn sequence<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
) -> Result<&'a [Node], TemplateError> {
    match &node.value {
        Value::Sequence(items) => Ok(items),
        _ => Err(context.error(Some(node.position), path, "expected a sequence")),
    }
}
pub(super) fn string<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
) -> Result<&'a str, TemplateError> {
    match &node.value {
        Value::String(value) => Ok(value),
        _ => Err(context.error(
            Some(node.position),
            path,
            "expected a string; quote numeric values",
        )),
    }
}
pub(super) fn string_length<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
    minimum: usize,
    maximum: usize,
) -> Result<&'a str, TemplateError> {
    let value = string(node, path, context)?;
    let count = value.chars().count();
    if (minimum..=maximum).contains(&count) {
        Ok(value)
    } else {
        Err(context.error(
            Some(node.position),
            path,
            format!("expected {minimum}–{maximum} characters"),
        ))
    }
}
fn integer(node: &Node, path: &str, context: &Context<'_>) -> Result<i64, TemplateError> {
    match node.value {
        Value::Integer(value) => Ok(value),
        _ => Err(context.error(Some(node.position), path, "expected an integer")),
    }
}
fn integer_range(
    node: &Node,
    path: &str,
    context: &Context<'_>,
    minimum: i64,
    maximum: i64,
) -> Result<i64, TemplateError> {
    let value = integer(node, path, context)?;
    if (minimum..=maximum).contains(&value) {
        Ok(value)
    } else {
        Err(context.error(
            Some(node.position),
            path,
            format!("expected an integer from {minimum} to {maximum}"),
        ))
    }
}
fn boolean(node: &Node, path: &str, context: &Context<'_>) -> Result<bool, TemplateError> {
    match node.value {
        Value::Boolean(value) => Ok(value),
        _ => Err(context.error(Some(node.position), path, "expected true or false")),
    }
}
pub(super) fn choice<'a>(
    node: &'a Node,
    path: &str,
    context: &Context<'_>,
    values: &[&str],
) -> Result<&'a str, TemplateError> {
    let value = string(node, path, context)?;
    if values.contains(&value) {
        Ok(value)
    } else {
        Err(context.error(
            Some(node.position),
            path,
            format!("expected one of: {}", values.join(", ")),
        ))
    }
}
pub(super) fn length(
    count: usize,
    minimum: usize,
    maximum: usize,
    node: &Node,
    path: &str,
    context: &Context<'_>,
) -> Result<(), TemplateError> {
    if (minimum..=maximum).contains(&count) {
        Ok(())
    } else {
        Err(context.error(
            Some(node.position),
            path,
            format!("expected {minimum}–{maximum} items"),
        ))
    }
}
